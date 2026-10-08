//! The shell jobs a thread's engine runs opened, held in memory only.
//!
//! An engine's commands and background jobs (a dev server, a watcher, a test
//! run) live as long as the engine process that started them. The Forge keeps
//! each job's lifecycle and the newest part of its raw output on a board in
//! memory, and pushes them to the thread's subscribers as
//! [`Event::ThreadTerminals`](crate::Event::ThreadTerminals). Nothing here is
//! written to the observation ledger: output is unbounded over a job's
//! lifetime, and nobody replays it after the Forge stops.
//!
//! Output travels as raw bytes, escape sequences included. A reader keeps
//! the bytes and interprets them as a whole, so a sequence split across two
//! pushes is never misread.

use crate::{RunId, UnixMillis};

/// Most raw output bytes the Forge keeps per terminal. Older output is
/// dropped; [`ThreadTerminalOutput::start`] says how much.
pub const THREAD_TERMINAL_OUTPUT_MAX_BYTES: usize = 256 * 1024;

/// Most terminals one thread snapshot lists.
pub const THREAD_TERMINALS_MAX: usize = 32;

/// Every terminal a subscribed thread's engine runs opened, with the output
/// each one produced since the subscriber's previous push.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadTerminals {
    /// The thread.
    pub thread_id: crate::ThreadId,
    /// The thread's terminals, oldest first. A terminal missing from a later
    /// snapshot was dropped by the Forge and is gone.
    pub terminals: Vec<ThreadTerminal>,
}

/// One shell job of a thread's engine run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadTerminal {
    /// Engine-native job identity, unique within the thread: Claude's tool
    /// call id, Codex's command item id.
    pub terminal_id: String,
    /// The run whose engine opened it.
    pub run_id: RunId,
    /// The command line, when the engine reported one.
    pub command: Option<String>,
    /// Whether the job runs in the background while the agent goes on.
    pub background: bool,
    /// Where the job is in its life.
    pub state: ThreadTerminalState,
    /// When the job was opened.
    pub started_at: UnixMillis,
    /// When the job stopped, once it did.
    pub ended_at: Option<UnixMillis>,
    /// The output produced since the subscriber's previous push.
    pub output: ThreadTerminalOutput,
}

/// Where one shell job is in its life.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadTerminalState {
    /// The job is running.
    Running,
    /// The job exited on its own; a non-zero code is a failure.
    Exited {
        /// The exit code, when the engine reported one.
        code: Option<i32>,
    },
    /// The engine reported the job failed without an exit code.
    Failed,
    /// The job was stopped before it finished.
    Killed,
    /// The engine process that owned the job ended while it ran, which ends
    /// the job too.
    Ended,
}

impl ThreadTerminalState {
    /// Whether the job is still running.
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether the job stopped badly: a non-zero exit, a failure, or a kill.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        match self {
            Self::Exited { code: Some(code) } => code != 0,
            Self::Failed | Self::Killed => true,
            Self::Running | Self::Exited { code: None } | Self::Ended => false,
        }
    }
}

/// A span of one job's raw output, positioned in everything the job ever
/// wrote.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadTerminalOutput {
    /// Byte offset of `bytes[0]` in the job's whole output.
    pub start: u64,
    /// Raw output, escape sequences included.
    pub bytes: Vec<u8>,
}

impl ThreadTerminalOutput {
    /// Byte offset just past the last byte of this span.
    #[must_use]
    pub fn end(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }
}
