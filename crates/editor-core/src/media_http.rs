//! Credential-free HTTP for public transcript media. Forge requests retain
//! their separate pinned, paired client. Every body is bounded while reading.
use futures::future::BoxFuture;
use gpui::http_client::{HttpClient, HttpResponse};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::task::AbortOnDropHandle;

const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
struct ResourceHttpClient {
    redirects: reqwest::Client,
    direct: reqwest::Client,
}

/// The application's public media transport, sharing its existing runtime.
#[expect(
    clippy::expect_used,
    reason = "a client with only timeouts and a redirect policy set builds unless TLS is broken"
)]
pub fn client() -> Arc<dyn HttpClient> {
    let build = |redirects| {
        reqwest::Client::builder()
            .redirect(redirects)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("build public media HTTP client")
    };
    Arc::new(ResourceHttpClient {
        redirects: build(reqwest::redirect::Policy::limited(5)),
        direct: build(reqwest::redirect::Policy::none()),
    })
}
impl HttpClient for ResourceHttpClient {
    fn get(
        &self,
        url: &str,
        follow_redirects: bool,
    ) -> BoxFuture<'static, anyhow::Result<HttpResponse>> {
        let request_client = if follow_redirects {
            &self.redirects
        } else {
            &self.direct
        }
        .clone();
        let request = request_client.get(url).build();
        Box::pin(async move {
            let request = request?;
            anyhow::ensure!(
                matches!(request.url().scheme(), "http" | "https"),
                "Media requires HTTP or HTTPS"
            );
            // Cancellation of the GPUI asset cancels its network work as well.
            let task = AbortOnDropHandle::new(crate::runtime::runtime().spawn(async move {
                let response = request_client.execute(request).await?;
                let status = response.status();
                let body = crate::client::read_body(response, MAX_BODY_BYTES).await?;
                Ok::<_, anyhow::Error>(HttpResponse { status, body })
            }));
            task.await?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    #[test]
    fn declared_oversized_media_body_is_rejected_before_reading() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 2048];
            let _ = socket.read(&mut request).unwrap();
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_BODY_BYTES + 1
            )
            .unwrap();
        });
        let error =
            futures::executor::block_on(client().get(&format!("http://{address}/image"), false))
                .err()
                .unwrap();
        assert!(error.to_string().contains("exceeds"));
        server.join().unwrap();
    }

    #[test]
    fn public_media_client_rejects_non_http_schemes() {
        assert!(futures::executor::block_on(client().get("file:///tmp/image.png", false)).is_err());
    }
}
