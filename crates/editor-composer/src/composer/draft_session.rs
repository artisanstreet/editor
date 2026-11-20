use super::attachment::{PendingUpload, ReadImageError, UploadImageError};
use super::draft_body::{DraftBody, EMPTY_BODY, RemoteBody};
use artisan_api::{
    BlobHash, CreateThread, DraftRevision, DraftSnapshot, DraftWriteResult, ErrorCode, Failure,
    ItemId, ResourceFailure, SendMessage, ThreadId,
};
use editor_core::client::CallError;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc, time::Duration};

pub(crate) const SAVE_DELAY: Duration = Duration::from_millis(400);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    pub message: SendMessage,
    pub create: Option<CreateThread>,
    pub thread: ThreadId,
}

impl Submission {
    fn resends(&self, other: &Self) -> bool {
        let same_thread = match (&self.create, &other.create) {
            (None, None) => self.thread == other.thread,
            (Some(created), Some(other)) => {
                created.project_id == other.project_id
                    && created.config == other.config
                    && created.title == other.title
                    && created.parent_thread_id == other.parent_thread_id
                    && created.spawned_by == other.spawned_by
            }
            (Some(_), None) | (None, Some(_)) => false,
        };
        same_thread
            && self.message.text == other.message.text
            && self.message.images == other.message.images
            && self.message.config == other.message.config
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Posting {
    Idle,
    Sending(ThreadId),
    Accepted(ThreadId),
    Failed,
}

#[derive(Debug)]
pub enum Operation {
    Load,
    Write {
        expected_revision: u64,
        body: Box<DraftBody>,
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
                Some(
                    Failure::Resource(
                        ResourceFailure::TooLarge { .. }
                            | ResourceFailure::NotWebp
                            | ResourceFailure::TooManyPixels { .. }
                    ) | Failure::Legacy {
                        code: ErrorCode::BadRequest
                    }
                )
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

impl DraftFailure {
    fn problem(&self) -> editor_core::Problem {
        let headline = format!("{self}.");
        match self {
            Self::Load { source, .. } | Self::Save { source, .. } => {
                editor_core::Problem::calling_forge(&headline, source)
            }
            Self::Send { source, .. } => editor_core::Problem::caused_by(headline, source),
            Self::Upload { source, .. } => editor_core::Problem::caused_by(headline, source),
        }
    }
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
    #[serde(default)]
    released: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeptLostImage {
    pub(crate) name: String,
    pub(crate) reason: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptDraft {
    pub(crate) body: DraftBody,
    #[serde(default)]
    pub(crate) base: Option<Base>,
    #[serde(default)]
    pub(crate) post: Option<UnsentPost>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) lost: Vec<KeptLostImage>,
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

#[derive(Debug)]
enum Request {
    Idle,
    Loading,
    Writing { body: Box<DraftBody>, edited: u64 },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LostKey(u64);

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LostImage {
    pub(crate) key: LostKey,
    pub(crate) name: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unrecovered {
    SetAside(PathBuf),
    Left(PathBuf),
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

    const fn settle(&mut self) {
        self.saved = self.latest;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    Own,
    Changed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arrival {
    Kept,
    Replaced,
}

#[derive(Debug)]
struct Draft {
    body: DraftBody,
    remote: Remote,
    edits: Edits,
    uploading: bool,
    adopting: Vec<Adoption>,
    lost: Vec<LostImage>,
    next_key: u64,
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
    epoch: u64,
    handed_back: Option<u64>,
    unrecovered: Option<Unrecovered>,
    state: State,
}

fn fold_back(sent: &DraftBody, typed: DraftBody) -> DraftBody {
    if typed == *sent {
        return typed;
    }
    let DraftBody {
        text,
        images,
        files,
        config,
        project,
        follow_ups: mut parked,
    } = typed;
    if !text.trim().is_empty() || !images.is_empty() || !files.is_empty() {
        parked.push(DraftBody {
            text,
            images,
            files,
            config,
            project,
            follow_ups: Vec::new(),
        });
    }
    DraftBody {
        follow_ups: parked,
        ..sent.clone()
    }
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

    fn in_flight(&self) -> Option<&Outgoing> {
        self.outgoing()
            .filter(|outgoing| outgoing.stage != Stage::Released)
    }

    fn released(&self) -> bool {
        self.outgoing()
            .is_some_and(|outgoing| outgoing.stage == Stage::Released)
    }

    fn holds_only_the_message_in_flight(&self) -> bool {
        self.body.is_empty()
            || self.in_flight().is_some_and(|outgoing| {
                outgoing.sent == self.body && outgoing.sent.follow_ups.is_empty()
            })
    }

    fn next_key(&mut self) -> u64 {
        let key = self.next_key;
        self.next_key = self
            .next_key
            .checked_add(1)
            .expect("invariant: a draft hands out fewer than u64::MAX image keys");
        key
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

    fn hand_back(&mut self) -> bool {
        let Send::Outgoing(outgoing) = &mut self.send else {
            return false;
        };
        if outgoing.stage == Stage::Released {
            return false;
        }
        outgoing.stage = Stage::Released;
        if self.body != outgoing.sent {
            self.body = fold_back(&outgoing.sent, std::mem::take(&mut self.body));
            self.edits.edit();
        }
        true
    }

    fn fail(&mut self, scope: &str, failure: DraftFailure) -> bool {
        tracing::warn!(
            %scope,
            error = %diagnostics::chain(&failure),
            "draft operation failed; handing the queued message back"
        );
        let handed_back = self.hand_back();
        self.halt = match std::mem::replace(&mut self.halt, Halt::Clear) {
            Halt::Conflict { .. } => Halt::Conflict {
                failed: Some(failure),
            },
            Halt::Clear | Halt::Failed(_) => Halt::Failed(failure),
        };
        handed_back
    }

    const fn local_edits(&self) -> bool {
        self.edits.unsaved() || self.uploading
    }

    fn halt_on_conflict(&mut self, scope: &str) {
        tracing::warn!(%scope, "draft changed on another client; asking which version to keep");
        self.halt = match std::mem::replace(&mut self.halt, Halt::Clear) {
            Halt::Clear => Halt::Conflict { failed: None },
            Halt::Failed(failure) => Halt::Conflict {
                failed: Some(failure),
            },
            conflict @ Halt::Conflict { .. } => conflict,
        };
    }

    fn arrived(&mut self, scope: &str, snapshot: Snapshot, origin: Origin) -> Arrival {
        if let Some(outgoing) = self.in_flight()
            && snapshot.body.matches(&outgoing.sent)
        {
            let edited = outgoing.edited;
            self.edits.saved_through(edited);
        }
        let gives_way = !self.uploading
            && self.holds_only_the_message_in_flight()
            && match origin {
                Origin::Own => false,
                Origin::Changed => true,
                Origin::Unknown => !snapshot.body.is_empty(),
            };
        let arrival = if snapshot.body.matches(&self.body) {
            self.edits.settle();
            Arrival::Kept
        } else if !self.local_edits() {
            self.show(snapshot.body.shown())
        } else if origin == Origin::Own {
            Arrival::Kept
        } else if gives_way {
            self.edits.settle();
            self.show(snapshot.body.shown())
        } else {
            if !snapshot.body.is_empty() {
                self.halt_on_conflict(scope);
            }
            Arrival::Kept
        };
        self.remote = Remote::Loaded(snapshot);
        self.requeue_waiting();
        arrival
    }

    fn show(&mut self, body: DraftBody) -> Arrival {
        if self.body == body {
            Arrival::Kept
        } else {
            self.body = body;
            Arrival::Replaced
        }
    }

    fn adopt(&mut self, image: PendingUpload) {
        let key = AdoptionKey(self.next_key());
        self.adopting.push(Adoption {
            key,
            name: image.name().to_owned(),
            image: AdoptedImage::Unread(image),
            upload: Upload::Queued,
        });
    }

    fn lose(&mut self, name: String, reason: String) {
        let key = LostKey(self.next_key());
        self.lost.push(LostImage { key, name, reason });
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
            self.lose(adoption.name, diagnostics::chain(&failure));
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

    fn post_ready(&self, conflicted: bool) -> bool {
        self.outgoing().is_some_and(|outgoing| {
            outgoing.stage == Stage::Queued && (conflicted || self.edits.saved >= outgoing.edited)
        })
    }

    fn plan(&self) -> Option<Planned> {
        if !matches!(self.request, Request::Idle) {
            return None;
        }
        let loaded = matches!(self.remote, Remote::Loaded(_));
        match &self.halt {
            Halt::Failed(_) | Halt::Conflict { failed: Some(_) } => None,
            Halt::Conflict { failed: None } => {
                (loaded && self.post_ready(true)).then_some(Planned::Post)
            }
            Halt::Clear if !loaded => Some(Planned::Load),
            Halt::Clear if self.post_ready(false) => Some(Planned::Post),
            Halt::Clear => self.edits.unsaved().then_some(Planned::Write),
        }
    }

    fn base(&self) -> Option<Base> {
        match (&self.remote, &self.request) {
            (Remote::Unloaded(base), _) => base.clone(),
            (Remote::Loaded(snapshot), Request::Writing { body, .. }) => Some(Base {
                revision: snapshot.revision,
                written: Some(body.clone()),
            }),
            (Remote::Loaded(snapshot), Request::Idle | Request::Loading | Request::Posting) => {
                Some(Base {
                    revision: snapshot.revision,
                    written: None,
                })
            }
            (Remote::Ambiguous { snapshot, written }, _) => Some(Base {
                revision: snapshot.revision,
                written: Some(written.clone()),
            }),
        }
    }
}

impl DraftSession {
    pub fn new(scope: String) -> Self {
        Self {
            scope,
            epoch: 0,
            handed_back: None,
            unrecovered: None,
            state: State::Open(Box::new(Draft {
                body: DraftBody::default(),
                remote: Remote::Unloaded(None),
                edits: Edits {
                    latest: 0,
                    saved: 0,
                },
                uploading: false,
                adopting: Vec::new(),
                lost: Vec::new(),
                next_key: 0,
                request: Request::Idle,
                halt: Halt::Clear,
                send: Send::Idle,
            })),
        }
    }

    pub(crate) fn scope(&self) -> &str {
        &self.scope
    }

    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    fn replaced(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
    }

    fn handed_back(&mut self) {
        self.replaced();
        self.handed_back = Some(self.epoch);
    }

    fn dropped(&self, work: &'static str) {
        tracing::info!(
            scope = %self.scope,
            work,
            "this draft's Forge pairing was removed, so its work is dropped"
        );
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

    pub(crate) fn shown(&self) -> Option<&DraftBody> {
        let draft = self.draft()?;
        let sending_it = draft
            .in_flight()
            .is_some_and(|outgoing| outgoing.sent == draft.body);
        let nothing_yet = matches!(draft.remote, Remote::Unloaded(_)) && draft.body.is_empty();
        (!sending_it && !nothing_yet).then_some(&draft.body)
    }

    pub(crate) fn unreadable(&self) -> Option<&serde_json::Error> {
        self.draft()
            .filter(|draft| !draft.edits.unsaved())
            .and_then(Draft::snapshot)
            .and_then(|snapshot| snapshot.body.unreadable())
    }

    pub(crate) const fn take_unrecovered(&mut self) -> Option<Unrecovered> {
        self.unrecovered.take()
    }

    pub(crate) fn set_unrecovered(&mut self, unrecovered: Unrecovered) {
        self.unrecovered = Some(unrecovered);
    }

    pub fn is_loaded(&self) -> bool {
        self.draft()
            .is_some_and(|draft| !matches!(draft.remote, Remote::Unloaded(_)))
    }

    pub fn conflict(&self) -> bool {
        self.draft()
            .is_some_and(|draft| matches!(draft.halt, Halt::Conflict { .. }))
    }

    pub fn halted(&self) -> bool {
        self.draft().is_none_or(Draft::halted)
    }

    pub fn error(&self) -> Option<editor_core::Problem> {
        match &self.state {
            State::Removed => Some(editor_core::Problem::new(REMOVED)),
            State::Open(draft) => match &draft.halt {
                Halt::Clear => draft
                    .adopting
                    .iter()
                    .find_map(|adoption| match &adoption.upload {
                        Upload::Waiting(failure) => Some(failure.problem()),
                        Upload::Queued | Upload::Running => None,
                    }),
                Halt::Failed(failure) => Some(failure.problem()),
                Halt::Conflict { .. } => Some(editor_core::Problem::new(CONFLICT)),
            },
        }
    }

    pub fn evictable(&self) -> bool {
        self.draft().is_none_or(|draft| {
            matches!(draft.request, Request::Idle)
                && !draft.edits.unsaved()
                && draft.outgoing().is_none()
                && draft.adopting.is_empty()
                && draft.lost.is_empty()
        })
    }

    pub(crate) fn stuck(&self) -> bool {
        self.draft().is_none_or(|draft| {
            draft.halted()
                || (matches!(draft.request, Request::Idle)
                    && draft.plan().is_none()
                    && draft
                        .adopting
                        .iter()
                        .all(|adoption| matches!(adoption.upload, Upload::Waiting(_))))
        })
    }

    pub fn pending(&self) -> bool {
        self.draft().is_some_and(|draft| {
            draft.edits.unsaved() || draft.in_flight().is_some() || !draft.adopting.is_empty()
        })
    }

    pub(crate) fn keepable(&self) -> bool {
        self.pending()
            || self
                .draft()
                .is_some_and(|draft| !draft.lost.is_empty() || draft.released())
    }

    pub(crate) fn settled_with_forge(&self) -> bool {
        self.draft()
            .is_some_and(|draft| matches!(draft.remote, Remote::Loaded(_)))
            && !self.keepable()
    }

    pub(crate) fn recover(
        &mut self,
        recovery: KeptDraft,
        images: Vec<PendingUpload>,
        damaged: Vec<KeptLostImage>,
    ) {
        let State::Open(draft) = &mut self.state else {
            unreachable!(
                "invariant: draft {} is recovered as it opens, before it can be removed",
                self.scope
            );
        };
        draft.remote = Remote::Unloaded(recovery.base);
        draft.edits.edit();
        if let Some(post) = recovery.post {
            let differs = post.sent != recovery.body;
            draft.send = Send::Outgoing(Box::new(Outgoing {
                request: post.submission,
                sent: post.sent,
                stage: if post.released {
                    Stage::Released
                } else {
                    Stage::Queued
                },
                edited: draft.edits.latest,
            }));
            if differs {
                draft.edits.edit();
            }
        }
        draft.body = recovery.body;
        for lost in recovery.lost.into_iter().chain(damaged) {
            draft.lose(lost.name, lost.reason);
        }
        for image in images {
            draft.adopt(image);
        }
    }

    pub(crate) fn kept(&self) -> KeptDraft {
        KeptDraft {
            body: self.body().clone(),
            base: self.draft().and_then(Draft::base),
            post: self
                .draft()
                .and_then(Draft::outgoing)
                .map(|outgoing| UnsentPost {
                    submission: outgoing.request.clone(),
                    sent: outgoing.sent.clone(),
                    released: outgoing.stage == Stage::Released,
                }),
            lost: self.draft().map_or_else(Vec::new, |draft| {
                draft
                    .lost
                    .iter()
                    .map(|lost| KeptLostImage {
                        name: lost.name.clone(),
                        reason: lost.reason.clone(),
                    })
                    .collect()
            }),
        }
    }

    pub(crate) fn parked_beside_failed_send(&self) -> bool {
        self.draft()
            .is_some_and(|draft| draft.released() && !draft.body.follow_ups.is_empty())
    }

    pub(crate) fn lost(&self) -> &[LostImage] {
        self.draft().map_or(&[], |draft| draft.lost.as_slice())
    }

    pub(crate) fn dismiss_lost(&mut self, key: LostKey) {
        if let State::Open(draft) = &mut self.state {
            draft.lost.retain(|lost| lost.key != key);
        }
    }

    pub(crate) fn adopt(&mut self, image: PendingUpload) {
        let State::Open(draft) = &mut self.state else {
            self.dropped("an image handed over from the composer");
            return;
        };
        draft.adopt(image);
    }

    pub(crate) fn view_closed(&mut self) {
        if let State::Open(draft) = &mut self.state {
            draft.uploading = false;
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
            self.dropped("a handed-over image that was read");
            return;
        };
        if let Some(index) = draft.adoption(&self.scope, key) {
            draft.adopting[index].image = AdoptedImage::Read(bytes);
        }
    }

    pub(crate) fn unacknowledged_images(&self) -> Vec<(String, AdoptedImage)> {
        self.draft().map_or_else(Vec::new, |draft| {
            draft
                .adopting
                .iter()
                .map(|adoption| (adoption.name.clone(), adoption.image.clone()))
                .collect()
        })
    }

    pub(crate) fn adopted(
        &mut self,
        key: AdoptionKey,
        result: Result<BlobHash, AdoptImageError>,
    ) -> Option<BlobHash> {
        let State::Open(draft) = &mut self.state else {
            self.dropped("a handed-over image's upload result");
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
        self.draft()
            .is_some_and(|draft| draft.in_flight().is_some())
    }

    pub(crate) fn sendable(&self) -> bool {
        self.draft()
            .is_some_and(|draft| draft.in_flight().is_none())
    }

    pub fn posting(&self) -> Posting {
        let Some(draft) = self.draft() else {
            return Posting::Idle;
        };
        match &draft.send {
            Send::Idle => Posting::Idle,
            Send::Accepted(thread) => Posting::Accepted(thread.clone()),
            Send::Outgoing(outgoing) if outgoing.stage == Stage::Released => Posting::Failed,
            Send::Outgoing(outgoing) => Posting::Sending(outgoing.request.thread.clone()),
        }
    }

    pub fn send_failed(&self) -> bool {
        self.draft().is_some_and(Draft::released)
    }

    pub fn queue(&mut self, body: DraftBody, seen: u64, uploading: bool) -> bool {
        let State::Open(draft) = &mut self.state else {
            self.dropped("an edit");
            return false;
        };
        draft.uploading = uploading;
        let stale = seen != self.epoch;
        let over_hand_back = stale && self.handed_back == Some(self.epoch);
        let body = match draft.outgoing().filter(|_| over_hand_back) {
            Some(failed) => fold_back(&failed.sent, body),
            None => body,
        };
        if draft.body == body {
            return false;
        }
        let unseen = stale
            && !over_hand_back
            && draft
                .snapshot()
                .is_some_and(|snapshot| !snapshot.body.is_empty() && !snapshot.body.matches(&body));
        draft.body = body;
        draft.edits.edit();
        draft.clear_failure();
        if unseen {
            draft.halt_on_conflict(&self.scope);
        }
        true
    }

    pub fn submit(&mut self, submission: Submission) {
        let State::Open(draft) = &mut self.state else {
            self.dropped("a message sent from the composer");
            return;
        };
        if draft.in_flight().is_some() {
            return;
        }
        let request = match std::mem::replace(&mut draft.send, Send::Idle) {
            Send::Outgoing(failed) if failed.request.resends(&submission) => failed.request,
            Send::Outgoing(_) | Send::Idle | Send::Accepted(_) => submission,
        };
        draft.send = Send::Outgoing(Box::new(Outgoing {
            request,
            sent: draft.body.clone(),
            stage: Stage::Queued,
            edited: draft.edits.latest,
        }));
        draft.clear_failure();
    }

    pub fn retry(&mut self) {
        if let State::Open(draft) = &mut self.state {
            draft.clear_failure();
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
        }
    }

    pub fn use_remote(&mut self) {
        let State::Open(draft) = &mut self.state else {
            return;
        };
        draft.body = draft
            .snapshot()
            .map_or_else(DraftBody::default, |snapshot| snapshot.body.shown());
        draft.edits.settle();
        draft.adopting.clear();
        draft.halt = Halt::Clear;
        if draft.released() {
            draft.send = Send::Idle;
        }
        self.replaced();
    }

    pub fn remove(&mut self) {
        self.state = State::Removed;
        self.replaced();
    }

    pub fn next_operation(&mut self) -> Option<Operation> {
        let State::Open(draft) = &mut self.state else {
            return None;
        };
        let planned = draft.plan()?;
        let (request, operation) = match (planned, &draft.remote, &mut draft.send) {
            (Planned::Load, _, _) => (Request::Loading, Operation::Load),
            (Planned::Write, Remote::Loaded(snapshot), _) => (
                Request::Writing {
                    body: Box::new(draft.body.clone()),
                    edited: draft.edits.latest,
                },
                Operation::Write {
                    expected_revision: snapshot.revision,
                    body: Box::new(draft.body.clone()),
                },
            ),
            (Planned::Post, Remote::Loaded(snapshot), Send::Outgoing(outgoing)) => {
                outgoing.stage = Stage::Running;
                outgoing.request.message.draft =
                    snapshot
                        .body
                        .matches(&outgoing.sent)
                        .then(|| DraftRevision {
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
            self.dropped("a draft load's result");
            return;
        };
        let request = std::mem::replace(&mut draft.request, Request::Idle);
        assert!(
            matches!(request, Request::Loading),
            "invariant: draft {} finished a load it did not start",
            self.scope
        );
        match result {
            Ok(snapshot) => {
                let snapshot = Snapshot::from(snapshot);
                let own = |own: bool| if own { Origin::Own } else { Origin::Changed };
                let origin =
                    match &draft.remote {
                        Remote::Unloaded(None) => Origin::Unknown,
                        Remote::Unloaded(Some(base)) => own(snapshot.revision == base.revision
                            || base
                                .written
                                .as_ref()
                                .is_some_and(|written| snapshot.body.matches(written))),
                        Remote::Loaded(previous) => own(snapshot.revision == previous.revision),
                        Remote::Ambiguous {
                            snapshot: previous,
                            written,
                        } => own(snapshot.revision == previous.revision
                            || snapshot.body.matches(written)),
                    };
                if draft.arrived(&self.scope, snapshot, origin) == Arrival::Replaced {
                    self.replaced();
                }
            }
            Err(source) => {
                let failure = DraftFailure::Load {
                    scope: self.scope.clone(),
                    source,
                };
                if draft.fail(&self.scope, failure) {
                    self.handed_back();
                }
            }
        }
    }

    pub fn written(&mut self, result: Result<DraftWriteResult, CallError>) {
        let State::Open(draft) = &mut self.state else {
            self.dropped("a draft write's result");
            return;
        };
        let Request::Writing {
            body: written,
            edited,
        } = std::mem::replace(&mut draft.request, Request::Idle)
        else {
            unreachable!(
                "invariant: draft {} finished a write it did not start",
                self.scope
            );
        };
        match result {
            Ok(result) => {
                let current = Snapshot::from(result.current);
                if result.saved || current.body.matches(&written) {
                    draft.remote = Remote::Loaded(current);
                    draft.edits.saved_through(edited);
                    draft.requeue_waiting();
                } else if draft.arrived(&self.scope, current, Origin::Changed) == Arrival::Replaced
                {
                    self.replaced();
                }
            }
            Err(source) => {
                let snapshot = match std::mem::replace(&mut draft.remote, Remote::Unloaded(None)) {
                    Remote::Loaded(snapshot) | Remote::Ambiguous { snapshot, .. } => snapshot,
                    Remote::Unloaded(_) => {
                        unreachable!("invariant: draft {} wrote before it loaded", self.scope)
                    }
                };
                draft.remote = Remote::Ambiguous { snapshot, written };
                let failure = DraftFailure::Save {
                    scope: self.scope.clone(),
                    source,
                };
                if draft.fail(&self.scope, failure) {
                    self.handed_back();
                }
            }
        }
    }

    pub fn posted(&mut self, result: Result<(), PostMessageError>, thread: ThreadId) {
        let State::Open(draft) = &mut self.state else {
            self.dropped("a sent message's result");
            return;
        };
        let request = std::mem::replace(&mut draft.request, Request::Idle);
        assert!(
            matches!(request, Request::Posting),
            "invariant: draft {} finished a post it did not start",
            self.scope
        );
        let mut replaced = false;
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
                if let Some(cleared) = &outgoing.request.message.draft {
                    draft.remote = Remote::Unloaded(Some(Base {
                        revision: cleared.revision,
                        written: None,
                    }));
                }
                draft.requeue_waiting();
                if outgoing.edited == draft.edits.latest && draft.body == outgoing.sent {
                    let mut waiting = outgoing.sent.follow_ups;
                    if waiting.is_empty() {
                        let left = draft
                            .snapshot()
                            .filter(|_| outgoing.request.message.draft.is_none())
                            .map_or_else(DraftBody::default, |snapshot| snapshot.body.shown());
                        draft.edits.settle();
                        replaced = draft.show(left) == Arrival::Replaced;
                    } else {
                        let mut next = waiting.remove(0);
                        next.follow_ups = waiting;
                        draft.body = next;
                        draft.edits.edit();
                        replaced = true;
                    }
                }
                if !draft.edits.unsaved() {
                    draft.halt = match std::mem::replace(&mut draft.halt, Halt::Clear) {
                        Halt::Conflict {
                            failed: Some(failure),
                        } => Halt::Failed(failure),
                        Halt::Clear | Halt::Conflict { failed: None } => Halt::Clear,
                        failed @ Halt::Failed(_) => failed,
                    };
                }
            }
            Err(source) => {
                let failure = DraftFailure::Send {
                    scope: self.scope.clone(),
                    source,
                };
                if draft.fail(&self.scope, failure) {
                    self.handed_back();
                }
            }
        }
        if replaced {
            self.replaced();
        }
    }
}

#[cfg(test)]
#[path = "../../tests/composer/draft_session.rs"]
mod tests;
