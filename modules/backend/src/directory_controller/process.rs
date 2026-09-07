//! Private process and bounded-stream ownership for the directory
//! controller.
//!
//! Everything here serves the single owner task in
//! [`crate::directory_controller::operation`]: exact-child spawning, the sole
//! stdin lifeline writer, bounded-before-allocation stdout framing, count-only
//! stderr draining, and the fixed cleanup sequence that ends in an observed
//! reap or retained custody. No detached per-pipe reader tasks exist; every
//! pump is one small cancel-safe step the owner interleaves with the child
//! wait and its control signals on one task.
//!
//! Payload hygiene: nothing in this module retains stderr bytes, formats
//! operating-system error strings into public values, or exposes executable
//! or filesystem paths. Raw [`io::Error`] values stay private to the owner
//! and are reduced to typed causes upstream.

#[cfg(test)]
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};

use crate::directory_helper::INTERNAL_DIRECTORY_HELPER_FLAG;
use crate::directory_helper_codec::{HEADER_LEN, ResponseHeaderFault, parse_response_header};

/// Fixed independent cleanup budget: how long the owner waits after closing
/// the lifeline before requesting termination.
pub(crate) const CLEANUP_GRACE: Duration = Duration::from_secs(5);

/// Fixed independent cleanup budget: how long the owner waits after a
/// termination request before declaring the reap unobservable.
pub(crate) const CLEANUP_KILL_GRACE: Duration = Duration::from_secs(5);

/// Hard stderr bound: at most this many bytes are counted; content is never
/// retained. One final crossing read acts as the cap sentinel.
pub(crate) const STDERR_CAP_BYTES: usize = 8192;

/// Scratch size for one stderr counting read; the cap above stays the real
/// bound because counting stops as soon as it is crossed.
const STDERR_CHUNK: usize = 512;

/// The exact executable launch the owner task performs.
///
/// Production always launches the caller-supplied absolute Forge executable
/// with only the helper-mode flag. The `cfg(test)` variant exists solely for
/// the private headless fixtures described in the root contract: it targets
/// the declared test-only protocol binary, resolved through Bazel runfiles,
/// with a child-only scenario environment variable. It is never constructed
/// outside tests, and production argv/environment behavior is untouched.
pub(crate) enum LaunchRecipe {
    /// The trusted production Forge executable path supplied by the caller.
    Production {
        /// Exact absolute executable path; never replaced or discovered.
        executable: PathBuf,
    },
    /// Test-only protocol-child recipe for real child/pipe/reap coverage.
    #[cfg(test)]
    Fixture {
        /// The declared protocol fixture executable resolved through runfiles.
        program: PathBuf,
        /// Explicit fixture arguments (empty for the current protocol fixture).
        args: Vec<OsString>,
        /// Child-only scenario name carried in [`FIXTURE_SCENARIO_ENV`].
        scenario: &'static str,
    },
}

/// Environment variable naming the child-side fixture role.
///
/// Defined (and read) only under `cfg(test)`; production builds never carry
/// the name and the helper mode never consults the environment.
#[cfg(test)]
pub(crate) const FIXTURE_SCENARIO_ENV: &str = "ARTISAN_DIRECTORY_CONTROLLER_TEST_SCENARIO";

/// Spawns one helper child from the supplied recipe.
///
/// All three standard streams are piped, `kill_on_drop(true)` is set so the
/// final `Child` drop is best-effort containment, and on Windows
/// `CREATE_NO_WINDOW` is applied through the safe pinned Tokio `Command` API
/// so no console window appears. The environment is inherited unchanged and
/// no shell, PATH lookup, sibling discovery, or job object is involved.
///
/// Synchronous operating-system spawning has no absolute wall-time guarantee;
/// the caller runs this on its owner task, never on a UI thread.
///
/// # Errors
///
/// Returns the raw spawn failure; the caller reduces it to a typed,
/// payload-free cause and never surfaces the operating-system message.
pub(crate) fn spawn_helper(recipe: &LaunchRecipe) -> io::Result<Child> {
    let mut command = match recipe {
        LaunchRecipe::Production { executable } => {
            let mut command = tokio::process::Command::new(executable);
            command.arg(INTERNAL_DIRECTORY_HELPER_FLAG);
            command
        }
        #[cfg(test)]
        LaunchRecipe::Fixture {
            program,
            args,
            scenario,
        } => {
            let mut command = tokio::process::Command::new(program);
            command.env(FIXTURE_SCENARIO_ENV, scenario);
            for argument in args {
                command.arg(argument);
            }
            command
        }
    };

    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let child = command.spawn()?;
    witness_spawned();
    Ok(child)
}

/// The taken sole stdin writer kept open for the whole operation.
///
/// The writer is removed from the [`Child`] immediately after spawn so the
/// child's own `wait()` can never close it implicitly; it closes exactly
/// when the owner drops it—normally only after an observed exit, or first
/// in every abnormal cleanup sequence.
pub(crate) struct LifelineWriter(Option<ChildStdin>);

impl LifelineWriter {
    /// Takes the child's piped stdin as the sole lifeline writer.
    pub(crate) fn take(child: &mut Child) -> Self {
        Self(child.stdin.take())
    }

    /// Writes and flushes exactly one request frame.
    ///
    /// # Errors
    ///
    /// Returns the raw write failure; the pipe may be broken, in which case
    /// the helper's own read observation becomes its lifeline-lost ending.
    pub(crate) async fn write_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        let Some(writer) = self.0.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "lifeline writer already closed",
            ));
        };
        writer.write_all(frame).await?;
        writer.flush().await
    }

    /// Closes the sole writer end explicitly.
    ///
    /// Per the helper contract, completing the parent's stdin read is what
    /// tells a working helper the operation ended; every abnormal sequence
    /// calls this before waiting or killing.
    pub(crate) fn close(&mut self) {
        self.0 = None;
    }
}

/// One bounded step of response-frame acquisition from stdout.
///
/// The reader validates the echoed generation against this operation's
/// expected value *before* any payload allocation, so a stale or hostile
/// header can never trigger memory use. It allocates only after the
/// eighteen-byte header parsed within every structural rule. One [`Self::pump`]
/// call performs at most one operating-system read (plus delivery of an
/// already-decoded frame); the future resolves as soon as that read settles,
/// making interleaving with the child wait and control signals exact rather
/// than approximated.
pub(crate) struct BoundedStdoutReader {
    stdout: Option<ChildStdout>,
    expected_generation: u64,
    header: Box<[u8; HEADER_LEN]>,
    header_filled: usize,
    stage: StdoutStage,
}

enum StdoutStage {
    /// Collecting the eighteen-byte header.
    ReadingHeader,
    /// Header accepted; collecting the declared payload.
    ReadingPayload {
        tag: u8,
        payload: Vec<u8>,
        filled: usize,
    },
    /// Frame bytes complete; the decoded frame is delivered on the next
    /// pump, then the one trailing-byte sentinel is read.
    AwaitingTrailer { ready_frame: Option<(u8, Vec<u8>)> },
    /// Terminal; further pumps are idempotent.
    Done,
}

/// What one stdout pump step observed.
pub(crate) enum StdoutEvent {
    /// Progress was made; the frame is not complete yet.
    NeedMore,
    /// Exactly one structurally valid frame with this operation's own
    /// generation was consumed. Delivered for every tag, including the
    /// empty-payload tags 2 through 6.
    FrameReady {
        /// Raw outcome tag within the six defined values.
        tag: u8,
        /// Bounded payload bytes (empty for every tag except `Selected`).
        payload: Vec<u8>,
    },
    /// End of stream arrived with no extra byte after a complete frame.
    EofClean,
    /// End of stream arrived mid-frame.
    Truncated,
    /// At least one byte followed the complete frame.
    Trailing,
    /// The header violated a structural rule before any payload allocation.
    Malformed(ResponseHeaderFault),
    /// The header echoed another generation; nothing was allocated.
    StaleGeneration,
    /// The operating-system read failed.
    ReadFailed,
}

impl BoundedStdoutReader {
    /// Wraps the child's piped stdout for one expected generation.
    pub(crate) fn new(stdout: Option<ChildStdout>, expected_generation: u64) -> Self {
        Self {
            stdout,
            expected_generation,
            header: Box::new([0_u8; HEADER_LEN]),
            header_filled: 0,
            stage: StdoutStage::ReadingHeader,
        }
    }

    /// Performs at most one bounded read toward the single response frame.
    ///
    /// Cancellation safety: every stage keeps its complete state inside
    /// `self` across the one awaited operating-system read; transitions are
    /// written back synchronously after that read resolves. Another owner
    /// select branch winning therefore drops nothing — the next pump call
    /// resumes the exact partial header, payload, or trailer state.
    pub(crate) async fn pump(&mut self) -> StdoutEvent {
        let Some(stdout) = self.stdout.as_mut() else {
            self.stage = StdoutStage::Done;
            return StdoutEvent::ReadFailed;
        };
        let slot = &mut self.stage;
        match slot {
            StdoutStage::Done => StdoutEvent::EofClean,
            StdoutStage::ReadingHeader => {
                let read = stdout.read(&mut self.header[self.header_filled..]).await;
                match read {
                    Ok(0) => {
                        *slot = StdoutStage::Done;
                        StdoutEvent::Truncated
                    }
                    Ok(count) => {
                        self.header_filled += count;
                        if self.header_filled < HEADER_LEN {
                            return StdoutEvent::NeedMore;
                        }
                        let (next, event) = Self::decide_after_header(
                            self.header.as_ref(),
                            self.expected_generation,
                        );
                        *slot = next;
                        event
                    }
                    Err(_) => {
                        *slot = StdoutStage::Done;
                        StdoutEvent::ReadFailed
                    }
                }
            }
            StdoutStage::ReadingPayload {
                tag,
                payload,
                filled,
            } => {
                let read = stdout.read(&mut payload[*filled..]).await;
                match read {
                    Ok(0) => {
                        *slot = StdoutStage::Done;
                        StdoutEvent::Truncated
                    }
                    Ok(count) => {
                        *filled += count;
                        if *filled < payload.len() {
                            return StdoutEvent::NeedMore;
                        }
                        // Fully collected within its validated bound; queue
                        // delivery before reading the trailing sentinel.
                        let tag = *tag;
                        let payload = std::mem::take(payload);
                        *slot = StdoutStage::AwaitingTrailer {
                            ready_frame: Some((tag, payload)),
                        };
                        StdoutEvent::NeedMore
                    }
                    Err(_) => {
                        *slot = StdoutStage::Done;
                        StdoutEvent::ReadFailed
                    }
                }
            }
            StdoutStage::AwaitingTrailer { ready_frame } => {
                if let Some((tag, payload)) = ready_frame.take() {
                    // Deliver the decoded frame — every tag, including the
                    // empty-payload responses — before the sentinel read.
                    return StdoutEvent::FrameReady { tag, payload };
                }
                // One overflow/trailing sentinel byte beyond the frame: a
                // clean end of stream proves no trailing output exists.
                let mut sentinel = [0_u8; 1];
                match stdout.read(&mut sentinel).await {
                    Ok(0) => {
                        *slot = StdoutStage::Done;
                        StdoutEvent::EofClean
                    }
                    Ok(_) => {
                        *slot = StdoutStage::Done;
                        StdoutEvent::Trailing
                    }
                    Err(_) => {
                        *slot = StdoutStage::Done;
                        StdoutEvent::ReadFailed
                    }
                }
            }
        }
    }

    /// Decides the follow-on stage once all eighteen header bytes exist.
    ///
    /// Structural parsing and generation correlation happen here, BEFORE
    /// any payload allocation: a malformed header or stale generation is
    /// terminal at zero payload cost, and only an accepted header may
    /// allocate its exact declared, bound-checked payload. This is purely
    /// synchronous; the caller writes the returned stage back into `self`
    /// immediately after its one awaited read resolved, so no state ever
    /// sits in a local across an await.
    fn decide_after_header(header: &[u8], expected_generation: u64) -> (StdoutStage, StdoutEvent) {
        match parse_response_header(header) {
            Err(fault) => (StdoutStage::Done, StdoutEvent::Malformed(fault)),
            Ok(prelude) => {
                // Correlation is decided before anything is allocated: a
                // stale response costs zero payload memory and settles
                // nothing later.
                if prelude.generation != expected_generation {
                    return (StdoutStage::Done, StdoutEvent::StaleGeneration);
                }
                let next = if prelude.payload_len == 0 {
                    StdoutStage::AwaitingTrailer {
                        ready_frame: Some((prelude.tag, Vec::new())),
                    }
                } else {
                    StdoutStage::ReadingPayload {
                        tag: prelude.tag,
                        payload: vec![0_u8; prelude.payload_len],
                        filled: 0,
                    }
                };
                (next, StdoutEvent::NeedMore)
            }
        }
    }
}

/// Count-only stderr draining state.
///
/// Bytes are counted toward [`STDERR_CAP_BYTES`] and discarded; nothing is
/// ever retained, printed, or formatted. Once the cap is crossed reads stop
/// permanently while the read handle stays retained, so exact pipe custody
/// still moves whole into quarantine retention.
pub(crate) struct StderrCounter {
    stderr: Option<ChildStderr>,
    counted: usize,
    state: StderrState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StderrState {
    /// The stream is open and within the cap.
    Open,
    /// The cap was crossed; reads stopped while the handle stays retained.
    Capped,
    /// Clean end of stream within the cap.
    ClosedWithinCap,
    /// The operating-system read failed.
    Failed,
}

/// What one stderr pump step observed.
pub(crate) enum StderrEvent {
    /// More bytes were counted; the stream stays open within the cap.
    WithinCap,
    /// The cap was crossed; counting stopped permanently.
    CapExceeded,
    /// Clean end of stream within the cap.
    Closed,
    /// The operating-system read failed.
    ReadFailed,
}

impl StderrCounter {
    /// Wraps the child's piped stderr.
    pub(crate) fn new(stderr: Option<ChildStderr>) -> Self {
        Self {
            stderr,
            counted: 0,
            state: StderrState::Open,
        }
    }

    /// Current counting state.
    pub(crate) fn state(&self) -> StderrState {
        self.state
    }

    /// Performs at most one bounded counting read.
    ///
    /// The read window is the remaining allowed bytes plus exactly one
    /// crossing sentinel, so a misbehaving child can overshoot the cap by
    /// no more than that single sentinel byte. Terminal states stop further
    /// reads but never discard the pipe handle: exact custody moves whole
    /// through abnormal cleanup into quarantine retention.
    pub(crate) async fn pump(&mut self) -> StderrEvent {
        match self.state {
            StderrState::Capped => return StderrEvent::CapExceeded,
            StderrState::ClosedWithinCap => return StderrEvent::Closed,
            StderrState::Failed => return StderrEvent::ReadFailed,
            StderrState::Open => {}
        }
        let Some(stderr) = self.stderr.as_mut() else {
            self.state = StderrState::Failed;
            return StderrEvent::ReadFailed;
        };
        let remaining_allowed = STDERR_CAP_BYTES.saturating_sub(self.counted);
        let window = remaining_allowed.min(STDERR_CHUNK) + 1;
        let mut chunk = [0_u8; STDERR_CHUNK + 1];
        match stderr.read(&mut chunk[..window]).await {
            Ok(0) => {
                self.state = StderrState::ClosedWithinCap;
                StderrEvent::Closed
            }
            Ok(count) => {
                {
                    #[cfg(test)]
                    witness_stderr_bytes(count);
                }
                self.counted += count;
                if self.counted > STDERR_CAP_BYTES {
                    // The one crossing read acted as the cap sentinel; its
                    // bytes are counted and discarded, nothing retained.
                    self.state = StderrState::Capped;
                    StderrEvent::CapExceeded
                } else {
                    StderrEvent::WithinCap
                }
            }
            Err(_) => {
                self.state = StderrState::Failed;
                StderrEvent::ReadFailed
            }
        }
    }
}

/// Exact custody retained when a child's death could not be observed.
///
/// The already-closed lifeline writer and both pipe reader states stay
/// owned alongside the exact child until a later observed reap releases
/// them; quarantine never discards pipe handles early.
pub(crate) struct RetainedHelper {
    /// The exact child, boxed for long-term ownership.
    pub(crate) child: Box<Child>,
    /// The closed sole writer, kept as part of exact custody.
    pub(crate) lifeline: LifelineWriter,
    /// The stdout reader state at abandonment.
    pub(crate) stdout_reader: BoundedStdoutReader,
    /// The stderr counting state at abandonment.
    pub(crate) stderr_counter: StderrCounter,
}

/// How an abnormal sequence ended.
pub(crate) enum CleanupObservation {
    /// The child exited and was reaped within the first grace period.
    ReapedWithoutKill(ExitStatus),
    /// Termination was requested and the exit was then actually observed.
    ReapedAfterKill(ExitStatus),
    /// No reap could be observed within the bounded budgets; exact custody
    /// of the child and its pipes is returned (boxed) for quarantine
    /// retention.
    Retained(Box<RetainedHelper>),
}

/// Runs the fixed, uncancellable cleanup sequence for one unresolved child.
///
/// Order is contractual: close the sole lifeline writer FIRST, then wait up
/// to [`CLEANUP_GRACE`], request termination with `start_kill` if no exit
/// was observed, then wait up to [`CLEANUP_KILL_GRACE`]. Pipe resources are
/// released only together with a successful observed reap or moved whole
/// into [`RetainedHelper`]. A termination-request error is harmless only if
/// the subsequent bounded wait actually observes an exit. Once started,
/// this sequence cannot be cancelled by the operation's control signals.
pub(crate) async fn cleanup_after_abort(parts: ChildParts) -> CleanupObservation {
    let ChildParts {
        mut child,
        mut lifeline,
        stdout_reader,
        stderr_counter,
    } = parts;

    // The helper's own watcher learns of this ending through its next stdin
    // read; closing the sole writer is therefore always step one.
    lifeline.close();

    if let Ok(status) = wait_bounded(&mut child, CLEANUP_GRACE).await {
        witness_reaped(status);
        drop(stdout_reader);
        drop(stderr_counter);
        drop(lifeline);
        return CleanupObservation::ReapedWithoutKill(status);
    }

    witness_kill_requested();
    let _termination_error = child.start_kill();
    if let Ok(status) = wait_bounded(&mut child, CLEANUP_KILL_GRACE).await {
        witness_reaped(status);
        drop(stdout_reader);
        drop(stderr_counter);
        drop(lifeline);
        return CleanupObservation::ReapedAfterKill(status);
    }
    CleanupObservation::Retained(Box::new(RetainedHelper {
        child: Box::new(child),
        lifeline,
        stdout_reader,
        stderr_counter,
    }))
}

/// The complete set of resources one active operation owns.
pub(crate) struct ChildParts {
    /// The exact child; kept owned until an observed reap or quarantine.
    pub(crate) child: Child,
    /// The sole stdin writer.
    pub(crate) lifeline: LifelineWriter,
    /// Bounded stdout reader state.
    pub(crate) stdout_reader: BoundedStdoutReader,
    /// Count-only stderr state.
    pub(crate) stderr_counter: StderrCounter,
}

/// Waits for the child to exit within a fixed budget.
///
/// # Errors
///
/// Returns a timed-out error when the budget elapses first and the raw wait
/// failure when the operating-system wait itself fails; both keep the child
/// owned by the caller.
async fn wait_bounded(child: &mut Child, limit: Duration) -> io::Result<ExitStatus> {
    match tokio::time::timeout(limit, child.wait()).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "bounded wait elapsed",
        )),
    }
}

/// Awaits the death of one retained helper once, without a deadline.
///
/// Custody is released only when the wait actually observes an exit; a
/// failed wait hands the whole retained custody back so the owner can park
/// while retaining it. No retry loop exists.
pub(crate) async fn eventual_wait_once(
    mut retained: Box<RetainedHelper>,
) -> Result<ExitStatus, Box<RetainedHelper>> {
    match retained.child.wait().await {
        Ok(status) => {
            witness_reaped(status);
            Ok(status)
        }
        Err(_) => Err(retained),
    }
}

// ---------------------------------------------------------------------------
// Private cfg(test) bounded phase witnesses.
//
// These counters sit directly on the real controller path (spawn, kill
// request, observed reap) and exist purely so the private headless tests can
// assert causal facts such as "zero-budget operations never spawned" or
// "cancellation reaped the child without a termination request". They are
// compiled out of production builds entirely.
// ---------------------------------------------------------------------------

/// Witness counts gathered on the actual controller path.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WitnessCounts {
    /// Children successfully spawned.
    pub(crate) spawned: u64,
    /// Termination requests issued by cleanup sequences.
    pub(crate) kills_requested: u64,
    /// Exits actually observed through a completed wait.
    pub(crate) reaps_observed: u64,
    /// Stderr bytes counted on the real pipe; a hanging fixture emits one
    /// readiness byte after consuming its request, so a nonzero count is
    /// causal protocol-readiness evidence, never mere spawn bookkeeping.
    pub(crate) stderr_bytes_seen: u64,
    /// Exit code of the MOST RECENTLY observed reap (`-1` when none yet).
    /// A count alone cannot distinguish a fixture watchdog exit from the
    /// intended lifeline-lost ending; this preserves the actual code so
    /// tests can assert the causal ending they claim.
    pub(crate) last_exit_code: i32,
    /// How many observed reaps ended with the fixture watchdog exit (99).
    /// Watchdog containment is ALWAYS failure; this bounded counter lets
    /// every scenario prove none of its reaps hid such an ending behind a
    /// later overwrite of [`Self::last_exit_code`].
    pub(crate) watchdog_failures_seen: u64,
}

/// Witness sentinel: no exit observed yet.
#[cfg(test)]
pub(crate) const NO_EXIT_CODE_WITNESSED: i32 = -1;

/// The fixture watchdog exit code mirrored for witness classification.
#[cfg(test)]
pub(crate) const WATCHDOG_FAILURE_EXIT: i32 = 99;

#[cfg(test)]
static WITNESS_SPAWNED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
static WITNESS_KILLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
static WITNESS_REAPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
static WITNESS_STDERR_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
static WITNESS_LAST_EXIT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
#[cfg(test)]
static WITNESS_WATCHDOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
fn witness_spawned() {
    WITNESS_SPAWNED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
fn witness_kill_requested() {
    WITNESS_KILLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
fn witness_reaped(status: ExitStatus) {
    WITNESS_REAPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Preserve the ACTUAL ending (0 success, 3 lifeline-lost, 7 nonzero,
    // 99 watchdog failure...) so counts never stand in for causal facts.
    let code = status.code().unwrap_or(-2);
    WITNESS_LAST_EXIT.store(code, std::sync::atomic::Ordering::Relaxed);
    if code == WATCHDOG_FAILURE_EXIT {
        WITNESS_WATCHDOG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
fn witness_stderr_bytes(count: usize) {
    WITNESS_STDERR_BYTES.fetch_add(
        u64::try_from(count).unwrap_or(u64::MAX),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Resets and reads the phase witnesses (test-only).
#[cfg(test)]
pub(crate) fn reset_witnesses() {
    WITNESS_SPAWNED.store(0, std::sync::atomic::Ordering::Relaxed);
    WITNESS_KILLS.store(0, std::sync::atomic::Ordering::Relaxed);
    WITNESS_REAPS.store(0, std::sync::atomic::Ordering::Relaxed);
    WITNESS_STDERR_BYTES.store(0, std::sync::atomic::Ordering::Relaxed);
    WITNESS_LAST_EXIT.store(NO_EXIT_CODE_WITNESSED, std::sync::atomic::Ordering::Relaxed);
    WITNESS_WATCHDOG.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// Reads the current phase witnesses (test-only).
#[cfg(test)]
pub(crate) fn witness_counts() -> WitnessCounts {
    let load =
        |counter: &std::sync::atomic::AtomicU64| counter.load(std::sync::atomic::Ordering::Relaxed);
    WitnessCounts {
        spawned: load(&WITNESS_SPAWNED),
        kills_requested: load(&WITNESS_KILLS),
        reaps_observed: load(&WITNESS_REAPS),
        stderr_bytes_seen: load(&WITNESS_STDERR_BYTES),
        last_exit_code: WITNESS_LAST_EXIT.load(std::sync::atomic::Ordering::Relaxed),
        watchdog_failures_seen: load(&WITNESS_WATCHDOG),
    }
}

/// Records one exit observed on the SUCCESS path of the owner loop, where
/// no cleanup sequence runs (test-only).
#[cfg(test)]
pub(crate) fn note_observed_reap_for_tests(status: ExitStatus) {
    witness_reaped(status);
}

#[cfg(not(test))]
fn witness_spawned() {}
#[cfg(not(test))]
fn witness_kill_requested() {}
#[cfg(not(test))]
fn witness_reaped(_status: ExitStatus) {}
