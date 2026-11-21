use super::attachment::{PendingUpload, ReadImageError, upload_image};
use super::draft_body::DraftBody;
use super::draft_session::{
    AdoptImageError, AdoptedImage, AdoptionKey, DraftSession, KeptDraft, KeptLostImage, LostKey,
    Operation, PostMessageError, SAVE_DELAY, Submission, Unrecovered,
};
use super::spool::{
    BesideSlot, DraftSpool, Holder, KeepSpooledDraftError, KeptBesideError, Slot, SpooledDraft,
    TakeSpooledDraftError,
};
use artisan_api::{BlobHash, ItemId, ThreadId, WriteDraft};
use editor_core::{Core, ForgeClient, HostId, HostInstance, runtime::runtime};
use std::{
    collections::{HashMap, VecDeque},
    num::NonZeroUsize,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak},
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

#[derive(Clone, Debug)]
pub enum SessionEvent {
    Changed,
    Accepted { thread: ThreadId, item: ItemId },
    ImageAdopted(BlobHash),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kept {
    Nothing,
    Recovered(Slot),
    Written(Slot),
    Unforgotten(Slot),
}

impl Kept {
    const fn slot(self) -> Option<Slot> {
        match self {
            Self::Nothing => None,
            Self::Recovered(slot) | Self::Written(slot) | Self::Unforgotten(slot) => Some(slot),
        }
    }
}

struct Shared {
    state: DraftSession,
    viewers: Vec<Weak<Viewer>>,
    timer: Option<tokio::task::AbortHandle>,
    version: u64,
    kept: Kept,
}

struct Inner {
    host: HostId,
    scope: String,
    client: ForgeClient,
    spool: DraftSpool,
    rejections: Rejections,
    shared: Mutex<Shared>,
    changed: watch::Sender<u64>,
    cancelled: CancellationToken,
}

pub const MAX_OPEN_DRAFTS: usize = 64;

struct Keeping {
    scope: String,
    draft: KeptDraft,
    images: Vec<(String, AdoptedImage)>,
    holder: Holder,
}

impl Keeping {
    fn of(shared: &Shared) -> Option<Self> {
        let state = &shared.state;
        state.keepable().then(|| Self {
            scope: state.scope().to_owned(),
            draft: state.kept(),
            images: state.unacknowledged_images(),
            holder: shared.kept.slot().map_or(Holder::Newcomer, Holder::Owner),
        })
    }
}

#[derive(Debug, thiserror::Error)]
enum KeepLocallyError {
    #[error("read an image that Forge has not acknowledged")]
    Image(#[source] ReadImageError),
    #[error("write the draft to this device")]
    Spool(#[source] KeepSpooledDraftError),
}

struct Unkept {
    session: DraftHandle,
    error: KeepLocallyError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnkeptDrafts {
    pub count: usize,
    pub generation: u64,
    pub problem: editor_core::Problem,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "There are {MAX_OPEN_DRAFTS} open or unsaved drafts. Finish saving a draft, then reopen this thread."
)]
pub struct DraftsFull {
    pub host: HostId,
    pub scope: String,
}

impl Inner {
    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|poisoned| {
            panic!(
                "invariant: draft {} on Forge {} is not left half-updated by a panic: {poisoned}",
                self.scope, self.host
            )
        })
    }
}

impl Viewer {
    fn events(&self) -> MutexGuard<'_, VecDeque<SessionEvent>> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Clone)]
pub struct DraftHandle(Arc<Inner>);

impl std::fmt::Debug for DraftHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftHandle")
            .field("host", &self.0.host)
            .field("scope", &self.0.scope)
            .finish_non_exhaustive()
    }
}

struct Viewer {
    events: Mutex<VecDeque<SessionEvent>>,
    wake: aqua::mailbox::Sender<()>,
}

pub struct DraftLease {
    handle: DraftHandle,
    viewer: Arc<Viewer>,
    wake: aqua::mailbox::Mailbox<()>,
}

impl std::fmt::Debug for DraftLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftLease")
            .field("handle", &self.handle)
            .finish_non_exhaustive()
    }
}

impl DraftLease {
    fn new(handle: DraftHandle) -> Self {
        let wake = aqua::mailbox::Mailbox::new(NonZeroUsize::MIN);
        let viewer = Arc::new(Viewer {
            events: Mutex::new(VecDeque::new()),
            wake: wake.sender(),
        });
        handle.0.shared().viewers.push(Arc::downgrade(&viewer));
        Self {
            handle,
            viewer,
            wake,
        }
    }

    pub fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        &self.wake
    }

    pub fn drain_events(&mut self) -> Vec<SessionEvent> {
        self.wake.drain();
        self.viewer.events().drain(..).collect()
    }
}

impl Shared {
    fn emit(&mut self, event: &SessionEvent) {
        self.viewers.retain(|viewer| {
            let Some(viewer) = viewer.upgrade() else {
                return false;
            };
            viewer.events().push_back(event.clone());
            match viewer.wake.send(()) {
                Ok(())
                | Err(
                    aqua::mailbox::SendError::Full(()) | aqua::mailbox::SendError::Closed(()),
                ) => true,
            }
        });
    }
}

impl std::ops::Deref for DraftLease {
    type Target = DraftHandle;
    fn deref(&self) -> &DraftHandle {
        &self.handle
    }
}

impl Drop for DraftLease {
    fn drop(&mut self) {
        self.handle.mutate(|shared| shared.state.view_closed());
        self.handle.flush();
    }
}

pub struct DraftSessions {
    sessions: HashMap<(HostId, HostInstance, String), DraftHandle>,
    spool: DraftSpool,
    rejections: Rejections,
    held: Vec<Unkept>,
    generation: u64,
}

impl std::fmt::Debug for DraftSessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftSessions")
            .field("sessions", &self.sessions)
            .field("spool", &self.spool)
            .field("held", &self.held.len())
            .finish_non_exhaustive()
    }
}

pub struct HeldDrafts {
    spool: DraftSpool,
    rejections: Rejections,
    held: Vec<Unkept>,
    generation: u64,
}

impl std::fmt::Debug for HeldDrafts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldDrafts")
            .field("spool", &self.spool)
            .field("held", &self.held.len())
            .finish_non_exhaustive()
    }
}

impl Drop for HeldDrafts {
    fn drop(&mut self) {
        pending_shutdown().extend(self.held.drain(..).map(|held| held.session));
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Saved,
    Incomplete {
        failed: usize,
        timed_out: usize,
        kept_locally: usize,
    },
}

struct Rejected {
    host: HostId,
    scope: String,
    slot: Slot,
}

impl Rejected {
    fn holds(&self, host: &HostId, scope: &str) -> bool {
        self.host == *host && self.scope == scope
    }
}

#[derive(Clone, Default)]
struct Rejections(Arc<Mutex<Vec<Rejected>>>);

impl Rejections {
    fn lock(&self) -> MutexGuard<'_, Vec<Rejected>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn adopt(&mut self, handed: Self) {
        if Arc::ptr_eq(&self.0, &handed.0) {
            return;
        }
        let mut adopted = handed.lock();
        adopted.append(&mut self.lock());
        drop(adopted);
        *self = handed;
    }
}

fn pending_shutdown() -> MutexGuard<'static, Vec<DraftHandle>> {
    static PENDING: OnceLock<Mutex<Vec<DraftHandle>>> = OnceLock::new();
    PENDING
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

pub async fn drain_pending(timeout: Duration) -> ShutdownOutcome {
    let sessions = std::mem::take(&mut *pending_shutdown());
    let deadline = tokio::time::Instant::now() + timeout;
    let mut failed = 0;
    let mut timed_out = 0;
    let mut kept_locally = 0;
    for session in sessions {
        if session.0.cancelled.is_cancelled() {
            match session.keep_locally() {
                Ok(true) => kept_locally += 1,
                Ok(false) => {}
                Err(_) => failed += 1,
            }
            continue;
        }
        let mut changed = session.subscribe();
        loop {
            let (pending, stuck) = session.read(|state| (state.pending(), state.stuck()));
            if !pending {
                break;
            }
            if stuck {
                failed += 1;
                break;
            }
            if tokio::time::timeout_at(deadline, changed.changed())
                .await
                .is_err()
            {
                match session.deadline_outcome() {
                    ShutdownOutcome::Saved => {}
                    ShutdownOutcome::Incomplete {
                        failed: failures,
                        timed_out: timeouts,
                        ..
                    } => {
                        failed += failures;
                        timed_out += timeouts;
                    }
                }
                break;
            }
        }
        if matches!(session.keep_locally(), Ok(true)) {
            kept_locally += 1;
        }
    }
    match (failed, timed_out) {
        (0, 0) => ShutdownOutcome::Saved,
        _ => ShutdownOutcome::Incomplete {
            failed,
            timed_out,
            kept_locally,
        },
    }
}

impl Drop for DraftSessions {
    #[expect(
        clippy::iter_over_hash_type,
        reason = "each session flushes to its own file, so the order sessions are flushed in is irrelevant"
    )]
    fn drop(&mut self) {
        let mut pending = pending_shutdown();
        pending.retain(|session| session.read(DraftSession::keepable));
        for session in self.sessions.values() {
            session.forget_unforgotten();
            session.flush();
            if session.read(DraftSession::keepable) {
                pending.push(session.clone());
            }
        }
        pending.extend(self.held.drain(..).map(|held| held.session));
    }
}

impl DraftSessions {
    #[must_use]
    pub fn with_spool(spool: DraftSpool) -> Self {
        Self {
            sessions: HashMap::new(),
            spool,
            rejections: Rejections::default(),
            held: Vec::new(),
            generation: 0,
        }
    }

    pub fn hand_off_held(&mut self) -> HeldDrafts {
        HeldDrafts {
            spool: self.spool.clone(),
            rejections: self.rejections.clone(),
            held: std::mem::take(&mut self.held),
            generation: self.generation,
        }
    }

    pub fn take_held(&mut self, mut handed: HeldDrafts) {
        let generation = self.generation.max(handed.generation);
        self.generation =
            if handed.spool == self.spool && !handed.held.is_empty() && !self.held.is_empty() {
                generation.wrapping_add(1)
            } else {
                generation
            };
        if handed.spool == self.spool {
            self.held.append(&mut handed.held);
            self.rejections.adopt(handed.rejections.clone());
        }
    }

    fn hold(&mut self, session: DraftHandle, error: KeepLocallyError) {
        self.generation = self.generation.wrapping_add(1);
        self.held.push(Unkept { session, error });
    }

    #[must_use]
    pub fn unkept(&self) -> Option<UnkeptDrafts> {
        let first = self.held.first()?;
        let count = self.held.len();
        let headline = if count == 1 {
            "Artisan couldn't save your unsent draft to this device. Try again keeps it.".to_owned()
        } else {
            format!(
                "Artisan couldn't save {count} unsent drafts to this device. Try again keeps them."
            )
        };
        Some(UnkeptDrafts {
            count,
            generation: self.generation,
            problem: editor_core::Problem::caused_by(headline, &first.error),
        })
    }

    pub fn keep_unkept(&mut self) -> Option<UnkeptDrafts> {
        for Unkept { session, .. } in std::mem::take(&mut self.held) {
            if let Err(error) = session.retire() {
                self.hold(session, error);
            }
        }
        self.unkept()
    }

    fn take_back(&mut self, host: &HostId, scope: &str) -> Option<DraftHandle> {
        let index = self
            .held
            .iter()
            .position(|held| held.session.0.host == *host && held.session.0.scope == scope)?;
        Some(self.held.remove(index).session)
    }

    pub fn open(
        &mut self,
        host: HostId,
        instance: HostInstance,
        scope: String,
        client: ForgeClient,
    ) -> Result<DraftLease, DraftsFull> {
        let key = (host.clone(), instance, scope.clone());
        if let Some(session) = self.sessions.get(&key) {
            return Ok(DraftLease::new(session.clone()));
        }
        if self.sessions.len() >= MAX_OPEN_DRAFTS {
            let removable = self
                .sessions
                .iter()
                .find(|(_, session)| {
                    Arc::strong_count(&session.0) == 1 && session.0.shared().state.evictable()
                })
                .map(|(key, _)| key.clone());
            if let Some(key) = removable {
                self.sessions.remove(&key);
            }
        }
        if self.sessions.len() >= MAX_OPEN_DRAFTS {
            return Err(DraftsFull { host, scope });
        }
        let mut state = DraftSession::new(scope.clone());
        let rejected_slots = self.rejected_slots(&host, &scope);
        let skip = |slot: Slot| rejected_slots.contains(&slot);
        let mut beside = self
            .kept_beside(&host, &scope, skip, &mut state)
            .into_iter();
        let recovered = match self
            .take_back(&host, &scope)
            .and_then(|held| held.hand_over())
        {
            Some((draft, images, kept)) => {
                tracing::info!(
                    host = %host,
                    %scope,
                    images = images.len(),
                    "took back a draft this device couldn't keep"
                );
                state.recover(draft, images, Vec::new());
                kept
            }
            None => match self.recover_spooled(&host, &scope, skip(Slot::Record), &mut state) {
                Kept::Nothing => beside.next().map_or(Kept::Nothing, |(slot, spooled)| {
                    let (draft, images, damaged) = unspool(&host, &scope, spooled);
                    tracing::info!(
                        host = %host,
                        %scope,
                        images = images.len(),
                        damaged = damaged.len(),
                        "recovered a draft kept beside another on this device"
                    );
                    state.recover(draft, images, damaged);
                    Kept::Recovered(Slot::Beside(slot))
                }),
                kept @ (Kept::Recovered(_) | Kept::Written(_) | Kept::Unforgotten(_)) => kept,
            },
        };
        for (slot, spooled) in
            beside.filter(|(slot, _)| recovered.slot() != Some(Slot::Beside(*slot)))
        {
            let (draft, images, damaged) = unspool(&host, &scope, spooled);
            tracing::info!(
                host = %host,
                %scope,
                "offering a draft kept beside this one on this device"
            );
            state.offer_rival(slot, draft, images, damaged);
        }
        let (changed, _) = watch::channel(0);
        let session = DraftHandle(Arc::new(Inner {
            host,
            scope,
            client,
            spool: self.spool.clone(),
            rejections: self.rejections.clone(),
            shared: Mutex::new(Shared {
                state,
                viewers: Vec::new(),
                timer: None,
                version: 0,
                kept: recovered,
            }),
            changed,
            cancelled: CancellationToken::new(),
        }));
        self.sessions.insert(key, session.clone());
        let lease = DraftLease::new(session.clone());
        session.sweep_tombstones();
        session.pump();
        Ok(lease)
    }

    fn kept_beside(
        &self,
        host: &HostId,
        scope: &str,
        skip: impl Fn(Slot) -> bool,
        state: &mut DraftSession,
    ) -> Vec<(BesideSlot, SpooledDraft)> {
        self.spool
            .kept_beside(host, scope, skip)
            .into_iter()
            .filter_map(|beside| match beside {
                Ok(beside) => Some((beside.slot, beside.spooled)),
                Err(error) => {
                    tracing::error!(
                        host = %host,
                        %scope,
                        error = %::diagnostics::chain(&error),
                        "a draft kept beside another on this device can't be offered"
                    );
                    match error {
                        KeptBesideError::List { .. } => {}
                        KeptBesideError::SetAside { set_aside, .. } => {
                            state.set_unrecovered(Unrecovered::SetAside(set_aside));
                        }
                        KeptBesideError::Left { path, .. } => {
                            state.set_unrecovered(Unrecovered::Left(path));
                        }
                    }
                    None
                }
            })
            .collect()
    }

    fn rejected_slots(&self, host: &HostId, scope: &str) -> Vec<Slot> {
        self.rejections
            .lock()
            .iter()
            .filter(|rejected| rejected.holds(host, scope))
            .map(|rejected| rejected.slot)
            .collect()
    }

    fn recover_spooled(
        &self,
        host: &HostId,
        scope: &str,
        rejected: bool,
        state: &mut DraftSession,
    ) -> Kept {
        if rejected {
            return Kept::Nothing;
        }
        match self.spool.take(host, scope) {
            Ok(Some(spooled)) => {
                let (draft, images, damaged) = unspool(host, scope, spooled);
                tracing::info!(
                    host = %host,
                    %scope,
                    images = images.len(),
                    damaged = damaged.len(),
                    "recovered a draft kept locally at shutdown"
                );
                state.recover(draft, images, damaged);
                Kept::Recovered(Slot::Record)
            }
            Ok(None) => Kept::Nothing,
            Err(error) => {
                tracing::error!(
                    host = %host,
                    %scope,
                    error = %::diagnostics::chain(&error),
                    "a draft kept locally at shutdown is unreadable"
                );
                match error {
                    TakeSpooledDraftError::SetAside { set_aside, .. } => {
                        state.set_unrecovered(Unrecovered::SetAside(set_aside));
                    }
                    TakeSpooledDraftError::Left {
                        path, unreadable, ..
                    } => {
                        tracing::error!(
                            host = %host,
                            %scope,
                            error = %::diagnostics::chain(&unreadable),
                            "this draft will be kept beside the unreadable one so it isn't overwritten"
                        );
                        state.set_unrecovered(Unrecovered::Left(path));
                    }
                }
                Kept::Nothing
            }
        }
    }

    pub fn retire(&mut self) {
        let retired: Vec<DraftHandle> = self.sessions.drain().map(|(_, session)| session).collect();
        for session in retired {
            session.forget_unforgotten();
            if let Err(error) = session.retire() {
                self.hold(session, error);
            }
        }
    }

    pub fn retain_hosts(&mut self, core: &Core) {
        let mut unkept = Vec::new();
        self.sessions.retain(|(id, instance, _), session| {
            if core.host_if_instance(id, *instance).is_some() {
                return true;
            }
            if let Err(error) = session.retire() {
                unkept.push((session.clone(), error));
            }
            false
        });
        for (session, error) in unkept {
            self.hold(session, error);
        }
    }
}

fn unspool(
    host: &HostId,
    scope: &str,
    spooled: SpooledDraft,
) -> (KeptDraft, Vec<PendingUpload>, Vec<KeptLostImage>) {
    let mut images = Vec::new();
    let mut damaged = Vec::new();
    for image in spooled.images {
        match image {
            Ok(bytes) => images.push(PendingUpload::kept(bytes)),
            Err(error) => {
                tracing::error!(
                    host = %host,
                    %scope,
                    error = %::diagnostics::chain(&error),
                    "an image kept with a draft on this device is unreadable; keeping the rest of the draft"
                );
                damaged.push(KeptLostImage {
                    name: PendingUpload::KEPT_NAME.to_owned(),
                    reason: ::diagnostics::chain(&error),
                });
            }
        }
    }
    (spooled.draft, images, damaged)
}

impl DraftHandle {
    fn deadline_outcome(&self) -> ShutdownOutcome {
        self.read(|state| match (state.pending(), state.stuck()) {
            (false, _) => ShutdownOutcome::Saved,
            (true, true) => ShutdownOutcome::Incomplete {
                failed: 1,
                timed_out: 0,
                kept_locally: 0,
            },
            (true, false) => {
                self.0.cancelled.cancel();
                ShutdownOutcome::Incomplete {
                    failed: 0,
                    timed_out: 1,
                    kept_locally: 0,
                }
            }
        })
    }

    fn keep_locally(&self) -> Result<bool, KeepLocallyError> {
        let keeping = Keeping::of(&self.0.shared());
        match keeping {
            Some(keeping) => self.keep(keeping).map(|()| true),
            None => Ok(false),
        }
    }

    fn keep(&self, keeping: Keeping) -> Result<(), KeepLocallyError> {
        let Keeping {
            scope,
            mut draft,
            images,
            holder,
        } = keeping;
        let images = images
            .into_iter()
            .filter_map(|(name, image)| match image {
                AdoptedImage::Read(bytes) => Some(bytes),
                AdoptedImage::Unread(upload) => match upload.read() {
                    Ok(bytes) => Some(Arc::from(bytes)),
                    Err(source) => {
                        let error = KeepLocallyError::Image(source);
                        tracing::error!(
                            host = %self.0.host,
                            %scope,
                            image = %name,
                            error = %::diagnostics::chain(&error),
                            "couldn't keep a pasted image on this device; keeping the rest of the draft"
                        );
                        draft.lost.push(KeptLostImage {
                            name,
                            reason: ::diagnostics::chain(&error),
                        });
                        None
                    }
                },
            })
            .collect::<Vec<_>>();
        let kept = self
            .0
            .spool
            .keep(&self.0.host, &scope, draft, &images, holder)
            .map_err(KeepLocallyError::Spool);
        match &kept {
            Ok(slot) => {
                let mut shared = self.0.shared();
                if shared.kept == Kept::Nothing {
                    shared.kept = Kept::Written(*slot);
                }
                drop(shared);
                match slot {
                    Slot::Record => tracing::warn!(
                        host = %self.0.host,
                        %scope,
                        "kept an unsaved draft on this device until Forge can take it"
                    ),
                    Slot::Beside(_) => tracing::warn!(
                        host = %self.0.host,
                        %scope,
                        "kept an unsaved draft on this device beside another draft kept for the same thread"
                    ),
                }
            }
            Err(error) => {
                tracing::error!(
                    host = %self.0.host,
                    %scope,
                    error = %::diagnostics::chain(error),
                    "couldn't keep an unsaved draft on this device; holding it in memory"
                );
            }
        }
        kept.map(drop)
    }

    fn forget_recovered(&self, shared: &mut Shared) {
        if matches!(shared.kept, Kept::Recovered(_) | Kept::Written(_)) {
            self.forget_acknowledged(shared);
        }
    }

    fn forget_unforgotten(&self) {
        let mut shared = self.0.shared();
        if shared.kept != Kept::Nothing {
            self.forget_acknowledged(&mut shared);
        }
    }

    fn forget_acknowledged(&self, shared: &mut Shared) {
        let Some(slot) = shared.kept.slot() else {
            return;
        };
        if !shared.state.settled_with_forge() {
            return;
        }
        let scope = shared.state.scope();
        match self.0.spool.forget(&self.0.host, scope, slot) {
            Ok(()) => shared.kept = Kept::Nothing,
            Err(error) if error.slot_freed() => {
                tracing::error!(
                    host = %self.0.host,
                    %scope,
                    error = %::diagnostics::chain(&error),
                    "set aside a recovered draft that Forge has saved but couldn't remove it; removing it when this draft next opens"
                );
                shared.kept = Kept::Nothing;
            }
            Err(error) => {
                tracing::error!(
                    host = %self.0.host,
                    %scope,
                    error = %::diagnostics::chain(&error),
                    "couldn't remove a recovered draft that Forge has saved; trying again when the Editor closes"
                );
                shared.kept = Kept::Unforgotten(slot);
            }
        }
    }

    pub fn adopt_image(&self, upload: PendingUpload) {
        self.mutate(|shared| shared.state.adopt(upload));
        self.start_uploads();
    }

    pub(crate) fn dismiss_lost(&self, key: LostKey) {
        self.mutate(|shared| shared.state.dismiss_lost(key));
    }

    async fn read_and_upload(
        &self,
        key: AdoptionKey,
        image: AdoptedImage,
    ) -> Result<BlobHash, AdoptImageError> {
        let bytes = match image {
            AdoptedImage::Read(bytes) => bytes,
            AdoptedImage::Unread(upload) => {
                let bytes: Arc<[u8]> = tokio::task::spawn_blocking(move || upload.read())
                    .await
                    .map_err(AdoptImageError::ReadTask)?
                    .map_err(AdoptImageError::Read)?
                    .into();
                self.mutate(|shared| shared.state.image_read(key, Arc::clone(&bytes)));
                bytes
            }
        };
        upload_image(bytes, self.0.client.clone())
            .await
            .map_err(AdoptImageError::from)
    }

    fn start_uploads(&self) {
        let uploads = self.0.shared().state.next_uploads();
        for (key, image) in uploads {
            let session = self.clone();
            let span = tracing::info_span!("adopt_image", host = %session.0.host);
            runtime().spawn(
                async move {
                    let cancelled = session.0.cancelled.clone();
                    let result = tokio::select! {
                        biased;
                        () = cancelled.cancelled() => Err(AdoptImageError::Closed),
                        result = session.read_and_upload(key, image) => result,
                    };
                    session.mutate(|shared| {
                        if let Some(hash) = shared.state.adopted(key, result) {
                            shared.emit(&SessionEvent::ImageAdopted(hash));
                        }
                    });
                    session.flush();
                }
                .instrument(span),
            );
        }
    }

    fn retire(&self) -> Result<(), KeepLocallyError> {
        self.mutate(|shared| {
            if let Some(timer) = shared.timer.take() {
                timer.abort();
            }
        });
        self.0.cancelled.cancel();
        loop {
            let (version, keeping) = {
                let shared = self.0.shared();
                (shared.version, Keeping::of(&shared))
            };
            if let Some(keeping) = keeping {
                self.keep(keeping)?;
            }
            let shared = self.0.shared();
            if shared.version == version {
                self.publish(shared, |shared| shared.state.remove());
                return Ok(());
            }
        }
    }

    fn hand_over(&self) -> Option<(KeptDraft, Vec<PendingUpload>, Kept)> {
        let shared = self.0.shared();
        let keeping = Keeping::of(&shared)?;
        let kept = shared.kept;
        self.publish(shared, |shared| shared.state.remove());
        let images = keeping
            .images
            .into_iter()
            .map(|(_, image)| match image {
                AdoptedImage::Unread(upload) => upload,
                AdoptedImage::Read(bytes) => PendingUpload::kept(bytes.to_vec()),
            })
            .collect();
        Some((keeping.draft, images, kept))
    }

    pub(crate) fn take_unrecovered(&self) -> Vec<Unrecovered> {
        self.0.shared().state.take_unrecovered()
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.changed.subscribe()
    }

    pub fn read<T>(&self, read: impl FnOnce(&DraftSession) -> T) -> T {
        read(&self.0.shared().state)
    }

    fn mutate(&self, update: impl FnOnce(&mut Shared)) {
        self.publish(self.0.shared(), update);
    }

    fn publish(&self, mut shared: MutexGuard<'_, Shared>, update: impl FnOnce(&mut Shared)) {
        update(&mut shared);
        self.forget_recovered(&mut shared);
        shared.emit(&SessionEvent::Changed);
        shared.version = shared.version.wrapping_add(1);
        let version = shared.version;
        drop(shared);
        self.0.changed.send_replace(version);
    }

    pub fn queue(&self, body: DraftBody, seen: u64, uploading: bool) {
        let mut shared = self.0.shared();
        if !shared.state.queue(body, seen, uploading) {
            return;
        }
        if let Some(timer) = shared.timer.take() {
            timer.abort();
        }
        let session = self.clone();
        let task = runtime().spawn(async move {
            tokio::time::sleep(SAVE_DELAY).await;
            session.flush();
        });
        shared.timer = Some(task.abort_handle());
        shared.emit(&SessionEvent::Changed);
        shared.version = shared.version.wrapping_add(1);
        let version = shared.version;
        drop(shared);
        self.0.changed.send_replace(version);
    }

    pub(crate) fn flush(&self) {
        self.mutate(|shared| {
            if let Some(timer) = shared.timer.take() {
                timer.abort();
            }
        });
        self.pump();
    }

    pub fn submit(&self, submission: Submission) {
        self.mutate(|shared| shared.state.submit(submission));
        self.flush();
    }

    pub fn retry(&self) {
        self.mutate(|shared| shared.state.retry());
        self.flush();
    }

    pub(crate) fn keep_local(&self) {
        let mut rejected = None;
        self.mutate(|shared| rejected = shared.state.keep_local());
        if let Some(slot) = rejected {
            self.set_aside(
                Some(Slot::Beside(slot)),
                "the draft the user chose not to keep",
            );
        }
        self.flush();
    }

    pub(crate) fn use_remote(&self) {
        let mut replaced = None;
        self.mutate(|shared| {
            if let Some(slot) = shared.state.use_remote() {
                replaced = Some(shared.kept.slot());
                shared.kept = Kept::Recovered(Slot::Beside(slot));
            }
        });
        if let Some(rejected) = replaced {
            self.set_aside(
                rejected,
                "the draft the user replaced with the one kept beside it",
            );
            self.flush();
        }
    }

    fn set_aside(&self, slot: Option<Slot>, which: &'static str) {
        if !self.read(DraftSession::keeping_set_aside) {
            if let Some(slot) = slot {
                self.forget_slot(slot, which);
            }
            return;
        }
        if let Some(slot) = slot {
            self.reject(slot);
        }
        let session = self.clone();
        runtime().spawn_blocking(move || {
            match (session.keep_locally(), slot) {
                (Ok(_), Some(slot)) => session.discard(slot, which),
                (Ok(_), None) => {}
                (Err(error), slot) => {
                    tracing::error!(
                        host = %session.0.host,
                        scope = %session.0.scope,
                        which,
                        error = %::diagnostics::chain(&error),
                        "couldn't keep the messages sent from a draft the user set aside on this device; sending them anyway and leaving that draft where it is"
                    );
                    if let Some(slot) = slot {
                        session.unreject(slot);
                    }
                }
            }
            session.mutate(|shared| shared.state.set_aside_kept());
            session.pump();
        });
    }

    fn forget_slot(&self, slot: Slot, which: &'static str) {
        self.reject(slot);
        let session = self.clone();
        runtime().spawn_blocking(move || session.discard(slot, which));
    }

    fn reject(&self, slot: Slot) {
        let inner = &self.0;
        inner.rejections.lock().push(Rejected {
            host: inner.host.clone(),
            scope: inner.scope.clone(),
            slot,
        });
    }

    fn unreject(&self, slot: Slot) {
        let inner = &self.0;
        let mut rejected = inner.rejections.lock();
        if let Some(index) = rejected
            .iter()
            .position(|rejected| rejected.slot == slot && rejected.holds(&inner.host, &inner.scope))
        {
            rejected.remove(index);
        }
    }

    fn discard(&self, slot: Slot, which: &'static str) {
        let inner = &self.0;
        let path = inner.spool.slot_path(&inner.host, &inner.scope, slot);
        match inner.spool.forget(&inner.host, &inner.scope, slot) {
            Ok(()) => {}
            Err(error) if error.slot_freed() => tracing::error!(
                host = %inner.host,
                scope = %inner.scope,
                which,
                path = %path.display(),
                error = %::diagnostics::chain(&error),
                "set aside a draft the user chose against but couldn't remove it; removing it when this draft next opens"
            ),
            Err(error) => tracing::error!(
                host = %inner.host,
                scope = %inner.scope,
                which,
                path = %path.display(),
                error = %::diagnostics::chain(&error),
                "couldn't set aside a draft the user chose against; it is offered again when this draft next opens"
            ),
        }
        self.unreject(slot);
        self.mutate(|_| {});
    }

    fn sweep_tombstones(&self) {
        let session = self.clone();
        runtime().spawn_blocking(move || {
            let inner = &session.0;
            if let Err(error) = inner.spool.sweep_tombstones(&inner.host, &inner.scope) {
                tracing::error!(
                    host = %inner.host,
                    scope = %inner.scope,
                    error = %::diagnostics::chain(&error),
                    "couldn't remove drafts set aside on this device; trying again when this draft next opens"
                );
            }
            session.mutate(|_| {});
        });
    }

    fn pump(&self) {
        self.start_uploads();
        let operation = self.0.shared().state.next_operation();
        let Some(operation) = operation else {
            return;
        };
        let session = self.clone();
        let span = tracing::info_span!("draft", host = %session.0.host);
        runtime().spawn(
            async move {
                let cancelled = session.0.cancelled.clone();
                tokio::select! {
                    biased;
                    () = cancelled.cancelled() => {}
                    () = session.perform(operation) => {}
                }
            }
            .instrument(span),
        );
    }

    async fn perform(self, operation: Operation) {
        let session = self;
        let client = &session.0.client;
        match operation {
            Operation::Load => {
                let result = client.draft(&session.0.scope).await;
                session.mutate(|shared| shared.state.loaded(result));
            }
            Operation::Write {
                expected_revision,
                body,
            } => {
                let request = WriteDraft {
                    expected_revision,
                    body: body.to_wire(),
                };
                let result = client.set_draft(&session.0.scope, &request).await;
                session.mutate(|shared| shared.state.written(result));
            }
            Operation::PostSetAside(request) => {
                let span = tracing::info_span!(
                    "send_set_aside_message",
                    thread = %request.thread,
                    message = %request.message.id
                );
                let result = post(client, &request).instrument(span).await;
                session.mutate(|shared| shared.state.posted_set_aside(result));
            }
            Operation::Post(request) => {
                let span = tracing::info_span!(
                    "send_message",
                    thread = %request.thread,
                    message = %request.message.id
                );
                let result = post(client, &request).instrument(span).await;
                let accepted = result.is_ok();
                session.mutate(|shared| {
                    shared.state.posted(result, request.thread.clone());
                    if accepted && !shared.state.removed() {
                        shared.emit(&SessionEvent::Accepted {
                            thread: request.thread,
                            item: request.message.id,
                        });
                    }
                });
            }
        }
        session.pump();
    }
}

async fn post(client: &ForgeClient, request: &Submission) -> Result<(), PostMessageError> {
    if let Some(create) = &request.create {
        client
            .create_thread(create)
            .await
            .map_err(|source| PostMessageError::CreateThread {
                thread: request.thread.clone(),
                source,
            })?;
    }
    client
        .send_message(&request.thread, &request.message)
        .await
        .map(drop)
        .map_err(|source| PostMessageError::Send {
            thread: request.thread.clone(),
            message: request.message.id.clone(),
            source,
        })
}

#[cfg(test)]
#[path = "../../tests/composer/persistence/deadline.rs"]
mod deadline_tests;

#[cfg(test)]
#[path = "../../tests/composer/persistence/recovery.rs"]
mod recovery_tests;
