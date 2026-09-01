//! TEST-ONLY engine-owner protocol child fixture.
//!
//! One ordinary `main` in a `testonly` Bazel `rust_binary`: no libtest
//! harness, no banner, never shipped. It implements eight frozen scenarios:
//! six P0 first-wave readiness/health cases, one finite P4 transport
//! prerequisite `prompt_text_then_terminal` that serves a bounded
//! `GET /api/health` → `POST /api/session/test-session/prompt` →
//! `GET /api/experimental/session/test-session/log?after=0&follow=true` SSE
//! sequence, and one child-custody proof scenario. No product module is
//! imported; the only non-`std` dependency is the pinned
//! `@crates//:serde_json` (1.0.151).
//!
//! Selector is child-only `ARTISAN_ENGINE_OWNER_TEST_SCENARIO` set on the
//! spawned `Command` environment. Missing, non-Unicode or unknown exits 87
//! with no stdout. Parent must never mutate global env and must never read
//! real engine credentials. Health/prompt/log scenarios compare the exact
//! fixture credential from `ARTISAN_ENGINE_OWNER_TEST_AUTHORIZATION` (bounded,
//! CR/LF-free, supplied by the parent) to the request `Authorization`
//! header. This is not shipping auth policy.
//!
//! Main is dedicated to the stdin lifeline: EOF exits 3; any unexpected
//! input byte or read error exits 87. A fixture-local 20s watchdog exits
//! 99 and is always failure. Ordinary scenarios use at most one server
//! thread; the custody proof alone starts one explicit descendant helper and
//! writes one marker file. There is no thread-per-request or recursive
//! discovery. Connections are sequential with `Connection: close`.
//! Blocking accept/read cannot prevent main from observing lifeline EOF. OS
//! sockets/threads die with the fixture process; the parent must observe
//! process custody completion.

use std::io::{BufRead, OpenOptions, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{self, Command, Stdio};
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
/// Child-only marker path for the descendant-custody proof.
const DESCENDANT_SENTINEL_MARKER_ENV: &str = "ARTISAN_ENGINE_OWNER_DESCENDANT_SENTINEL_MARKER";
/// Fixed response served by the descendant helper.
const DESCENDANT_SENTINEL_RESPONSE: &[u8] = b"descendant-alive";

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
/// Prompt JSON body cap (bounded, synthetic).
const PROMPT_BODY_CAP: usize = 4096;
/// Fixed session id for the P4 flow.
const FIXTURE_SESSION_ID: &str = "test-session";
/// Fixed run id for the P4 SSE flow.
#[cfg(test)]
const FIXTURE_RUN_ID: &str = "fixture-run";
/// Watchdog bound.
const WATCHDOG_SECS: u64 = 20;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--descendant-sentinel") {
        run_descendant_sentinel_helper();
    }

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
        "prompt_text_then_terminal" => run_prompt_text_then_terminal(),
        "descendant_holds_sentinel" => run_descendant_holds_sentinel(),
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

fn run_prompt_text_then_terminal() -> ! {
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
        .name("engine-owner-fixture-p4".to_owned())
        .spawn(move || p4_server_loop(&listener, &expected_auth))
        .expect("p4 thread should spawn");
    wait_for_lifeline_eof();
}

fn run_descendant_holds_sentinel() -> ! {
    let expected_auth = get_expected_auth_or_exit();
    let marker_path = match std::env::var_os(DESCENDANT_SENTINEL_MARKER_ENV) {
        Some(path) if !path.is_empty() => path,
        _ => process::exit(SCENARIO_REFUSED_EXIT),
    };
    let executable = std::env::current_exe().expect("fixture executable path");
    let mut helper_command = Command::new(executable);
    helper_command
        .env_clear()
        .arg("--descendant-sentinel")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SYSTEMROOT") {
        helper_command.env("SYSTEMROOT", system_root);
    }
    let mut helper = helper_command
        .spawn()
        .expect("descendant helper should spawn");
    let helper_stdout = helper
        .stdout
        .take()
        .expect("descendant helper stdout should be piped");
    let mut helper_ready = String::new();
    std::io::BufReader::new(helper_stdout)
        .read_line(&mut helper_ready)
        .expect("descendant helper readiness should be readable");
    let port = parse_descendant_ready(&helper_ready)
        .expect("descendant helper readiness should be READY <port>");

    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker_path)
        .expect("descendant marker should be created once");
    writeln!(marker, "{port}").expect("descendant marker should be written");
    marker
        .sync_all()
        .expect("descendant marker should be durable");
    drop(marker);

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
        .name("engine-owner-fixture-descendant-proof".to_owned())
        .spawn(move || p4_server_loop(&listener, &expected_auth))
        .expect("descendant proof server thread should spawn");
    wait_for_lifeline_eof();
}

fn parse_descendant_ready(line: &str) -> Option<u16> {
    let port_text = line.strip_prefix("READY ")?.strip_suffix('\n')?;
    let port_text = port_text.strip_suffix('\r').unwrap_or(port_text);
    let port = port_text.parse().ok()?;
    (port != 0).then_some(port)
}

fn run_descendant_sentinel_helper() -> ! {
    let listener = TcpListener::bind("127.0.0.1:0").expect("descendant bind");
    let port = listener.local_addr().expect("descendant local_addr").port();
    assert!(port != 0, "descendant port must be nonzero");
    {
        let mut out = std::io::stdout().lock();
        writeln!(out, "READY {port}").expect("descendant readiness write");
        out.flush().expect("descendant readiness flush");
    }
    loop {
        let Ok((mut stream, _)) = listener.accept() else {
            process::exit(SCENARIO_REFUSED_EXIT);
        };
        let _ = stream.write_all(DESCENDANT_SENTINEL_RESPONSE);
        let _ = stream.flush();
    }
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
    let (headers, _) = read_bounded_headers_with_remainder(reader)?;
    Ok(headers)
}

fn read_bounded_headers_with_remainder<R: Read>(
    reader: &mut R,
) -> Result<(Vec<u8>, Vec<u8>), &'static str> {
    let mut buf = Vec::with_capacity(REQUEST_HEADER_CAP + 1);
    let mut tmp = [0_u8; 512];
    loop {
        if let Some(pos) = find_crlfcrlf(&buf) {
            let header_end = pos + 4;
            if header_end > REQUEST_HEADER_CAP {
                return Err("header too large");
            }
            let remainder = buf.split_off(header_end);
            return Ok((buf, remainder));
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

// ---------------------------------------------------------------------------
// Strict header parsing shared by health / prompt / log.
// ---------------------------------------------------------------------------

struct CommonHeaders {
    method: String,
    path: String,
    version: String,
    content_length: Option<usize>,
}

fn parse_common_headers(raw: &[u8], expected_auth: &str) -> Result<CommonHeaders, u16> {
    if raw.len() > REQUEST_HEADER_CAP {
        return Err(400);
    }
    let text = std::str::from_utf8(raw).map_err(|_| 400_u16)?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(400_u16)?;
    let mut parts = request_line.split(' ');
    let method = parts.next().ok_or(400_u16)?;
    let path = parts.next().ok_or(400_u16)?;
    let version = parts.next().ok_or(400_u16)?;
    if parts.next().is_some() {
        return Err(400);
    }
    if version != "HTTP/1.1" {
        return Err(400);
    }
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
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(400);
            }
            content_length_value = Some(value.to_owned());
        }
    }
    if has_transfer_encoding {
        return Err(400);
    }
    let content_length = if let Some(v) = content_length_value {
        let n: usize = v.parse().map_err(|_| 400_u16)?;
        Some(n)
    } else {
        None
    };
    let Some(provided) = auth_value else {
        return Err(401);
    };
    if !expected_ok {
        return Err(401);
    }
    if provided != expected_auth {
        return Err(401);
    }
    Ok(CommonHeaders {
        method: method.to_owned(),
        path: path.to_owned(),
        version: version.to_owned(),
        content_length,
    })
}

fn validate_health_request(raw: &[u8], expected_auth: &str) -> Result<(), u16> {
    let headers = parse_common_headers(raw, expected_auth)?;
    if headers.method != "GET" {
        return Err(405);
    }
    if headers.path != "/api/health" {
        return Err(404);
    }
    if headers.version != "HTTP/1.1" {
        return Err(400);
    }
    if headers.content_length.is_some_and(|n| n != 0) {
        return Err(400);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// P4 prompt + SSE helpers
// ---------------------------------------------------------------------------

fn prompt_route() -> String {
    format!("/api/session/{FIXTURE_SESSION_ID}/prompt")
}

fn log_route() -> String {
    format!("/api/experimental/session/{FIXTURE_SESSION_ID}/log?after=0&follow=true")
}

fn prompt_success_body() -> String {
    r#"{"ok":true}"#.to_owned()
}

fn create_session_route() -> String {
    "/api/session".to_owned()
}

fn create_session_success_body() -> String {
    r#"{"id":"test-session"}"#.to_owned()
}

fn create_session_http_response() -> Vec<u8> {
    let body = create_session_success_body();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = Vec::with_capacity(header.len() + body.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(body.as_bytes());
    out
}

fn send_create_session_ok(stream: &mut TcpStream) {
    let bytes = create_session_http_response();
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
}

fn sse_body_bytes() -> Vec<u8> {
    // Deterministic SSE stream:
    // : keepalive (comment)
    // blank
    // multiline data event (sequence 1) split across two data lines
    // blank
    // terminal succeeded event (sequence 2)
    // blank
    // The two data lines join with \n to valid JSON with run_id fixture-run
    // and session_id test-session (required by configured turn's
    // decode_sse_event_for_run which enforces both identities).
    let mut s = String::new();
    s.push_str(": keepalive\n");
    s.push('\n');
    s.push_str(
        "data: {\"run_id\":\"fixture-run\",\"session_id\":\"test-session\",\"sequence\":1,\n",
    );
    s.push_str("data: \"delta\":\"hello world\"}\n");
    s.push('\n');
    s.push_str("data: {\"run_id\":\"fixture-run\",\"session_id\":\"test-session\",\"sequence\":2,\"state\":\"succeeded\"}\n");
    s.push('\n');
    s.into_bytes()
}

fn prompt_http_response() -> Vec<u8> {
    let body = prompt_success_body();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = Vec::with_capacity(header.len() + body.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(body.as_bytes());
    out
}

fn sse_http_response() -> Vec<u8> {
    let body = sse_body_bytes();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = Vec::with_capacity(header.len() + body.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&body);
    out
}

fn send_prompt_ok(stream: &mut TcpStream) {
    let bytes = prompt_http_response();
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
}

fn send_sse_ok(stream: &mut TcpStream) {
    let bytes = sse_http_response();
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
}

#[cfg(test)]
fn read_exact_body<R: Read>(reader: &mut R, declared: usize, cap: usize) -> Result<Vec<u8>, u16> {
    read_exact_body_with_prefix(reader, declared, cap, Vec::new())
}

fn read_exact_body_with_prefix<R: Read>(
    reader: &mut R,
    declared: usize,
    cap: usize,
    mut buf: Vec<u8>,
) -> Result<Vec<u8>, u16> {
    if declared > cap || buf.len() > declared {
        return Err(400);
    }
    buf.reserve(declared - buf.len());
    let mut tmp = [0_u8; 512];
    let mut remaining = declared - buf.len();
    while remaining > 0 {
        let to_read = remaining.min(tmp.len());
        let n = reader.read(&mut tmp[..to_read]).map_err(|_| 400_u16)?;
        if n == 0 {
            return Err(400);
        }
        buf.extend_from_slice(&tmp[..n]);
        remaining -= n;
        if buf.len() > cap {
            return Err(400);
        }
    }
    Ok(buf)
}

fn validate_prompt_json(body: &[u8]) -> Result<(), u16> {
    if body.len() > PROMPT_BODY_CAP {
        return Err(400);
    }
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| 400_u16)?;
    let obj = v.as_object().ok_or(400_u16)?;
    let delivery = obj.get("delivery").ok_or(400_u16)?;
    if !delivery.is_string() {
        return Err(400);
    }
    let files = obj.get("files").ok_or(400_u16)?;
    let arr = files.as_array().ok_or(400_u16)?;
    for entry in arr {
        let eobj = entry.as_object().ok_or(400_u16)?;
        let uri = eobj.get("uri").ok_or(400_u16)?;
        if !uri.is_string() {
            return Err(400);
        }
        let name = eobj.get("name").ok_or(400_u16)?;
        if !name.is_string() {
            return Err(400);
        }
    }
    let id = obj.get("id").ok_or(400_u16)?;
    if !id.is_string() {
        return Err(400);
    }
    let resume = obj.get("resume").ok_or(400_u16)?;
    if !resume.is_boolean() {
        return Err(400);
    }
    let text = obj.get("text").ok_or(400_u16)?;
    let s = text.as_str().ok_or(400_u16)?;
    if s.is_empty() {
        return Err(400);
    }
    Ok(())
}

fn validate_prompt_request(raw: &[u8], expected_auth: &str) -> Result<usize, u16> {
    let headers = parse_common_headers(raw, expected_auth)?;
    if headers.method != "POST" {
        return Err(405);
    }
    if headers.path != prompt_route() {
        return Err(404);
    }
    if headers.version != "HTTP/1.1" {
        return Err(400);
    }
    let Some(n) = headers.content_length else {
        return Err(400);
    };
    if n > PROMPT_BODY_CAP {
        return Err(400);
    }
    if n == 0 {
        return Err(400);
    }
    Ok(n)
}

fn validate_create_session_request(raw: &[u8], expected_auth: &str) -> Result<usize, u16> {
    let headers = parse_common_headers(raw, expected_auth)?;
    if headers.method != "POST" {
        return Err(405);
    }
    if headers.path != create_session_route() {
        return Err(404);
    }
    if headers.version != "HTTP/1.1" {
        return Err(400);
    }
    let Some(n) = headers.content_length else {
        return Err(400);
    };
    if n > PROMPT_BODY_CAP {
        return Err(400);
    }
    if n == 0 {
        return Err(400);
    }
    Ok(n)
}

fn validate_create_session_json(body: &[u8]) -> Result<(), u16> {
    if body.len() > PROMPT_BODY_CAP {
        return Err(400);
    }
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| 400_u16)?;
    let obj = v.as_object().ok_or(400_u16)?;
    // Minimal check: must contain profile/model/permission like the real owner.
    // Accept any that is a valid JSON object with at least one key; strict
    // shape is not needed for the fixture seam but we ensure it's not empty.
    if obj.is_empty() {
        return Err(400);
    }
    Ok(())
}

fn validate_log_request(raw: &[u8], expected_auth: &str) -> Result<(), u16> {
    let headers = parse_common_headers(raw, expected_auth)?;
    if headers.method != "GET" {
        return Err(405);
    }
    if headers.path != log_route() {
        return Err(404);
    }
    if headers.version != "HTTP/1.1" {
        return Err(400);
    }
    if headers.content_length.is_some_and(|n| n != 0) {
        return Err(400);
    }
    Ok(())
}

fn p4_server_loop(listener: &TcpListener, expected_auth: &str) {
    if !serve_p4_health_phase(listener, expected_auth) {
        return;
    }
    if !serve_p4_prompt_phase(listener, expected_auth) {
        return;
    }
    serve_p4_log_phase(listener, expected_auth);
}

fn serve_p4_health_phase(listener: &TcpListener, expected_auth: &str) -> bool {
    // Step 1: GET /api/health
    let Ok((mut stream, _)) = listener.accept() else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(raw) = read_bounded_headers(&mut stream) else {
        send_error(&mut stream, 400);
        return false;
    };
    match validate_health_request(&raw, expected_auth) {
        Ok(()) => send_health_ok(&mut stream, EXPECTED_HEALTH_VERSION),
        Err(code) => {
            send_error(&mut stream, code);
            return false;
        }
    }
    drop(stream);
    true
}

fn serve_p4_prompt_phase(listener: &TcpListener, expected_auth: &str) -> bool {
    // Step 2 is flexible: either POST /api/session (configured create) or
    // POST /api/session/test-session/prompt (smoke direct). The smoke harness
    // historically sent prompt immediately after health, so we accept either to
    // keep the fixture backward compatible.
    let Ok((mut stream2, _)) = listener.accept() else {
        return false;
    };
    let _ = stream2.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok((raw2, body_prefix2)) = read_bounded_headers_with_remainder(&mut stream2) else {
        send_error(&mut stream2, 400);
        return false;
    };

    // Try create-session first.
    if let Ok(declared_create) = validate_create_session_request(&raw2, expected_auth) {
        if !serve_p4_create_session(&mut stream2, declared_create, body_prefix2) {
            return false;
        }
        drop(stream2);

        // Step 3: POST /api/session/test-session/prompt (after create)
        serve_p4_prompt_after_create(listener, expected_auth)
    } else if let Ok(declared_prompt) = validate_prompt_request(&raw2, expected_auth) {
        // Smoke path: prompt directly without prior create.
        if !serve_p4_prompt_body(&mut stream2, declared_prompt, body_prefix2) {
            return false;
        }
        drop(stream2);
        true
    } else {
        // Neither create nor prompt matches.
        send_error(&mut stream2, 400);
        false
    }
}

fn serve_p4_create_session(stream: &mut TcpStream, declared: usize, body_prefix: Vec<u8>) -> bool {
    let body = match read_exact_body_with_prefix(stream, declared, PROMPT_BODY_CAP, body_prefix) {
        Ok(body) => body,
        Err(code) => {
            send_error(stream, code);
            return false;
        }
    };
    if validate_create_session_json(&body).is_err() {
        send_error(stream, 400);
        return false;
    }
    send_create_session_ok(stream);
    true
}

fn serve_p4_prompt_after_create(listener: &TcpListener, expected_auth: &str) -> bool {
    let Ok((mut stream, _)) = listener.accept() else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok((raw, body_prefix)) = read_bounded_headers_with_remainder(&mut stream) else {
        send_error(&mut stream, 400);
        return false;
    };
    let declared = match validate_prompt_request(&raw, expected_auth) {
        Ok(n) => n,
        Err(code) => {
            send_error(&mut stream, code);
            return false;
        }
    };
    if !serve_p4_prompt_body(&mut stream, declared, body_prefix) {
        return false;
    }
    drop(stream);
    true
}

fn serve_p4_prompt_body(stream: &mut TcpStream, declared: usize, body_prefix: Vec<u8>) -> bool {
    let body = match read_exact_body_with_prefix(stream, declared, PROMPT_BODY_CAP, body_prefix) {
        Ok(body) => body,
        Err(code) => {
            send_error(stream, code);
            return false;
        }
    };
    if validate_prompt_json(&body).is_err() {
        send_error(stream, 400);
        return false;
    }
    send_prompt_ok(stream);
    true
}

fn serve_p4_log_phase(listener: &TcpListener, expected_auth: &str) {
    // Next: GET /api/experimental/session/test-session/log?after=0&follow=true
    let Ok((mut stream3, _)) = listener.accept() else {
        return;
    };
    let _ = stream3.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(raw3) = read_bounded_headers(&mut stream3) else {
        send_error(&mut stream3, 400);
        return;
    };
    match validate_log_request(&raw3, expected_auth) {
        Ok(()) => send_sse_ok(&mut stream3),
        Err(code) => send_error(&mut stream3, code),
    }
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
            | "prompt_text_then_terminal"
            | "descendant_holds_sentinel"
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

    fn valid_prompt_body() -> Vec<u8> {
        serde_json::json!({
            "delivery": "test",
            "files": [{"uri": "file:///tmp/a.txt", "name": "a.txt"}],
            "id": "fixture-run",
            "resume": false,
            "text": "hello"
        })
        .to_string()
        .into_bytes()
    }

    fn valid_prompt_raw(auth: &str, body: &[u8]) -> Vec<u8> {
        format!(
            "POST {path} HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: {}\r\nHost: 127.0.0.1\r\n\r\n",
            body.len(),
            path = prompt_route()
        )
        .into_bytes()
        .into_iter()
        .chain(body.iter().copied())
        .collect()
    }

    fn valid_log_raw(auth: &str) -> Vec<u8> {
        format!(
            "GET {} HTTP/1.1\r\nAuthorization: {auth}\r\nHost: 127.0.0.1\r\n\r\n",
            log_route()
        )
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
            "prompt_text_then_terminal",
            "descendant_holds_sentinel",
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

    // -----------------------------------------------------------------------
    // P4 prompt + SSE tests
    // -----------------------------------------------------------------------

    #[test]
    fn valid_prompt_headers_and_body_accepted() {
        let auth = "Basic ok";
        let body = valid_prompt_body();
        let header = format!(
            "POST {} HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: {}\r\nHost: 127.0.0.1\r\n\r\n",
            prompt_route(),
            body.len()
        )
        .into_bytes();
        // Header validation returns declared length.
        let declared = validate_prompt_request(&header, auth).unwrap();
        assert_eq!(declared, body.len());
        // Body parsing consumes exactly declared bytes.
        let mut cur = Cursor::new(body.clone());
        let out = read_exact_body(&mut cur, declared, PROMPT_BODY_CAP).unwrap();
        assert_eq!(out, body);
        assert!(validate_prompt_json(&out).is_ok());
    }

    #[test]
    fn prompt_headers_and_body_in_same_read_pass() {
        let auth = "Basic ok";
        let body = valid_prompt_body();
        let mut cur = Cursor::new(valid_prompt_raw(auth, &body));
        let (headers, body_prefix) = read_bounded_headers_with_remainder(&mut cur).unwrap();
        let declared = validate_prompt_request(&headers, auth).unwrap();
        let consumed =
            read_exact_body_with_prefix(&mut cur, declared, PROMPT_BODY_CAP, body_prefix).unwrap();
        assert_eq!(consumed, body);
        assert!(validate_prompt_json(&consumed).is_ok());
    }

    #[test]
    fn valid_prompt_headers_lowercase_auth() {
        let auth = "Basic ok";
        let body = valid_prompt_body();
        let raw = format!(
            "POST {} HTTP/1.1\r\nauthorization: {auth}\r\ncontent-length: {}\r\n\r\n",
            prompt_route(),
            body.len()
        )
        .into_bytes();
        let declared = validate_prompt_request(&raw, auth).unwrap();
        assert_eq!(declared, body.len());
    }

    #[test]
    fn prompt_wrong_auth_rejected() {
        let body = valid_prompt_body();
        let header = format!(
            "POST {} HTTP/1.1\r\nAuthorization: Basic wrong\r\nContent-Length: {}\r\n\r\n",
            prompt_route(),
            body.len()
        )
        .into_bytes();
        assert_eq!(
            validate_prompt_request(&header, "Basic correct").unwrap_err(),
            401
        );
    }

    #[test]
    fn prompt_wrong_method_rejected() {
        let auth = "Basic ok";
        let body = valid_prompt_body();
        let raw = format!(
            "GET {} HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: {}\r\n\r\n",
            prompt_route(),
            body.len()
        )
        .into_bytes();
        assert_eq!(validate_prompt_request(&raw, auth).unwrap_err(), 405);
    }

    #[test]
    fn prompt_wrong_path_rejected() {
        let auth = "Basic ok";
        let body = valid_prompt_body();
        let raw = format!(
            "POST /api/session/wrong/prompt HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        assert_eq!(validate_prompt_request(&raw, auth).unwrap_err(), 404);
    }

    #[test]
    fn prompt_wrong_version_rejected() {
        let auth = "Basic ok";
        let body = valid_prompt_body();
        let raw = format!(
            "POST {} HTTP/1.0\r\nAuthorization: {auth}\r\nContent-Length: {}\r\n\r\n",
            prompt_route(),
            body.len()
        )
        .into_bytes();
        assert_eq!(validate_prompt_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn prompt_transfer_encoding_rejected() {
        let auth = "Basic ok";
        let body = valid_prompt_body();
        let raw = format!(
            "POST {} HTTP/1.1\r\nAuthorization: {auth}\r\nTransfer-Encoding: chunked\r\nContent-Length: {}\r\n\r\n",
            prompt_route(),
            body.len()
        )
        .into_bytes();
        assert_eq!(validate_prompt_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn prompt_duplicate_content_length_rejected() {
        let auth = "Basic ok";
        let raw = format!(
            "POST {} HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\n",
            prompt_route()
        )
        .into_bytes();
        assert_eq!(validate_prompt_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn prompt_invalid_content_length_rejected() {
        let auth = "Basic ok";
        let raw = format!(
            "POST {} HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: abc\r\n\r\n",
            prompt_route()
        )
        .into_bytes();
        assert_eq!(validate_prompt_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn prompt_missing_content_length_rejected() {
        let auth = "Basic ok";
        let raw = format!(
            "POST {} HTTP/1.1\r\nAuthorization: {auth}\r\n\r\n",
            prompt_route()
        )
        .into_bytes();
        assert_eq!(validate_prompt_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn prompt_over_cap_rejected() {
        let auth = "Basic ok";
        let big = PROMPT_BODY_CAP + 1;
        let raw = format!(
            "POST {} HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: {big}\r\n\r\n",
            prompt_route()
        )
        .into_bytes();
        assert_eq!(validate_prompt_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn prompt_truncated_body_rejected() {
        let body = valid_prompt_body();
        let declared = body.len();
        // Provide only declared-1 bytes.
        let mut truncated = body.clone();
        truncated.pop();
        let mut cur = Cursor::new(truncated);
        assert_eq!(
            read_exact_body(&mut cur, declared, PROMPT_BODY_CAP).unwrap_err(),
            400
        );
    }

    #[test]
    fn prompt_body_over_cap_rejected_via_read() {
        let big = vec![b'a'; PROMPT_BODY_CAP + 1];
        let mut cur = Cursor::new(big);
        assert_eq!(
            read_exact_body(&mut cur, PROMPT_BODY_CAP + 1, PROMPT_BODY_CAP).unwrap_err(),
            400
        );
    }

    #[test]
    fn prompt_malformed_json_rejected() {
        let bad = b"{not json}".to_vec();
        assert_eq!(validate_prompt_json(&bad).unwrap_err(), 400);
    }

    #[test]
    fn prompt_missing_field_rejected() {
        let v = serde_json::json!({
            "delivery": "x",
            "files": [],
            "id": "a",
            "resume": false
        });
        assert_eq!(
            validate_prompt_json(&v.to_string().into_bytes()).unwrap_err(),
            400
        );
    }

    #[test]
    fn prompt_wrong_field_types_rejected() {
        // delivery not string
        let v1 = serde_json::json!({"delivery": 123, "files": [], "id": "a", "resume": false, "text": "hi"});
        assert_eq!(
            validate_prompt_json(&v1.to_string().into_bytes()).unwrap_err(),
            400
        );
        // files not array
        let v2 = serde_json::json!({"delivery": "x", "files": {}, "id": "a", "resume": false, "text": "hi"});
        assert_eq!(
            validate_prompt_json(&v2.to_string().into_bytes()).unwrap_err(),
            400
        );
        // files entry missing name
        let v3 = serde_json::json!({"delivery": "x", "files": [{"uri": "u"}], "id": "a", "resume": false, "text": "hi"});
        assert_eq!(
            validate_prompt_json(&v3.to_string().into_bytes()).unwrap_err(),
            400
        );
        // id not string
        let v4 = serde_json::json!({"delivery": "x", "files": [], "id": 1, "resume": false, "text": "hi"});
        assert_eq!(
            validate_prompt_json(&v4.to_string().into_bytes()).unwrap_err(),
            400
        );
        // resume not bool
        let v5 = serde_json::json!({"delivery": "x", "files": [], "id": "a", "resume": "false", "text": "hi"});
        assert_eq!(
            validate_prompt_json(&v5.to_string().into_bytes()).unwrap_err(),
            400
        );
        // text not string
        let v6 = serde_json::json!({"delivery": "x", "files": [], "id": "a", "resume": false, "text": 123});
        assert_eq!(
            validate_prompt_json(&v6.to_string().into_bytes()).unwrap_err(),
            400
        );
        // text empty
        let v7 = serde_json::json!({"delivery": "x", "files": [], "id": "a", "resume": false, "text": ""});
        assert_eq!(
            validate_prompt_json(&v7.to_string().into_bytes()).unwrap_err(),
            400
        );
        // files entry uri not string
        let v8 = serde_json::json!({"delivery": "x", "files": [{"uri": 1, "name": "n"}], "id": "a", "resume": false, "text": "hi"});
        assert_eq!(
            validate_prompt_json(&v8.to_string().into_bytes()).unwrap_err(),
            400
        );
    }

    #[test]
    fn prompt_success_response_exact_length_and_json() {
        let bytes = prompt_http_response();
        let text = String::from_utf8(bytes).unwrap();
        let (header, body) = text.split_once("\r\n\r\n").unwrap();
        let cl_line = header
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
            .unwrap();
        let declared: usize = cl_line.split(':').nth(1).unwrap().trim().parse().unwrap();
        assert_eq!(declared, body.len());
        let v: Value = serde_json::from_str(body).unwrap();
        assert_eq!(v.get("ok").and_then(Value::as_bool), Some(true));
        assert!(
            header
                .to_ascii_lowercase()
                .contains("content-type: application/json")
        );
        assert!(header.to_ascii_lowercase().contains("connection: close"));
    }

    #[test]
    fn sse_response_exact_length_content_type_and_events() {
        let bytes = sse_http_response();
        let text = String::from_utf8(bytes.clone()).unwrap();
        let (header, body) = text.split_once("\r\n\r\n").unwrap();
        let cl_line = header
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
            .unwrap();
        let declared: usize = cl_line.split(':').nth(1).unwrap().trim().parse().unwrap();
        assert_eq!(declared, body.len());
        assert!(
            header
                .to_ascii_lowercase()
                .contains("content-type: text/event-stream")
        );
        assert!(header.to_ascii_lowercase().contains("connection: close"));
        // Body must start with comment.
        assert!(body.starts_with(": keepalive\n"));
        // Exactly two event boundaries = two blank-line-terminated data events.
        // Count dispatched events by splitting on \n\n and filtering for data:.
        let events: Vec<&str> = body.split("\n\n").filter(|s| s.contains("data:")).collect();
        assert_eq!(events.len(), 2, "should have two data events");
        // First event must be multiline data (two data: lines).
        let first = events[0];
        let first_data_lines: Vec<&str> =
            first.lines().filter(|l| l.starts_with("data:")).collect();
        assert_eq!(first_data_lines.len(), 2, "first event should be multiline");
        // Second event single data line.
        let second = events[1];
        let second_data_lines: Vec<&str> =
            second.lines().filter(|l| l.starts_with("data:")).collect();
        assert_eq!(second_data_lines.len(), 1);
        // Extract JSON by joining multiline data with \n.
        let first_json = first_data_lines
            .iter()
            .map(|l| {
                l.strip_prefix("data: ")
                    .unwrap_or(l.strip_prefix("data:").unwrap_or(""))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let second_json = second_data_lines
            .iter()
            .map(|l| {
                l.strip_prefix("data: ")
                    .unwrap_or(l.strip_prefix("data:").unwrap_or(""))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let v1: Value = serde_json::from_str(&first_json).unwrap();
        let v2: Value = serde_json::from_str(&second_json).unwrap();
        // Stable run id and sequences.
        assert_eq!(
            v1.get("run_id").and_then(Value::as_str),
            Some(FIXTURE_RUN_ID)
        );
        assert_eq!(
            v2.get("run_id").and_then(Value::as_str),
            Some(FIXTURE_RUN_ID)
        );
        assert_eq!(v1.get("sequence").and_then(Value::as_u64), Some(1));
        assert_eq!(v2.get("sequence").and_then(Value::as_u64), Some(2));
        // Distinct payloads: first contains delta text, second terminal succeeded.
        assert!(v1.get("delta").and_then(Value::as_str).is_some());
        assert_eq!(v2.get("state").and_then(Value::as_str), Some("succeeded"));
        assert_ne!(first_json, second_json);
    }

    #[test]
    fn valid_log_request_accepts_exact_route() {
        let auth = "Basic ok";
        let raw = valid_log_raw(auth);
        assert!(validate_log_request(&raw, auth).is_ok());
    }

    #[test]
    fn log_wrong_route_rejected() {
        let auth = "Basic ok";
        let raw = format!(
            "GET /api/experimental/session/{FIXTURE_SESSION_ID}/log?after=1&follow=true HTTP/1.1\r\nAuthorization: {auth}\r\n\r\n"
        )
        .into_bytes();
        assert_eq!(validate_log_request(&raw, auth).unwrap_err(), 404);
        let raw2 = format!(
            "GET /api/experimental/session/wrong/log?after=0&follow=true HTTP/1.1\r\nAuthorization: {auth}\r\n\r\n"
        )
        .into_bytes();
        assert_eq!(validate_log_request(&raw2, auth).unwrap_err(), 404);
    }

    #[test]
    fn log_wrong_method_rejected() {
        let auth = "Basic ok";
        let raw = format!(
            "POST {} HTTP/1.1\r\nAuthorization: {auth}\r\n\r\n",
            log_route()
        )
        .into_bytes();
        assert_eq!(validate_log_request(&raw, auth).unwrap_err(), 405);
    }

    #[test]
    fn log_wrong_auth_rejected() {
        let raw = valid_log_raw("Basic wrong");
        assert_eq!(
            validate_log_request(&raw, "Basic correct").unwrap_err(),
            401
        );
    }

    #[test]
    fn log_transfer_encoding_rejected() {
        let auth = "Basic ok";
        let raw = format!(
            "GET {} HTTP/1.1\r\nAuthorization: {auth}\r\nTransfer-Encoding: chunked\r\n\r\n",
            log_route()
        )
        .into_bytes();
        assert_eq!(validate_log_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn log_duplicate_content_length_rejected() {
        let auth = "Basic ok";
        let raw = format!(
            "GET {} HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
            log_route()
        )
        .into_bytes();
        assert_eq!(validate_log_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn log_nonzero_content_length_rejected() {
        let auth = "Basic ok";
        let raw = format!(
            "GET {} HTTP/1.1\r\nAuthorization: {auth}\r\nContent-Length: 5\r\n\r\n",
            log_route()
        )
        .into_bytes();
        assert_eq!(validate_log_request(&raw, auth).unwrap_err(), 400);
    }

    #[test]
    fn prompt_trailing_bytes_rejected() {
        let auth = "Basic ok";
        let body = valid_prompt_body();
        let mut with_trailing = valid_prompt_raw(auth, &body);
        with_trailing.extend_from_slice(b"TRAILING");
        let mut cur = Cursor::new(with_trailing);
        let (headers, body_prefix) = read_bounded_headers_with_remainder(&mut cur).unwrap();
        let declared = validate_prompt_request(&headers, auth).unwrap();
        assert_eq!(
            read_exact_body_with_prefix(&mut cur, declared, PROMPT_BODY_CAP, body_prefix)
                .unwrap_err(),
            400
        );
    }
}
