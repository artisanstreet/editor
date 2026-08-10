use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::{CliError, Result};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const ORDINARY_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
pub struct PairResponse {
    pub code: String,
}

#[derive(Deserialize)]
pub struct StatusResponse {
    pub instance_id: String,
    pub pid: u32,
}

fn endpoint(endpoint: &str) -> Result<SocketAddr> {
    let rest = endpoint
        .strip_prefix("http://")
        .ok_or_else(|| CliError::Control("Forge endpoint must use HTTP loopback".into()))?;
    let authority = rest.trim_end_matches('/');
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| CliError::Control("Forge endpoint has no explicit port".into()))?;
    let host = host.trim_matches(['[', ']']);
    let ip: IpAddr = host
        .parse()
        .map_err(|_| CliError::Control("Forge endpoint host is invalid".into()))?;
    if !ip.is_loopback() {
        return Err(CliError::Control("Forge endpoint is not loopback".into()));
    }
    let port = port
        .parse()
        .map_err(|_| CliError::Control("Forge endpoint port is invalid".into()))?;
    Ok(SocketAddr::new(ip, port))
}

pub fn request(endpoint_url: &str, path: &str, token: &str, method: &str) -> Result<Vec<u8>> {
    request_until(
        endpoint_url,
        path,
        token,
        method,
        Instant::now() + ORDINARY_REQUEST_TIMEOUT,
    )
}

/// Performs one bounded loopback request before a monotonic deadline. Every
/// connect, write, and read receives only the remaining wall-clock budget.
pub fn request_until(
    endpoint_url: &str,
    path: &str,
    token: &str,
    method: &str,
    deadline: Instant,
) -> Result<Vec<u8>> {
    let address = endpoint(endpoint_url)?;
    let mut stream = TcpStream::connect_timeout(&address, remaining(deadline)?)
        .map_err(|error| CliError::Control(error.to_string()))?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
    write_all_until(&mut stream, request.as_bytes(), deadline)?;
    let response = read_until(&mut stream, deadline)?;
    let split = response
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .ok_or_else(|| CliError::Control("malformed Forge response".into()))?;
    let headers = String::from_utf8_lossy(&response[..split]);
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_default();
    if !status.starts_with('2') {
        return Err(CliError::Control(format!("Forge returned HTTP {status}")));
    }
    let body = &response[(split + 4)..];
    let body = if headers.lines().any(|line| {
        line.eq_ignore_ascii_case("transfer-encoding: chunked")
            || line
                .to_ascii_lowercase()
                .starts_with("transfer-encoding: chunked")
    }) {
        decode_chunked(body)?
    } else {
        body.to_vec()
    };
    remaining(deadline)?;
    Ok(body)
}

fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| CliError::Control("Forge request timed out".into()))
}

fn write_all_until(stream: &mut TcpStream, mut bytes: &[u8], deadline: Instant) -> Result<()> {
    while !bytes.is_empty() {
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(|error| CliError::Control(error.to_string()))?;
        let written = stream
            .write(bytes)
            .map_err(|error| CliError::Control(error.to_string()))?;
        if written == 0 {
            return Err(CliError::Control("Forge request write stalled".into()));
        }
        bytes = &bytes[written..];
    }
    remaining(deadline)?;
    Ok(())
}

fn read_until(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>> {
    let mut response = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|error| CliError::Control(error.to_string()))?;
        let remaining_bytes = (MAX_RESPONSE_BYTES + 1).saturating_sub(response.len());
        if remaining_bytes == 0 {
            return Err(CliError::Control(
                "Forge response exceeded its bound".into(),
            ));
        }
        let chunk_length = chunk.len().min(remaining_bytes);
        let read = stream
            .read(&mut chunk[..chunk_length])
            .map_err(|error| CliError::Control(error.to_string()))?;
        if read == 0 {
            remaining(deadline)?;
            return Ok(response);
        }
        response.extend_from_slice(&chunk[..read]);
        if response.len() > MAX_RESPONSE_BYTES {
            return Err(CliError::Control(
                "Forge response exceeded its bound".into(),
            ));
        }
    }
}

pub fn healthy(endpoint_url: &str, token: &str) -> bool {
    status(endpoint_url, token).is_ok()
}

/// Reads the bounded, authenticated Forge status payload used for exact
/// instance ownership checks before an editor can request shutdown.
pub fn status(endpoint_url: &str, token: &str) -> Result<StatusResponse> {
    status_until(
        endpoint_url,
        token,
        Instant::now() + ORDINARY_REQUEST_TIMEOUT,
    )
}

pub fn status_until(endpoint_url: &str, token: &str, deadline: Instant) -> Result<StatusResponse> {
    let body = request_until(endpoint_url, "/api/control/status", token, "GET", deadline)?;
    serde_json::from_slice(&body)
        .map_err(|error| CliError::Control(format!("invalid Forge status response: {error}")))
}

fn decode_chunked(mut input: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|part| part == b"\r\n")
            .ok_or_else(|| CliError::Control("malformed chunked Forge response".into()))?;
        let size_text = std::str::from_utf8(&input[..line_end])
            .map_err(|_| CliError::Control("invalid chunk length".into()))?
            .split(';')
            .next()
            .unwrap_or_default();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| CliError::Control("invalid chunk length".into()))?;
        input = &input[(line_end + 2)..];
        if size == 0 {
            return Ok(output);
        }
        if size > input.len() || output.len().saturating_add(size) > 1024 * 1024 {
            return Err(CliError::Control(
                "Forge response exceeded its bound".into(),
            ));
        }
        output.extend_from_slice(&input[..size]);
        input = input
            .get((size + 2)..)
            .ok_or_else(|| CliError::Control("truncated chunked Forge response".into()))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_bounded_chunked_body() {
        assert_eq!(decode_chunked(b"4\r\ntest\r\n0\r\n\r\n").unwrap(), b"test");
    }

    #[test]
    fn rejects_non_loopback_endpoints() {
        assert!(endpoint("http://192.0.2.1:1234").is_err());
    }
}
