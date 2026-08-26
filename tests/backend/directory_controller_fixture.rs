//! TEST-ONLY directory-controller protocol child fixture.
//!
//! One ordinary `main` in a `testonly` Bazel `rust_binary`: no libtest
//! harness, no banner, never shipped. It path-links the ACTUAL shared
//! private codec (`directory_helper_codec.rs`) so both wire directions are
//! proven against the single real encoder/decoder, and it speaks the exact
//! helper-side conversation its parent-side scenarios assert.
//!
//! The scenario comes from the child-only environment variable
//! `ARTISAN_DIRECTORY_CONTROLLER_TEST_SCENARIO`, set by the parent's
//! private launch recipe on the spawned `Command`; the parent environment
//! is never mutated. Unknown or non-Unicode scenarios exit nonzero and
//! never improvise. A finite local watchdog contains broken scenario logic;
//! a watchdog exit is ALWAYS failure evidence, never successful
//! cancellation or reap evidence.
//!
//! Resource discipline: this fixture creates NO temporary directories and
//! no other resources that need dropping across `process::exit` (which
//! skips destructors). Success payloads canonicalize directories that
//! already exist — the process working directory or the parent-owned
//! anchor supplied over the wire — so nothing can leak. Staleness is
//! deterministic and generation-keyed with checked arithmetic: generation
//! 1 answers one generation ahead; every later generation answers exactly
//! itself, letting one controller prove that a stale response never
//! settles later work without any marker file or cross-child state.

use std::io::{Read, Write};
use std::process;

use artisan_domain::ROOT_PATH_MAX_BYTES;

#[path = "../../modules/backend/src/directory_helper_codec.rs"]
mod directory_helper_codec;

use directory_helper_codec::{HelperRequest, Response, encode_response, read_request};

/// Fixture-local lifeline-lost exit code; mirrors the helper's contractual
/// value 3 without exporting any production internal. Parent-side tests
/// assert equality against the actual helper constant.
const FIXTURE_EXIT_LIFELINE_LOST: i32 = 3;

/// Fixed exit code of the fixture-local watchdog: ALWAYS failure evidence,
/// never successful cancellation or reap proof.
const WATCHDOG_EXIT: i32 = 99;

/// Fixed exit code for unknown or non-Unicode scenarios: fail nonzero
/// instead of improvising.
const SCENARIO_REFUSED_EXIT: i32 = 87;

/// Child-only scenario selector, written solely onto the spawned Command.
const SCENARIO_ENV: &str = "ARTISAN_DIRECTORY_CONTROLLER_TEST_SCENARIO";

fn main() {
    // Finite containment for broken logic; healthy scenarios finish far
    // inside the bound and the thread dies with the process either way.
    std::thread::Builder::new()
        .name("directory-controller-fixture-watchdog".to_owned())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(20));
            process::exit(WATCHDOG_EXIT);
        })
        .expect("watchdog thread should spawn");

    let Some(value) = std::env::var_os(SCENARIO_ENV) else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    let Ok(scenario) = value.into_string() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    run_scenario(&scenario);
}

/// Reads exactly one request through the real shared codec.
fn read_one_request() -> Option<(u64, HelperRequest)> {
    let mut stdin = std::io::stdin().lock();
    read_request(&mut stdin).ok()
}

/// Writes and flushes one encoded response frame to stdout.
fn emit(frame: &[u8]) {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(frame)
        .and_then(|()| stdout.flush())
        .expect("fixture stdout delivery");
}

/// Encodes one response frame through the real shared encoder.
fn frame_for(generation: u64, response: &Response) -> Vec<u8> {
    encode_response(generation, response).expect("fixture responses stay in contract")
}

/// Blocks until the parent's stdin read completes, mimicking a helper busy
/// inside its chooser; every completion then ends the child with the
/// lifeline-lost code, making abandonment/cancellation/deadline cleanup
/// causally observable from the observed exit status alone.
fn block_on_lifeline_then_exit() -> ! {
    let mut breach = [0_u8; 1];
    let _outcome = std::io::stdin().lock().read(&mut breach);
    process::exit(FIXTURE_EXIT_LIFELINE_LOST);
}

/// Canonical directory text for success fixtures: the existing process
/// working directory, canonicalized, with nothing created and nothing
/// leaked.
fn working_directory_text() -> String {
    let current = std::env::current_dir().expect("working directory should resolve");
    current
        .canonicalize()
        .expect("existing directory should canonicalize")
        .to_str()
        .expect("canonical text stays unicode")
        .to_owned()
}

/// Dispatches one deterministic scenario; every handler ends the process.
fn run_scenario(name: &str) -> ! {
    match name {
        "pick_success" => pick_success(),
        "validate_success" => validate_success(),
        "cancelled"
        | "invalid_path"
        | "unsupported_encoding"
        | "unsupported_platform"
        | "dialog_failed" => empty_payload_outcome(name),
        "malformed_magic" => malformed_magic(),
        "truncated" => truncated(),
        "trailing" => trailing(),
        "oversized_payload" => oversized_payload(),
        "stale_generation" => stale_generation(),
        "stderr_flood" => stderr_flood(),
        "exit_nonzero" => exit_nonzero(),
        "hang_until_lifeline" => hang_until_lifeline(),
        _ => process::exit(SCENARIO_REFUSED_EXIT),
    }
}

/// Answers one `Pick` with the canonicalized process working directory.
fn pick_success() -> ! {
    let Some((generation, HelperRequest::Pick)) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    let canonical_path = working_directory_text();
    emit(&frame_for(
        generation,
        &Response::Selected { canonical_path },
    ));
    process::exit(0);
}

/// Canonicalizes the PARENT-OWNED anchor received over the wire.
fn validate_success() -> ! {
    let Some((generation, HelperRequest::Validate { path_text })) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    // The candidate is a PARENT-OWNED anchor directory that stays alive
    // through this child's reap; nothing is created here.
    let Ok(canonical) = std::fs::canonicalize(&path_text) else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    let Some(canonical_path) = canonical.to_str() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    emit(&frame_for(
        generation,
        &Response::Selected {
            canonical_path: canonical_path.to_owned(),
        },
    ));
    process::exit(0);
}

/// Answers one `Pick` with the named empty-payload outcome tag.
fn empty_payload_outcome(name: &str) -> ! {
    let Some((generation, HelperRequest::Pick)) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    let response = match name {
        "cancelled" => Response::Cancelled,
        "invalid_path" => Response::InvalidPath,
        "unsupported_encoding" => Response::UnsupportedEncoding,
        "unsupported_platform" => Response::UnsupportedPlatform,
        _ => Response::DialogFailed,
    };
    emit(&frame_for(generation, &response));
    process::exit(0);
}

/// Emits a frame whose magic bytes are foreign.
fn malformed_magic() -> ! {
    let Some((generation, HelperRequest::Pick)) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    let mut frame = vec![b'X'; 4];
    frame.push(1);
    frame.push(1);
    frame.extend_from_slice(&generation.to_le_bytes());
    frame.extend_from_slice(&0_u32.to_le_bytes());
    emit(&frame);
    process::exit(0);
}

/// Emits only the first ten bytes of a complete frame.
fn truncated() -> ! {
    let Some((_generation, HelperRequest::Pick)) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    let full = frame_for(7, &Response::Cancelled);
    emit(&full[..10]);
    process::exit(0);
}

/// Emits one complete frame followed by a trailing byte.
fn trailing() -> ! {
    let Some((generation, HelperRequest::Pick)) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    let mut frame = frame_for(generation, &Response::Cancelled);
    frame.push(b'!');
    emit(&frame);
    process::exit(0);
}

/// Declares a payload beyond the shared bound in an otherwise sound header.
fn oversized_payload() -> ! {
    let Some((generation, HelperRequest::Pick)) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    let declared = u32::try_from(ROOT_PATH_MAX_BYTES + 1).expect("bound plus one fits");
    let mut frame = b"ASDR".to_vec();
    frame.push(1);
    frame.push(1);
    frame.extend_from_slice(&generation.to_le_bytes());
    frame.extend_from_slice(&declared.to_le_bytes());
    frame.extend_from_slice(&[b'a'; 16]);
    emit(&frame);
    process::exit(0);
}

/// Deterministic, generation-keyed staleness with checked arithmetic: the
/// very first operation of a controller carries generation 1 and is
/// answered one ahead (stale); every later generation is echoed exactly,
/// proving a stale response cannot settle later work.
fn stale_generation() -> ! {
    let Some((generation, HelperRequest::Pick)) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    if generation == 1 {
        let Some(stale) = generation.checked_add(1) else {
            process::exit(SCENARIO_REFUSED_EXIT);
        };
        emit(&frame_for(stale, &Response::Cancelled));
    } else {
        emit(&frame_for(generation, &Response::Cancelled));
    }
    process::exit(0);
}

/// Floods stderr past the parent's count-only cap, then answers normally.
fn stderr_flood() -> ! {
    let Some((generation, HelperRequest::Pick)) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    let mut stderr = std::io::stderr().lock();
    let noise = [b'e'; 512];
    for _ in 0..20 {
        let _written = stderr.write_all(&noise);
        let _flushed = stderr.flush();
    }
    drop(stderr);
    emit(&frame_for(generation, &Response::Cancelled));
    process::exit(0);
}

/// Completes a well-formed exchange, then exits nonzero.
fn exit_nonzero() -> ! {
    let Some((generation, HelperRequest::Pick)) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    emit(&frame_for(generation, &Response::Cancelled));
    process::exit(7);
}

/// Signals readiness on stderr, then blocks on the lifeline like a helper
/// busy inside its chooser.
fn hang_until_lifeline() -> ! {
    let Some((_generation, _request)) = read_one_request() else {
        process::exit(SCENARIO_REFUSED_EXIT);
    };
    // One readiness byte on stderr gives the parent's count-only witness
    // causal protocol-readiness evidence: the request was consumed and
    // chooser work is in progress.
    {
        let mut stderr = std::io::stderr().lock();
        let _written = stderr.write_all(b"R");
        let _flushed = stderr.flush();
    }
    block_on_lifeline_then_exit();
}
