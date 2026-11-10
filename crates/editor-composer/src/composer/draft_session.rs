use artisan_api::{
    CreateThread, DraftRevision, DraftSnapshot, DraftWriteResult, SendMessage, ThreadId, WriteDraft,
};
use serde_json::Value;
use std::time::Duration;

pub const SAVE_DELAY: Duration = Duration::from_millis(400);

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

struct Outgoing {
    request: Submission,
    state: Sending,
}

pub enum Operation {
    Load,
    Write { request: WriteDraft, edited: u64 },
    Post(Submission),
}

pub struct DraftSession {
    pub scope: String,
    pub body: Value,
    pub loaded: bool,
    pub error: Option<String>,
    pub conflict: bool,
    snapshot: DraftSnapshot,
    edited: u64,
    dirty: bool,
    busy: bool,
    removed: bool,
    outgoing: Option<Outgoing>,
}

impl DraftSession {
    pub fn new(scope: String) -> Self {
        Self {
            scope,
            body: Value::Null,
            loaded: false,
            error: None,
            conflict: false,
            snapshot: DraftSnapshot::default(),
            edited: 0,
            dirty: false,
            busy: false,
            removed: false,
            outgoing: None,
        }
    }

    pub fn evictable(&self) -> bool {
        !self.busy && !self.dirty && self.outgoing.is_none()
    }

    pub fn pending(&self) -> bool {
        !self.removed && (self.dirty || self.outgoing.is_some())
    }

    pub fn removed(&self) -> bool {
        self.removed
    }

    pub fn sending(&self) -> bool {
        !self.conflict
            && self.error.is_none()
            && self
                .outgoing
                .as_ref()
                .is_some_and(|outgoing| outgoing.state != Sending::Failed)
    }

    pub fn queue(&mut self, body: Value) -> bool {
        if self.removed || self.body == body {
            return false;
        }
        self.body = body;
        self.edited = self.edited.wrapping_add(1);
        self.dirty = true;
        if !self.sending() {
            self.outgoing = None;
        }
        if !self.conflict {
            self.error = None;
        }
        true
    }

    pub fn submit(&mut self, submission: Submission) {
        if self.removed || self.sending() {
            return;
        }
        match &mut self.outgoing {
            Some(outgoing) => outgoing.state = Sending::Queued,
            None => {
                self.outgoing = Some(Outgoing {
                    request: submission,
                    state: Sending::Queued,
                })
            }
        }
        if !self.conflict {
            self.error = None;
        }
    }

    pub fn retry(&mut self) {
        if self.removed {
            return;
        }
        self.error = None;
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.state = Sending::Queued;
        }
    }

    pub fn keep_local(&mut self) {
        if self.removed {
            return;
        }
        self.conflict = false;
        self.error = None;
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.state = Sending::Queued;
        }
    }

    pub fn use_remote(&mut self) {
        if self.removed {
            return;
        }
        self.body = self.snapshot.body.clone();
        self.dirty = false;
        self.conflict = false;
        self.error = None;
        self.outgoing = None;
    }

    pub fn remove(&mut self) {
        self.removed = true;
        self.busy = false;
        self.dirty = false;
        self.outgoing = None;
        self.body = Value::Null;
        self.snapshot.body = Value::Null;
        self.error = Some("This Forge pairing was removed.".into());
    }

    pub fn next(&mut self) -> Option<Operation> {
        if self.removed || self.busy || self.conflict || self.error.is_some() {
            return None;
        }
        let operation = if !self.loaded {
            Operation::Load
        } else if self.dirty {
            Operation::Write {
                request: WriteDraft {
                    expected_revision: self.snapshot.revision,
                    body: self.body.clone(),
                },
                edited: self.edited,
            }
        } else {
            let outgoing = self.outgoing.as_mut()?;
            if outgoing.state != Sending::Queued {
                return None;
            }
            outgoing.state = Sending::Running;
            outgoing.request.message.draft = Some(DraftRevision {
                scope: self.scope.clone(),
                revision: self.snapshot.revision,
            });
            Operation::Post(outgoing.request.clone())
        };
        self.busy = true;
        Some(operation)
    }

    pub fn loaded(&mut self, result: Result<DraftSnapshot, String>) {
        if self.removed {
            return;
        }
        self.busy = false;
        match result {
            Ok(snapshot) => {
                if !self.dirty {
                    self.body = snapshot.body.clone();
                }
                self.snapshot = snapshot;
                self.loaded = true;
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
        if self.removed {
            return;
        }
        self.busy = false;
        match result {
            Ok(result) => {
                let matched = result.saved || result.current.body == request.body;
                self.snapshot = result.current;
                if matched {
                    if self.edited == edited {
                        self.dirty = false;
                    }
                } else {
                    self.conflict = true;
                    self.fail(
                        "This draft changed on another client. Choose which version to keep."
                            .into(),
                    );
                }
            }
            Err(error) => self.fail(format!("Couldn't save the draft: {error}")),
        }
    }

    fn fail(&mut self, error: String) {
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.state = Sending::Failed;
        }
        self.error = Some(error);
    }

    pub fn posted(&mut self, result: Result<(), String>) {
        if self.removed {
            return;
        }
        self.busy = false;
        match result {
            Ok(()) => {
                self.snapshot.revision = self.snapshot.revision.saturating_add(1);
                self.snapshot.body = Value::Null;
                self.body = Value::Null;
                self.dirty = false;
                self.outgoing = None;
                self.error = None;
                self.loaded = false;
            }
            Err(error) => self.fail(format!("Couldn't send the message: {error}")),
        }
    }
}
