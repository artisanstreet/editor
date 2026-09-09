//! Fixture-only coverage for the native engine-usage packet.
//!
//! Cursor mapping (split/single plan pools, on-demand spend, display-message
//! fallback, 401/403 auth states) runs against fixture JSON and a loopback
//! HTTP server; Codex and Claude spawn custody (happy paths, login-gated,
//! malformed, oversize, hang/timeout, exit-status) runs against fixture
//! children re-executed from this same test binary; the freshness service
//! (TTL, force refresh, narrowing, failure isolation, per-engine timeout,
//! unsupported surfaces) runs against scripted readers; and the Forge query
//! arm runs against real migrated storage. No real credentials, providers,
//! or network endpoints are touched.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use artisan_backend::account_usage_cursor::{
    CursorEndpoint, CursorUsageConfig, CursorUsageError, map_cursor_period_usage,
    read_cursor_access_token, read_cursor_usage,
};
use artisan_backend::account_usage_service::{
    ACCOUNT_USAGE_FRESHNESS, AccountUsageReader, AccountUsageService, ReaderFailure,
    UnsupportedAccountUsageReader,
};
use artisan_backend::{ForgeStorage, RequestHandler};
use artisan_database::SqliteConfig;
use artisan_domain::Query;
use artisan_domain::{
    EngineUsageAuthentication, EngineUsageWindow, EngineUsageWindowKind, QuotaSurface,
    ReadAccountUsage, RequestId, iso_millis, validate_iso_timestamp,
};
use artisan_native_engine::account_usage::{ProviderUsage, UsageReaderError};
use artisan_native_engine::{
    ClaudeUsageConfig, CliResolveInput, CodexUsageConfig, resolve_cli_with,
};
use artisan_protocol::{ClientRequest, ErrorCode, ResponsePayload};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

/// Fixture-child selector; set only on spawned children.
const CHILD_MODE_ENV: &str = "ARTISAN_ENGINE_USAGE_FIXTURE_MODE";
/// Exact libtest filter selecting the fixture child branch below.
const FIXTURE_TEST_NAME: &str = "engine_usage_fixture_child";
/// Backstop exit when a fixture child outlives its budget.
const CHILD_WATCHDOG_EXIT: i32 = 88;
/// Fixture child failure exit.
const CHILD_FAILURE_EXIT: i32 = 86;
/// Child-local containment budget.
const CHILD_WATCHDOG_BUDGET: Duration = Duration::from_secs(20);

fn child_watchdog() {
    std::thread::Builder::new()
        .name("engine-usage-fixture-watchdog".to_owned())
        .spawn(|| {
            std::thread::sleep(CHILD_WATCHDOG_BUDGET);
            process::exit(CHILD_WATCHDOG_EXIT);
        })
        .expect("fixture watchdog should spawn");
}

fn child_respond(id: &serde_json::Value, result: serde_json::Value) {
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{}",
        serde_json::json!({"id": id, "result": result})
    )
    .expect("fixture response should write");
    stdout.flush().expect("fixture response should flush");
}

fn child_error(id: &serde_json::Value, message: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{}",
        serde_json::json!({"id": id, "error": {"code": -32001, "message": message}})
    )
    .expect("fixture error should write");
    stdout.flush().expect("fixture error should flush");
}

fn codex_rate_limits_fixture() -> serde_json::Value {
    serde_json::json!({
        "rateLimitsByLimitId": {
            "codex": {
                "limitId": "codex",
                "primary": {"resetsAt": 1_788_955_200, "usedPercent": 42.5, "windowDurationMins": 300},
                "secondary": null
            }
        }
    })
}

fn run_codex_child(mode: &str) -> ! {
    child_watchdog();
    if mode == "codex-inherit" {
        spawn_pipe_holder("pipe-holder");
    }
    if mode == "codex-inherit-stuck" {
        spawn_pipe_holder("pipe-holder-long");
    }
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.unwrap_or_default();
        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(_) => continue,
        };
        let id = request
            .get("id")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if id.is_null() {
            continue;
        }
        let method = request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        match (mode, method) {
            (_, "initialize") => child_respond(&id, serde_json::json!({"capabilities": {}})),
            (_, "account/read") => match mode {
                "codex-happy" => child_respond(
                    &id,
                    serde_json::json!({
                        "account": {"type": "chatgpt", "email": "owner@example.test", "planType": "plus"},
                        "requiresOpenaiAuth": false,
                    }),
                ),
                "codex-login-error" => {
                    child_error(&id, "Not logged in. Run `codex login` to continue.");
                }
                "codex-shape-error" => child_respond(
                    &id,
                    serde_json::json!({"account": 42, "requiresOpenaiAuth": false}),
                ),
                "codex-garbage" => {
                    let mut stdout = std::io::stdout().lock();
                    writeln!(stdout, "this is not json").expect("garbage should write");
                    stdout.flush().expect("garbage should flush");
                    process::exit(0);
                }
                "codex-oversize" => {
                    let mut stdout = std::io::stdout().lock();
                    let big = "x".repeat(2 * 1024 * 1024);
                    writeln!(stdout, "{big}").expect("oversize should write");
                    stdout.flush().expect("oversize should flush");
                    process::exit(0);
                }
                "codex-flood" => {
                    // No-newline flood: the reader must stop at its byte
                    // bound without growing the line allocation.
                    let mut stdout = std::io::stdout().lock();
                    let big = "y".repeat(2 * 1024 * 1024);
                    stdout
                        .write_all(big.as_bytes())
                        .expect("flood should write");
                    stdout.flush().expect("flood should flush");
                    process::exit(0);
                }
                "codex-hang" => {}
                "codex-inherit" => {}
                "codex-inherit-stuck" => {}
                _ => process::exit(CHILD_FAILURE_EXIT),
            },
            (_, "account/rateLimits/read") => match mode {
                "codex-happy" => child_respond(&id, codex_rate_limits_fixture()),
                _ => process::exit(CHILD_FAILURE_EXIT),
            },
            _ => {}
        }
    }
    process::exit(0);
}

fn run_claude_child(mode: &str) -> ! {
    child_watchdog();
    let mut stdout = std::io::stdout().lock();
    match mode {
        "claude-happy" => {
            writeln!(
                stdout,
                "{{\
                    \"result\": \"Current session: 42% used, resets Sept 9, 5pm (UTC)\\n\
                    Current week (all models): 17% used\"\
                }}"
            )
            .expect("happy should write");
        }
        "claude-garbage" => {
            writeln!(stdout, "not json at all").expect("garbage should write");
        }
        "claude-shape" => {
            writeln!(stdout, "{{\"ok\": true}}").expect("shape should write");
        }
        "claude-empty" => {
            writeln!(stdout, "{{\"result\": \"nothing useful here\"}}")
                .expect("empty should write");
        }
        "claude-oversize" => {
            let big = "a".repeat(2 * 1024 * 1024);
            writeln!(stdout, "{big}").expect("oversize should write");
        }
        "claude-nonzero" => {
            stdout.flush().expect("flush should succeed");
            process::exit(3);
        }
        "claude-hang" => {
            stdout.flush().expect("flush should succeed");
            std::thread::sleep(Duration::from_secs(30));
            process::exit(CHILD_WATCHDOG_EXIT);
        }
        _ => process::exit(CHILD_FAILURE_EXIT),
    }
    stdout.flush().expect("flush should succeed");
    process::exit(0);
}

#[test]
fn engine_usage_fixture_child() {
    let Ok(mode) = env::var(CHILD_MODE_ENV) else {
        return;
    };
    if mode == "pipe-holder" {
        std::thread::sleep(Duration::from_secs(1));
        process::exit(0);
    }
    if mode == "pipe-holder-long" {
        std::thread::sleep(Duration::from_secs(15));
        process::exit(0);
    }
    if mode.starts_with("codex-") {
        run_codex_child(&mode);
    }
    if mode.starts_with("claude-") {
        run_claude_child(&mode);
    }
    process::exit(CHILD_FAILURE_EXIT);
}

fn spawn_pipe_holder(mode: &str) {
    let exe = env::current_exe().expect("current test executable should be available");
    Command::new(exe)
        .arg(FIXTURE_TEST_NAME)
        .arg("--exact")
        .arg("--nocapture")
        .env(CHILD_MODE_ENV, mode)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("pipe-holder grandchild should spawn");
}

fn fixture_argv() -> Vec<String> {
    vec![
        FIXTURE_TEST_NAME.to_owned(),
        "--exact".to_owned(),
        "--nocapture".to_owned(),
        "--quiet".to_owned(),
        "--".to_owned(),
    ]
}

fn codex_config(mode: &str, timeout: Duration) -> CodexUsageConfig {
    let mut config = CodexUsageConfig::new(
        env::current_exe().expect("current test executable should be available"),
    );
    config.executable_args = fixture_argv();
    config.overall_timeout = timeout;
    config
        .spawn_env
        .push((CHILD_MODE_ENV.to_owned(), mode.to_owned()));
    config
}

fn claude_config(mode: &str, timeout: Duration) -> ClaudeUsageConfig {
    let mut config = ClaudeUsageConfig::new(
        env::current_exe().expect("current test executable should be available"),
    );
    config.executable_args = fixture_argv();
    config.timeout = timeout;
    config
        .spawn_env
        .push((CHILD_MODE_ENV.to_owned(), mode.to_owned()));
    config
}

#[test]
fn codex_happy_path_reports_authenticated_windows() {
    let usage = artisan_native_engine::read_codex_usage(&codex_config(
        "codex-happy",
        Duration::from_secs(10),
    ))
    .expect("fixture codex should answer");
    assert_eq!(usage.auth.state(), EngineUsageAuthentication::Authenticated);
    assert_eq!(usage.account_email.as_deref(), Some("owner@example.test"));
    assert_eq!(usage.quota_surface, QuotaSurface::Supported);
    assert_eq!(usage.windows.len(), 1);
    assert_eq!(usage.windows[0].id(), "codex:primary");
    assert_eq!(usage.windows[0].percent_used(), 42.5);
    assert_eq!(usage.windows[0].resets_at(), Some("2026-09-09T12:00:00Z"));
}

#[test]
fn codex_login_error_is_unauthenticated_not_a_failure() {
    let usage = artisan_native_engine::read_codex_usage(&codex_config(
        "codex-login-error",
        Duration::from_secs(10),
    ))
    .expect("login-gated codex should report unauthenticated");
    assert_eq!(
        usage.auth.state(),
        EngineUsageAuthentication::Unauthenticated
    );
    assert!(usage.windows.is_empty());
    assert_eq!(usage.quota_surface, QuotaSurface::Supported);
}

#[test]
fn codex_malformed_shape_is_rejected() {
    assert_eq!(
        artisan_native_engine::read_codex_usage(&codex_config(
            "codex-shape-error",
            Duration::from_secs(10),
        )),
        Err(UsageReaderError::Malformed)
    );
}

#[test]
fn codex_garbage_lines_end_closed() {
    assert_eq!(
        artisan_native_engine::read_codex_usage(&codex_config(
            "codex-garbage",
            Duration::from_secs(10),
        )),
        Err(UsageReaderError::Closed)
    );
}

#[test]
fn codex_oversize_line_is_rejected() {
    assert_eq!(
        artisan_native_engine::read_codex_usage(&codex_config(
            "codex-oversize",
            Duration::from_secs(10),
        )),
        Err(UsageReaderError::TooLarge)
    );
}

#[test]
fn codex_no_newline_flood_is_bounded() {
    let start = std::time::Instant::now();
    assert_eq!(
        artisan_native_engine::read_codex_usage(&codex_config(
            "codex-flood",
            Duration::from_secs(10),
        )),
        Err(UsageReaderError::TooLarge)
    );
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "the flood must stop at the byte bound, not at a newline"
    );
}

#[test]
fn codex_hang_times_out_and_reaps() {
    let start = std::time::Instant::now();
    assert_eq!(
        artisan_native_engine::read_codex_usage(&codex_config(
            "codex-hang",
            Duration::from_millis(500),
        )),
        Err(UsageReaderError::Timeout)
    );
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "the hang must settle at the deadline"
    );
}

#[test]
fn codex_inherited_pipe_returns_by_deadline_with_bounded_cleanup() {
    // The fixture grandchild holds the inherited pipes ~1s while the direct
    // child never answers. The call must return at the configured deadline
    // and teardown (kill, bounded reap, bounded joins) must complete: the
    // grandchild exits inside the join grace, so no drain thread lingers.
    let start = std::time::Instant::now();
    assert_eq!(
        artisan_native_engine::read_codex_usage(&codex_config(
            "codex-inherit",
            Duration::from_millis(500),
        )),
        Err(UsageReaderError::Timeout)
    );
    assert!(
        start.elapsed() < Duration::from_secs(6),
        "return plus bounded cleanup must settle well inside grace"
    );
}

#[test]
fn codex_stuck_inherited_pipe_still_returns_within_grace() {
    // The fixture grandchild holds the inherited pipes 15s, past every
    // join grace. Tree-kill terminates the whole job, pipes close, and
    // teardown returns fast with no lingering reader.
    let start = std::time::Instant::now();
    assert_eq!(
        artisan_native_engine::read_codex_usage(&codex_config(
            "codex-inherit-stuck",
            Duration::from_millis(500),
        )),
        Err(UsageReaderError::Timeout)
    );
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "tree-kill teardown must stay fast"
    );
}

fn shim_bin(label: &str) -> PathBuf {
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let dir = env::temp_dir().join(format!(
        "artisan-usage-shim-{label}-{}-{}",
        process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("shim directory should be created");
    dir
}

#[cfg(windows)]
#[test]
fn codex_cmd_shim_reads_end_to_end() {
    let dir = shim_bin("codex");
    let shim = dir.join("codex.cmd");
    fs::write(
        &shim,
        "@echo off\r\necho {\"id\":1,\"result\":{\"capabilities\":{}}}\r\necho {\"id\":2,\"result\":{\"account\":{\"type\":\"chatgpt\",\"email\":\"owner@example.test\"},\"requiresOpenaiAuth\":false}}\r\necho {\"id\":3,\"result\":{\"rateLimitsByLimitId\":{\"codex\":{\"limitId\":\"codex\",\"primary\":{\"usedPercent\":5.0}}}}}\r\n",
    )
    .expect("shim fixture should write");
    let empty_root = shim_bin("empty-root");
    let launch = resolve_cli_with(&CliResolveInput {
        tool: "codex",
        override_var: "ARTISAN_TEST_CODEX_OVERRIDE",
        configured: None,
        local_app_data: Some(empty_root.clone()),
        path_dirs: vec![dir.clone()],
        arch: "x86_64",
    });
    assert_eq!(launch.program, PathBuf::from("cmd"));
    assert!(
        launch
            .prefix_args
            .contains(&shim.to_string_lossy().into_owned())
    );
    let mut config = CodexUsageConfig::launched(&launch);
    config.overall_timeout = Duration::from_secs(10);
    let usage = artisan_native_engine::read_codex_usage(&config).expect("shim should answer");
    assert_eq!(usage.auth.state(), EngineUsageAuthentication::Authenticated);
    assert_eq!(usage.account_email.as_deref(), Some("owner@example.test"));
    assert_eq!(usage.windows.len(), 1);
    assert_eq!(usage.windows[0].percent_used(), 5.0);
    fs::remove_dir_all(&dir).ok();
    fs::remove_dir_all(&empty_root).ok();
}

#[cfg(windows)]
#[test]
fn claude_cmd_shim_reads_end_to_end() {
    let dir = shim_bin("claude");
    let shim = dir.join("claude.cmd");
    fs::write(
        &shim,
        "@echo off\r\necho {\"result\":\"Current session: 9%% used\"}\r\n",
    )
    .expect("shim fixture should write");
    let empty_root = shim_bin("empty-root");
    let launch = resolve_cli_with(&CliResolveInput {
        tool: "claude",
        override_var: "ARTISAN_TEST_CLAUDE_OVERRIDE",
        configured: None,
        local_app_data: Some(empty_root.clone()),
        path_dirs: vec![dir.clone()],
        arch: "x86_64",
    });
    assert_eq!(launch.program, PathBuf::from("cmd"));
    let mut config = ClaudeUsageConfig::launched(&launch);
    config.timeout = Duration::from_secs(10);
    let usage = artisan_native_engine::read_claude_usage(&config).expect("shim should answer");
    assert_eq!(usage.windows.len(), 1);
    assert_eq!(usage.windows[0].id(), "five_hour");
    assert_eq!(usage.windows[0].percent_used(), 9.0);
    fs::remove_dir_all(&dir).ok();
    fs::remove_dir_all(&empty_root).ok();
}

#[test]
fn claude_happy_path_reports_session_and_weekly_windows() {
    let usage = artisan_native_engine::read_claude_usage(&claude_config(
        "claude-happy",
        Duration::from_secs(10),
    ))
    .expect("fixture claude should answer");
    assert_eq!(usage.auth.state(), EngineUsageAuthentication::Authenticated);
    assert_eq!(usage.windows.len(), 2);
    assert_eq!(usage.windows[0].id(), "five_hour");
    assert_eq!(usage.windows[0].percent_used(), 42.0);
    assert_eq!(usage.windows[1].id(), "seven_day");
    assert_eq!(usage.windows[1].percent_used(), 17.0);
}

#[test]
fn claude_garbage_shape_and_empty_are_typed() {
    assert_eq!(
        artisan_native_engine::read_claude_usage(&claude_config(
            "claude-garbage",
            Duration::from_secs(10),
        )),
        Err(UsageReaderError::Malformed)
    );
    assert_eq!(
        artisan_native_engine::read_claude_usage(&claude_config(
            "claude-shape",
            Duration::from_secs(10),
        )),
        Err(UsageReaderError::Malformed)
    );
    assert_eq!(
        artisan_native_engine::read_claude_usage(&claude_config(
            "claude-empty",
            Duration::from_secs(10),
        )),
        Err(UsageReaderError::Empty)
    );
    assert_eq!(
        artisan_native_engine::read_claude_usage(&claude_config(
            "claude-nonzero",
            Duration::from_secs(10),
        )),
        Err(UsageReaderError::ExitStatus)
    );
    assert_eq!(
        artisan_native_engine::read_claude_usage(&claude_config(
            "claude-oversize",
            Duration::from_secs(10),
        )),
        Err(UsageReaderError::TooLarge)
    );
}

#[test]
fn claude_hang_times_out_and_reaps() {
    let start = std::time::Instant::now();
    assert_eq!(
        artisan_native_engine::read_claude_usage(&claude_config(
            "claude-hang",
            Duration::from_millis(500),
        )),
        Err(UsageReaderError::Timeout)
    );
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "the hang must settle at the deadline"
    );
}

fn token_file(contents: &str) -> PathBuf {
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let path = env::temp_dir().join(format!(
        "artisan-usage-token-{}-{}.json",
        process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, contents).expect("fixture token file should write");
    path
}

#[test]
fn cursor_token_file_reads_read_only_and_bounded() {
    let missing = env::temp_dir().join("artisan-usage-token-absent.json");
    let _ = fs::remove_file(&missing);
    assert_eq!(
        read_cursor_access_token(&missing, 1_048_576).expect("absent token is not an error"),
        None
    );

    let valid = token_file(r#"{"accessToken": "fixture-token-1"}"#);
    assert_eq!(
        read_cursor_access_token(&valid, 1_048_576).expect("valid token should read"),
        Some("fixture-token-1".to_owned())
    );
    let _ = fs::remove_file(&valid);

    let empty = token_file(r#"{"accessToken": ""}"#);
    assert_eq!(
        read_cursor_access_token(&empty, 1_048_576).expect("empty token reads as absent"),
        None
    );
    let _ = fs::remove_file(&empty);

    let malformed = token_file(r#"{"accessToken": "#);
    assert_eq!(
        read_cursor_access_token(&malformed, 1_048_576),
        Err(CursorUsageError::TokenMalformed)
    );
    let _ = fs::remove_file(&malformed);

    let big = token_file(&format!("{{\"accessToken\": \"{}\"}}", "t".repeat(100)));
    assert_eq!(
        read_cursor_access_token(&big, 16),
        Err(CursorUsageError::TokenTooLarge)
    );
    let _ = fs::remove_file(&big);
}

fn split_plan_fixture() -> serde_json::Value {
    serde_json::json!({
        "billingCycleStart": 1_756_867_200_000_i64,
        "billingCycleEnd": 1_759_459_200_000_i64,
        "planUsage": {
            "totalSpend": 30.0,
            "limit": 100.0,
            "autoPercentUsed": 40.0,
            "apiPercentUsed": 10.0
        },
        "displayMessage": "plan"
    })
}

#[test]
fn cursor_split_and_single_plan_pools_map_to_monthly_windows() {
    let windows = map_cursor_period_usage(&split_plan_fixture()).expect("split plan maps");
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].id(), "cursor:cursor-models");
    assert_eq!(windows[0].label(), Some("Cursor models"));
    assert_eq!(windows[0].percent_used(), 40.0);
    assert_eq!(windows[0].kind(), EngineUsageWindowKind::Monthly);
    let expected_reset = iso_millis(1_759_459_200_000);
    assert_eq!(windows[0].resets_at(), Some(expected_reset.as_str()));
    assert_eq!(windows[0].window_minutes(), Some(43_200));
    assert_eq!(windows[1].id(), "cursor:other-models");
    assert_eq!(windows[1].percent_used(), 10.0);

    let single = serde_json::json!({
        "billingCycleStart": 1_756_867_200_000_i64,
        "billingCycleEnd": 1_759_459_200_000_i64,
        "planUsage": {"totalSpend": 25.0, "limit": 50.0}
    });
    let windows = map_cursor_period_usage(&single).expect("single plan maps");
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].id(), "cursor:included-usage");
    assert_eq!(windows[0].percent_used(), 50.0);

    let spend = serde_json::json!({
        "planUsage": {"totalSpend": 1.0, "limit": 2.0},
        "spendLimitUsage": {"overallLimit": 100.0, "overallUsed": 25.0}
    });
    let windows = map_cursor_period_usage(&spend).expect("spend maps");
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[1].id(), "cursor:on-demand");
    assert_eq!(windows[1].label(), Some("On-demand"));
    assert_eq!(windows[1].percent_used(), 25.0);

    let display = serde_json::json!({
        "planUsage": {},
        "displayMessage": "You have used 73% of your plan"
    });
    let windows = map_cursor_period_usage(&display).expect("display percent maps");
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].percent_used(), 73.0);

    assert_eq!(
        map_cursor_period_usage(&serde_json::json!({})),
        Err(CursorUsageError::BodyMalformed)
    );
    assert_eq!(
        map_cursor_period_usage(&serde_json::json!({"planUsage": {"limit": "lots"}})),
        Err(CursorUsageError::BodyMalformed)
    );
}

async fn serve_once(status: u16, body: &'static str) -> (u16, oneshot::Receiver<String>) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fixture listener should bind");
    let port = listener.local_addr().expect("fixture address").port();
    let (sender, receiver) = oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("fixture accept");
        let mut request = Vec::new();
        loop {
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).await.expect("fixture read");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
            if let Some(headers_end) = find_headers_end(&request) {
                let content_length = content_length_of(&request[..headers_end]);
                if request.len() >= headers_end + content_length {
                    break;
                }
            }
            if request.len() > 64 * 1024 {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 {status} {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            if status == 200 { "OK" } else { "Error" },
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("fixture write");
        stream.flush().await.expect("fixture flush");
        let _send_result = sender.send(String::from_utf8_lossy(&request).into_owned());
    });
    (port, receiver)
}

fn find_headers_end(request: &[u8]) -> Option<usize> {
    request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn content_length_of(headers: &[u8]) -> usize {
    let text = String::from_utf8_lossy(headers).to_lowercase();
    text.lines()
        .find_map(|line| {
            line.strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0)
}

fn cursor_config(port: u16, token: &str) -> CursorUsageConfig {
    let file = token_file(&format!(r#"{{"accessToken": "{token}"}}"#));
    CursorUsageConfig {
        auth_file: Some(file),
        endpoint: CursorEndpoint::loopback(port),
        timeout: Duration::from_secs(10),
        max_bytes: 1_048_576,
    }
}

#[tokio::test]
async fn cursor_happy_path_posts_and_maps() {
    let body = serde_json::to_string(&split_plan_fixture()).expect("fixture serializes");
    let body: &'static str = Box::leak(body.into_boxed_str());
    let (port, request) = serve_once(200, body).await;
    let config = cursor_config(port, "fixture-token-1");
    let auth_file = config.auth_file.clone();
    let usage = read_cursor_usage(&config)
        .await
        .expect("fixture dashboard should answer");
    assert_eq!(usage.auth.state(), EngineUsageAuthentication::Authenticated);
    assert_eq!(usage.windows.len(), 2);
    assert_eq!(usage.windows[0].id(), "cursor:cursor-models");
    let request = request
        .await
        .expect("fixture server should see the request");
    assert!(request.starts_with("POST /aiserver.v1.DashboardService/GetCurrentPeriodUsage "));
    assert!(request.contains("authorization: Bearer fixture-token-1"));
    assert!(request.contains("connect-protocol-version: 1"));
    assert!(request.contains("x-cursor-client-type: cli"));
    assert!(request.contains("content-type: application/json"));
    let _ = fs::remove_file(auth_file.expect("auth file is set"));
}

#[tokio::test]
async fn cursor_auth_statuses_report_unauthenticated() {
    for status in [401_u16, 403] {
        let (port, _) = serve_once(status, "{}").await;
        let config = cursor_config(port, "fixture-token-1");
        let auth_file = config.auth_file.clone();
        let usage = read_cursor_usage(&config)
            .await
            .expect("auth statuses report unauthenticated");
        assert_eq!(
            usage.auth.state(),
            EngineUsageAuthentication::Unauthenticated
        );
        assert!(usage.windows.is_empty());
        assert_eq!(usage.quota_surface, QuotaSurface::Supported);
        let _ = fs::remove_file(auth_file.expect("auth file is set"));
    }
}

#[tokio::test]
async fn cursor_failures_are_typed() {
    let (port, _) = serve_once(500, "{}").await;
    let config = cursor_config(port, "fixture-token-1");
    let auth_file = config.auth_file.clone();
    assert_eq!(
        read_cursor_usage(&config).await,
        Err(CursorUsageError::HttpFailure)
    );
    let _ = fs::remove_file(auth_file.expect("auth file is set"));

    let (port, _) = serve_once(200, "oops").await;
    let config = cursor_config(port, "fixture-token-1");
    let auth_file = config.auth_file.clone();
    assert_eq!(
        read_cursor_usage(&config).await,
        Err(CursorUsageError::BodyMalformed)
    );
    let _ = fs::remove_file(auth_file.expect("auth file is set"));

    let missing = CursorUsageConfig {
        auth_file: Some(env::temp_dir().join("artisan-usage-token-absent-2.json")),
        endpoint: CursorEndpoint::loopback(port),
        timeout: Duration::from_secs(5),
        max_bytes: 1_048_576,
    };
    let usage = read_cursor_usage(&missing)
        .await
        .expect("missing token reports unauthenticated");
    assert_eq!(
        usage.auth.state(),
        EngineUsageAuthentication::Unauthenticated
    );
}

#[tokio::test]
async fn cursor_endpoint_rejects_plaintext_outside_loopback() {
    assert!(
        CursorEndpoint::new(
            "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage".to_owned()
        )
        .is_ok()
    );
    assert!(CursorEndpoint::new("http://127.0.0.1:9/x".to_owned()).is_ok());
    assert!(CursorEndpoint::new("http://localhost:9/x".to_owned()).is_ok());
    assert_eq!(
        CursorEndpoint::new("http://dashboard.example/x".to_owned()),
        Err(CursorUsageError::InsecureEndpoint)
    );
    let production = CursorEndpoint::production();
    assert!(production.url().starts_with("https://"));
}

#[tokio::test]
async fn cursor_hang_times_out() {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fixture listener should bind");
    let port = listener.local_addr().expect("fixture address").port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("fixture accept");
        let mut chunk = [0_u8; 1024];
        let _ = stream.read(&mut chunk).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
    });
    let config = CursorUsageConfig {
        auth_file: Some(token_file(r#"{"accessToken": "fixture-token-1"}"#)),
        endpoint: CursorEndpoint::loopback(port),
        timeout: Duration::from_millis(300),
        max_bytes: 1_048_576,
    };
    let auth_file = config.auth_file.clone();
    assert_eq!(
        read_cursor_usage(&config).await,
        Err(CursorUsageError::Timeout)
    );
    let _ = fs::remove_file(auth_file.expect("auth file is set"));
}

#[derive(Debug, Clone)]
enum StubOutcome {
    Ok(Vec<EngineUsageWindow>),
    Failure(ReaderFailure),
    Hang,
}

#[derive(Debug)]
struct StubReader {
    engine_id: &'static str,
    display_name: &'static str,
    calls: AtomicUsize,
    outcome: StubOutcome,
}

impl StubReader {
    fn ok(engine_id: &'static str, display_name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            engine_id,
            display_name,
            calls: AtomicUsize::new(0),
            outcome: StubOutcome::Ok(vec![
                EngineUsageWindow::new(
                    "stub:window".to_owned(),
                    EngineUsageWindowKind::Session,
                    None,
                    11.0,
                    None,
                    Some(300),
                )
                .expect("stub window is valid"),
            ]),
        })
    }

    fn failing(engine_id: &'static str, display_name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            engine_id,
            display_name,
            calls: AtomicUsize::new(0),
            outcome: StubOutcome::Failure(ReaderFailure::unavailable("stub provider is down")),
        })
    }

    fn hanging(engine_id: &'static str, display_name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            engine_id,
            display_name,
            calls: AtomicUsize::new(0),
            outcome: StubOutcome::Hang,
        })
    }

    fn calls(reader: &Arc<Self>) -> usize {
        reader.calls.load(Ordering::Relaxed)
    }
}

impl AccountUsageReader for StubReader {
    fn engine_id(&self) -> &'static str {
        self.engine_id
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn read(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ProviderUsage, ReaderFailure>> + Send + '_>,
    > {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let outcome = self.outcome.clone();
        Box::pin(async move {
            match outcome {
                StubOutcome::Ok(windows) => Ok(ProviderUsage::authenticated(windows)),
                StubOutcome::Failure(failure) => Err(failure),
                StubOutcome::Hang => {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Err(ReaderFailure::unavailable(
                        "stub hang should have timed out",
                    ))
                }
            }
        })
    }
}

fn stub_service(
    readers: Vec<Arc<StubReader>>,
    freshness: Duration,
    per_engine_timeout: Duration,
) -> (AccountUsageService, Vec<Arc<StubReader>>) {
    let erased: Vec<Arc<dyn AccountUsageReader>> = readers
        .iter()
        .map(|reader| Arc::clone(reader) as Arc<dyn AccountUsageReader>)
        .collect();
    (
        AccountUsageService::with_readers(erased, freshness, per_engine_timeout),
        readers,
    )
}

fn all_query() -> ReadAccountUsage {
    ReadAccountUsage::new(None, false).expect("query is valid")
}

#[tokio::test]
async fn usage_service_caches_forces_and_narrows() {
    let codex = StubReader::ok("codex", "Codex");
    let claude = StubReader::ok("claude", "Claude");
    let (service, _) = stub_service(
        vec![Arc::clone(&codex), Arc::clone(&claude)],
        Duration::from_millis(150),
        Duration::from_secs(5),
    );
    let snapshot = service.read(&all_query()).await;
    assert_eq!(snapshot.engines().len(), 2);
    assert_eq!(snapshot.engines()[0].engine_id(), "codex");
    assert_eq!(snapshot.engines()[1].engine_id(), "claude");
    validate_iso_timestamp(snapshot.fetched_at(), "fetched_at").expect("fetch instant is valid");
    assert_eq!(StubReader::calls(&codex), 1);
    assert_eq!(StubReader::calls(&claude), 1);

    let _ = service.read(&all_query()).await;
    assert_eq!(StubReader::calls(&codex), 1);
    assert_eq!(StubReader::calls(&claude), 1);

    let forced = ReadAccountUsage::new(None, true).expect("query is valid");
    let _ = service.read(&forced).await;
    assert_eq!(StubReader::calls(&codex), 2);
    assert_eq!(StubReader::calls(&claude), 2);

    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = service.read(&all_query()).await;
    assert_eq!(StubReader::calls(&codex), 3);

    let narrowed = ReadAccountUsage::new(Some("claude".to_owned()), true).expect("query is valid");
    let snapshot = service.read(&narrowed).await;
    assert_eq!(snapshot.engines().len(), 1);
    assert_eq!(snapshot.engines()[0].engine_id(), "claude");
    assert_eq!(StubReader::calls(&codex), 3);
    assert_eq!(StubReader::calls(&claude), 4);
}

#[test]
fn usage_freshness_matches_the_electron_window() {
    assert_eq!(ACCOUNT_USAGE_FRESHNESS, Duration::from_secs(180));
}

#[derive(Debug)]
struct FlakyReader {
    engine_id: &'static str,
    display_name: &'static str,
    calls: AtomicUsize,
    failed: std::sync::atomic::AtomicBool,
}

impl FlakyReader {
    fn codex() -> Arc<Self> {
        Arc::new(Self {
            engine_id: "codex",
            display_name: "Codex",
            calls: AtomicUsize::new(0),
            failed: std::sync::atomic::AtomicBool::new(false),
        })
    }
}

impl AccountUsageReader for FlakyReader {
    fn engine_id(&self) -> &'static str {
        self.engine_id
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn read(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ProviderUsage, ReaderFailure>> + Send + '_>,
    > {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let failed = self.failed.load(Ordering::Relaxed);
        Box::pin(async move {
            if failed {
                return Err(ReaderFailure::unavailable("stub provider went down"));
            }
            Ok(ProviderUsage::authenticated(vec![
                EngineUsageWindow::new(
                    "stub:window".to_owned(),
                    EngineUsageWindowKind::Session,
                    None,
                    11.0,
                    None,
                    Some(300),
                )
                .expect("stub window is valid"),
            ]))
        })
    }
}

fn narrowed(engine_id: &str) -> ReadAccountUsage {
    ReadAccountUsage::new(Some(engine_id.to_owned()), true).expect("query is valid")
}

#[tokio::test]
async fn usage_service_preserves_last_good_with_original_time() {
    let flaky = FlakyReader::codex();
    let erased: Vec<Arc<dyn AccountUsageReader>> =
        vec![Arc::clone(&flaky) as Arc<dyn AccountUsageReader>];
    let service =
        AccountUsageService::with_readers(erased, Duration::from_secs(60), Duration::from_secs(5));
    let first = service.read(&narrowed("codex")).await;
    assert_eq!(first.engines().len(), 1);
    assert!(first.engines()[0].failure().is_none());
    assert_eq!(first.engines()[0].windows().len(), 1);

    flaky.failed.store(true, Ordering::Relaxed);
    let second = service.read(&narrowed("codex")).await;
    assert_eq!(second.engines().len(), 1);
    // Last-good windows survive with the original fetch time, while the
    // refresh failure is exposed honestly on the served report.
    assert_eq!(second.engines()[0].windows().len(), 1);
    assert_eq!(
        second.engines()[0].failure(),
        Some("stub provider went down")
    );
    assert_eq!(second.fetched_at(), first.fetched_at());
}

#[tokio::test]
async fn narrowed_queries_carry_per_engine_observation_time() {
    let codex = StubReader::ok("codex", "Codex");
    let claude = StubReader::ok("claude", "Claude");
    let erased: Vec<Arc<dyn AccountUsageReader>> = vec![
        Arc::clone(&codex) as Arc<dyn AccountUsageReader>,
        Arc::clone(&claude) as Arc<dyn AccountUsageReader>,
    ];
    let service =
        AccountUsageService::with_readers(erased, Duration::from_secs(60), Duration::from_secs(5));
    let codex_snapshot = service.read(&narrowed("codex")).await;
    let claude_snapshot = service.read(&narrowed("claude")).await;
    assert_eq!(codex_snapshot.engines().len(), 1);
    assert_eq!(claude_snapshot.engines().len(), 1);
    // The aggregate snapshot carries the latest observation across its
    // reports; exact per-engine times come from narrowed queries.
    let aggregate = service.read(&all_query()).await;
    let expected = codex_snapshot
        .fetched_at()
        .max(claude_snapshot.fetched_at())
        .to_owned();
    assert_eq!(aggregate.fetched_at(), expected);
}

#[tokio::test]
async fn usage_service_isolates_failures_and_unknown_engines() {
    let codex = StubReader::ok("codex", "Codex");
    let cursor = StubReader::failing("cursor", "Cursor");
    let (service, _) = stub_service(
        vec![Arc::clone(&codex), Arc::clone(&cursor)],
        Duration::from_secs(60),
        Duration::from_secs(5),
    );
    let snapshot = service.read(&all_query()).await;
    assert_eq!(snapshot.engines().len(), 2);
    assert!(snapshot.engines()[0].failure().is_none());
    assert_eq!(
        snapshot.engines()[1].failure(),
        Some("stub provider is down")
    );
    assert_eq!(
        snapshot.engines()[1].quota_surface(),
        Some(QuotaSurface::Unknown)
    );

    let unknown = ReadAccountUsage::new(Some("nope".to_owned()), false).expect("query is valid");
    let snapshot = service.read(&unknown).await;
    assert_eq!(snapshot.engines().len(), 1);
    assert_eq!(snapshot.engines()[0].failure(), Some("unknown engine id"));
    assert_eq!(StubReader::calls(&codex), 1);
}

#[tokio::test]
async fn usage_service_enforces_per_engine_timeout() {
    let hanging = StubReader::hanging("codex", "Codex");
    let (service, _) = stub_service(
        vec![Arc::clone(&hanging)],
        Duration::from_secs(60),
        Duration::from_millis(100),
    );
    let start = std::time::Instant::now();
    let snapshot = service.read(&all_query()).await;
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(snapshot.engines().len(), 1);
    assert_eq!(
        snapshot.engines()[0].failure(),
        Some("engine usage read timed out")
    );
}

#[test]
fn unsupported_readers_report_honest_surfaces() {
    let grok = UnsupportedAccountUsageReader::new(
        "grok",
        "Grok Build",
        "Grok Build exposes no account-usage surface.",
    );
    let failure = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(grok.read())
        .expect_err("unsupported should fail");
    assert_eq!(failure.quota_surface, QuotaSurface::Unsupported);
    assert_eq!(
        failure.failure,
        "Grok Build exposes no account-usage surface."
    );
}

#[test]
fn production_codex_reader_maps_spawn_failure() {
    let config = CodexUsageConfig::new(PathBuf::from("/nonexistent-artisan-codex-xyz"));
    assert_eq!(
        artisan_native_engine::read_codex_usage(&config),
        Err(UsageReaderError::Spawn)
    );
}

struct TemporaryDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let directory = env::temp_dir().join(format!(
            "artisan-usage-handler-{label}-{}-{}",
            process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).expect("temporary database directory should be created");
        let database = directory.join("forge.sqlite3");
        Self {
            directory,
            database,
        }
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.directory);
    }
}

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("request id is valid")
}

#[tokio::test]
async fn usage_query_arm_needs_a_service_and_echoes_correlation() {
    let temporary = TemporaryDatabase::new("unavailable");
    let storage =
        ForgeStorage::open(SqliteConfig::file(temporary.database.clone()).sqlx_logging(false))
            .await
            .expect("storage should open");
    let handler = RequestHandler::new(storage.repository().clone());
    let query = ClientRequest::Query(Query::ReadAccountUsage(all_query()));
    let failure = handler
        .respond(&request_id("usage-unavailable"), &query)
        .await
        .expect_err("missing service should fail closed");
    assert_eq!(failure.code, ErrorCode::UnsupportedFeature);
    assert!(!failure.retryable);
    assert_eq!(
        failure.request_id.as_ref(),
        Some(&request_id("usage-unavailable"))
    );
    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn usage_query_arm_returns_isolated_snapshot() {
    let temporary = TemporaryDatabase::new("snapshot");
    let storage =
        ForgeStorage::open(SqliteConfig::file(temporary.database.clone()).sqlx_logging(false))
            .await
            .expect("storage should open");
    let codex = StubReader::ok("codex", "Codex");
    let cursor = StubReader::failing("cursor", "Cursor");
    let erased: Vec<Arc<dyn AccountUsageReader>> = vec![
        Arc::clone(&codex) as Arc<dyn AccountUsageReader>,
        Arc::clone(&cursor) as Arc<dyn AccountUsageReader>,
    ];
    let service =
        AccountUsageService::with_readers(erased, Duration::from_secs(60), Duration::from_secs(5));
    let handler =
        RequestHandler::new(storage.repository().clone()).with_account_usage_service(service);

    let response = handler
        .respond(
            &request_id("usage-snapshot"),
            &ClientRequest::Query(Query::ReadAccountUsage(all_query())),
        )
        .await
        .expect("usage query should answer");
    assert_eq!(response.request_id, request_id("usage-snapshot"));
    match response.payload {
        ResponsePayload::AccountUsage(snapshot) => {
            assert_eq!(snapshot.engines().len(), 2);
            assert!(snapshot.engines()[0].failure().is_none());
            assert_eq!(
                snapshot.engines()[1].failure(),
                Some("stub provider is down")
            );
            validate_iso_timestamp(snapshot.fetched_at(), "fetched_at")
                .expect("fetch instant is valid");
        }
        _ => panic!("expected accountUsage payload"),
    }

    let unknown = ReadAccountUsage::new(Some("nope".to_owned()), false).expect("query is valid");
    let response = handler
        .respond(
            &request_id("usage-unknown"),
            &ClientRequest::Query(Query::ReadAccountUsage(unknown)),
        )
        .await
        .expect("unknown engine still answers a snapshot");
    match response.payload {
        ResponsePayload::AccountUsage(snapshot) => {
            assert_eq!(snapshot.engines().len(), 1);
            assert_eq!(snapshot.engines()[0].failure(), Some("unknown engine id"));
        }
        _ => panic!("expected accountUsage payload"),
    }
    storage.close().await.expect("storage should close");
}

#[test]
fn request_id_correlation_survives_the_usage_path() {
    let query = ReadAccountUsage::new(Some("codex".to_owned()), true).expect("query is valid");
    let request = ClientRequest::Query(Query::ReadAccountUsage(query));
    let map = HashMap::from([("usage", request)]);
    match map.get("usage") {
        Some(ClientRequest::Query(Query::ReadAccountUsage(query))) => {
            assert_eq!(query.engine_id(), Some("codex"));
            assert!(query.force());
        }
        _ => panic!("expected usage query"),
    }
}
