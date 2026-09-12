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
//! `codex-wire-resume_interleave`, `codex-wire-resume_mismatch`,
//! `codex-wire-steer_burst`, or `codex-wire-steer_reject`, plus the
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
//! `steer_burst` and `steer_reject` serve the mid-turn steer path against
//! the production `turn/steer` verb (`expectedTurnId` + `threadId` +
//! non-empty text `input`, per `engine_owner/codex.rs`). Their `turn/start`
//! answers the actual turn id plus one initial valid agent delta and then
//! stays live with no terminal event. Every received `turn/steer` request
//! (valid or not) is appended with its id and params to
//! `steer-requests.jsonl` in the working directory, flushed per line, so a
//! test can count exactly one provider write. `steer_burst` answers a valid
//! steer with one bounded reasoning frame plus 64 valid
//! `item/agentMessage/delta` frames (same thread, turn, and item;
//! cumulative distinct content) BEFORE the correlated success
//! result, then stays live until `turn/interrupt` (which answers its ack
//! plus a cancelled terminal) or EOF. `steer_reject` answers every steer
//! with a correlated JSON-RPC error and never emits a terminal, a success,
//! or a new run. Invalid steers (wrong thread/turn id or malformed input)
//! are rejected with a typed JSON-RPC error in both scenarios. Nothing is
//! ever slept to fake an ack: all replies are emitted synchronously while
//! handling the requesting line.
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
/// Per-request steer record filename inside the spawned working directory.
///
/// Every received `turn/steer` request appends one JSON line carrying its
/// id and params, flushed before any reply, so a test can count exactly
/// one provider write per steer.
const STEER_RECORD_FILENAME: &str = "steer-requests.jsonl";
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
/// Item identity for the initial live-turn delta in the steer scenarios.
const STEER_OPEN_ITEM_ID: &str = "item-1";
/// Item identity shared by every burst delta in `steer_burst`.
const STEER_BURST_ITEM_ID: &str = "item-steer-1";
/// Number of burst observations `steer_burst` emits before the steer ack.
///
/// Deliberately larger than the backend test's `observation_capacity` so
/// reading the ack causally requires draining the burst first.
const STEER_BURST_COUNT: usize = 64;

#[expect(
    clippy::too_many_lines,
    reason = "the fixture binary is one linear scripted app-server session; splitting would duplicate the JSONL wiring"
)]
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
    let steer_record_path = match std::env::current_dir() {
        Ok(cwd) => cwd.join(STEER_RECORD_FILENAME),
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
        let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
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
                            "itemId": STEER_OPEN_ITEM_ID,
                            "turnId": TURN_ID,
                            "threadId": THREAD_ID,
                            "delta": DELTA_TEXT,
                        }}),
                    );
                    // The steer scenarios stay live after the opening delta:
                    // no terminal event here. `steer_burst` drives its burst
                    // off `turn/steer` and closes on `turn/interrupt` (or
                    // EOF); every other accepted scenario still completes
                    // inline exactly as before.
                    if scenario != "steer_burst" && scenario != "steer_reject" {
                        emit(
                            &mut output,
                            &serde_json::json!({"method": "turn/completed", "params": {
                                "threadId": THREAD_ID,
                                "turn": {"id": TURN_ID, "status": "completed"},
                            }}),
                        );
                    }
                }
            }
            "turn/steer" => {
                // Steer handling exists only in the steer scenarios; every
                // other scenario keeps its historical no-reply behavior via
                // the catch-all below.
                if scenario != "steer_burst" && scenario != "steer_reject" {
                    continue;
                }
                let params = value
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                // Durable per-request record (id + params), flushed before
                // any reply, so a test can count exactly one provider write
                // per steer. A lost record would silently break that count,
                // so a write failure refuses loudly instead.
                record_steer_request(&steer_record_path, &id, &params);
                let valid = valid_steer_params(&params);
                if !valid || scenario == "steer_reject" {
                    // Typed JSON-RPC error, correlated by id: an invalid
                    // target or malformed input in either scenario, and any
                    // steer at all in `steer_reject`. No terminal, no new
                    // run, and never a success.
                    let message = if valid {
                        "turn/steer rejected by provider"
                    } else {
                        "Invalid request: bad threadId, expectedTurnId, or input"
                    };
                    let code = if valid { -32001 } else { -32600 };
                    emit(
                        &mut output,
                        &serde_json::json!({"id": id, "error": {
                            "code": code,
                            "message": message,
                        }}),
                    );
                    continue;
                }
                // `steer_burst` valid steer: one bounded reasoning frame
                // precedes the full burst, synchronously — no sleep fakes
                // the ack. The thinking trace streams through the
                // production activity path while the turn is held.
                emit(
                    &mut output,
                    &serde_json::json!({"method": "item/reasoning/summaryTextDelta", "params": {
                        "threadId": THREAD_ID,
                        "turnId": TURN_ID,
                        "itemId": "item-steer-rs1",
                        "summaryIndex": 1,
                        "delta": "thinking trace ",
                    }}),
                );
                for index in 0..STEER_BURST_COUNT {
                    emit(
                        &mut output,
                        &serde_json::json!({"method": "item/agentMessage/delta", "params": {
                            "itemId": STEER_BURST_ITEM_ID,
                            "turnId": TURN_ID,
                            "threadId": THREAD_ID,
                            "delta": format!("burst-{index:02} "),
                        }}),
                    );
                }
                emit(
                    &mut output,
                    &serde_json::json!({"id": id, "result": {"ok": true}}),
                );
            }
            "turn/interrupt" => {
                // Only `steer_burst` answers interrupts: the ack plus a
                // cancelled terminal close the live turn. Every other
                // scenario keeps its historical no-reply behavior.
                if scenario != "steer_burst" {
                    continue;
                }
                emit(
                    &mut output,
                    &serde_json::json!({"id": id, "result": {"ok": true}}),
                );
                emit(
                    &mut output,
                    &serde_json::json!({"method": "turn/completed", "params": {
                        "threadId": THREAD_ID,
                        "turn": {"id": TURN_ID, "status": "interrupted"},
                    }}),
                );
            }
            // Usage reads and any future request need no reply for the
            // covered owner paths.
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

/// Appends one `turn/steer` request record (id plus params) to the
/// per-test record file and flushes before returning, so a test counting
/// lines observes exactly one record per provider write. A lost record
/// would silently break that count, so a write failure refuses loudly.
fn record_steer_request(
    path: &std::path::Path,
    id: &serde_json::Value,
    params: &serde_json::Value,
) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|_| std::process::exit(REFUSED_EXIT));
    let mut text = serde_json::to_string(&serde_json::json!({
        "id": id,
        "method": "turn/steer",
        "params": params,
    }))
    .expect("fixture json serializes");
    text.push('\n');
    file.write_all(text.as_bytes())
        .unwrap_or_else(|_| std::process::exit(REFUSED_EXIT));
    file.flush()
        .unwrap_or_else(|_| std::process::exit(REFUSED_EXIT));
}

/// Validates `turn/steer` params against the production verb shape
/// (`engine_owner/codex.rs`): exact thread id, actual provider turn id as
/// `expectedTurnId`, and non-empty text input.
fn valid_steer_params(params: &serde_json::Value) -> bool {
    let thread_ok = params
        .get("threadId")
        .and_then(|value| value.as_str())
        .is_some_and(|thread| thread == THREAD_ID);
    let turn_ok = params
        .get("expectedTurnId")
        .and_then(|value| value.as_str())
        .is_some_and(|turn| turn == TURN_ID);
    let input_ok = params
        .get("input")
        .and_then(|value| value.as_array())
        .is_some_and(|items| {
            items.first().is_some_and(|item| {
                item.get("text")
                    .and_then(|text| text.as_str())
                    .is_some_and(|text| !text.is_empty())
            })
        });
    thread_ok && turn_ok && input_ok
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
        "strict" | "reject_always" | "interleave" | "resume_interleave" | "resume_mismatch"
        | "steer_burst" | "steer_reject" => Some(scenario.to_owned()),
        _ => None,
    }
}
