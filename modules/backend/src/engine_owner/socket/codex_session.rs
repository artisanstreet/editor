//! Opened Codex app-server session for the socket seam.
//!
//! The open phase spawns the verified `codex app-server --stdio` child under
//! the owner custody contract and performs the complete preflight handshake
//! (`initialize`, the `initialized` notification, then `thread/start` — or
//! `thread/resume` for a gated continuation). The returned
//! [`CodexOpenSession`] owns the child, its sole stdin writer, the bounded
//! stdout reader, and the count-only stderr state; the configured drive phase
//! consumes it through [`CodexDriveSession`] and never spawns a second child.
//!
//! Failure stays honest: every spawn/handshake failure cleans the child up
//! with the same bounded abort sequence the drive phase uses, and a child
//! whose death cannot be observed inside the close budget is returned to the
//! owner as quarantined custody on [`EngineOpenOutcome::Failed`].

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::any::Any;
use std::time::Duration;

use artisan_domain::{
    EngineObservationTag, EngineOpenError, EngineOpenInput, EngineOpenOutcome, EngineResumeToken,
    EngineRun, EngineSelection, EngineSocketSession, RootPath,
};
use artisan_native_engine::VerifiedCodexLaunch;
use tokio::io::BufReader;
use tokio::process::{ChildStdin, ChildStdout};
use tokio::time::Instant;

use super::super::codex as codex_runtime;
use super::super::codex::CodexSettings;
use super::super::process::{
    ChildParts, CleanupObservation, LifelineWriter, RetainedEngine, StderrCounter,
    cleanup_after_abort, spawn_codex_engine,
};
use super::SocketTurnContext;

/// Observation tag named by an opened Codex run.
///
/// Codex emits its run-level state frames under the shared `run_state`
/// vocabulary tag; the run handle names the same tag.
const CODEX_RUN_OBSERVATION_TAG: EngineObservationTag = EngineObservationTag::RunState;

/// Runtime-owned custody of one opened Codex session.
///
/// Fields stay private: the drive phase consumes the whole session through
/// [`CodexOpenSession::into_drive`] instead of reaching into individual
/// handles, so the sole stdin lifeline and the stdout reader can never be
/// separated from the child they belong to.
pub(crate) struct CodexOpenSession {
    parts: ChildParts,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    thread_id: String,
    next_id: u64,
    settings: CodexSettings,
}

impl EngineSocketSession for CodexOpenSession {
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
        self
    }
}

impl CodexOpenSession {
    /// Consumes the opened session into the drive-phase parts.
    #[must_use]
    pub(crate) fn into_drive(self) -> CodexDriveSession {
        CodexDriveSession {
            parts: self.parts,
            stdin: self.stdin,
            reader: self.reader,
            thread_id: self.thread_id,
            next_id: self.next_id,
            settings: self.settings,
        }
    }
}

/// Everything the drive phase owns after a successful open.
pub(crate) struct CodexDriveSession {
    /// Exact child custody plus the closed lifeline slot and stderr state.
    pub(crate) parts: ChildParts,
    /// The sole stdin writer for the whole turn.
    pub(crate) stdin: ChildStdin,
    /// Bounded stdout reader positioned after the handshake.
    pub(crate) reader: BufReader<ChildStdout>,
    /// Native provider thread identity returned by the handshake.
    pub(crate) thread_id: String,
    /// Next JSON-RPC request id; the handshake consumed 1 and 2.
    pub(crate) next_id: u64,
    /// Typed Codex settings derived for this run's selection.
    pub(crate) settings: CodexSettings,
}

/// Quarantine bridge: a child whose death could not be observed inside the
/// close budget crosses the domain seam as type-erased custody.
impl EngineSocketSession for RetainedEngine {
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
        self
    }
}

/// Outcome of one bounded preflight reply wait.
enum CodexPreflightWait {
    /// The matching result arrived; the raw line stays in the caller's buffer
    /// for id-specific extraction.
    Ready,
    /// Shutdown, cancellation, EOF, or a matching error envelope.
    Failed(EngineOpenError),
}

/// Opens one Codex run: spawn, preflight handshake, native thread identity.
///
/// Applies exactly the persisted phase budget and cancellation signals the
/// drive phase would have applied: the earlier of `prompt_budget` and the
/// attempt deadline bounds every preflight wait, and owner shutdown or
/// caller cancellation aborts the open. A failed open reaps the child
/// through the shared bounded cleanup and never returns silently retained
/// custody.
#[expect(
    clippy::too_many_lines,
    reason = "one linear spawn-and-handshake sequence over the owned child; extraction would thread the same custody state"
)]
pub(crate) async fn open_codex_session(
    launch: &VerifiedCodexLaunch,
    context: &SocketTurnContext<'_>,
    input: EngineOpenInput,
) -> EngineOpenOutcome {
    let EngineSelection::Codex(selection) = context.settings.config().selection() else {
        return EngineOpenOutcome::failed(EngineOpenError::InvalidInput);
    };
    let Ok(settings) = CodexSettings::from_selection(selection) else {
        return EngineOpenOutcome::failed(EngineOpenError::InvalidInput);
    };
    if settings.profile_id() != launch.profile_id().as_str() {
        return EngineOpenOutcome::failed(EngineOpenError::InvalidInput);
    }
    let Ok(project_root) = RootPath::parse(input.working_directory.clone()) else {
        return EngineOpenOutcome::failed(EngineOpenError::InvalidInput);
    };
    let Ok(mut child) = spawn_codex_engine(launch, &project_root) else {
        return EngineOpenOutcome::failed(EngineOpenError::SpawnFailed);
    };
    let stdin_opt = child.stdin.take();
    let stdout_opt = child.stdout.take();
    let stderr_opt = child.stderr.take();
    let lifeline = LifelineWriter::take(&mut child);
    let stderr_counter = StderrCounter::new(stderr_opt, context.bounds.stderr_cap_bytes);
    let (Some(stdin), Some(stdout)) = (stdin_opt, stdout_opt) else {
        let parts = ChildParts {
            child,
            lifeline,
            stdout: None,
            stderr_counter,
        };
        return fail_open(parts, EngineOpenError::SpawnFailed, context.limits.close).await;
    };
    let parts = ChildParts {
        child,
        lifeline,
        stdout: None,
        stderr_counter,
    };
    let mut reader = BufReader::new(stdout);
    let mut stdin = stdin;
    let mut line = String::new();
    let mut next_id: u64 = 1;

    // initialize ---------------------------------------------------------
    let init_line = codex_runtime::request_line(
        next_id,
        "initialize",
        &codex_runtime::initialize_params("artisan-editor", "0.3.0"),
    );
    next_id += 1;
    if codex_runtime::write_codex_line(&mut stdin, &init_line)
        .await
        .is_err()
    {
        return fail_open(
            parts,
            EngineOpenError::HandshakeFailed,
            context.limits.close,
        )
        .await;
    }
    match await_preflight_result(&mut reader, &mut line, 1, context).await {
        CodexPreflightWait::Ready => {}
        CodexPreflightWait::Failed(error) => {
            return fail_open(parts, error, context.limits.close).await;
        }
    }
    // Official handshake order (`Handshake` in
    // `modules/engines/src/codex/app-server-session.ts`): the client notifies
    // `initialized` (no id, no params) once the `initialize` result arrives,
    // before any `thread/*` request. A notification never consumes a request
    // id, so `next_id` still names the `thread/*` request below.
    let initialized_line = codex_runtime::notification_line("initialized");
    if codex_runtime::write_codex_line(&mut stdin, &initialized_line)
        .await
        .is_err()
    {
        return fail_open(
            parts,
            EngineOpenError::HandshakeFailed,
            context.limits.close,
        )
        .await;
    }
    // thread/start or thread/resume ---------------------------------------
    // A gated continuation reopens the stored provider thread
    // (`thread/resume` over the same options a fresh start would use); the
    // response must name the same thread id or the open fails closed. Fresh
    // turns start exactly one thread. Either way provider-owned state is
    // resumed, never invented, and a restart never duplicates provider
    // effects with a second thread.
    let thread_id = if let Some(stored) = input
        .resume
        .as_ref()
        .map(|token| token.native_thread_id.as_str())
    {
        let Some(resume_params) =
            codex_runtime::thread_resume_params(&settings, &project_root, stored)
        else {
            return fail_open(parts, EngineOpenError::InvalidInput, context.limits.close).await;
        };
        let thread_request_id = next_id;
        let resume_line =
            codex_runtime::request_line(thread_request_id, "thread/resume", &resume_params);
        next_id += 1;
        if codex_runtime::write_codex_line(&mut stdin, &resume_line)
            .await
            .is_err()
        {
            return fail_open(
                parts,
                EngineOpenError::HandshakeFailed,
                context.limits.close,
            )
            .await;
        }
        match await_preflight_result(&mut reader, &mut line, thread_request_id, context).await {
            CodexPreflightWait::Ready => {}
            CodexPreflightWait::Failed(error) => {
                return fail_open(parts, error, context.limits.close).await;
            }
        }
        let Some(thread_id) =
            codex_runtime::codex_resumed_thread_id(&line, thread_request_id, stored)
        else {
            return fail_open(parts, EngineOpenError::ResumeRejected, context.limits.close).await;
        };
        thread_id
    } else {
        let thread_request_id = next_id;
        let thread_line = codex_runtime::request_line(
            thread_request_id,
            "thread/start",
            &settings.thread_params(&project_root),
        );
        next_id += 1;
        if codex_runtime::write_codex_line(&mut stdin, &thread_line)
            .await
            .is_err()
        {
            return fail_open(
                parts,
                EngineOpenError::HandshakeFailed,
                context.limits.close,
            )
            .await;
        }
        match await_preflight_result(&mut reader, &mut line, thread_request_id, context).await {
            CodexPreflightWait::Ready => {}
            CodexPreflightWait::Failed(error) => {
                return fail_open(parts, error, context.limits.close).await;
            }
        }
        let Some(thread_id) = codex_runtime::codex_thread_id(&line, thread_request_id) else {
            return fail_open(
                parts,
                EngineOpenError::HandshakeFailed,
                context.limits.close,
            )
            .await;
        };
        thread_id
    };

    EngineOpenOutcome::Opened(EngineRun {
        native_thread_id: EngineResumeToken {
            native_thread_id: thread_id.clone(),
        },
        observation_tag: CODEX_RUN_OBSERVATION_TAG,
        session: Box::new(CodexOpenSession {
            parts,
            stdin,
            reader,
            thread_id,
            next_id,
            settings,
        }),
    })
}

/// Waits for one preflight reply (`initialize`, `thread/start`,
/// `thread/resume`) while surviving interleaved traffic.
///
/// The real server emits notifications (for example `remoteControl/*`,
/// `deprecationNotice`, `mcpStartup`, `threadStatus`, `thread/started`)
/// before the matching result, so every line is correlated by request id
/// instead of assuming the next line is the reply. Non-matching traffic —
/// method notifications, uncorrelated results/errors, unparseable lines —
/// keeps the wait alive inside the same absolute phase deadline; the run is
/// not yet authorized, so nothing is forwarded to the observation sink. A
/// matching error envelope fails fast, and a failed resume never falls back
/// to a fresh start.
async fn await_preflight_result(
    reader: &mut BufReader<ChildStdout>,
    line: &mut String,
    expected_id: u64,
    context: &SocketTurnContext<'_>,
) -> CodexPreflightWait {
    let deadline = phase_deadline(context.limits.prompt, context.attempt_deadline);
    loop {
        if codex_runtime::read_codex_line(reader, line, deadline, context.shutdown, context.control)
            .await
            .is_err()
        {
            let error = if context.shutdown.is_cancelled() {
                EngineOpenError::Shutdown
            } else if context.control.is_cancelled() {
                EngineOpenError::Cancelled
            } else {
                EngineOpenError::HandshakeFailed
            };
            return CodexPreflightWait::Failed(error);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
        if codex_runtime::is_codex_error_response(&trimmed)
            && codex_runtime::codex_response_id_matches(&trimmed, expected_id)
        {
            return CodexPreflightWait::Failed(EngineOpenError::HandshakeFailed);
        }
        if codex_runtime::is_codex_result_for(&trimmed, expected_id) {
            return CodexPreflightWait::Ready;
        }
    }
}

/// Absolute deadline for one handshake phase.
///
/// Mirrors the owner's `phase_deadline` contract exactly: the earlier of
/// `now + budget` and the attempt deadline, with the attempt deadline as the
/// fail-safe when the sum is unrepresentable.
fn phase_deadline(budget: Duration, attempt_deadline: Instant) -> Instant {
    Instant::now()
        .checked_add(budget)
        .map_or(attempt_deadline, |candidate| {
            candidate.min(attempt_deadline)
        })
}

/// Runs the fixed bounded cleanup for a failed open.
///
/// A reaped child yields a custody-free typed failure. A child whose death
/// could not be observed moves whole into the returned outcome so the owner
/// quarantines it exactly as the drive phase would have.
async fn fail_open(
    parts: ChildParts,
    error: EngineOpenError,
    close_budget: Duration,
) -> EngineOpenOutcome {
    match cleanup_after_abort(parts, close_budget).await {
        CleanupObservation::ReapedWithoutKill(_) | CleanupObservation::ReapedAfterKill(_) => {
            EngineOpenOutcome::Failed {
                error,
                custody: None,
            }
        }
        CleanupObservation::Retained(retained) => {
            let custody: Box<dyn EngineSocketSession> = retained;
            EngineOpenOutcome::Failed {
                error,
                custody: Some(custody),
            }
        }
    }
}
