//! The shell jobs of every thread's engine runs, held in memory only.
//!
//! An engine's commands and background jobs live as long as the engine
//! process that started them, and their output is unbounded over that time.
//! The run dispatcher therefore keeps each job's lifecycle and the newest
//! part of its raw output here instead of writing them to the observation
//! ledger, and every delivery driver pushes what changed to the thread's
//! subscribers. Nothing here survives the Forge stopping.
//!
//! A command the agent waits for leaves the board once it finishes: the
//! transcript already shows it. A background job stays after it stops, so a
//! reader can still see why a dev server died.

#![forbid(unsafe_code)]

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
};

use artisan_domain::{
    RunId, THREAD_TERMINAL_OUTPUT_MAX_BYTES, THREAD_TERMINALS_MAX, ThreadId, ThreadTerminal,
    ThreadTerminalOutput, ThreadTerminalState, UnixMillis,
};

/// One change to a shell job the engine opened: a command, or a job it runs
/// in the background.
///
/// Engines report these on their observation channel. Shell rows never reach
/// the observation ledger: the dispatcher applies them to the board.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellJobRow {
    /// The run whose engine opened the job.
    pub run_id: RunId,
    /// Engine-native job identity, unique within the run.
    pub job_id: String,
    /// What happened.
    pub change: ShellJobChange,
}

/// What happened to one shell job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShellJobChange {
    /// The engine started the job.
    Opened {
        /// The command line, when the engine reported one.
        command: Option<String>,
        /// Whether the job runs in the background from the start.
        background: bool,
    },
    /// A foreground job moved to the background.
    Backgrounded,
    /// The job wrote these raw bytes, escape sequences included.
    Output(Vec<u8>),
    /// The job stopped.
    Settled(ShellJobEnd),
}

/// How one shell job stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellJobEnd {
    /// It exited on its own, with its exit code when the engine reported one.
    Exited(Option<i32>),
    /// The engine reported a failure without an exit code.
    Failed,
    /// It was stopped before it finished.
    Killed,
}

/// Most threads the board keeps jobs for. A thread without running jobs that
/// was touched least recently goes first.
const THREADS_MAX: usize = 64;

/// The shell jobs of every thread with one.
#[derive(Clone, Debug, Default)]
pub struct TerminalBoard {
    threads: Arc<Mutex<HashMap<ThreadId, ThreadJobs>>>,
}

#[derive(Debug)]
struct ThreadJobs {
    /// Bumped on every change, so a driver skips an unchanged thread without
    /// walking its jobs.
    revision: u64,
    touched: UnixMillis,
    /// Oldest first.
    jobs: Vec<Job>,
}

#[derive(Debug)]
struct Job {
    terminal_id: String,
    run_id: RunId,
    command: Option<String>,
    background: bool,
    state: ThreadTerminalState,
    started_at: UnixMillis,
    ended_at: Option<UnixMillis>,
    /// Offset of `output[0]` in everything the job wrote.
    output_start: u64,
    output: VecDeque<u8>,
}

/// What a subscriber already has of one thread's jobs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeliveredTerminals {
    revision: u64,
    /// Output end per terminal.
    ends: HashMap<String, u64>,
}

impl TerminalBoard {
    /// Creates an empty board.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // A panic while holding the lock cannot leave a job half written in a way
    // that matters: the worst case is output the next change appends after.
    fn lock(&self) -> MutexGuard<'_, HashMap<ThreadId, ThreadJobs>> {
        self.threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Applies one change an engine reported for a job of `thread_id`.
    /// Returns whether anything a subscriber sees changed.
    pub fn apply(&self, thread_id: &ThreadId, row: &ShellJobRow, at: UnixMillis) -> bool {
        let mut threads = self.lock();
        let terminal_id = terminal_id(&row.run_id, &row.job_id);
        let opens = matches!(row.change, ShellJobChange::Opened { .. });
        if !opens && !threads.contains_key(thread_id) {
            return false;
        }
        let thread = threads.entry(thread_id.clone()).or_insert(ThreadJobs {
            revision: 0,
            touched: at,
            jobs: Vec::new(),
        });
        let index = thread
            .jobs
            .iter()
            .position(|job| job.terminal_id == terminal_id);
        let changed = match (&row.change, index) {
            (
                ShellJobChange::Opened {
                    command,
                    background,
                },
                None,
            ) => {
                thread.jobs.push(Job {
                    terminal_id,
                    run_id: row.run_id.clone(),
                    command: command.clone(),
                    background: *background,
                    state: ThreadTerminalState::Running,
                    started_at: at,
                    ended_at: None,
                    output_start: 0,
                    output: VecDeque::new(),
                });
                true
            }
            // A repeated open keeps the job; a command learned late fills in.
            (
                ShellJobChange::Opened {
                    command,
                    background,
                },
                Some(index),
            ) => {
                let job = &mut thread.jobs[index];
                let before = (job.command.clone(), job.background);
                if job.command.is_none() {
                    job.command.clone_from(command);
                }
                job.background |= *background;
                before != (job.command.clone(), job.background)
            }
            (ShellJobChange::Backgrounded, Some(index)) => {
                let job = &mut thread.jobs[index];
                !std::mem::replace(&mut job.background, true)
            }
            (ShellJobChange::Output(bytes), Some(index)) => {
                append_output(&mut thread.jobs[index], bytes);
                !bytes.is_empty()
            }
            (ShellJobChange::Settled(end), Some(index)) => {
                let job = &mut thread.jobs[index];
                if job.state.is_running() {
                    job.state = settled_state(*end);
                    job.ended_at = Some(at);
                    true
                } else {
                    false
                }
            }
            // Output or a settlement of a job never opened has nothing to
            // attach to.
            (_, None) => false,
        };
        if !changed {
            return false;
        }
        thread
            .jobs
            .retain(|job| job.background || job.state.is_running());
        trim_jobs(&mut thread.jobs);
        thread.revision += 1;
        thread.touched = at;
        trim_threads(&mut threads);
        true
    }

    /// Ends every job of `run_id` that still runs: the engine process that
    /// owned them is gone, and its jobs with it. Returns whether any ended.
    pub fn end_run(&self, thread_id: &ThreadId, run_id: &RunId, at: UnixMillis) -> bool {
        let mut threads = self.lock();
        let Some(thread) = threads.get_mut(thread_id) else {
            return false;
        };
        let mut changed = false;
        for job in &mut thread.jobs {
            if job.run_id == *run_id && job.state.is_running() {
                job.state = ThreadTerminalState::Ended;
                job.ended_at = Some(at);
                changed = true;
            }
        }
        if changed {
            thread
                .jobs
                .retain(|job| job.background || job.state.is_running());
            thread.revision += 1;
            thread.touched = at;
        }
        changed
    }

    /// The thread's jobs with the output each produced past what `delivered`
    /// already has, or `None` when nothing changed since. Updates
    /// `delivered` to what the returned snapshot carries.
    #[must_use]
    pub fn changes_since(
        &self,
        thread_id: &ThreadId,
        delivered: &mut DeliveredTerminals,
    ) -> Option<Vec<ThreadTerminal>> {
        let threads = self.lock();
        let thread = threads.get(thread_id)?;
        if thread.revision == delivered.revision {
            return None;
        }
        let mut ends = HashMap::with_capacity(thread.jobs.len());
        let terminals = thread
            .jobs
            .iter()
            .map(|job| {
                let output_end = job.output_start + job.output.len() as u64;
                let from = delivered
                    .ends
                    .get(&job.terminal_id)
                    .copied()
                    .unwrap_or(0)
                    .clamp(job.output_start, output_end);
                let skip = usize::try_from(from - job.output_start).unwrap_or(usize::MAX);
                ends.insert(job.terminal_id.clone(), output_end);
                ThreadTerminal {
                    terminal_id: job.terminal_id.clone(),
                    run_id: job.run_id.clone(),
                    command: job.command.clone(),
                    background: job.background,
                    state: job.state,
                    started_at: job.started_at,
                    ended_at: job.ended_at,
                    output: ThreadTerminalOutput {
                        start: from,
                        bytes: job.output.iter().skip(skip).copied().collect(),
                    },
                }
            })
            .collect();
        delivered.revision = thread.revision;
        delivered.ends = ends;
        Some(terminals)
    }
}

/// A terminal identity unique within its thread: engines number jobs per
/// run, not per thread.
fn terminal_id(run_id: &RunId, job_id: &str) -> String {
    format!("{}:{job_id}", run_id.as_str())
}

fn settled_state(end: ShellJobEnd) -> ThreadTerminalState {
    match end {
        ShellJobEnd::Exited(code) => ThreadTerminalState::Exited { code },
        ShellJobEnd::Failed => ThreadTerminalState::Failed,
        ShellJobEnd::Killed => ThreadTerminalState::Killed,
    }
}

/// Appends raw output and keeps the newest
/// [`THREAD_TERMINAL_OUTPUT_MAX_BYTES`].
fn append_output(job: &mut Job, bytes: &[u8]) {
    let keep = bytes.len().min(THREAD_TERMINAL_OUTPUT_MAX_BYTES);
    let skipped = bytes.len() - keep;
    job.output_start += skipped as u64;
    let overflow = (job.output.len() + keep).saturating_sub(THREAD_TERMINAL_OUTPUT_MAX_BYTES);
    job.output.drain(..overflow.min(job.output.len()));
    job.output_start += overflow as u64;
    job.output.extend(&bytes[skipped..]);
}

/// Drops the oldest stopped jobs past [`THREAD_TERMINALS_MAX`], then the
/// oldest running ones if every job still runs.
fn trim_jobs(jobs: &mut Vec<Job>) {
    while jobs.len() > THREAD_TERMINALS_MAX {
        let index = jobs
            .iter()
            .position(|job| !job.state.is_running())
            .unwrap_or(0);
        jobs.remove(index);
    }
}

fn trim_threads(threads: &mut HashMap<ThreadId, ThreadJobs>) {
    while threads.len() > THREADS_MAX {
        let idle = threads
            .iter()
            .filter(|(_, thread)| !thread.jobs.iter().any(|job| job.state.is_running()))
            .min_by_key(|(_, thread)| thread.touched)
            .or_else(|| threads.iter().min_by_key(|(_, thread)| thread.touched))
            .map(|(id, _)| id.clone());
        match idle {
            Some(id) => threads.remove(&id),
            None => return,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread() -> ThreadId {
        ThreadId::parse("thread-1".to_owned()).expect("thread id")
    }

    fn run() -> RunId {
        RunId::parse("run-1".to_owned()).expect("run id")
    }

    fn row(job: &str, change: ShellJobChange) -> ShellJobRow {
        ShellJobRow {
            run_id: run(),
            job_id: job.to_owned(),
            change,
        }
    }

    fn open(job: &str, background: bool) -> ShellJobRow {
        row(
            job,
            ShellJobChange::Opened {
                command: Some("npm run dev".to_owned()),
                background,
            },
        )
    }

    const AT: UnixMillis = UnixMillis::from_millis(1);

    #[test]
    fn delivers_only_output_a_subscriber_lacks() {
        let board = TerminalBoard::new();
        let mut delivered = DeliveredTerminals::default();
        assert!(board.apply(&thread(), &open("a", true), AT));
        assert!(board.apply(
            &thread(),
            &row("a", ShellJobChange::Output(b"hello ".to_vec())),
            AT
        ));
        let first = board
            .changes_since(&thread(), &mut delivered)
            .expect("first push");
        assert_eq!(first[0].output.bytes, b"hello ");
        assert_eq!(first[0].output.start, 0);
        assert!(board.changes_since(&thread(), &mut delivered).is_none());
        board.apply(
            &thread(),
            &row("a", ShellJobChange::Output(b"world".to_vec())),
            AT,
        );
        let second = board
            .changes_since(&thread(), &mut delivered)
            .expect("second push");
        assert_eq!(second[0].output.bytes, b"world");
        assert_eq!(second[0].output.start, 6);
    }

    #[test]
    fn keeps_the_newest_output_and_reports_where_it_starts() {
        let board = TerminalBoard::new();
        board.apply(&thread(), &open("a", true), AT);
        let chunk = vec![b'x'; THREAD_TERMINAL_OUTPUT_MAX_BYTES];
        board.apply(&thread(), &row("a", ShellJobChange::Output(chunk)), AT);
        board.apply(
            &thread(),
            &row("a", ShellJobChange::Output(b"tail".to_vec())),
            AT,
        );
        let terminals = board
            .changes_since(&thread(), &mut DeliveredTerminals::default())
            .expect("push");
        let output = &terminals[0].output;
        assert_eq!(output.start, 4);
        assert_eq!(output.bytes.len(), THREAD_TERMINAL_OUTPUT_MAX_BYTES);
        assert!(output.bytes.ends_with(b"tail"));
    }

    #[test]
    fn a_finished_command_leaves_but_a_stopped_background_job_stays() {
        let board = TerminalBoard::new();
        board.apply(&thread(), &open("fg", false), AT);
        board.apply(&thread(), &open("bg", true), AT);
        board.apply(
            &thread(),
            &row("fg", ShellJobChange::Settled(ShellJobEnd::Exited(Some(0)))),
            AT,
        );
        board.apply(
            &thread(),
            &row("bg", ShellJobChange::Settled(ShellJobEnd::Exited(Some(1)))),
            AT,
        );
        let terminals = board
            .changes_since(&thread(), &mut DeliveredTerminals::default())
            .expect("push");
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].terminal_id, "run-1:bg");
        assert_eq!(
            terminals[0].state,
            ThreadTerminalState::Exited { code: Some(1) }
        );
    }

    #[test]
    fn a_run_ending_ends_its_running_jobs() {
        let board = TerminalBoard::new();
        board.apply(&thread(), &open("bg", true), AT);
        assert!(board.end_run(&thread(), &run(), AT));
        assert!(!board.end_run(&thread(), &run(), AT));
        let terminals = board
            .changes_since(&thread(), &mut DeliveredTerminals::default())
            .expect("push");
        assert_eq!(terminals[0].state, ThreadTerminalState::Ended);
    }

    #[test]
    fn output_for_an_unknown_job_changes_nothing() {
        let board = TerminalBoard::new();
        assert!(!board.apply(
            &thread(),
            &row("ghost", ShellJobChange::Output(b"x".to_vec())),
            AT
        ));
    }
}
