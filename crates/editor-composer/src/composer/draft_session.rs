use super::attachment::{PendingUpload, UploadImageError};
use super::controller::DraftBody;
use artisan_api::{
    BlobHash, CreateThread, DraftRevision, DraftSnapshot, DraftWriteResult, ItemId, SendMessage,
    ThreadId, WriteDraft,
};
use editor_core::client::CallError;
use serde_json::Value;
use std::time::Duration;

pub(crate) const SAVE_DELAY: Duration = Duration::from_millis(400);

#[derive(Clone, Debug)]
pub struct Submission {
    pub message: SendMessage,
    pub create: Option<CreateThread>,
    pub thread: ThreadId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Posting {
    Idle,
    Sending(ThreadId),
    Blocked(ThreadId),
    Accepted(ThreadId),
    Failed,
}

#[derive(Debug)]
pub enum Operation {
    Load,
    Write { request: WriteDraft, edited: u64 },
    Post(Box<Submission>),
}

#[derive(Debug, thiserror::Error)]
pub enum PostMessageError {
    #[error("create thread {thread}")]
    CreateThread {
        thread: ThreadId,
        #[source]
        source: CallError,
    },
    #[error("send message {message} to thread {thread}")]
    Send {
        thread: ThreadId,
        message: ItemId,
        #[source]
        source: CallError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum AdoptImageError {
    #[error("the draft closed before the image finished uploading")]
    Closed,
    #[error(transparent)]
    Upload(#[from] UploadImageError),
}

#[derive(Debug, thiserror::Error)]
pub enum DraftFailure {
    #[error("Couldn't load the draft")]
    Load {
        scope: String,
        #[source]
        source: CallError,
    },
    #[error("Couldn't save the draft")]
    Save {
        scope: String,
        #[source]
        source: CallError,
    },
    #[error("Couldn't send the message")]
    Send {
        scope: String,
        #[source]
        source: PostMessageError,
    },
    #[error("Couldn't upload an image pasted before leaving this draft")]
    Upload {
        scope: String,
        #[source]
        source: AdoptImageError,
    },
    #[error("Couldn't add an image pasted before leaving this draft")]
    AddImage {
        scope: String,
        #[source]
        source: serde_json::Error,
    },
}

const CONFLICT: &str = "This draft changed on another client. Choose which version to keep.";
const REMOVED: &str = "This Forge pairing was removed.";
const NO_BODY: &Value = &Value::Null;

#[derive(Debug)]
enum Remote {
    Unloaded,
    Loaded(DraftSnapshot),
    Ambiguous {
        snapshot: DraftSnapshot,
        written: Value,
    },
}

#[derive(Debug)]
enum Halt {
    Clear,
    Failed(DraftFailure),
    Conflict { failed: Option<DraftFailure> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Idle,
    Loading,
    Writing,
    Posting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Queued,
    Running,
    Released,
}

#[derive(Debug)]
struct Outgoing {
    request: Submission,
    stage: Stage,
    edited: u64,
}

#[derive(Debug)]
enum Send {
    Idle,
    Outgoing(Box<Outgoing>),
    Accepted(ThreadId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AdoptionKey(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Upload {
    Queued,
    Running,
    Failed,
}

#[derive(Debug)]
struct Adoption {
    key: AdoptionKey,
    image: PendingUpload,
    upload: Upload,
}

#[derive(Clone, Copy, Debug)]
struct Edits {
    latest: u64,
    saved: u64,
}

impl Edits {
    const fn unsaved(self) -> bool {
        self.saved < self.latest
    }

    fn edit(&mut self) {
        self.latest = self
            .latest
            .checked_add(1)
            .expect("invariant: a draft is edited fewer than u64::MAX times");
    }

    fn saved_through(&mut self, edited: u64) {
        self.saved = self.saved.max(edited);
    }

    fn settle(&mut self) {
        self.saved = self.latest;
    }
}

#[derive(Debug)]
struct Draft {
    body: Value,
    remote: Remote,
    edits: Edits,
    adopting: Vec<Adoption>,
    next_adoption: u64,
    request: Request,
    halt: Halt,
    send: Send,
}

#[derive(Debug)]
enum State {
    Open(Box<Draft>),
    Removed,
}

#[derive(Debug)]
pub struct DraftSession {
    scope: String,
    state: State,
}

impl Draft {
    const fn halted(&self) -> bool {
        !matches!(self.halt, Halt::Clear)
    }

    fn outgoing(&self) -> Option<&Outgoing> {
        match &self.send {
            Send::Outgoing(outgoing) => Some(outgoing),
            Send::Idle | Send::Accepted(_) => None,
        }
    }

    fn sending(&self) -> bool {
        !self.halted()
            && self
                .outgoing()
                .is_some_and(|outgoing| outgoing.stage != Stage::Released)
    }

    fn requeue(&mut self) {
        if let Send::Outgoing(outgoing) = &mut self.send {
            outgoing.stage = Stage::Queued;
        }
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

    fn queue(&mut self, body: Value) -> bool {
        if self.body == body {
            return false;
        }
        self.body = body;
        self.edits.edit();
        if !self.sending()
            && self.request != Request::Posting
            && let Send::Outgoing(_) = self.send
        {
            self.send = Send::Idle;
        }
        self.clear_failure();
        true
    }

    fn release_submission(&mut self) {
        if let Send::Outgoing(outgoing) = &mut self.send {
            outgoing.stage = Stage::Released;
        }
    }

    fn fail(&mut self, scope: &str, failure: DraftFailure) {
        tracing::warn!(
            %scope,
            error = %diagnostics::chain(&failure),
            "draft operation failed; releasing queued submission"
        );
        self.release_submission();
        self.halt = match std::mem::replace(&mut self.halt, Halt::Clear) {
            Halt::Conflict { .. } => Halt::Conflict {
                failed: Some(failure),
            },
            Halt::Clear | Halt::Failed(_) => Halt::Failed(failure),
        };
    }

    fn halt_on_conflict(&mut self, scope: &str) {
        tracing::warn!(%scope, "draft changed on another client; releasing queued submission");
        self.release_submission();
        self.halt = match std::mem::replace(&mut self.halt, Halt::Clear) {
            Halt::Clear => Halt::Conflict { failed: None },
            Halt::Failed(failure) => Halt::Conflict {
                failed: Some(failure),
            },
            conflict @ Halt::Conflict { .. } => conflict,
        };
    }

    fn adopt(&mut self, image: PendingUpload) {
        let key = AdoptionKey(self.next_adoption);
        self.next_adoption = self
            .next_adoption
            .checked_add(1)
            .expect("invariant: a draft adopts fewer than u64::MAX images");
        self.adopting.push(Adoption {
            key,
            image,
            upload: Upload::Queued,
        });
    }

    fn finish(&mut self, scope: &str, expected: Request) {
        assert_eq!(
            self.request, expected,
            "invariant: draft {scope} finished a request it did not start"
        );
        self.request = Request::Idle;
    }
}

impl DraftSession {
    pub fn new(scope: String) -> Self {
        Self {
            scope,
            state: State::Open(Box::new(Draft {
                body: Value::Null,
                remote: Remote::Unloaded,
                edits: Edits {
                    latest: 0,
                    saved: 0,
                },
                adopting: Vec::new(),
                next_adoption: 0,
                request: Request::Idle,
                halt: Halt::Clear,
                send: Send::Idle,
            })),
        }
    }

    pub(crate) fn scope(&self) -> &str {
        &self.scope
    }

    const fn draft(&self) -> Option<&Draft> {
        match &self.state {
            State::Open(draft) => Some(draft),
            State::Removed => None,
        }
    }

    pub fn body(&self) -> &Value {
        self.draft().map_or(NO_BODY, |draft| &draft.body)
    }

    pub fn is_loaded(&self) -> bool {
        self.draft()
            .is_some_and(|draft| !matches!(draft.remote, Remote::Unloaded))
    }

    pub fn conflict(&self) -> bool {
        self.draft()
            .is_some_and(|draft| matches!(draft.halt, Halt::Conflict { .. }))
    }

    pub fn halted(&self) -> bool {
        self.draft().is_none_or(Draft::halted)
    }

    pub fn error(&self) -> Option<String> {
        match &self.state {
            State::Removed => Some(REMOVED.to_owned()),
            State::Open(draft) => match &draft.halt {
                Halt::Clear => None,
                Halt::Failed(failure) => Some(diagnostics::chain(failure)),
                Halt::Conflict { .. } => Some(CONFLICT.to_owned()),
            },
        }
    }

    pub fn evictable(&self) -> bool {
        self.draft().is_none_or(|draft| {
            draft.request == Request::Idle
                && !draft.edits.unsaved()
                && draft.outgoing().is_none()
                && draft.adopting.is_empty()
        })
    }

    pub fn pending(&self) -> bool {
        self.draft().is_some_and(|draft| {
            draft.edits.unsaved() || draft.outgoing().is_some() || !draft.adopting.is_empty()
        })
    }

    pub(crate) fn recover(&mut self, body: Value, images: Vec<PendingUpload>) {
        let State::Open(draft) = &mut self.state else {
            unreachable!(
                "invariant: draft {} is recovered as it opens, before it can be removed",
                self.scope
            );
        };
        if !body.is_null() {
            draft.body = body;
            draft.edits.edit();
        }
        for image in images {
            draft.adopt(image);
        }
    }

    pub(crate) fn adopt(&mut self, image: PendingUpload) {
        if let State::Open(draft) = &mut self.state {
            draft.adopt(image);
        }
    }

    pub(crate) fn next_uploads(&mut self) -> Vec<(AdoptionKey, PendingUpload)> {
        let State::Open(draft) = &mut self.state else {
            return Vec::new();
        };
        let halted = draft.halted();
        draft
            .adopting
            .iter_mut()
            .filter(|adoption| match adoption.upload {
                Upload::Queued => true,
                Upload::Failed => !halted,
                Upload::Running => false,
            })
            .map(|adoption| {
                adoption.upload = Upload::Running;
                (adoption.key, adoption.image.clone())
            })
            .collect()
    }

    pub(crate) fn unacknowledged_images(&self) -> Vec<PendingUpload> {
        self.draft().map_or_else(Vec::new, |draft| {
            draft
                .adopting
                .iter()
                .map(|adoption| adoption.image.clone())
                .collect()
        })
    }

    pub(crate) fn adopted(
        &mut self,
        key: AdoptionKey,
        result: Result<BlobHash, AdoptImageError>,
    ) -> Option<BlobHash> {
        let State::Open(draft) = &mut self.state else {
            return None;
        };
        let Some(index) = draft
            .adopting
            .iter()
            .position(|adoption| adoption.key == key)
        else {
            tracing::info!(scope = %self.scope, "an image upload finished after its draft version was discarded");
            return None;
        };
        assert_eq!(
            draft.adopting[index].upload,
            Upload::Running,
            "invariant: draft {} finished an image upload it started",
            self.scope
        );
        let hash = match result {
            Ok(hash) => {
                draft.adopting.remove(index);
                hash
            }
            Err(source) => {
                draft.adopting[index].upload = Upload::Failed;
                let failure = DraftFailure::Upload {
                    scope: self.scope.clone(),
                    source,
                };
                draft.fail(&self.scope, failure);
                return None;
            }
        };
        let mut body = match &draft.body {
            Value::Null => DraftBody::default(),
            body @ (Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Array(_)
            | Value::Object(_)) => match serde_json::from_value::<DraftBody>(body.clone()) {
                Ok(body) => body,
                Err(source) => {
                    let failure = DraftFailure::AddImage {
                        scope: self.scope.clone(),
                        source,
                    };
                    draft.fail(&self.scope, failure);
                    return None;
                }
            },
        };
        if !body.images.contains(&hash) {
            body.images.push(hash.clone());
        }
        draft.queue(serde_json::to_value(body).expect("invariant: DraftBody serializes"));
        Some(hash)
    }

    pub const fn removed(&self) -> bool {
        matches!(self.state, State::Removed)
    }

    pub fn sending(&self) -> bool {
        self.draft().is_some_and(Draft::sending)
    }

    pub fn posting(&self) -> Posting {
        let Some(draft) = self.draft() else {
            return Posting::Idle;
        };
        match &draft.send {
            Send::Idle => Posting::Idle,
            Send::Accepted(thread) => Posting::Accepted(thread.clone()),
            Send::Outgoing(outgoing) if draft.sending() => {
                Posting::Sending(outgoing.request.thread.clone())
            }
            Send::Outgoing(outgoing) if outgoing.stage == Stage::Released => Posting::Failed,
            Send::Outgoing(outgoing) => Posting::Blocked(outgoing.request.thread.clone()),
        }
    }

    pub fn send_failed(&self) -> bool {
        self.draft()
            .and_then(Draft::outgoing)
            .is_some_and(|outgoing| outgoing.stage == Stage::Released)
    }

    pub fn queue(&mut self, body: Value) -> bool {
        match &mut self.state {
            State::Open(draft) => draft.queue(body),
            State::Removed => false,
        }
    }

    pub fn submit(&mut self, submission: Submission) {
        let State::Open(draft) = &mut self.state else {
            return;
        };
        if draft.sending() {
            return;
        }
        match &mut draft.send {
            Send::Outgoing(outgoing) => outgoing.stage = Stage::Queued,
            Send::Idle | Send::Accepted(_) => {
                draft.send = Send::Outgoing(Box::new(Outgoing {
                    request: submission,
                    stage: Stage::Queued,
                    edited: draft.edits.latest,
                }));
            }
        }
        draft.clear_failure();
    }

    pub fn retry(&mut self) {
        if let State::Open(draft) = &mut self.state {
            draft.clear_failure();
            draft.requeue();
        }
    }

    pub fn keep_local(&mut self) {
        if let State::Open(draft) = &mut self.state {
            draft.halt = match std::mem::replace(&mut draft.halt, Halt::Clear) {
                Halt::Conflict {
                    failed: Some(failure),
                } => Halt::Failed(failure),
                Halt::Clear | Halt::Failed(_) | Halt::Conflict { failed: None } => Halt::Clear,
            };
            draft.requeue();
        }
    }

    pub fn use_remote(&mut self) {
        if let State::Open(draft) = &mut self.state {
            draft.body = draft
                .snapshot()
                .map_or(Value::Null, |snapshot| snapshot.body.clone());
            draft.edits.settle();
            draft.halt = Halt::Clear;
            if let Send::Outgoing(_) = draft.send {
                draft.send = Send::Idle;
            }
        }
    }

    pub fn remove(&mut self) {
        self.state = State::Removed;
    }

    pub fn next_operation(&mut self) -> Option<Operation> {
        let State::Open(draft) = &mut self.state else {
            return None;
        };
        if draft.request != Request::Idle || draft.halted() {
            return None;
        }
        let saved = draft.edits.saved;
        let posting = draft
            .outgoing()
            .is_some_and(|outgoing| outgoing.stage == Stage::Queued && saved >= outgoing.edited);
        let (request, operation) = match &draft.remote {
            Remote::Unloaded | Remote::Ambiguous { .. } => (Request::Loading, Operation::Load),
            Remote::Loaded(snapshot) if draft.edits.unsaved() && !posting => (
                Request::Writing,
                Operation::Write {
                    request: WriteDraft {
                        expected_revision: snapshot.revision,
                        body: draft.body.clone(),
                    },
                    edited: draft.edits.latest,
                },
            ),
            Remote::Loaded(snapshot) => {
                let revision = snapshot.revision;
                let Send::Outgoing(outgoing) = &mut draft.send else {
                    return None;
                };
                if outgoing.stage != Stage::Queued {
                    return None;
                }
                outgoing.stage = Stage::Running;
                outgoing.request.message.draft = Some(DraftRevision {
                    scope: self.scope.clone(),
                    revision,
                });
                (
                    Request::Posting,
                    Operation::Post(Box::new(outgoing.request.clone())),
                )
            }
        };
        draft.request = request;
        Some(operation)
    }

    pub fn loaded(&mut self, result: Result<DraftSnapshot, CallError>) {
        let State::Open(draft) = &mut self.state else {
            return;
        };
        draft.finish(&self.scope, Request::Loading);
        match result {
            Ok(snapshot) => {
                let (landed, unchanged) = match &draft.remote {
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
                let unsaved = draft.edits.unsaved();
                let unseen = !landed
                    && !unchanged
                    && unsaved
                    && !snapshot.body.is_null()
                    && snapshot.body != draft.body;
                if !unsaved {
                    draft.body = snapshot.body.clone();
                }
                draft.remote = Remote::Loaded(snapshot);
                if unseen {
                    draft.halt_on_conflict(&self.scope);
                }
            }
            Err(source) => {
                let failure = DraftFailure::Load {
                    scope: self.scope.clone(),
                    source,
                };
                draft.fail(&self.scope, failure);
            }
        }
    }

    pub fn written(
        &mut self,
        request: &WriteDraft,
        edited: u64,
        result: Result<DraftWriteResult, CallError>,
    ) {
        let State::Open(draft) = &mut self.state else {
            return;
        };
        draft.finish(&self.scope, Request::Writing);
        match result {
            Ok(result) => {
                let matched = result.saved || result.current.body == request.body;
                draft.remote = Remote::Loaded(result.current);
                if matched {
                    draft.edits.saved_through(edited);
                } else {
                    draft.halt_on_conflict(&self.scope);
                }
            }
            Err(source) => {
                let snapshot = match std::mem::replace(&mut draft.remote, Remote::Unloaded) {
                    Remote::Loaded(snapshot) | Remote::Ambiguous { snapshot, .. } => snapshot,
                    Remote::Unloaded => {
                        unreachable!("invariant: draft {} wrote before it loaded", self.scope)
                    }
                };
                draft.remote = Remote::Ambiguous {
                    snapshot,
                    written: request.body.clone(),
                };
                let failure = DraftFailure::Save {
                    scope: self.scope.clone(),
                    source,
                };
                draft.fail(&self.scope, failure);
            }
        }
    }

    pub fn posted(&mut self, result: Result<(), PostMessageError>, thread: ThreadId) {
        let State::Open(draft) = &mut self.state else {
            return;
        };
        draft.finish(&self.scope, Request::Posting);
        match result {
            Ok(()) => {
                let Send::Outgoing(outgoing) =
                    std::mem::replace(&mut draft.send, Send::Accepted(thread))
                else {
                    unreachable!(
                        "invariant: draft {} posted a message it was not sending",
                        self.scope
                    );
                };
                draft.remote = Remote::Unloaded;
                if outgoing.edited == draft.edits.latest {
                    draft.body = Value::Null;
                    draft.edits.settle();
                } else {
                    draft.edits.saved = outgoing.edited;
                }
            }
            Err(source) => {
                let failure = DraftFailure::Send {
                    scope: self.scope.clone(),
                    source,
                };
                draft.fail(&self.scope, failure);
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/composer/draft_session.rs"]
mod tests;
