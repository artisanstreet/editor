use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    time::Duration,
};

use serde::Deserialize;

use crate::{CliError, Result};

#[derive(Deserialize)]
pub struct PairResponse {
    pub code: String,
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
    let address = endpoint(endpoint_url)?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .map_err(|error| CliError::Control(error.to_string()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| CliError::Control(error.to_string()))?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| CliError::Control(error.to_string()))?;
    let mut response = Vec::new();
    stream
        .take(1024 * 1024)
        .read_to_end(&mut response)
        .map_err(|error| CliError::Control(error.to_string()))?;
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
    if headers.lines().any(|line| {
        line.eq_ignore_ascii_case("transfer-encoding: chunked")
            || line.to_ascii_lowercase().starts_with("transfer-encoding: chunked")
    }) {
        decode_chunked(body)
    } else {
        Ok(body.to_vec())
    }
}

pub fn healthy(endpoint_url: &str, token: &str) -> bool {
    request(endpoint_url, "/api/control/status", token, "GET").is_ok()
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
            return Err(CliError::Control("Forge response exceeded its bound".into()));
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
        assert_eq!(
            decode_chunked(b"4\r\ntest\r\n0\r\n\r\n").unwrap(),
            b"test"
        );
    }

    #[test]
    fn rejects_non_loopback_endpoints() {
        assert!(endpoint("http://192.0.2.1:1234").is_err());
    }
}
