//! Headless real-child lifeline coverage for the internal directory helper.
//!
//! One parent test re-executes this same test binary (`current_exe`) three
//! times—EOF, one trailing byte, and an injected non-EOF read error—and the
//! child branch of the very same test function runs the *extracted
//! production body* [`watch_input_lifeline`] on a helper-local thread with a
//! real child-stdin reader. A readiness marker is written from inside that
//! production read call, so readiness is causal evidence that the body
//! actually invoked `Read`, not a sleep or a thread-creation receipt.
//!
//! Honesty boundaries: the injected-error case is wrapper-injected, not an
//! actual OS pipe-failure proof; finite deadlines and the child watchdog are
//! not hard real-time guarantees; and this component fixture proves nothing
//! about ordinary Forge entry, wire startup, COM, or modal chooser behavior.
//!
//! Ownership: one [`ScenarioRun`] owns the sole stdin writer, the exact
//! `Child`, and the joined stderr monitor together. Every exit path closes
//! the writer, kills/reaps the child as needed, and only then joins the
//! monitor—child death guarantees the bounded reader reaches EOF, so the
//! join cannot deadlock. Child stdout is null so libtest progress cannot
//! reach the readiness channel. Timeouts, watchdog expiry, malformed
//! readiness, and trailing output are failures, never evidence.

use std::env;
use std::io::{self, Read, Write};
use std::process::{self, Child, ChildStderr, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use super::EXIT_CODE_LIFELINE_LOST;
use super::watch_input_lifeline;

/// Environment selector distinguishing fixture children from the parent.
///
/// Set only through `Command::env` on the spawned child; the running test
/// process environment is never mutated. Only an absent variable means
/// parent; unknown or non-Unicode values are child-side protocol failures.
const SCENARIO_VARIABLE: &str = "ARTISAN_DIRECTORY_HELPER_FIXTURE_SCENARIO";
const SCENARIO_EOF: &str = "lifeline-eof";
const SCENARIO_TRAILING: &str = "lifeline-trailing";
const SCENARIO_INJECTED: &str = "lifeline-injected-error";

/// Fixed 8-byte readiness marker written by the wrapper inside production Read.
const READINESS_MARKER: [u8; 8] = *b"LIFELINE";
/// The single trailing byte the parent writes in the trailing scenario.
const TRAILING_BYTE: u8 = 0x21;
/// The fixed control byte that legitimately arms the injected-error mode.
const TRIGGER_BYTE: u8 = 0x7F;

/// Parent budget for receiving the readiness marker.
const READINESS_BUDGET: Duration = Duration::from_secs(5);
/// Parent budget for observing termination after the stimulus.
const TERMINATION_BUDGET: Duration = Duration::from_secs(5);
/// Polling interval while watching the owned child handle.
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Child-local containment watchdog budget.
const WATCHDOG_BUDGET: Duration = Duration::from_secs(15);
/// Hard cap on trailing stderr diagnostics retained after readiness.
const TRAILING_CAP: usize = 1024;
/// Largest single trailing read, always clamped to remaining capacity.
const TRAILING_CHUNK_MAX: usize = 256;

/// Distinct non-lifeline child exit codes used only as failure signals.
/// Watchdog expiry means this fixture child stalled or misbehaved past its
/// budget, whatever the cause; firing is always failure, never a pass.
const WATCHDOG_FIRED_EXIT: i32 = 84;
/// Fixture-local thread/watchdog could not be started in the child.
const CHILD_SETUP_FAILED_EXIT: i32 = 85;
/// Unknown or non-Unicode scenario selector, or wrong trigger byte.
const CHILD_PROTOCOL_FAILED_EXIT: i32 = 86;

/// The FULL libtest path selecting exactly the fixture test in the child.
const FIXTURE_TEST_FILTER: &str = "directory_helper::directory_helper_fixture::lifeline_watcher_ends_the_process_on_eof_trailing_byte_and_injected_error";

/// Why one fixture step failed; labels stay static, never embed child text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepFault {
    /// `current_exe` was unavailable to the parent.
    ExecutablePath,
    /// The child could not be spawned.
    Spawn,
    /// The readiness marker did not arrive within its budget.
    ReadinessTimeout,
    /// Readiness arrived malformed or the marker read failed.
    ReadinessFault(&'static str),
    /// The owned child exited before its stimulus arrived.
    ExitedBeforeStimulus,
    /// The single stimulus byte could not be written or flushed.
    StimulusWrite,
    /// The expected termination did not occur within its budget.
    TerminationTimeout,
    /// `try_wait` errored while polling the owned handle.
    PollIo,
    /// The observed exit code was not the fixed lifeline code.
    WrongExitCode {
        /// The required fixed lifeline exit code.
        expected: i32,
        /// The code the child actually reported, if any.
        actual: Option<i32>,
    },
    /// Explicit reap (kill-if-needed plus successful wait) failed; the child
    /// handle stayed owned for retry.
    ReapIo,
    /// The joined monitor thread panicked instead of reporting.
    MonitorJoin,
    /// Trailing stderr bytes appeared where silence was required.
    UnexpectedTrailingDiagnostics {
        /// How many capped trailing bytes were retained.
        count: usize,
    },
    /// Trailing stderr exceeded the hard diagnostic cap.
    TrailingOverflow,
    /// The trailing stderr read itself errored; not silent silence.
    TrailingReadFault,
}

impl StepFault {
    fn label(self) -> String {
        match self {
            Self::ExecutablePath => "current_exe unavailable".to_owned(),
            Self::Spawn => "fixture child spawn failed".to_owned(),
            Self::ReadinessTimeout => "readiness marker timed out".to_owned(),
            Self::ReadinessFault(reason) => format!("readiness fault: {reason}"),
            Self::ExitedBeforeStimulus => "child exited before its stimulus".to_owned(),
            Self::StimulusWrite => "stimulus write failed".to_owned(),
            Self::TerminationTimeout => "expected termination timed out".to_owned(),
            Self::PollIo => "try_wait errored while polling".to_owned(),
            Self::WrongExitCode { expected, actual } => {
                format!("wrong exit code: expected {expected}, got {actual:?}")
            }
            Self::ReapIo => "explicit reap failed; child handle remains owned".to_owned(),
            Self::MonitorJoin => "stderr monitor join failed".to_owned(),
            Self::UnexpectedTrailingDiagnostics { count } => {
                format!("unexpected trailing stderr diagnostics: {count} bytes")
            }
            Self::TrailingOverflow => "trailing stderr exceeded the diagnostic cap".to_owned(),
            Self::TrailingReadFault => "trailing stderr read failed".to_owned(),
        }
    }
}

/// The single parent-side fixture test; branches into the child under the
/// env-only selector so there is no ignored or no-op test and no recursion.
///
/// Only an ABSENT selector means parent. Any unknown value, or a non-Unicode
/// selector, fails the child with a distinct non-lifeline exit instead of
/// falling back to the parent loop and recursively spawning children.
#[test]
fn lifeline_watcher_ends_the_process_on_eof_trailing_byte_and_injected_error() {
    match env::var(SCENARIO_VARIABLE) {
        Err(env::VarError::NotPresent) => run_parent_scenarios(),
        Ok(value) => match value.as_str() {
            SCENARIO_EOF => run_child_scenario(SCENARIO_EOF),
            SCENARIO_TRAILING => run_child_scenario(SCENARIO_TRAILING),
            SCENARIO_INJECTED => run_child_scenario(SCENARIO_INJECTED),
            _unknown_value => process::exit(CHILD_PROTOCOL_FAILED_EXIT),
        },
        Err(env::VarError::NotUnicode(_)) => process::exit(CHILD_PROTOCOL_FAILED_EXIT),
    }
}

// ---------------------------------------------------------------------------
// Child side
// ---------------------------------------------------------------------------

/// Reader handed to the extracted production body inside the fixture child.
///
/// Announces readiness from *inside* `read`, which is what makes the marker
/// causal evidence that the production body actually invoked `Read`. In the
/// injected scenario it consumes exactly one control byte first, validates
/// that it is the fixed trigger byte, and only then returns a deterministic
/// non-EOF error instead of delegating; otherwise it delegates to the real
/// child standard input unchanged.
struct AnnouncingReader {
    inner: io::Stdin,
    announced: bool,
    injected: bool,
}

impl Read for AnnouncingReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.announced {
            self.announced = true;
            let mut stderr = io::stderr();
            stderr.write_all(&READINESS_MARKER)?;
            stderr.flush()?;
            if self.injected {
                // Wait for the parent's control byte on real stdin, accept
                // only the fixed trigger, then fail deterministically rather
                // than delegating. Foreign input is a protocol failure, not a
                // substitute trigger.
                let mut trigger = [0_u8; 1];
                self.inner.read_exact(&mut trigger)?;
                if trigger != [TRIGGER_BYTE] {
                    process::exit(CHILD_PROTOCOL_FAILED_EXIT);
                }
                return Err(io::Error::other("fixture-injected non-eof read failure"));
            }
        }
        self.inner.read(buf)
    }
}

/// Runs one fixture child scenario: watchdog, then the exact production body.
fn run_child_scenario(scenario: &'static str) {
    // Containment watchdog for THIS fixture child: any stalled, hung, or
    // misbehaving scenario past the budget trips it, whatever the cause.
    // Firing is always failure evidence; it is never a success path.
    let watchdog = thread::Builder::new()
        .name("directory-helper-fixture-watchdog".to_owned())
        .spawn(move || {
            thread::sleep(WATCHDOG_BUDGET);
            process::exit(WATCHDOG_FIRED_EXIT);
        });
    if watchdog.is_err() {
        process::exit(CHILD_SETUP_FAILED_EXIT);
    }

    // The SAME extracted production read-and-exit function drives a real
    // stdin-backed wrapper; it diverges through the fixed lifeline exit code.
    let body = thread::Builder::new()
        .name("directory-helper-fixture-body".to_owned())
        .spawn(move || {
            watch_input_lifeline(AnnouncingReader {
                inner: io::stdin(),
                announced: false,
                injected: scenario == SCENARIO_INJECTED,
            })
        });
    match body {
        Ok(handle) => {
            // The production body never returns; joining parks this thread
            // until the lifeline exit ends the whole process.
            let _never = handle.join();
        }
        Err(_) => process::exit(CHILD_SETUP_FAILED_EXIT),
    }
}

// ---------------------------------------------------------------------------
// Parent side
// ---------------------------------------------------------------------------

/// Owned child handle kept owned until an actual successful wait.
struct OwnedChild {
    child: Option<Child>,
}

impl OwnedChild {
    fn adopt(child: Child) -> Self {
        Self { child: Some(child) }
    }

    /// The owned handle; released only after a successful wait.
    fn get_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("owned child released early")
    }

    /// Kills the still-owned child if needed, then waits for actual exit.
    ///
    /// Calibration: a failed `kill` call is harmless ONLY once an actual
    /// successful `wait` observes the exit, so the kill result carries no
    /// information by itself. Until that observation, the handle STAYS OWNED
    /// for later retry or drop-time containment and failures are reported;
    /// it is never discarded before reaping succeeds.
    fn force_reap(&mut self) -> Result<(), StepFault> {
        if let Some(child) = self.child.as_mut() {
            let _kill_attempted = child.kill();
            match child.wait() {
                Ok(_observed_exit) => {
                    self.child = None;
                    Ok(())
                }
                Err(_) => Err(StepFault::ReapIo),
            }
        } else {
            Ok(())
        }
    }
}

impl Drop for OwnedChild {
    /// Best-effort containment over the exact owned child.
    ///
    /// This is a retry, not a guarantee: if this final OS reap also fails,
    /// returning from Drop ends custody without observed death — cleanup may
    /// remain incomplete and the stderr reader detached. No success is
    /// claimed in that terminal case; the scenario has already failed closed.
    fn drop(&mut self) {
        let _best_effort_reap = self.force_reap();
    }
}

/// What the joined stderr reader observed after readiness.
struct TrailingOutcome {
    bytes: Vec<u8>,
    overflow: bool,
    read_faulted: bool,
}

/// Joined stderr monitor: exactly one readiness result plus capped trailing.
struct StderrMonitor {
    readiness: Receiver<Result<(), &'static str>>,
    reader: Option<thread::JoinHandle<TrailingOutcome>>,
}

impl StderrMonitor {
    /// Starts the single parent reader thread over the piped child stderr.
    ///
    /// Byte caps bound data, not time: the trailing loop finishes when the
    /// (eventually dead) child's stderr reaches EOF, which is why every exit
    /// path reaps the child BEFORE joining this reader.
    fn start(stderr: ChildStderr) -> Self {
        let (sender, receiver) = sync_channel(1);
        let reader = thread::Builder::new()
            .name("directory-helper-fixture-stderr".to_owned())
            .spawn(move || monitor_stderr(stderr, &sender))
            .expect("stderr monitor thread should start");
        Self {
            readiness: receiver,
            reader: Some(reader),
        }
    }

    /// Waits within the readiness budget for the causal marker.
    fn await_ready(&self) -> Result<(), StepFault> {
        match self.readiness.recv_timeout(READINESS_BUDGET) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(reason)) => Err(StepFault::ReadinessFault(reason)),
            Err(RecvTimeoutError::Timeout) => Err(StepFault::ReadinessTimeout),
            Err(RecvTimeoutError::Disconnected) => {
                Err(StepFault::ReadinessFault("monitor ended before readiness"))
            }
        }
    }

    /// Joins the reader and returns its classified trailing observations.
    fn finish(&mut self) -> Result<TrailingOutcome, StepFault> {
        match self.reader.take() {
            Some(reader) => reader.join().map_err(|_| StepFault::MonitorJoin),
            None => Ok(TrailingOutcome {
                bytes: Vec::new(),
                overflow: false,
                read_faulted: false,
            }),
        }
    }
}

/// Monitor thread body: one readiness send, then capacity-clamped trailing.
///
/// Each read asks only for the REMAINING capacity, so the buffer can never
/// exceed [`TRAILING_CAP`]; reaching the cap triggers one single-byte
/// sentinel read to classify true overflow. Read errors are reported, never
/// silently treated as clean silence.
fn monitor_stderr(
    mut stderr: ChildStderr,
    sender: &SyncSender<Result<(), &'static str>>,
) -> TrailingOutcome {
    let mut marker = [0_u8; READINESS_MARKER.len()];
    let outcome = match stderr.read_exact(&mut marker) {
        Ok(()) if marker == READINESS_MARKER => Ok(()),
        Ok(()) => Err("malformed readiness marker"),
        Err(_) => Err("readiness read failed"),
    };
    let _single_send = sender.send(outcome);

    let mut trailing = TrailingOutcome {
        bytes: Vec::new(),
        overflow: false,
        read_faulted: false,
    };
    while trailing.bytes.len() < TRAILING_CAP {
        let remaining = TRAILING_CAP - trailing.bytes.len();
        let want = remaining.min(TRAILING_CHUNK_MAX);
        let mut chunk = vec![0_u8; want];
        match stderr.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => trailing.bytes.extend_from_slice(&chunk[..read]),
            Err(_) => {
                trailing.read_faulted = true;
                break;
            }
        }
    }
    if trailing.bytes.len() == TRAILING_CAP && !trailing.read_faulted {
        let mut sentinel = [0_u8; 1];
        match stderr.read(&mut sentinel) {
            Ok(0) => {}
            Ok(_one_extra_byte) => trailing.overflow = true,
            Err(_) => trailing.read_faulted = true,
        }
    }
    trailing
}

/// Single owner for one scenario's child, sole writer, and joined monitor.
///
/// Every exit path—success, step fault, timeout, panic unwind—runs the same
/// ordered containment: close the writer, kill/reap the exact child as
/// needed, and only after the child is gone join the bounded reader. The
/// ordering matters: a live child would keep the reader parked forever, so
/// the monitor is never joined before the child is reaped.
struct ScenarioRun {
    writer: Option<ChildStdin>,
    owned: OwnedChild,
    monitor: Option<StderrMonitor>,
}

impl ScenarioRun {
    /// Adopts the spawned child and its pipes immediately after spawn.
    ///
    /// The pipe takes read this spawn's own configuration, which piped stdin
    /// and stderr and nulled stdout; their presence is a configuration
    /// invariant checked before anything fallible runs.
    fn adopt(mut child: Child) -> Self {
        let writer = child.stdin.take();
        let stderr = child.stderr.take();
        Self {
            writer,
            owned: OwnedChild::adopt(child),
            monitor: Some(StderrMonitor::start(
                stderr.expect("this spawn piped stderr"),
            )),
        }
    }

    /// Ordinary-path containment with fully reported cleanup faults.
    /// Ordinary-path containment with fully reported cleanup faults.
    ///
    /// Ordered custody: close the writer, then reap the child, and only on
    /// an actually observed exit join the bounded reader. When the reap
    /// fails, the child may still be live: custody is retained, the monitor
    /// is deliberately NOT joined (a join would pretend death was observed),
    /// `reap_unresolved` is set so no replacement scenario may start, and
    /// one drop-time best-effort retry remains as described on [`Drop for
    /// ScenarioRun`].
    fn settle(&mut self) -> CleanupReport {
        let mut report = CleanupReport {
            faults: Vec::new(),
            reap_unresolved: false,
        };
        drop(self.writer.take());
        if let Err(reap) = self.owned.force_reap() {
            report.faults.push(reap.label());
            report.reap_unresolved = true;
            return report;
        }
        if let Some(monitor) = self.monitor.as_mut() {
            match monitor.finish() {
                Err(join) => report.faults.push(join.label()),
                Ok(outcome) => {
                    if outcome.read_faulted {
                        report.faults.push(StepFault::TrailingReadFault.label());
                    }
                    if outcome.overflow {
                        report.faults.push(StepFault::TrailingOverflow.label());
                    }
                    if !outcome.bytes.is_empty() {
                        report.faults.push(
                            StepFault::UnexpectedTrailingDiagnostics {
                                count: outcome.bytes.len(),
                            }
                            .label(),
                        );
                    }
                }
            }
        }
        self.monitor = None;
        report
    }
}

/// Ordinary-path containment outcome for one scenario.
struct CleanupReport {
    /// Every cleanup fault observed, one static label each.
    faults: Vec<String>,
    /// Set when the child's exit was never actually observed; no further
    /// scenario may launch while this is set.
    reap_unresolved: bool,
}

impl Drop for ScenarioRun {
    /// Non-panicking unwind containment in the mandated order: writer, then
    /// child reap, then—only after an observed exit—the monitor join.
    ///
    /// The reap here is a best-effort retry of any failure `settle`
    /// reported. It is not a guarantee: if this final OS reap also fails,
    /// returning from Drop ends custody without observed death — cleanup
    /// stays incomplete and the reader stays detached — and nothing claims
    /// otherwise.
    fn drop(&mut self) {
        drop(self.writer.take());
        let death_observed = self.owned.force_reap().is_ok();
        if !death_observed {
            return;
        }
        if let Some(mut monitor) = self.monitor.take() {
            let _classified_after_observed_exit = monitor.finish();
        }
    }
}

/// Runs all three scenarios sequentially against one live child at a time.
///
/// If a scenario ends with unresolved custody (its child's exit was never
/// actually observed), no replacement scenario launches: the loop stops and
/// the whole fixture fails closed on that terminal condition.
fn run_parent_scenarios() {
    let mut failures = Vec::new();
    let mut custody_unresolved = false;
    for scenario in [SCENARIO_EOF, SCENARIO_TRAILING, SCENARIO_INJECTED] {
        let outcome = run_parent_scenario(scenario);
        match outcome {
            Ok(()) => {}
            Err(FixtureFailure { report, unresolved }) => {
                failures.push(format!("{scenario}: {report}"));
                if unresolved {
                    custody_unresolved = true;
                    failures.push(
                        "stopping before further scenarios: previous cleanup left \
                         the child exit unobserved"
                            .to_owned(),
                    );
                    break;
                }
            }
        }
    }
    if custody_unresolved {
        failures.push("custody unresolved; remaining scenarios not launched".to_owned());
    }
    assert!(
        failures.is_empty(),
        "lifeline fixture failures: {}",
        failures.join("; ")
    );
}

/// Why one scenario failed, plus whether custody stayed unresolved.
struct FixtureFailure {
    report: String,
    unresolved: bool,
}

/// Executes one full parent scenario with deterministic ownership and honest
/// reporting of the primary fault alongside any cleanup faults. A cleanup
/// fault fails the scenario even when its primary steps passed — there is no
/// green fallback for broken containment.
fn run_parent_scenario(scenario: &'static str) -> Result<(), FixtureFailure> {
    let executable = env::current_exe().map_err(|_| FixtureFailure {
        report: StepFault::ExecutablePath.label(),
        unresolved: false,
    })?;
    let mut command = Command::new(executable);
    command
        .arg(FIXTURE_TEST_FILTER)
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(SCENARIO_VARIABLE, scenario)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let spawned = command.spawn().map_err(|_| FixtureFailure {
        report: StepFault::Spawn.label(),
        unresolved: false,
    })?;
    let mut run = ScenarioRun::adopt(spawned);

    let primary = perform_steps(scenario, &mut run);
    let cleanup = run.settle();
    let failure = match (primary, cleanup.faults.as_slice()) {
        (Ok(()), []) => None,
        (Ok(()), cleanup_faults) => Some(format!(
            "cleanup failed after success: {}",
            cleanup_faults.join("; ")
        )),
        (Err(fault), []) => Some(fault.label()),
        (Err(fault), cleanup_faults) => Some(format!(
            "{}; additionally, {}",
            fault.label(),
            cleanup_faults.join("; ")
        )),
    };
    match failure {
        None => Ok(()),
        Some(report) => Err(FixtureFailure {
            report,
            unresolved: cleanup.reap_unresolved,
        }),
    }
}

/// The ordered scenario steps: readiness, liveness, stimulus, exit-code
/// assertion, and the explicit success-path reap.
fn perform_steps(scenario: &'static str, run: &mut ScenarioRun) -> Result<(), StepFault> {
    run.monitor
        .as_mut()
        .expect("monitor present until settle")
        .await_ready()?;

    // Confirm the owned child is still alive before any stimulus.
    match run.owned.get_mut().try_wait() {
        Ok(None) => {}
        Ok(Some(_early_status)) => return Err(StepFault::ExitedBeforeStimulus),
        Err(_) => return Err(StepFault::PollIo),
    }

    perform_stimulus(scenario, &mut run.writer)?;

    let status: ExitStatus = await_exit(&mut run.owned)?;
    if status.code() != Some(i32::from(EXIT_CODE_LIFELINE_LOST)) {
        return Err(StepFault::WrongExitCode {
            expected: i32::from(EXIT_CODE_LIFELINE_LOST),
            actual: status.code(),
        });
    }

    // Explicit reap on the success path; settle finds nothing left to reap.
    run.owned.force_reap()
}

/// Applies the scenario stimulus; EOF drops the sole writer, the other two
/// retain it through the child's exit so EOF cannot fake their outcomes.
fn perform_stimulus(
    scenario: &'static str,
    writer: &mut Option<ChildStdin>,
) -> Result<(), StepFault> {
    match scenario {
        SCENARIO_EOF => {
            drop(writer.take());
            Ok(())
        }
        SCENARIO_TRAILING | SCENARIO_INJECTED => {
            let pipe = writer.as_mut().ok_or(StepFault::StimulusWrite)?;
            let byte = if scenario == SCENARIO_TRAILING {
                TRAILING_BYTE
            } else {
                TRIGGER_BYTE
            };
            pipe.write_all(&[byte])
                .and_then(|()| pipe.flush())
                .map_err(|_| StepFault::StimulusWrite)
        }
        _ => Err(StepFault::ReadinessFault("unknown fixture scenario")),
    }
}

/// Watches the owned handle with `try_wait` polling until the budget ends.
fn await_exit(owned: &mut OwnedChild) -> Result<ExitStatus, StepFault> {
    let deadline = Instant::now() + TERMINATION_BUDGET;
    loop {
        match owned.get_mut().try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    return Err(StepFault::TerminationTimeout);
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return Err(StepFault::PollIo),
        }
    }
}
