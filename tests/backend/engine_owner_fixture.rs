//! TEST-ONLY engine-owner protocol child fixture.
//!
//! One ordinary `main` in a `testonly` Bazel `rust_binary`: no libtest
//! harness, no banner, never shipped. It implements only the six frozen
//! P0 first-wave scenarios and no P4 session/SSE flows. No product module
//! is imported; the only non-`std` dependency is the pinned
//! `@crates//:serde_json` (1.0.151).
//!
//! Selector is child-only `ARTISAN_ENGINE_OWNER_TEST_SCENARIO` set on the
//! spawned `Command` environment. Missing, non-Unicode or unknown exits 87
//! with no stdout. Parent must never mutate global env and must never read
//! real engine credentials. Health scenarios compare the exact fixture
//! credential from `ARTISAN_ENGINE_OWNER_TEST_AUTHORIZATION` (bounded,
//! CR/LF-free, supplied by the parent) to the request `Authorization`
//! header. This is not shipping auth policy.
//!
//! Main is dedicated to the stdin lifeline: EOF exits 3; any unexpected
//! input byte or read error exits 87. A fixture-local 20s watchdog exits
//! 99 and is always failure. At most one health-server thread plus the
//! watchdog exists; no thread-per-request, nested processes, temporary
//! directories, marker files or recursive discovery. Blocking accept/read
//! cannot prevent main from observing lifeline EOF. OS sockets/threads die
//! with this test process; parent must observe `Child::wait`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process;
use std::time::Duration;

/// Fixture-local lifeline-lost exit code (mirrors helper contract value 3).
const FIXTURE_EXIT_LIFELINE_LOST: i32 = 3;
/// Fixture-local watchdog failure exit (always failure).
const WATCHDOG_EXIT: i32 = 99;
/// Unknown / non-Unicode / missing scenario refusal.
const SCENARIO_REFUSED_EXIT: i32 = 87;
/// Abrupt direct-child exit code.
const ABRUPT_EXIT_CODE: i32 = 7;
/// Child-only scenario selector.
const SCENARIO_ENV: &str = "ARTISAN_ENGINE_OWNER_TEST_SCENARIO";
/// Child-only expected Authorization credential for health scenarios.
const AUTH_ENV: &str = "ARTISAN_ENGINE_OWNER_TEST_AUTHORIZATION";

// Synthetic fixture values — private, never product defaults.
/// Expected health version for `ready_ok`.
const EXPECTED_HEALTH_VERSION: &str = "0.0.0-fixture";
/// Incompatible health version for `health_version_reject`.
const INCOMPATIBLE_VERSION: &str = "0.0.0-fixture-incompatible";
/// Readiness-test limit (parent configures this limit to 256).
const READINESS_LIMIT: usize = 256;
/// Oversized readiness length (exactly one byte over the limit, pre-newline).
const OVERSIZED_READY_LEN: usize = 257;
/// Request-header cap including request line through `CRLFCRLF`.
const REQUEST_HEADER_CAP: usize = 4096;
/// Expected-Authorization cap (synthetic).
const EXPECTED_AUTH_CAP: usize = 512;
/// Watchdog bound.
const WATCHDOG_SECS: u64 = 20;

fn main() {
    std::thread::Builder::new()
        .name("engine-owner-fixture-watchdog".to_owned())
        .spawn(|| {
            std::thread::sleep(Duration::from_secs(WATCHDOG_SECS));
            process::exit(WATCHDOG_EXIT);
        })
        .expect("watchdog thread should spawn");

    let scenario = match std::env::var_os(SCENARIO_ENV) {
        Some(v) => match v.into_string() {
            Ok(s) => s,
            Err(_) => process::exit(SCENARIO_REFUSED_EXIT),
        },
        None => process::exit(SCENARIO_REFUSED_EXIT),
    };

    if !is_known_scenario(&scenario) {
        process::exit(SCENARIO_REFUSED_EXIT);
    }

    match scenario.as_str() {
        "ready_ok" => run_ready_ok(EXPECTED_HEALTH_VERSION),
        "ready_malformed" => run_ready_malformed(),
        "ready_oversized_bounded_reject" => run_ready_oversized(),
        "health_version_reject" => run_ready_ok(INCOMPATIBLE_VERSION),
        "hang_until_lifeline" => run_hang_until_lifeline(),
        "abrupt_child_exit_nonzero" => process::exit(ABRUPT_EXIT_CODE),
        _ => process::exit(SCENARIO_REFUSED_EXIT),
    }
}

fn run_ready_malformed() -> ! {
    let mut out = std::io::stdout().lock();
    // One malformed JSON line, flushed, no HTTP service.
    let _ = writeln!(out, "not-json");
    let _ = out.flush();
    wait_for_lifeline_eof();
}

fn run_ready_oversized() -> ! {
    let line = build_oversized_line();
    debug_assert_eq!(line.len(), OVERSIZED_READY_LEN);
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
    wait_for_lifeline_eof();
}

fn run_hang_until_lifeline() -> ! {
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(b"R");
    let _ = err.flush();
    wait_for_lifeline_eof();
}

fn get_expected_auth_or_exit() -> String {
    let Some(os_val) = std::env::var_os(AUTH_ENV) else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    let Ok(val) = os_val.into_string() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    if val.is_empty() || val.len() > EXPECTED_AUTH_CAP || val.contains('\r') || val.contains('\n') {
        process::exit(SCENARIO_REFUSED_EXIT);
    }
    val
}

fn run_ready_ok(version: &'static str) -> ! {
    let expected_auth = get_expected_auth_or_exit();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let port = listener.local_addr().expect("local_addr").port();
    assert!(port != 0, "advertised port must be nonzero");
    let readiness = build_readiness_line(port);
    debug_assert!(readiness.len() <= READINESS_LIMIT);
    {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{readiness}").expect("readiness write");
        out.flush().expect("readiness flush");
    }
    std::thread::Builder::new()
        .name("engine-owner-fixture-health".to_owned())
        .spawn(move || health_server_loop(listener, &expected_auth, version))
        .expect("health thread should spawn");
    wait_for_lifeline_eof();
}

fn build_readiness_line(port: u16) -> String {
    format!(r#"{{"url":"http://127.0.0.1:{port}"}}"#)
}

fn build_oversized_line() -> String {
    // Syntactically valid JSON padded with JSON whitespace (spaces) to
    // exactly OVERSIZED_READY_LEN bytes pre-newline, without giant allocation.
    let base = r#"{"url":"http://127.0.0.1:1"}"#;
    let pad = OVERSIZED_READY_LEN - base.len();
    let mut s = String::with_capacity(OVERSIZED_READY_LEN);
    s.push_str(base);
    s.push_str(&" ".repeat(pad));
    s
}

fn wait_for_lifeline_eof() -> ! {
    let stdin = std::io::stdin();
    let mut lock = stdin.lock();
    let mut buf = [0_u8; 1];
    match lock.read(&mut buf) {
        Ok(0) => process::exit(FIXTURE_EXIT_LIFELINE_LOST),
        Ok(_) | Err(_) => process::exit(SCENARIO_REFUSED_EXIT),
    }
}

// ---------------------------------------------------------------------------
// Health server (at most one thread). Only GET /api/health is implemented.
// ---------------------------------------------------------------------------

fn health_server_loop(listener: TcpListener, expected_auth: &str, version: &'static str) {
    let Ok((mut stream, _)) = listener.accept() else {
        return;
    };
    drop(listener);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = handle_one_connection(&mut stream, expected_auth, version);
}

fn handle_one_connection(
    stream: &mut TcpStream,
    expected_auth: &str,
    version: &'static str,
) -> bool {
    let Ok(raw) = read_bounded_headers(stream) else {
        send_error(stream, 400);
        return false;
    };
    match validate_health_request(&raw, expected_auth) {
        Ok(()) => {
            send_health_ok(stream, version);
            true
        }
        Err(code) => {
            send_error(stream, code);
            false
        }
    }
}

fn find_crlfcrlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn read_bounded_headers<R: Read>(reader: &mut R) -> Result<Vec<u8>, &'static str> {
    let mut buf = Vec::with_capacity(REQUEST_HEADER_CAP + 1);
    let mut tmp = [0_u8; 512];
    loop {
        if let Some(pos) = find_crlfcrlf(&buf) {
            if pos + 4 > REQUEST_HEADER_CAP {
                return Err("header too large");
            }
            buf.truncate(pos + 4);
            return Ok(buf);
        }
        if buf.len() > REQUEST_HEADER_CAP {
            return Err("header too large");
        }
        if buf.len() == REQUEST_HEADER_CAP + 1 {
            return Err("header too large");
        }
        let remaining = (REQUEST_HEADER_CAP + 1) - buf.len();
        let to_read = remaining.min(tmp.len());
        let n = reader.read(&mut tmp[..to_read]).map_err(|_| "read error")?;
        if n == 0 {
            return Err("closed");
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

fn validate_health_request(raw: &[u8], expected_auth: &str) -> Result<(), u16> {
    // Bound raw header length through CRLFCRLF inclusive.
    if raw.len() > REQUEST_HEADER_CAP {
        return Err(400);
    }
    let text = std::str::from_utf8(raw).map_err(|_| 400_u16)?;
    // Split request line and headers.
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(400_u16)?;
    let mut parts = request_line.split(' ');
    let method = parts.next().ok_or(400_u16)?;
    let path = parts.next().ok_or(400_u16)?;
    let http_version = parts.next().ok_or(400_u16)?;
    if parts.next().is_some() {
        return Err(400);
    }
    if method != "GET" {
        return Err(405);
    }
    if path != "/api/health" {
        return Err(404);
    }
    if http_version != "HTTP/1.1" {
        return Err(400);
    }

    // Synthetic capability checks for expected credential.
    // If expected credential itself is over cap or contains CR/LF, treat as
    // unsatisfiable so no request can be authorized (without panicking).
    let expected_ok = expected_auth.len() <= EXPECTED_AUTH_CAP
        && !expected_auth.contains('\r')
        && !expected_auth.contains('\n');

    let mut auth_value: Option<String> = None;
    let mut content_length_value: Option<String> = None;
    let mut has_transfer_encoding = false;

    for line in lines {
        if line.is_empty() {
            break;
        }
        let colon = line.find(':').ok_or(400_u16)?;
        let name = line[..colon].trim();
        let value = line[colon + 1..].trim();
        if name.is_empty() {
            return Err(400);
        }
        // Header value must not contain CR/LF (already split) and for
        // Authorization must be bounded to EXPECTED_AUTH_CAP.
        let name_lower = name.to_ascii_lowercase();
        if name_lower == "authorization" {
            if auth_value.is_some() {
                return Err(400);
            }
            if value.len() > EXPECTED_AUTH_CAP {
                return Err(400);
            }
            if value.contains('\r') || value.contains('\n') {
                return Err(400);
            }
            auth_value = Some(value.to_owned());
        } else if name_lower == "transfer-encoding" {
            has_transfer_encoding = true;
        } else if name_lower == "content-length" {
            if content_length_value.is_some() {
                return Err(400);
            }
            // Ambiguous / duplicate is already handled; also reject non-numeric.
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(400);
            }
            content_length_value = Some(value.to_owned());
        }
    }

    if has_transfer_encoding {
        return Err(400);
    }
    if let Some(ref v) = content_length_value {
        // Accept absent or one unambiguous zero. Any nonzero is rejected.
        if v != "0" {
            return Err(400);
        }
        // Duplicate already rejected; ambiguous already rejected via numeric check.
    }
    // Authorization is required for health.
    let Some(provided) = auth_value else {
        return Err(401);
    };
    if !expected_ok {
        return Err(401);
    }
    if provided != expected_auth {
        return Err(401);
    }
    Ok(())
}

fn health_body(pid: u32, version: &str) -> String {
    format!(r#"{{"healthy":true,"pid":{pid},"version":"{version}"}}"#)
}

fn health_http_response(pid: u32, version: &str) -> Vec<u8> {
    let body = health_body(pid, version);
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = Vec::with_capacity(header.len() + body.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(body.as_bytes());
    out
}

fn send_health_ok(stream: &mut TcpStream, version: &str) {
    let pid = process::id();
    let bytes = health_http_response(pid, version);
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
}

fn send_error(stream: &mut TcpStream, code: u16) {
    let status = match code {
        401 => "401 Unauthorized",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        _ => "400 Bad Request",
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn is_known_scenario(name: &str) -> bool {
    matches!(
        name,
        "ready_ok"
            | "ready_malformed"
            | "ready_oversized_bounded_reject"
            | "health_version_reject"
            | "hang_until_lifeline"
            | "abrupt_child_exit_nonzero"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::io::Cursor;

    fn valid_health_raw(auth: &str) -> Vec<u8> {
        format!("GET /api/health HTTP/1.1\r\nAuthorization: {auth}\r\nHost: 127.0.0.1\r\n\r\n")
            .into_bytes()
    }

    struct FragmentedReader {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
    }
    impl FragmentedReader {
        fn new(data: Vec<u8>, chunk: usize) -> Self {
            Self {
                data,
                pos: 0,
                chunk,
            }
        }
    }
    impl Read for FragmentedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            let remaining = self.data.len() - self.pos;
            let take = remaining.min(self.chunk).min(buf.len());
            buf[..take].copy_from_slice(&self.data[self.pos..self.pos + take]);
            self.pos += take;
            Ok(take)
        }
    }

    #[test]
    fn scenario_refusal_unknown() {
        assert!(!is_known_scenario("unknown_scenario"));
        assert!(!is_known_scenario(""));
        assert!(!is_known_scenario("READY_OK"));
    }

    #[test]
    fn scenario_refusal_known() {
        for s in [
            "ready_ok",
            "ready_malformed",
            "ready_oversized_bounded_reject",
            "health_version_reject",
            "hang_until_lifeline",
            "abrupt_child_exit_nonzero",
        ] {
            assert!(is_known_scenario(s), "{s} should be known");
        }
    }

    #[test]
    fn readiness_line_is_valid_json_and_url() {
        let line = build_readiness_line(12345);
        assert!(line.len() < READINESS_LIMIT);
        assert!(!line.contains('\n'));
        let v: Value = serde_json::from_str(&line).expect("readiness should be valid JSON");
        let url = v.get("url").and_then(|u| u.as_str()).expect("url field");
        assert!(url.starts_with("http://127.0.0.1:"));
        let port: u16 = url.rsplit(':').next().unwrap().parse().unwrap();
        assert!(port != 0);
    }

    #[test]
    fn readiness_newline_is_single_lf() {
        let line = build_readiness_line(8080);
        let with_nl = format!("{line}\n");
        assert!(with_nl.ends_with('\n'));
        assert!(!with_nl.ends_with("\r\n"));
        assert_eq!(with_nl.matches('\n').count(), 1);
    }

    #[test]
    fn oversized_readiness_is_exactly_257_and_still_valid_json() {
        let line = build_oversized_line();
        assert_eq!(line.len(), OVERSIZED_READY_LEN);
        assert_eq!(OVERSIZED_READY_LEN, READINESS_LIMIT + 1);
        assert!(line.len() > READINESS_LIMIT);
        let v: Value = serde_json::from_str(line.trim()).expect("padded JSON should be valid");
        assert!(v.get("url").is_some());
        // Ensure padding is only JSON whitespace.
        let base = r#"{"url":"http://127.0.0.1:1"}"#;
        assert!(line.starts_with(base));
        assert!(line[base.len()..].chars().all(|c| c == ' '));
    }

    #[test]
    fn oversized_with_newline_is_258_bytes_total() {
        let line = build_oversized_line();
        let with_nl = format!("{line}\n");
        assert_eq!(with_nl.len(), OVERSIZED_READY_LEN + 1);
    }

    #[test]
    fn health_schema_expected_version() {
        let pid = 4242_u32;
        let body = health_body(pid, EXPECTED_HEALTH_VERSION);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v.get("healthy").and_then(Value::as_bool), Some(true));
        assert_eq!(v.get("pid").and_then(Value::as_u64), Some(u64::from(pid)));
        assert_eq!(
            v.get("version").and_then(|x| x.as_str()),
            Some(EXPECTED_HEALTH_VERSION)
        );
    }

    #[test]
    fn health_version_distinction() {
        assert_ne!(EXPECTED_HEALTH_VERSION, INCOMPATIBLE_VERSION);
        let ok_body = health_body(1, EXPECTED_HEALTH_VERSION);
        let bad_body = health_body(1, INCOMPATIBLE_VERSION);
        let ok_v: Value = serde_json::from_str(&ok_body).unwrap();
        let bad_v: Value = serde_json::from_str(&bad_body).unwrap();
        assert_ne!(
            ok_v.get("version").unwrap().as_str().unwrap(),
            bad_v.get("version").unwrap().as_str().unwrap()
        );
    }

    #[test]
    fn valid_health_request_parsing() {
        let auth = "Basic dGVzdDp0b2tlbg==";
        let raw = valid_health_raw(auth);
        assert!(validate_health_request(&raw, auth).is_ok());
    }

    #[test]
    fn valid_health_request_lowercase_header_name() {
        let auth = "Basic abc123";
        let raw =
            format!("GET /api/health HTTP/1.1\r\nauthorization: {auth}\r\nhost: 127.0.0.1\r\n\r\n")
                .into_bytes();
        assert!(validate_health_request(&raw, auth).is_ok());
    }

    #[test]
    fn wrong_route_rejected() {
        let auth = "Basic ok";
        let raw =
            format!("GET /api/session HTTP/1.1\r\nAuthorization: {auth}\r\n\r\n").into_bytes();
        assert_eq!(validate_health_request(&raw, auth).unwrap_err(), 404);
    }

    #[test]
    fn wrong_method_rejected() {
        let auth = "Basic ok";
        let raw =
            format!("POST /api/health HTTP/1.1\r\nAuthorization: {auth}\r\n\r\n").into_bytes();
        assert_eq!(validate_health_request(&raw, auth).unwrap_err(), 405);
    }

    #[test]
    fn missing_auth_rejected() {
        let raw = b"GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_vec();
        assert_eq!(
            validate_health_request(&raw, "Basic secret").unwrap_err(),
            401
        );
    }

    #[test]
    fn wrong_auth_rejected() {
        let raw = valid_health_raw("Basic wrong");
        assert_eq!(
            validate_health_request(&raw, "Basic correct").unwrap_err(),
            401
        );
    }

    #[test]
    fn duplicate_authorization_rejected() {
        let auth = "Basic dup";
        let raw = format!(
            "GET /api/health HTTP/1.1\r\nAuthorization: {auth}\r\nAuthorization: {auth}\r\n\r\n"
        )
        .into_bytes();
        assert_eq!(validate_health_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn transfer_encoding_rejected() {
        let auth = "Basic ok";
        let raw = format!("GET /api/health HTTP/1.1\r\nAuthorization: {auth}\r\nTransfer-Encoding: chunked\r\n\r\n").into_bytes();
        assert_eq!(validate_health_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn transfer_encoding_case_insensitive_rejected() {
        let auth = "Basic ok";
        let raw = format!("GET /api/health HTTP/1.1\r\nAuthorization: {auth}\r\ntransfer-encoding: chunked\r\n\r\n").into_bytes();
        assert_eq!(validate_health_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn duplicate_content_length_rejected() {
        let auth = "Basic ok";
        let raw = format!("GET /api/health HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n").into_bytes();
        assert_eq!(validate_health_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn nonzero_content_length_rejected() {
        let auth = "Basic ok";
        let raw = format!(
            "GET /api/health HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: 5\r\n\r\n"
        )
        .into_bytes();
        assert_eq!(validate_health_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn zero_content_length_accepted() {
        let auth = "Basic ok";
        let raw = format!(
            "GET /api/health HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: 0\r\n\r\n"
        )
        .into_bytes();
        assert!(validate_health_request(&raw, auth).is_ok());
    }

    #[test]
    fn absent_content_length_accepted() {
        let auth = "Basic ok";
        let raw = valid_health_raw(auth);
        assert!(validate_health_request(&raw, auth).is_ok());
    }

    #[test]
    fn authorization_cap_exceeded_rejected() {
        let long_auth = "a".repeat(EXPECTED_AUTH_CAP + 1);
        let raw =
            format!("GET /api/health HTTP/1.1\r\nAuthorization: {long_auth}\r\n\r\n").into_bytes();
        assert_eq!(validate_health_request(&raw, &long_auth).unwrap_err(), 400);
    }

    #[test]
    fn authorization_cap_exact_ok() {
        let exact_auth = "a".repeat(EXPECTED_AUTH_CAP);
        let raw =
            format!("GET /api/health HTTP/1.1\r\nAuthorization: {exact_auth}\r\n\r\n").into_bytes();
        assert!(validate_health_request(&raw, &exact_auth).is_ok());
    }

    #[test]
    fn header_cap_exact_4096_ok() {
        let auth = "Basic ok";
        let base = format!("GET /api/health HTTP/1.1\r\nAuthorization: {auth}\r\n");
        let suffix = "\r\n";
        // Pad with a single large header to exactly REQUEST_HEADER_CAP bytes including CRLFCRLF.
        let filler_len = REQUEST_HEADER_CAP - base.len() - "X-Pad: \r\n".len() - suffix.len();
        let filler = "a".repeat(filler_len);
        let raw = format!("{base}X-Pad: {filler}\r\n\r\n").into_bytes();
        assert_eq!(raw.len(), REQUEST_HEADER_CAP);
        assert!(validate_health_request(&raw, auth).is_ok());
    }

    #[test]
    fn header_cap_one_over_rejected() {
        let auth = "Basic ok";
        let base = format!("GET /api/health HTTP/1.1\r\nAuthorization: {auth}\r\n");
        let suffix = "\r\n";
        let filler_len = REQUEST_HEADER_CAP + 1 - base.len() - "X-Pad: \r\n".len() - suffix.len();
        let filler = "a".repeat(filler_len);
        let raw = format!("{base}X-Pad: {filler}\r\n\r\n").into_bytes();
        assert_eq!(raw.len(), REQUEST_HEADER_CAP + 1);
        // Validation should reject > cap.
        assert_eq!(validate_health_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn read_bounded_headers_short_request_passes() {
        let raw = b"GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_vec();
        let mut cur = Cursor::new(raw.clone());
        let out = read_bounded_headers(&mut cur).unwrap();
        assert_eq!(out, raw);
    }

    #[test]
    fn read_bounded_headers_cap_plus_one_rejects() {
        let mut raw = vec![b'a'; REQUEST_HEADER_CAP + 1];
        raw.extend_from_slice(b"\r\n\r\n");
        let mut cur = Cursor::new(raw);
        assert!(read_bounded_headers(&mut cur).is_err());
        assert_eq!(
            cur.position(),
            u64::try_from(REQUEST_HEADER_CAP + 1).expect("cap fits u64")
        );
    }

    #[test]
    fn read_bounded_headers_fragmented_reads() {
        let raw = valid_health_raw("Basic frag");
        for chunk in [1, 2, 3, 7, 13] {
            let mut frag = FragmentedReader::new(raw.clone(), chunk);
            let out = read_bounded_headers(&mut frag).unwrap();
            assert_eq!(out, raw, "chunk {chunk} should preserve bounded read");
        }
    }

    #[test]
    fn read_bounded_headers_exact_cap_fragmented() {
        let auth = "Basic ok";
        let base = format!("GET /api/health HTTP/1.1\r\nAuthorization: {auth}\r\n");
        let filler_len = REQUEST_HEADER_CAP - base.len() - "X-Pad: \r\n".len() - "\r\n".len();
        let filler = "a".repeat(filler_len);
        let raw = format!("{base}X-Pad: {filler}\r\n\r\n").into_bytes();
        assert_eq!(raw.len(), REQUEST_HEADER_CAP);
        let mut frag = FragmentedReader::new(raw.clone(), 5);
        let out = read_bounded_headers(&mut frag).unwrap();
        assert_eq!(out.len(), REQUEST_HEADER_CAP);
        assert_eq!(out, raw);
    }

    #[test]
    fn health_response_has_exact_content_length() {
        let pid = 12345_u32;
        let bytes = health_http_response(pid, EXPECTED_HEALTH_VERSION);
        let text = String::from_utf8(bytes).unwrap();
        let (header, body) = text.split_once("\r\n\r\n").unwrap();
        let cl_line = header
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
            .unwrap();
        let declared: usize = cl_line.split(':').nth(1).unwrap().trim().parse().unwrap();
        assert_eq!(declared, body.len());
        // Same bytes as server writer produce valid JSON.
        let v: Value = serde_json::from_str(body).unwrap();
        assert_eq!(v.get("healthy").and_then(Value::as_bool), Some(true));
        assert_eq!(v.get("pid").and_then(Value::as_u64), Some(u64::from(pid)));
        assert_eq!(
            v.get("version").and_then(|x| x.as_str()),
            Some(EXPECTED_HEALTH_VERSION)
        );
    }
}
