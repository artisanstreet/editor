use super::controller::DraftBody;
use artisan_api::{
    BlobHash, CreateThread, DraftRevision, DraftSnapshot, DraftWriteResult, SendMessage, ThreadId,
    WriteDraft,
};
use serde_json::Value;
use std::time::Duration;

pub(crate) const SAVE_DELAY: Duration = Duration::from_millis(400);

#[derive(Clone)]
pub struct Submission {
    pub message: SendMessage,
    pub create: Option<CreateThread>,
    pub thread: ThreadId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sending {
    Queued,
    Running,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Posting {
    Idle,
    Sending(ThreadId),
    Blocked(ThreadId),
    Accepted(ThreadId),
    Failed,
}

struct Outgoing {
    request: Submission,
    state: Sending,
    edited: u64,
}

pub enum Operation {
    Load,
    Write { request: WriteDraft, edited: u64 },
    Post(Submission),
}

const CONFLICT: &str = "This draft changed on another client. Choose which version to keep.";
const REMOVED: &str = "This Forge pairing was removed.";

enum Remote {
    Unloaded,
    Loaded(DraftSnapshot),
    Ambiguous {
        snapshot: DraftSnapshot,
        written: Value,
    },
}

enum Halt {
    Clear,
    Failed(String),
    Conflict,
    Removed,
}

pub struct DraftSession {
    scope: String,
    body: Value,
    remote: Remote,
    halt: Halt,
    adopting: usize,
    edited: u64,
    saved: u64,
    dirty: bool,
    busy: bool,
    outgoing: Option<Outgoing>,
    accepted: Option<ThreadId>,
}

impl DraftSession {
    pub fn new(scope: String) -> Self {
        Self {
            scope,
            body: Value::Null,
            remote: Remote::Unloaded,
            halt: Halt::Clear,
            adopting: 0,
            edited: 0,
            saved: 0,
            dirty: false,
            busy: false,
            outgoing: None,
            accepted: None,
        }
    }

    pub(crate) fn scope(&self) -> &str {
        &self.scope
    }

    pub const fn body(&self) -> &Value {
        &self.body
    }

    pub const fn is_loaded(&self) -> bool {
        !matches!(self.remote, Remote::Unloaded)
    }

    pub const fn conflict(&self) -> bool {
        matches!(self.halt, Halt::Conflict)
    }

    pub fn error(&self) -> Option<&str> {
        match &self.halt {
            Halt::Clear => None,
            Halt::Failed(error) => Some(error),
            Halt::Conflict => Some(CONFLICT),
            Halt::Removed => Some(REMOVED),
        }
    }

    const fn halted(&self) -> bool {
        !matches!(self.halt, Halt::Clear)
    }

    fn clear_failure(&mut self) {
        if matches!(self.halt, Halt::Failed(_)) {
            self.halt = Halt::Clear;
        }
    }

    fn snapshot(&self) -> Option<&DraftSnapshot> {
        match &self.remote {
            Remote::Unloaded => None,
            Remote::Loaded(snapshot) | Remote::Ambiguous { snapshot, .. } => Some(snapshot),
        }
    }

    pub fn evictable(&self) -> bool {
        !self.busy && !self.dirty && self.outgoing.is_none() && self.adopting == 0
    }

    pub fn pending(&self) -> bool {
        !self.removed() && (self.dirty || self.outgoing.is_some() || self.adopting > 0)
    }

    pub(crate) fn recover(&mut self, body: Value) {
        if self.removed() || body.is_null() {
            return;
        }
        self.body = body;
        self.edited = self.edited.wrapping_add(1);
        self.dirty = true;
    }

    pub(crate) fn adopting(&mut self) {
        self.adopting += 1;
    }

    pub(crate) fn adopted(&mut self, result: Result<BlobHash, String>) -> Option<BlobHash> {
        self.adopting = self
            .adopting
            .checked_sub(1)
            .expect("invariant: an adopted upload was counted when it started");
        if self.removed() {
            return None;
        }
        let hash = match result {
            Ok(hash) => hash,
            Err(error) => {
                self.fail(format!(
                    "Couldn't upload an image pasted before leaving this draft: {error}"
                ));
                return None;
            }
        };
        let mut body = match &self.body {
            Value::Null => DraftBody::default(),
            body => match serde_json::from_value::<DraftBody>(body.clone()) {
                Ok(body) => body,
                Err(error) => {
                    self.fail(format!(
                        "Couldn't add an image pasted before leaving this draft: {error}"
                    ));
                    return None;
                }
            },
        };
        if !body.images.contains(&hash) {
            body.images.push(hash.clone());
        }
        self.queue(serde_json::to_value(body).expect("invariant: DraftBody serializes"));
        Some(hash)
    }

    pub const fn removed(&self) -> bool {
        matches!(self.halt, Halt::Removed)
    }

    pub fn sending(&self) -> bool {
        !self.halted()
            && self
                .outgoing
                .as_ref()
                .is_some_and(|outgoing| outgoing.state != Sending::Failed)
    }

    pub fn posting(&self) -> Posting {
        match &self.outgoing {
            Some(outgoing) if self.sending() => Posting::Sending(outgoing.request.thread.clone()),
            Some(outgoing) if outgoing.state == Sending::Failed => Posting::Failed,
            Some(outgoing) => Posting::Blocked(outgoing.request.thread.clone()),
            None => self
                .accepted
                .clone()
                .map_or(Posting::Idle, Posting::Accepted),
        }
    }

    pub fn send_failed(&self) -> bool {
        self.outgoing
            .as_ref()
            .is_some_and(|outgoing| outgoing.state == Sending::Failed)
    }

    pub fn queue(&mut self, body: Value) -> bool {
        if self.removed() || self.body == body {
            return false;
        }
        self.body = body;
        self.edited = self.edited.wrapping_add(1);
        self.dirty = true;
        if !self.sending() {
            self.outgoing = None;
        }
        self.clear_failure();
        true
    }

    pub fn submit(&mut self, submission: Submission) {
        if self.removed() || self.sending() {
            return;
        }
        self.accepted = None;
        match &mut self.outgoing {
            Some(outgoing) => outgoing.state = Sending::Queued,
            None => {
                self.outgoing = Some(Outgoing {
                    request: submission,
                    state: Sending::Queued,
                    edited: self.edited,
                })
            }
        }
        self.clear_failure();
    }

    pub fn retry(&mut self) {
        if self.removed() {
            return;
        }
        self.clear_failure();
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.state = Sending::Queued;
        }
    }

    pub fn keep_local(&mut self) {
        if self.removed() {
            return;
        }
        self.halt = Halt::Clear;
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.state = Sending::Queued;
        }
    }

    pub fn use_remote(&mut self) {
        if self.removed() {
            return;
        }
        self.body = self
            .snapshot()
            .map_or(Value::Null, |snapshot| snapshot.body.clone());
        self.dirty = false;
        self.halt = Halt::Clear;
        self.outgoing = None;
    }

    pub fn remove(&mut self) {
        self.halt = Halt::Removed;
        self.busy = false;
        self.dirty = false;
        self.outgoing = None;
        self.accepted = None;
        self.body = Value::Null;
        self.remote = Remote::Unloaded;
    }

    pub fn next(&mut self) -> Option<Operation> {
        if self.busy || self.halted() {
            return None;
        }
        let posting = self.outgoing.as_ref().is_some_and(|outgoing| {
            outgoing.state == Sending::Queued && self.saved >= outgoing.edited
        });
        let operation = match &self.remote {
            Remote::Unloaded | Remote::Ambiguous { .. } => Operation::Load,
            Remote::Loaded(snapshot) if self.dirty && !posting => Operation::Write {
                request: WriteDraft {
                    expected_revision: snapshot.revision,
                    body: self.body.clone(),
                },
                edited: self.edited,
            },
            Remote::Loaded(snapshot) => {
                let revision = snapshot.revision;
                let outgoing = self.outgoing.as_mut()?;
                if outgoing.state != Sending::Queued {
                    return None;
                }
                outgoing.state = Sending::Running;
                outgoing.request.message.draft = Some(DraftRevision {
                    scope: self.scope.clone(),
                    revision,
                });
                Operation::Post(outgoing.request.clone())
            }
        };
        self.busy = true;
        Some(operation)
    }

    pub fn loaded(&mut self, result: Result<DraftSnapshot, String>) {
        if self.removed() {
            return;
        }
        self.busy = false;
        match result {
            Ok(snapshot) => {
                let (landed, unchanged) = match &self.remote {
                    Remote::Unloaded => (false, false),
                    Remote::Loaded(previous) => (false, snapshot.revision == previous.revision),
                    Remote::Ambiguous {
                        snapshot: previous,
                        written,
                    } => (
                        *written == snapshot.body,
                        snapshot.revision == previous.revision,
                    ),
                };
                let unseen = !landed
                    && !unchanged
                    && self.dirty
                    && !snapshot.body.is_null()
                    && snapshot.body != self.body;
                if !self.dirty {
                    self.body = snapshot.body.clone();
                }
                self.remote = Remote::Loaded(snapshot);
                if unseen {
                    self.halt_on_conflict();
                }
            }
            Err(error) => self.fail(format!("Couldn't load the draft: {error}")),
        }
    }

    pub fn written(
        &mut self,
        request: &WriteDraft,
        edited: u64,
        result: Result<DraftWriteResult, String>,
    ) {
        if self.removed() {
            return;
        }
        self.busy = false;
        match result {
            Ok(result) => {
                let matched = result.saved || result.current.body == request.body;
                self.remote = Remote::Loaded(result.current);
                if matched {
                    self.saved = edited;
                    if self.edited == edited {
                        self.dirty = false;
                    }
                } else {
                    self.halt_on_conflict();
                }
            }
            Err(error) => {
                let snapshot = match std::mem::replace(&mut self.remote, Remote::Unloaded) {
                    Remote::Loaded(snapshot) | Remote::Ambiguous { snapshot, .. } => snapshot,
                    Remote::Unloaded => {
                        unreachable!("invariant: draft {} wrote before it loaded", self.scope)
                    }
                };
                self.remote = Remote::Ambiguous {
                    snapshot,
                    written: request.body.clone(),
                };
                self.fail(format!("Couldn't save the draft: {error}"));
            }
        }
    }

    fn fail(&mut self, error: String) {
        tracing::warn!(scope = %self.scope, %error, "draft operation failed; releasing queued submission");
        self.release_submission();
        self.halt = Halt::Failed(error);
    }

    fn halt_on_conflict(&mut self) {
        tracing::warn!(scope = %self.scope, "draft changed on another client; releasing queued submission");
        self.release_submission();
        self.halt = Halt::Conflict;
    }

    fn release_submission(&mut self) {
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.state = Sending::Failed;
        }
    }

    pub fn posted(&mut self, result: Result<(), String>, thread: ThreadId) {
        if self.removed() {
            return;
        }
        self.busy = false;
        match result {
            Ok(()) => {
                let typed_since = self
                    .outgoing
                    .take()
                    .is_some_and(|outgoing| outgoing.edited != self.edited);
                self.accepted = Some(thread);
                self.remote = Remote::Unloaded;
                if typed_since {
                    self.dirty = true;
                } else {
                    self.body = Value::Null;
                    self.dirty = false;
                }
                self.halt = Halt::Clear;
            }
            Err(error) => self.fail(format!("Couldn't send the message: {error}")),
        }
    }
}
