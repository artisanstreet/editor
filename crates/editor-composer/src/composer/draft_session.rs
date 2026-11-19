use super::attachment::{PendingUpload, ReadImageError, UploadImageError};
use super::draft_body::{DraftBody, EMPTY_BODY, RemoteBody};
use artisan_api::{
    BlobHash, CreateThread, DraftRevision, DraftSnapshot, DraftWriteResult, Failure, ItemId,
    ResourceFailure, SendMessage, ThreadId,
};
use editor_core::client::CallError;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

pub(crate) const SAVE_DELAY: Duration = Duration::from_millis(400);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    Write {
        expected_revision: u64,
        body: Box<DraftBody>,
        edited: u64,
    },
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
    #[error("read the image")]
    Read(#[source] ReadImageError),
    #[error("the image reading task stopped")]
    ReadTask(#[source] tokio::task::JoinError),
    #[error(transparent)]
    Upload(#[from] UploadImageError),
}

impl AdoptImageError {
    fn permanent(&self) -> bool {
        match self {
            Self::Closed => false,
            Self::ReadTask(error) | Self::Upload(UploadImageError::Task(error)) => error.is_panic(),
            Self::Read(_) | Self::Upload(UploadImageError::Encode(_)) => true,
            Self::Upload(UploadImageError::Put(error)) => matches!(
                error.refused(),
                Some(Failure::Resource(
                    ResourceFailure::TooLarge { .. }
                        | ResourceFailure::NotWebp
                        | ResourceFailure::TooManyPixels { .. }
                ))
            ),
        }
    }
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
}

const CONFLICT: &str = "This draft changed on another client. Choose which version to keep.";
const REMOVED: &str = "This Forge pairing was removed.";
#[derive(Debug)]
struct Snapshot {
    revision: u64,
    body: RemoteBody,
}

impl From<DraftSnapshot> for Snapshot {
    fn from(snapshot: DraftSnapshot) -> Self {
        Self {
            revision: snapshot.revision,
            body: RemoteBody::from_wire(snapshot.body),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Base {
    revision: u64,
    written: Option<Box<DraftBody>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct UnsentPost {
    submission: Submission,
    sent: DraftBody,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptDraft {
    pub(crate) body: DraftBody,
    #[serde(default)]
    pub(crate) base: Option<Base>,
    #[serde(default)]
    pub(crate) post: Option<UnsentPost>,
}

#[derive(Debug)]
enum Remote {
    Unloaded(Option<Base>),
    Loaded(Snapshot),
    Ambiguous {
        snapshot: Snapshot,
        written: Box<DraftBody>,
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
    sent: DraftBody,
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

#[derive(Clone, Debug)]
pub(crate) enum AdoptedImage {
    Unread(PendingUpload),
    Read(Arc<[u8]>),
}

#[derive(Debug)]
enum Upload {
    Queued,
    Running,
    Waiting(DraftFailure),
}

#[derive(Debug)]
struct Adoption {
    key: AdoptionKey,
    name: String,
    image: AdoptedImage,
    upload: Upload,
}

#[derive(Debug)]
pub struct LostImage {
    pub name: String,
    pub failure: DraftFailure,
}

#[derive(Clone, Copy, Debug)]
enum Planned {
    Load,
    Write,
    Post,
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
    body: DraftBody,
    remote: Remote,
    edits: Edits,
    adopting: Vec<Adoption>,
    lost: Vec<LostImage>,
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

    fn snapshot(&self) -> Option<&Snapshot> {
        match &self.remote {
            Remote::Unloaded(_) => None,
            Remote::Loaded(snapshot) | Remote::Ambiguous { snapshot, .. } => Some(snapshot),
        }
    }

    fn queue(&mut self, body: DraftBody) -> bool {
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
            name: image.name().to_owned(),
            image: AdoptedImage::Unread(image),
            upload: Upload::Queued,
        });
    }

    fn requeue_waiting(&mut self) {
        for adoption in &mut self.adopting {
            if let Upload::Waiting(_) = adoption.upload {
                adoption.upload = Upload::Queued;
            }
        }
    }

    fn merge_image(&mut self, hash: BlobHash) {
        let mut body = self.body.clone();
        body.add_image(hash);
        if body != self.body {
            self.body = body;
            self.edits.edit();
        }
    }

    fn fail_adoption(&mut self, scope: &str, index: usize, source: AdoptImageError) {
        let permanent = source.permanent();
        let failure = DraftFailure::Upload {
            scope: scope.to_owned(),
            source,
        };
        tracing::warn!(
            %scope,
            permanent,
            error = %diagnostics::chain(&failure),
            "an image pasted before leaving the draft failed to upload"
        );
        if permanent {
            let adoption = self.adopting.remove(index);
            self.lost.push(LostImage {
                name: adoption.name,
                failure,
            });
        } else {
            self.adopting[index].upload = Upload::Waiting(failure);
        }
    }

    fn adoption(&self, scope: &str, key: AdoptionKey) -> Option<usize> {
        let index = self
            .adopting
            .iter()
            .position(|adoption| adoption.key == key);
        if index.is_none() {
            tracing::info!(%scope, "an image upload finished after its draft version was discarded");
        }
        index
    }

    fn plan(&self) -> Option<Planned> {
        if self.request != Request::Idle || self.halted() {
            return None;
        }
        let saved = self.edits.saved;
        let posting = self
            .outgoing()
            .is_some_and(|outgoing| outgoing.stage == Stage::Queued && saved >= outgoing.edited);
        match &self.remote {
            Remote::Unloaded(_) | Remote::Ambiguous { .. } => Some(Planned::Load),
            Remote::Loaded(_) if self.edits.unsaved() && !posting => Some(Planned::Write),
            Remote::Loaded(_) => posting.then_some(Planned::Post),
        }
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
                body: DraftBody::default(),
                remote: Remote::Unloaded(None),
                edits: Edits {
                    latest: 0,
                    saved: 0,
                },
                adopting: Vec::new(),
                lost: Vec::new(),
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

    pub fn body(&self) -> &DraftBody {
        self.draft().map_or(&EMPTY_BODY, |draft| &draft.body)
    }

    pub(crate) fn unreadable(&self) -> Option<&serde_json::Error> {
        self.draft()
            .filter(|draft| !draft.edits.unsaved())
            .and_then(Draft::snapshot)
            .and_then(|snapshot| snapshot.body.unreadable())
    }

    pub fn is_loaded(&self) -> bool {
        self.draft()
            .is_some_and(|draft| !matches!(draft.remote, Remote::Unloaded(_)))
    }

    pub(crate) fn presentable(&self) -> bool {
        self.draft().is_some_and(|draft| {
            !matches!(draft.remote, Remote::Unloaded(_)) || !draft.body.is_empty()
        })
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
                Halt::Clear => draft
                    .adopting
                    .iter()
                    .find_map(|adoption| match &adoption.upload {
                        Upload::Waiting(failure) => Some(diagnostics::chain(failure)),
                        Upload::Queued | Upload::Running => None,
                    }),
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
                && draft.lost.is_empty()
        })
    }

    pub fn stuck(&self) -> bool {
        self.draft().is_none_or(|draft| {
            draft.halted()
                || (draft.request == Request::Idle
                    && draft.plan().is_none()
                    && draft
                        .adopting
                        .iter()
                        .all(|adoption| matches!(adoption.upload, Upload::Waiting(_))))
        })
    }

    pub fn pending(&self) -> bool {
        self.draft().is_some_and(|draft| {
            draft.edits.unsaved() || draft.outgoing().is_some() || !draft.adopting.is_empty()
        })
    }

    pub(crate) fn recover(&mut self, recovery: KeptDraft, images: Vec<PendingUpload>) {
        let State::Open(draft) = &mut self.state else {
            unreachable!(
                "invariant: draft {} is recovered as it opens, before it can be removed",
                self.scope
            );
        };
        draft.remote = Remote::Unloaded(recovery.base);
        if let Some(post) = recovery.post {
            draft.body = post.sent.clone();
            draft.send = Send::Outgoing(Box::new(Outgoing {
                request: post.submission,
                sent: post.sent,
                stage: Stage::Queued,
                edited: draft.edits.latest,
            }));
        }
        if recovery.body != draft.body {
            draft.body = recovery.body;
            draft.edits.edit();
        }
        for image in images {
            draft.adopt(image);
        }
    }

    pub(crate) fn kept(&self) -> KeptDraft {
        KeptDraft {
            body: self.body().clone(),
            base: self.base(),
            post: self.unsent_post(),
        }
    }

    fn base(&self) -> Option<Base> {
        self.draft().and_then(|draft| match &draft.remote {
            Remote::Unloaded(base) => base.clone(),
            Remote::Loaded(snapshot) => Some(Base {
                revision: snapshot.revision,
                written: None,
            }),
            Remote::Ambiguous { snapshot, written } => Some(Base {
                revision: snapshot.revision,
                written: Some(written.clone()),
            }),
        })
    }

    fn unsent_post(&self) -> Option<UnsentPost> {
        self.draft()
            .and_then(Draft::outgoing)
            .filter(|outgoing| outgoing.stage != Stage::Released)
            .map(|outgoing| UnsentPost {
                submission: outgoing.request.clone(),
                sent: outgoing.sent.clone(),
            })
    }

    pub(crate) fn unsent(&self) -> Option<&DraftBody> {
        self.draft()
            .and_then(Draft::outgoing)
            .filter(|outgoing| outgoing.stage != Stage::Released)
            .map(|outgoing| &outgoing.sent)
    }

    pub(crate) fn adopt(&mut self, image: PendingUpload) {
        if let State::Open(draft) = &mut self.state {
            draft.adopt(image);
        }
    }

    pub(crate) fn next_uploads(&mut self) -> Vec<(AdoptionKey, AdoptedImage)> {
        let State::Open(draft) = &mut self.state else {
            return Vec::new();
        };
        draft
            .adopting
            .iter_mut()
            .filter(|adoption| match adoption.upload {
                Upload::Queued => true,
                Upload::Running | Upload::Waiting(_) => false,
            })
            .map(|adoption| {
                adoption.upload = Upload::Running;
                (adoption.key, adoption.image.clone())
            })
            .collect()
    }

    pub(crate) fn image_read(&mut self, key: AdoptionKey, bytes: Arc<[u8]>) {
        let State::Open(draft) = &mut self.state else {
            return;
        };
        if let Some(index) = draft.adoption(&self.scope, key) {
            draft.adopting[index].image = AdoptedImage::Read(bytes);
        }
    }

    pub fn take_lost_images(&mut self) -> Vec<LostImage> {
        match &mut self.state {
            State::Open(draft) => std::mem::take(&mut draft.lost),
            State::Removed => Vec::new(),
        }
    }

    pub(crate) fn unacknowledged_images(&self) -> Vec<AdoptedImage> {
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
        let index = draft.adoption(&self.scope, key)?;
        assert!(
            matches!(draft.adopting[index].upload, Upload::Running),
            "invariant: draft {} finished an image upload it started",
            self.scope
        );
        match result {
            Ok(hash) => {
                draft.adopting.remove(index);
                draft.merge_image(hash.clone());
                Some(hash)
            }
            Err(source) => {
                draft.fail_adoption(&self.scope, index, source);
                None
            }
        }
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

    pub fn queue(&mut self, body: DraftBody) -> bool {
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
                    sent: draft.body.clone(),
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
            draft.requeue_waiting();
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
                .map_or_else(DraftBody::default, |snapshot| snapshot.body.shown());
            draft.edits.settle();
            draft.adopting.clear();
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
        let planned = draft.plan()?;
        let (request, operation) = match (planned, &mut draft.remote, &mut draft.send) {
            (Planned::Load, _, _) => (Request::Loading, Operation::Load),
            (Planned::Write, Remote::Loaded(snapshot), _) => (
                Request::Writing,
                Operation::Write {
                    expected_revision: snapshot.revision,
                    body: Box::new(draft.body.clone()),
                    edited: draft.edits.latest,
                },
            ),
            (Planned::Post, Remote::Loaded(snapshot), Send::Outgoing(outgoing)) => {
                outgoing.stage = Stage::Running;
                outgoing.request.message.draft = Some(DraftRevision {
                    scope: self.scope.clone(),
                    revision: snapshot.revision,
                });
                (
                    Request::Posting,
                    Operation::Post(Box::new(outgoing.request.clone())),
                )
            }
            (Planned::Write | Planned::Post, Remote::Unloaded(_) | Remote::Ambiguous { .. }, _)
            | (Planned::Post, Remote::Loaded(_), Send::Idle | Send::Accepted(_)) => {
                unreachable!(
                    "invariant: draft {} plans writes and posts only against a loaded draft, and posts only an outgoing message",
                    self.scope
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
                let snapshot = Snapshot::from(snapshot);
                let (landed, unchanged) = match &draft.remote {
                    Remote::Unloaded(None) => (false, false),
                    Remote::Unloaded(Some(base)) => (
                        base.written
                            .as_ref()
                            .is_some_and(|written| snapshot.body.matches(written)),
                        snapshot.revision == base.revision,
                    ),
                    Remote::Loaded(previous) => (false, snapshot.revision == previous.revision),
                    Remote::Ambiguous {
                        snapshot: previous,
                        written,
                    } => (
                        snapshot.body.matches(written),
                        snapshot.revision == previous.revision,
                    ),
                };
                let unsaved = draft.edits.unsaved();
                let unseen = !landed
                    && !unchanged
                    && unsaved
                    && !snapshot.body.is_empty()
                    && !snapshot.body.matches(&draft.body);
                if !unsaved {
                    draft.body = snapshot.body.shown();
                }
                draft.remote = Remote::Loaded(snapshot);
                draft.requeue_waiting();
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
        written: &DraftBody,
        edited: u64,
        result: Result<DraftWriteResult, CallError>,
    ) {
        let State::Open(draft) = &mut self.state else {
            return;
        };
        draft.finish(&self.scope, Request::Writing);
        match result {
            Ok(result) => {
                let current = Snapshot::from(result.current);
                let matched = result.saved || current.body.matches(written);
                draft.remote = Remote::Loaded(current);
                draft.requeue_waiting();
                if matched {
                    draft.edits.saved_through(edited);
                } else {
                    draft.halt_on_conflict(&self.scope);
                }
            }
            Err(source) => {
                let snapshot = match std::mem::replace(&mut draft.remote, Remote::Unloaded(None)) {
                    Remote::Loaded(snapshot) | Remote::Ambiguous { snapshot, .. } => snapshot,
                    Remote::Unloaded(_) => {
                        unreachable!("invariant: draft {} wrote before it loaded", self.scope)
                    }
                };
                draft.remote = Remote::Ambiguous {
                    snapshot,
                    written: Box::new(written.clone()),
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
                draft.remote = Remote::Unloaded(None);
                draft.requeue_waiting();
                if outgoing.edited == draft.edits.latest {
                    draft.body = DraftBody::default();
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
