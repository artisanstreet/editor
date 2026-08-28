//! Pure keyed command and active-work policy for scoped attachment runners.
//!
//! This is the dependency-free counterpart of
//! `modules/frontend/src/lib/lifecycle/scoped-attachment-runner.ts`. The
//! mailbox retains only the latest command for each key, while the FIFO holds
//! each pending key once. [`ScopedAttachmentRunnerPolicy::next_effect`] emits
//! one transition at a time so a caller can perform an interrupt outside this
//! module and enqueue a replacement before the interrupted key is started
//! again.
//!
//! No fiber, executor, scope, browser observer, or cancellation primitive is
//! owned here. The caller executes [`RunnerEffect`] values and reports new
//! observations by calling [`Self::replace`] or [`Self::release`].

#![allow(clippy::module_name_repetitions)]

use std::collections::{HashMap, HashSet, VecDeque};

/// The stable string key used to scope one attachment's work.
pub type AttachmentKey = String;

/// The latest command for one attachment key.
///
/// A [`RunnerCommand::Run`] carries the input supplied by the attachment.
/// [`RunnerCommand::Release`] cancels work for its key and never starts work
/// itself. A later command for the same key replaces the earlier mailbox
/// entry before that key is taken.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunnerCommand<Input> {
    /// Start or replace work with the supplied input.
    Run {
        /// The attachment key whose work is being requested.
        key: AttachmentKey,
        /// The latest input to pass to the caller-owned runner.
        input: Input,
    },
    /// Release all work associated with the key.
    Release {
        /// The attachment key whose work is being released.
        key: AttachmentKey,
    },
}

impl<Input> RunnerCommand<Input> {
    /// Creates a run command from a key and input.
    #[must_use = "a runner command must be enqueued or applied"]
    pub fn run(key: impl Into<AttachmentKey>, input: Input) -> Self {
        Self::Run {
            key: key.into(),
            input,
        }
    }

    /// Creates a release command from a key.
    #[must_use = "a runner command must be enqueued or applied"]
    pub fn release(key: impl Into<AttachmentKey>) -> Self {
        Self::Release { key: key.into() }
    }

    /// Returns the command's stable attachment key.
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Run { key, .. } | Self::Release { key } => key,
        }
    }
}

/// Alias matching the command-oriented name used by the queue adapter.
pub type AttachmentCommand<Input> = RunnerCommand<Input>;

/// One pure effect emitted by the runner policy.
///
/// `Interrupt` is emitted before a replacement or release is applied to an
/// already-active key. The policy removes that key from active state before
/// returning the effect. The caller may enqueue while its interruption is in
/// progress; the following call to [`ScopedAttachmentRunnerPolicy::next_effect`]
/// observes that latest command. `Start` means the caller may begin the
/// supplied input. `NoOp` means that no work should be started.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunnerEffect<Input> {
    /// Interrupt the currently active work for `key`.
    Interrupt {
        /// The key whose active work must be interrupted.
        key: AttachmentKey,
    },
    /// Start the supplied latest input for `key`.
    Start {
        /// The attachment key for the new work.
        key: AttachmentKey,
        /// The input captured by the run command.
        input: Input,
    },
    /// No interrupt or start is required.
    NoOp,
}

/// Alias emphasizing that [`RunnerEffect`] is a transition result.
pub type TransitionEffect<Input> = RunnerEffect<Input>;

/// Alias for callers that name the result of one policy step a transition.
pub type RunnerTransition<Input> = RunnerEffect<Input>;

#[derive(Debug, Eq, PartialEq)]
struct PendingInterruption<Input> {
    key: AttachmentKey,
    command: RunnerCommand<Input>,
}

/// Deterministic mailbox/FIFO policy for keyed scoped attachment work.
///
/// The mailbox stores at most one latest command for each key. The FIFO and
/// its membership set store each pending key at most once, preserving the
/// order in which distinct keys first became pending. Active keys represent
/// work the caller was told to start. An active key is removed before an
/// [`RunnerEffect::Interrupt`] is emitted.
///
/// The policy has one explicit interruption boundary. After an interrupting
/// effect is returned, the caller can submit a replacement synchronously. The
/// next step consumes that replacement in preference to the command that was
/// taken before the interrupt. This is the synchronous equivalent of the
/// source runner's mailbox check after `Fiber::interrupt`.
#[derive(Debug, Eq, PartialEq)]
pub struct ScopedAttachmentRunnerPolicy<Input> {
    pending_keys: VecDeque<AttachmentKey>,
    queued_keys: HashSet<AttachmentKey>,
    mailbox: HashMap<AttachmentKey, RunnerCommand<Input>>,
    active_keys: HashSet<AttachmentKey>,
    interruption: Option<PendingInterruption<Input>>,
    next_attachment_id: u64,
}

impl<Input> Default for ScopedAttachmentRunnerPolicy<Input> {
    fn default() -> Self {
        Self {
            pending_keys: VecDeque::new(),
            queued_keys: HashSet::new(),
            mailbox: HashMap::new(),
            active_keys: HashSet::new(),
            interruption: None,
            next_attachment_id: 0,
        }
    }
}

impl<Input> ScopedAttachmentRunnerPolicy<Input> {
    /// Creates an empty policy whose first attachment key is
    /// `attachment:0`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates a stable key and queues its first run command.
    ///
    /// The suffix is monotonically increasing and checked rather than allowed
    /// to wrap and reuse an earlier key. Exhausting the representable suffix
    /// is unrecoverable for one policy and therefore panics.
    #[must_use]
    pub fn attach(&mut self, input: Input) -> AttachmentKey {
        let key = format!("attachment:{}", self.next_attachment_id);
        self.next_attachment_id = self
            .next_attachment_id
            .checked_add(1)
            .expect("attachment key counter exhausted");
        self.enqueue(RunnerCommand::run(key.clone(), input));
        key
    }

    /// Queues the latest run command for `key`.
    ///
    /// Replacing a key already in the FIFO changes only its mailbox value.
    /// The command is still independent when another key is pending.
    pub fn replace(&mut self, key: impl AsRef<str>, input: Input) {
        self.enqueue(RunnerCommand::run(key.as_ref(), input));
    }

    /// Synchronous alias for [`Self::replace`], matching the source adapter's
    /// unsafe ingress without importing an asynchronous queue.
    pub fn replace_unsafe(&mut self, key: impl AsRef<str>, input: Input) {
        self.replace(key, input);
    }

    /// Queues the latest release command for `key`.
    ///
    /// A release coalesces with a queued run. If work is active, the next
    /// effect for the key is [`RunnerEffect::Interrupt`] followed by
    /// [`RunnerEffect::NoOp`]; it never emits a `Start` for the release.
    pub fn release(&mut self, key: impl AsRef<str>) {
        self.enqueue(RunnerCommand::release(key.as_ref()));
    }

    /// Stores a command as the latest command for its key.
    ///
    /// A new key is appended to the distinct-key FIFO. An already queued key
    /// is not appended again, so its backlog remains bounded to one entry.
    pub fn enqueue(&mut self, command: RunnerCommand<Input>) {
        let key = command.key().to_owned();
        self.mailbox.insert(key.clone(), command);
        if self.queued_keys.insert(key.clone()) {
            self.pending_keys.push_back(key);
        }
    }

    /// Emits the next pure transition effect.
    ///
    /// An empty policy returns [`RunnerEffect::NoOp`]. When a key has active
    /// work, taking its latest command first removes that active key and
    /// returns `Interrupt`. The next call completes that interruption: it
    /// takes a newer same-key mailbox command when one arrived in the
    /// meantime, otherwise it applies the command already taken. This two
    /// step boundary lets a caller interrupt real work without blocking the
    /// mailbox and preserves replacement-during-interruption semantics.
    pub fn next_effect(&mut self) -> RunnerEffect<Input> {
        if self.interruption.is_some() {
            return self.complete_interruption();
        }

        let Some(key) = self.pending_keys.pop_front() else {
            return RunnerEffect::NoOp;
        };
        debug_assert!(self.queued_keys.remove(&key));

        let Some(command) = self.mailbox.remove(&key) else {
            // This cannot occur through the public mutation methods. Keeping
            // the branch makes the queue invariant defensive if the type is
            // extended later and reports the only safe effect.
            return RunnerEffect::NoOp;
        };

        if self.active_keys.remove(&key) {
            self.interruption = Some(PendingInterruption {
                key: key.clone(),
                command,
            });
            RunnerEffect::Interrupt { key }
        } else {
            self.apply_command(command)
        }
    }

    /// Alias for [`Self::next_effect`] using the source queue's take wording.
    pub fn take_next(&mut self) -> RunnerEffect<Input> {
        self.next_effect()
    }

    /// Completes the currently emitted interrupt, if any.
    ///
    /// This is equivalent to [`Self::next_effect`] while an interruption is
    /// pending, but returns `NoOp` when there is no interruption. It is useful
    /// for adapters that name the external interrupt acknowledgement
    /// explicitly.
    pub fn complete_interruption(&mut self) -> RunnerEffect<Input> {
        let Some(interruption) = self.interruption.take() else {
            return RunnerEffect::NoOp;
        };

        let key = interruption.key;
        let command = self.mailbox.remove(&key).unwrap_or(interruption.command);

        // A replacement submitted during interruption also entered the FIFO.
        // It is being consumed directly here, so remove that one pending
        // entry before applying it. Different keys retain their FIFO order.
        if self.queued_keys.remove(&key) {
            self.pending_keys.retain(|pending_key| pending_key != &key);
        }

        self.apply_command(command)
    }

    /// Returns the number of distinct keys waiting in the FIFO.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending_keys.len()
    }

    /// Returns whether the FIFO has no pending key.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending_keys.is_empty()
    }

    /// Returns the number of latest commands currently in the mailbox.
    #[must_use]
    pub fn mailbox_len(&self) -> usize {
        self.mailbox.len()
    }

    /// Returns the number of active keys whose work was started.
    #[must_use]
    pub fn active_len(&self) -> usize {
        self.active_keys.len()
    }

    /// Returns whether work for `key` is currently active.
    #[must_use]
    pub fn is_active(&self, key: impl AsRef<str>) -> bool {
        self.active_keys.contains(key.as_ref())
    }

    /// Returns whether an interrupt has been emitted and awaits completion.
    #[must_use]
    pub const fn is_interrupting(&self) -> bool {
        self.interruption.is_some()
    }

    /// Returns whether `key` currently has a FIFO entry.
    #[must_use]
    pub fn contains_pending_key(&self, key: impl AsRef<str>) -> bool {
        self.queued_keys.contains(key.as_ref())
    }

    /// Iterates over keys in the order in which they will be processed.
    pub fn pending_keys(&self) -> impl Iterator<Item = &str> {
        self.pending_keys.iter().map(String::as_str)
    }

    /// Borrows the latest mailbox command for `key`, when one is pending.
    #[must_use]
    pub fn latest_command(&self, key: impl AsRef<str>) -> Option<&RunnerCommand<Input>> {
        self.mailbox.get(key.as_ref())
    }

    fn apply_command(&mut self, command: RunnerCommand<Input>) -> RunnerEffect<Input> {
        match command {
            RunnerCommand::Run { key, input } => {
                self.active_keys.insert(key.clone());
                RunnerEffect::Start { key, input }
            }
            RunnerCommand::Release { .. } => RunnerEffect::NoOp,
        }
    }
}

/// Short alias for callers that use the source runner's name for the policy.
pub type ScopedAttachmentRunner<Input> = ScopedAttachmentRunnerPolicy<Input>;
