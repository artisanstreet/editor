//! TEST-ONLY Codex app-server wire fixture.
//!
//! One ordinary `main` speaking the `codex app-server --stdio` JSONL subset
//! the production owner drives: `initialize` (+ the `initialized`
//! notification), `thread/start` (or `thread/resume`), and `turn/start`
//! with the streaming notifications. No product module is imported; the only
//! non-`std` dependency is `serde_json`. The production spawner passes no
//! scenario plumbing, so the fixture selects behavior without any parent
//! environment mutation: the copied executable basename names the scenario
//! (`codex-wire-strict`, `codex-wire-reject_always`, `codex-wire-interleave`,
//! `codex-wire-resume_interleave`, or `codex-wire-resume_mismatch`, plus the
//! platform executable suffix), and the
//! received `turn/start` params are recorded to
//! `turn-start-params.json` inside the spawned working directory (the
//! owner's exact project root).
//!
//! The fixture validates the exact production defect on the live owner path:
//! `turn/start` is accepted only with a non-empty `threadId`. A missing
//! `threadId` answers the real-CLI error `-32600 Invalid request: missing
//! field threadId` and starts no inference, so an owner that omits the
//! binding fails fast here instead of pumping silence until lease expiry.
//!
//! Scenarios: `strict` validates and accepts, `reject_always` answers
//! `-32600` for every `turn/start`, and `interleave` validates and accepts
//! but emits the real-CLI `thread/started` notification between the
//! `thread/*` result and the `turn/start` result. `resume_interleave`
//! mirrors the real CLI around `thread/resume`: the `remoteControl`,
//! `deprecationNotice`, `mcpStartup`, and `threadStatus` notification burst
//! arrives before the id-matched resume result, which still reopens the
//! same thread. `resume_mismatch` answers `thread/resume` with a foreign
//! thread id so the owner must fail closed instead of silently starting
//! fresh.
//!
//! Notifications (no `id`) receive no reply. After the terminal turn event
//! the fixture holds stdin until EOF and exits 0, so owner teardown observes
//! a clean reap. A fixture-local 25s watchdog exits 99 and is always
//! failure; malformed input or an unknown basename exits 87.

use std::io::{BufRead, Write};

/// Basename prefix selecting the scenario from the copied executable name.
const SCENARIO_PREFIX: &str = "codex-wire-";
/// Record filename inside the spawned working directory.
const RECORD_FILENAME: &str = "turn-start-params.json";
/// Fixture-local watchdog failure exit (always failure).
const WATCHDOG_EXIT: i32 = 99;
/// Malformed input or unknown basename refusal.
const REFUSED_EXIT: i32 = 87;
/// Watchdog bound.
const WATCHDOG_SECS: u64 = 25;
/// Fixed native thread identity served to every turn.
const THREAD_ID: &str = "thread-fixture-1";
/// Fixed native turn identity served to every accepted turn.
const TURN_ID: &str = "turn-1";
/// Assistant text served to every accepted turn.
const DELTA_TEXT: &str = "hello wire";

fn main() {
    std::thread::Builder::new()
        .name("codex-wire-fixture-watchdog".to_owned())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(WATCHDOG_SECS));
            std::process::exit(WATCHDOG_EXIT);
        })
        .expect("watchdog thread should spawn");

    let scenario = match std::env::args().next() {
        Some(argv0) => match scenario_from_basename(&argv0) {
            Some(scenario) => scenario,
            None => std::process::exit(REFUSED_EXIT),
        },
        None => std::process::exit(REFUSED_EXIT),
    };
    let record_path = match std::env::current_dir() {
        Ok(cwd) => cwd.join(RECORD_FILENAME),
        Err(_) => std::process::exit(REFUSED_EXIT),
    };

    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();

    let mut buf = String::new();
    loop {
        buf.clear();
        match input.read_line(&mut buf) {
            Ok(0) => std::process::exit(0),
            Ok(_) => {}
            Err(_) => std::process::exit(REFUSED_EXIT),
        }
        let trimmed = buf.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(trimmed).unwrap_or_else(|_| std::process::exit(REFUSED_EXIT));
        // Notifications carry no `id` and receive no reply.
        if value.get("id").is_none() {
            continue;
        }
        let id = value
            .get("id")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let method = value
            .get("method")
            .and_then(|method| method.as_str())
            .unwrap_or("");
        match method {
            "initialize" => emit(
                &mut output,
                &serde_json::json!({"id": id, "result": {
                    "codexHome": "C:\\x",
                    "platformFamily": "windows",
                    "platformOs": "windows",
                    "userAgent": "test",
                }}),
            ),
            "thread/start" | "thread/resume" => {
                let resume_burst = method == "thread/resume" && scenario == "resume_interleave";
                if resume_burst {
                    // Real-CLI order: the notification burst arrives before
                    // the id-matched resume result.
                    for notice in [
                        serde_json::json!({"method": "remoteControl/status/changed", "params": {
                            "status": "connected",
                        }}),
                        serde_json::json!({"method": "deprecationNotice", "params": {
                            "message": "test deprecation",
                        }}),
                        serde_json::json!({"method": "mcpStartup", "params": {
                            "status": "ok",
                        }}),
                        serde_json::json!({"method": "threadStatus", "params": {
                            "threadId": THREAD_ID,
                            "status": "inProgress",
                        }}),
                    ] {
                        emit(&mut output, &notice);
                    }
                }
                let resumed_id = if method == "thread/resume" && scenario == "resume_mismatch" {
                    "thread-fixture-foreign"
                } else {
                    THREAD_ID
                };
                emit(
                    &mut output,
                    &serde_json::json!({"id": id, "result": {"thread": {"id": resumed_id}}}),
                );
                if scenario == "interleave" {
                    emit(
                        &mut output,
                        &serde_json::json!({"method": "thread/started", "params": {
                            "threadId": THREAD_ID,
                            "thread": {"id": THREAD_ID, "status": "inProgress"},
                        }}),
                    );
                }
            }
            "turn/start" => {
                let params = value
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let recorded = serde_json::to_string(&params).unwrap_or_default();
                let _ = std::fs::write(&record_path, recorded);
                let thread_bound = params
                    .get("threadId")
                    .and_then(|value| value.as_str())
                    .is_some_and(|thread| !thread.is_empty());
                if scenario == "reject_always" || !thread_bound {
                    emit(
                        &mut output,
                        &serde_json::json!({"id": id, "error": {
                            "code": -32600,
                            "message": "Invalid request: missing field threadId",
                        }}),
                    );
                } else {
                    emit(
                        &mut output,
                        &serde_json::json!({"id": id, "result": {"turn": {"id": TURN_ID}}}),
                    );
                    emit(
                        &mut output,
                        &serde_json::json!({"method": "item/agentMessage/delta", "params": {
                            "itemId": "item-1",
                            "turnId": TURN_ID,
                            "threadId": THREAD_ID,
                            "delta": DELTA_TEXT,
                        }}),
                    );
                    emit(
                        &mut output,
                        &serde_json::json!({"method": "turn/completed", "params": {
                            "threadId": THREAD_ID,
                            "turn": {"id": TURN_ID, "status": "completed"},
                        }}),
                    );
                }
            }
            // `turn/steer`, `turn/interrupt`, usage reads, and any future
            // request need no reply for the covered owner paths.
            _ => {}
        }
    }
}

fn emit(output: &mut impl Write, value: &serde_json::Value) {
    let mut text = serde_json::to_string(value).expect("fixture json serializes");
    text.push('\n');
    output
        .write_all(text.as_bytes())
        .expect("fixture stdout writes");
    output.flush().expect("fixture stdout flushes");
}

/// Selects the scenario from the copied executable basename.
///
/// The parent copies the built fixture to `codex-wire-<scenario>` per test,
/// so parallel tests never share global environment state. Returns `None`
/// for an unknown basename.
fn scenario_from_basename(argv0: &str) -> Option<String> {
    let stem = std::path::Path::new(argv0)
        .file_stem()
        .and_then(|stem| stem.to_str())?;
    let scenario = stem.strip_prefix(SCENARIO_PREFIX)?;
    match scenario {
        "strict" | "reject_always" | "interleave" | "resume_interleave" | "resume_mismatch" => {
            Some(scenario.to_owned())
        }
        _ => None,
    }
}
