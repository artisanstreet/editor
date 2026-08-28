//! Deterministic per-key command coalescing for scoped attachment adapters.
//!
//! The queue is the policy half of the TypeScript scoped attachment runner.
//! It owns a FIFO of keys and one latest command for each queued key. A
//! replacement changes the mailbox entry without adding another FIFO entry,
//! so a noisy key cannot grow the backlog beyond one entry for that key while
//! a different key keeps its place in the FIFO.
//!
//! This module deliberately has no executor, task, fiber, scope, attachment,
//! browser, or cancellation behavior. A future runtime adapter can take the
//! commands returned by [`ScopedAttachmentQueue::take_next`] and decide how
//! to execute or interrupt work.

use std::collections::{HashMap, HashSet, VecDeque};

/// The owned key type used by the queue and its commands.
pub type AttachmentKey = String;

/// The latest policy command for one attachment key.
///
/// `Run` carries the generic input supplied by the adapter. `Release` carries
/// no input and therefore supersedes a queued run for the same key. A later
/// run replaces that release in the mailbox before the key is taken.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttachmentCommand<Input> {
    /// Run the latest input for `key`.
    Run { key: AttachmentKey, input: Input },
    /// Release the work associated with `key`.
    Release { key: AttachmentKey },
}

impl<Input> AttachmentCommand<Input> {
    /// Creates a run command with an owned key.
    #[must_use]
    pub fn run(key: impl Into<AttachmentKey>, input: Input) -> Self {
        Self::Run {
            key: key.into(),
            input,
        }
    }

    /// Creates a release command with an owned key.
    #[must_use]
    pub fn release(key: impl Into<AttachmentKey>) -> Self {
        Self::Release { key: key.into() }
    }

    /// Returns the key carried by this command.
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Run { key, .. } | Self::Release { key } => key,
        }
    }
}

/// A pure FIFO/mailbox state machine for keyed attachment commands.
///
/// The private [`Self::pending_keys`] FIFO and [`Self::mailbox`] map are kept
/// in lockstep with [`Self::queued_keys`]: a key is inserted into the FIFO
/// only when it was not already queued, while the mailbox is always replaced
/// with that key's newest command. Consequently, every pending key appears
/// at most once and every pending key has one latest command.
///
/// Taking a command removes both the FIFO key and its mailbox entry. The key
/// can then be reused immediately by a later [`Self::replace`] or
/// [`Self::release`].
#[derive(Debug, Eq, PartialEq)]
pub struct ScopedAttachmentQueue<Input> {
    pending_keys: VecDeque<AttachmentKey>,
    queued_keys: HashSet<AttachmentKey>,
    mailbox: HashMap<AttachmentKey, AttachmentCommand<Input>>,
    next_attachment_id: u64,
}

impl<Input> Default for ScopedAttachmentQueue<Input> {
    fn default() -> Self {
        Self {
            pending_keys: VecDeque::new(),
            queued_keys: HashSet::new(),
            mailbox: HashMap::new(),
            next_attachment_id: 0,
        }
    }
}

impl<Input> ScopedAttachmentQueue<Input> {
    /// Creates an empty queue whose first attached key is `attachment:0`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates the next deterministic attachment key and queues its run.
    ///
    /// Keys start at `attachment:0` and increase by one for each call. The
    /// counter is checked so it can never wrap around and reuse an earlier
    /// key; exhausting the representable `u64` suffix is unrecoverable for
    /// this queue and panics rather than violating key uniqueness.
    #[must_use]
    pub fn attach(&mut self, input: Input) -> AttachmentKey {
        let key = format!("attachment:{}", self.next_attachment_id);
        self.next_attachment_id = self
            .next_attachment_id
            .checked_add(1)
            .expect("attachment key counter exhausted");
        self.enqueue(AttachmentCommand::run(key.clone(), input));
        key
    }

    /// Queues the latest run for `key`, coalescing with an existing pending
    /// command for that key.
    pub fn replace(&mut self, key: impl AsRef<str>, input: Input) {
        self.enqueue(AttachmentCommand::run(key.as_ref(), input));
    }

    /// Queues the latest release for `key`, coalescing with an existing
    /// pending command for that key.
    pub fn release(&mut self, key: impl AsRef<str>) {
        self.enqueue(AttachmentCommand::release(key.as_ref()));
    }

    /// Stores a command as the latest command for its key.
    ///
    /// If the key is already pending, only its mailbox entry changes. If it
    /// is not pending, its key is appended to the FIFO exactly once.
    pub fn enqueue(&mut self, command: AttachmentCommand<Input>) {
        let key = command.key().to_owned();
        self.mailbox.insert(key.clone(), command);
        if self.queued_keys.insert(key.clone()) {
            self.pending_keys.push_back(key);
        }
    }

    /// Takes the next latest command in distinct-key FIFO order.
    ///
    /// Returns `None` when no key is pending. For a non-empty queue, the
    /// returned command is removed from both the FIFO and mailbox, so the
    /// same key may be queued again immediately. Since all mutation methods
    /// preserve the queue invariant, a pending key always has a command.
    pub fn take_next(&mut self) -> Option<AttachmentCommand<Input>> {
        let key = self.pending_keys.pop_front()?;
        let was_queued = self.queued_keys.remove(&key);
        debug_assert!(was_queued);
        self.mailbox.remove(&key)
    }

    /// Returns the number of distinct keys currently waiting in the FIFO.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending_keys.len()
    }

    /// Returns whether no key is waiting to be taken.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending_keys.is_empty()
    }

    /// Returns the number of latest commands retained in the mailbox.
    ///
    /// This equals [`Self::pending_len`] for a valid queue state and is
    /// exposed to make the coalescing invariant easy to assert in focused
    /// tests.
    #[must_use]
    pub fn mailbox_len(&self) -> usize {
        self.mailbox.len()
    }

    /// Returns whether `key` currently has a pending FIFO entry.
    #[must_use]
    pub fn contains_pending_key(&self, key: impl AsRef<str>) -> bool {
        self.queued_keys.contains(key.as_ref())
    }

    /// Iterates over pending keys in the order in which they will be taken.
    #[must_use]
    pub fn pending_keys(&self) -> impl Iterator<Item = &str> {
        self.pending_keys.iter().map(String::as_str)
    }

    /// Borrows the latest command currently retained for `key`, if any.
    #[must_use]
    pub fn latest_command(&self, key: impl AsRef<str>) -> Option<&AttachmentCommand<Input>> {
        self.mailbox.get(key.as_ref())
    }
}
