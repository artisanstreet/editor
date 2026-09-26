//! The only network seam of managed engine installation.
//!
//! [`HttpsTransport`] is the production implementation: HTTPS only, no
//! redirects, no ambient proxy, bounded connect and request timeouts, and
//! bounded bodies. Tests substitute an in-memory transport, so no test ever
//! reaches the network.

use std::{io::Read, io::Write, time::Duration};

use reqwest::blocking::Client;

use super::feed::FeedRequest;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
const USER_AGENT: &str = concat!("artisan-forge/", env!("CARGO_PKG_VERSION"));

/// Bounded, URL-free transport failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportError {
    /// The request could not be sent or the body could not be read.
    Failed,
    /// The server answered with a non-success status or a redirect.
    Rejected,
    /// The declared or streamed body exceeded its bound.
    TooLarge,
    /// The declared length did not match the streamed body.
    Truncated,
    /// Writing the body to its destination failed.
    Sink,
}

impl TransportError {
    /// Returns the stable classification.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Failed => "download_failed",
            Self::Rejected => "download_rejected",
            Self::TooLarge => "download_too_large",
            Self::Truncated => "download_truncated",
            Self::Sink => "download_write_failed",
        }
    }
}

/// Fetches feed documents and streams artifacts.
pub trait ReleaseTransport: Send + Sync {
    /// Fetches one bounded feed document.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the request fails, is rejected, or
    /// exceeds `request.bound_bytes`.
    fn fetch(&self, request: &FeedRequest) -> Result<Vec<u8>, TransportError>;

    /// Streams one artifact into `sink`, returning the byte count.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the request fails, is rejected,
    /// exceeds `bound_bytes`, or `sink` fails.
    fn download(
        &self,
        url: &str,
        bound_bytes: u64,
        sink: &mut dyn Write,
    ) -> Result<u64, TransportError>;
}

/// Production HTTPS transport.
pub struct HttpsTransport {
    client: Client,
}

impl std::fmt::Debug for HttpsTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpsTransport")
            .finish_non_exhaustive()
    }
}

impl HttpsTransport {
    /// Builds the transport.
    ///
    /// Must not be called from inside an async runtime thread; the Forge runs
    /// engine management on a dedicated thread.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Failed`] when the TLS client cannot be built.
    pub fn new() -> Result<Self, TransportError> {
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| TransportError::Failed)?;
        Ok(Self { client })
    }

    fn send(
        &self,
        url: &str,
        accept: Option<&str>,
    ) -> Result<reqwest::blocking::Response, TransportError> {
        let mut request = self.client.get(url);
        if let Some(accept) = accept {
            request = request.header(reqwest::header::ACCEPT, accept);
        }
        let response = request.send().map_err(|_| TransportError::Failed)?;
        if !response.status().is_success() {
            return Err(TransportError::Rejected);
        }
        Ok(response)
    }
}

impl ReleaseTransport for HttpsTransport {
    fn fetch(&self, request: &FeedRequest) -> Result<Vec<u8>, TransportError> {
        let mut response = self.send(&request.url, request.accept)?;
        let declared = response.content_length();
        let mut body = Vec::new();
        copy_bounded(&mut response, &mut body, declared, request.bound_bytes)?;
        Ok(body)
    }

    fn download(
        &self,
        url: &str,
        bound_bytes: u64,
        sink: &mut dyn Write,
    ) -> Result<u64, TransportError> {
        let mut response = self.send(url, None)?;
        let declared = response.content_length();
        copy_bounded(&mut response, sink, declared, bound_bytes)
    }
}

/// Copies `reader` into `writer`, enforcing the declared length and bound.
///
/// # Errors
///
/// Returns [`TransportError`] when the body exceeds `bound`, disagrees with
/// `declared_length`, or either side fails.
pub fn copy_bounded<R: Read + ?Sized, W: Write + ?Sized>(
    reader: &mut R,
    writer: &mut W,
    declared_length: Option<u64>,
    bound: u64,
) -> Result<u64, TransportError> {
    if declared_length.is_some_and(|length| length > bound) {
        return Err(TransportError::TooLarge);
    }
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| TransportError::Failed)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .filter(|total| *total <= bound)
            .ok_or(TransportError::TooLarge)?;
        writer
            .write_all(&buffer[..read])
            .map_err(|_| TransportError::Sink)?;
    }
    if declared_length.is_some_and(|length| length != total) {
        return Err(TransportError::Truncated);
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bounded_copy_enforces_declared_and_streamed_bounds() {
        let copy = |body: &[u8], declared, bound| {
            let mut output = Vec::new();
            copy_bounded(
                &mut Cursor::new(body.to_vec()),
                &mut output,
                declared,
                bound,
            )
            .map(|total| (total, output))
        };
        assert_eq!(copy(b"12345", Some(6), 5), Err(TransportError::TooLarge));
        assert_eq!(copy(b"12345", Some(4), 5), Err(TransportError::Truncated));
        assert_eq!(copy(b"123456", None, 5), Err(TransportError::TooLarge));
        assert_eq!(copy(b"12345", Some(5), 5), Ok((5, b"12345".to_vec())));
    }

    #[test]
    fn production_transport_refuses_plain_http_without_sending() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/latest", listener.local_addr().unwrap());
        let transport = HttpsTransport::new().unwrap();
        let request = FeedRequest {
            url,
            accept: None,
            bound_bytes: 16,
        };
        assert_eq!(transport.fetch(&request), Err(TransportError::Failed));
        assert!(listener.accept().is_err(), "no connection may be attempted");
    }
}
