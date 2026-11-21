use super::attachment::{PendingUpload, ReadImageError, upload_image};
use super::controller::SendRefusal;
use super::draft_body::DraftBody;
use super::draft_session::{
    AdoptImageError, AdoptedImage, AdoptionKey, DraftSession, KeptDraft, KeptLostImage, LostKey,
    Operation, PostMessageError, SAVE_DELAY, Submission, Unrecovered,
};
use super::spool::{
    BesideSlot, DraftSpool, Holder, KeepSpooledDraftError, KeptBesideError, Serving, Slot,
    SpooledDraft, TakeSpooledDraftError, Turn,
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
    forgetting: bool,
}

struct Inner {
    host: HostId,
    scope: String,
    client: ForgeClient,
    spool: DraftSpool,
    kept: Mutex<Kept>,
    shared: Mutex<Shared>,
    changed: watch::Sender<u64>,
    cancelled: CancellationToken,
    held_by: HeldBy,
}

struct HeldBy {
    holdings: Weak<Mutex<Holdings>>,
    wake: aqua::mailbox::Sender<()>,
}

pub const MAX_OPEN_DRAFTS: usize = 64;

struct Snapshot {
    scope: String,
    draft: KeptDraft,
    images: Vec<(String, AdoptedImage)>,
}

impl Snapshot {
    fn of(state: &DraftSession) -> Self {
        Self {
            scope: state.scope().to_owned(),
            draft: state.kept(),
            images: state.unacknowledged_images(),
        }
    }
}

struct Keeping {
    turn: Turn,
    snapshot: Snapshot,
}

impl Keeping {
    fn of(inner: &Inner, shared: &Shared) -> Option<Self> {
        shared.state.keepable().then(|| Self {
            turn: inner.spool.turn(&inner.host, &inner.scope),
            snapshot: Snapshot::of(&shared.state),
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum Choice {
    Discard(BesideSlot),
    Adopt(BesideSlot),
}

impl Choice {
    const fn which(self) -> &'static str {
        match self {
            Self::Discard(_) => "the draft the user chose not to keep",
            Self::Adopt(_) => "the draft the user replaced with the one kept beside it",
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum KeepLocallyError {
    #[error("read an image that Forge has not acknowledged")]
    Image(#[source] ReadImageError),
    #[error("write the draft to this device")]
    Spool(#[source] KeepSpooledDraftError),
    #[error("the write to this device stopped before it finished")]
    Stopped,
}

enum Hold {
    Keeping(std::sync::mpsc::Receiver<Result<(), KeepLocallyError>>),
    Unkept(KeepLocallyError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    Retired,
    Open,
}

struct Holding {
    session: DraftHandle,
    hold: Hold,
    standing: Standing,
}

struct Holdings {
    held: Vec<Holding>,
    generation: u64,
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
    fn kept(&self) -> MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(|poisoned| {
            panic!(
                "invariant: what draft {} on Forge {} keeps on this device is not left half-updated by a panic: {poisoned}",
                self.scope, self.host
            )
        })
    }

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
    holdings: Arc<Mutex<Holdings>>,
    wake: aqua::mailbox::Mailbox<()>,
}

impl std::fmt::Debug for DraftSessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftSessions")
            .field("sessions", &self.sessions)
            .field("spool", &self.spool)
            .field("held", &lock_holdings(&self.holdings).held.len())
            .finish_non_exhaustive()
    }
}

pub struct HeldDrafts {
    spool: DraftSpool,
    held: Vec<Holding>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutdownLimits {
    pub forge: Duration,
    pub device: Duration,
}

fn pending_shutdown() -> MutexGuard<'static, Vec<DraftHandle>> {
    static PENDING: OnceLock<Mutex<Vec<DraftHandle>>> = OnceLock::new();
    PENDING
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unsaved {
    Retired,
    Counted,
}

pub async fn drain_pending(limits: ShutdownLimits) -> ShutdownOutcome {
    let sessions = std::mem::take(&mut *pending_shutdown());
    let deadline = tokio::time::Instant::now() + limits.forge;
    let mut failed = 0;
    let mut timed_out = 0;
    let mut kept_locally = 0;
    let mut keeping = Vec::new();
    for session in sessions {
        if session.0.cancelled.is_cancelled() {
            keeping.push((Unsaved::Retired, session.settle_on_device()));
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
        keeping.push((Unsaved::Counted, session.settle_on_device()));
    }
    let deadline = tokio::time::Instant::now() + limits.device;
    for (unsaved, kept) in keeping {
        match (tokio::time::timeout_at(deadline, kept).await, unsaved) {
            (Ok(Ok(Ok(true))), _) => kept_locally += 1,
            (Ok(Ok(Ok(false))), _) | (Ok(Ok(Err(_))) | Err(_), Unsaved::Counted) => {}
            (Ok(Ok(Err(_))), Unsaved::Retired) => failed += 1,
            (Err(_), Unsaved::Retired) => timed_out += 1,
            (Ok(Err(stopped)), _) => std::panic::resume_unwind(stopped.into_panic()),
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
        reason = "each session settles on its own on this device, so the order sessions are handed over in is irrelevant"
    )]
    fn drop(&mut self) {
        let held: Vec<Holding> = std::mem::take(&mut lock_holdings(&self.holdings).held)
            .into_iter()
            .filter(|held| held.standing == Standing::Retired)
            .collect();
        let mut pending = pending_shutdown();
        pending.retain(DraftHandle::unsettled_on_device);
        for session in self.sessions.values() {
            session.flush();
            if session.unsettled_on_device() {
                pending.push(session.clone());
            }
        }
        pending.extend(held.into_iter().map(|held| held.session));
    }
}

fn lock_holdings(holdings: &Mutex<Holdings>) -> MutexGuard<'_, Holdings> {
    holdings.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Holdings {
    fn settle(&mut self) {
        let mut generation = self.generation;
        self.held.retain_mut(|holding| {
            let Hold::Keeping(kept) = &holding.hold else {
                return true;
            };
            let failure = match kept.try_recv() {
                Ok(Ok(())) => return false,
                Err(std::sync::mpsc::TryRecvError::Empty) => return true,
                Ok(Err(error)) => error,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => KeepLocallyError::Stopped,
            };
            generation = generation.wrapping_add(1);
            holding.hold = Hold::Unkept(failure);
            true
        });
        self.generation = generation;
    }

    fn take_back(&mut self, host: &HostId, scope: &str) -> Option<DraftHandle> {
        self.settle();
        let index = self.held.iter().position(|held| {
            held.session.0.host == *host
                && held.session.0.scope == scope
                && held.standing == Standing::Retired
                && matches!(held.hold, Hold::Unkept(_))
        })?;
        Some(self.held.remove(index).session)
    }

    fn hold(&mut self, session: &DraftHandle, hold: Hold, standing: Standing) {
        self.held
            .retain(|held| !Arc::ptr_eq(&held.session.0, &session.0));
        self.held.push(Holding {
            session: session.clone(),
            hold,
            standing,
        });
    }
}

impl DraftSessions {
    #[must_use]
    pub fn with_spool(spool: DraftSpool) -> Self {
        Self {
            sessions: HashMap::new(),
            spool,
            holdings: Arc::new(Mutex::new(Holdings {
                held: Vec::new(),
                generation: 0,
            })),
            wake: aqua::mailbox::Mailbox::new(NonZeroUsize::MIN),
        }
    }

    pub fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        &self.wake
    }

    fn holdings(&self) -> MutexGuard<'_, Holdings> {
        lock_holdings(&self.holdings)
    }

    pub fn hand_off_held(&self) -> HeldDrafts {
        let mut holdings = self.holdings();
        HeldDrafts {
            spool: self.spool.clone(),
            held: std::mem::take(&mut holdings.held),
            generation: holdings.generation,
        }
    }

    pub fn take_held(&self, mut handed: HeldDrafts) {
        let same_spool = handed.spool == self.spool;
        let mut holdings = self.holdings();
        let generation = holdings.generation.max(handed.generation);
        holdings.generation = if same_spool && !handed.held.is_empty() && !holdings.held.is_empty()
        {
            generation.wrapping_add(1)
        } else {
            generation
        };
        if same_spool {
            holdings.held.append(&mut handed.held);
        }
    }

    fn retire_session(&self, session: &DraftHandle) {
        let kept = session.retire();
        self.holdings()
            .hold(session, Hold::Keeping(kept), Standing::Retired);
    }

    #[must_use]
    pub fn unkept(&mut self) -> Option<UnkeptDrafts> {
        self.wake.drain();
        let mut holdings = self.holdings();
        holdings.settle();
        let mut unkept = holdings.held.iter().filter_map(|held| match &held.hold {
            Hold::Unkept(error) => Some(error),
            Hold::Keeping(_) => None,
        });
        let first = unkept.next()?;
        let count = 1 + unkept.count();
        let headline = if count == 1 {
            "Artisan couldn't save your unsent draft to this device. Try again keeps it.".to_owned()
        } else {
            format!(
                "Artisan couldn't save {count} unsent drafts to this device. Try again keeps them."
            )
        };
        Some(UnkeptDrafts {
            count,
            generation: holdings.generation,
            problem: editor_core::Problem::caused_by(headline, first),
        })
    }

    pub fn keep_unkept(&mut self) -> Option<UnkeptDrafts> {
        let retrying = {
            let mut holdings = self.holdings();
            holdings.settle();
            let (unkept, keeping) = std::mem::take(&mut holdings.held)
                .into_iter()
                .partition(|held| matches!(held.hold, Hold::Unkept(_)));
            holdings.held = keeping;
            unkept
        };
        for Holding {
            session, standing, ..
        } in retrying
        {
            match standing {
                Standing::Retired => self.retire_session(&session),
                Standing::Open => session.rekeep(),
            }
        }
        self.unkept()
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
        let turn = self.spool.turn(&host, &scope);
        let (changed, _) = watch::channel(0);
        let session = DraftHandle(Arc::new(Inner {
            host,
            scope: scope.clone(),
            client,
            spool: self.spool.clone(),
            kept: Mutex::new(Kept::Nothing),
            shared: Mutex::new(Shared {
                state: DraftSession::recovering_from_device(scope),
                viewers: Vec::new(),
                timer: None,
                version: 0,
                forgetting: false,
            }),
            changed,
            cancelled: CancellationToken::new(),
            held_by: HeldBy {
                holdings: Arc::downgrade(&self.holdings),
                wake: self.wake.sender(),
            },
        }));
        self.sessions.insert(key, session.clone());
        let lease = DraftLease::new(session.clone());
        let holdings = Arc::clone(&self.holdings);
        runtime().spawn_blocking(move || session.recover(turn, &holdings));
        Ok(lease)
    }

    pub fn retire(&mut self) {
        let retired: Vec<DraftHandle> = self.sessions.drain().map(|(_, session)| session).collect();
        for session in &retired {
            self.retire_session(session);
        }
    }

    pub fn retain_hosts(&mut self, core: &Core) {
        let mut retired = Vec::new();
        self.sessions.retain(|(id, instance, _), session| {
            if core.host_if_instance(id, *instance).is_some() {
                return true;
            }
            retired.push(session.clone());
            false
        });
        for session in &retired {
            self.retire_session(session);
        }
    }
}

struct Recovery {
    kept: Kept,
    draft: Option<(KeptDraft, Vec<PendingUpload>, Vec<KeptLostImage>)>,
    rivals: Vec<(BesideSlot, SpooledDraft)>,
    unrecovered: Vec<Unrecovered>,
}

impl Recovery {
    fn read(
        serving: &Serving,
        host: &HostId,
        scope: &str,
        held: Option<(KeptDraft, Vec<PendingUpload>, Kept)>,
    ) -> Self {
        let mut unrecovered = Vec::new();
        let mut beside = kept_beside(serving, host, scope, &mut unrecovered).into_iter();
        let (kept, draft) = match held {
            Some((draft, images, kept)) => {
                tracing::info!(
                    host = %host,
                    %scope,
                    images = images.len(),
                    "took back a draft this device couldn't keep"
                );
                (kept, Some((draft, images, Vec::new())))
            }
            None => match recover_spooled(serving, host, scope, &mut unrecovered) {
                Some(draft) => (Kept::Recovered(Slot::Record), Some(draft)),
                None => beside
                    .next()
                    .map_or((Kept::Nothing, None), |(slot, spooled)| {
                        let draft = unspool(host, scope, spooled);
                        tracing::info!(
                            host = %host,
                            %scope,
                            images = draft.1.len(),
                            damaged = draft.2.len(),
                            "recovered a draft kept beside another on this device"
                        );
                        (Kept::Recovered(Slot::Beside(slot)), Some(draft))
                    }),
            },
        };
        let rivals = beside
            .filter(|(slot, _)| kept.slot() != Some(Slot::Beside(*slot)))
            .collect();
        Self {
            kept,
            draft,
            rivals,
            unrecovered,
        }
    }

    fn apply(self, host: &HostId, scope: &str, state: &mut DraftSession) {
        if let Some((draft, images, damaged)) = self.draft {
            state.recover(draft, images, damaged);
        }
        for (slot, spooled) in self.rivals {
            let (draft, images, damaged) = unspool(host, scope, spooled);
            tracing::info!(
                host = %host,
                %scope,
                "offering a draft kept beside this one on this device"
            );
            state.offer_rival(slot, draft, images, damaged);
        }
        for unrecovered in self.unrecovered {
            state.set_unrecovered(unrecovered);
        }
        state.recovered();
    }
}

fn kept_beside(
    serving: &Serving,
    host: &HostId,
    scope: &str,
    unrecovered: &mut Vec<Unrecovered>,
) -> Vec<(BesideSlot, SpooledDraft)> {
    serving
        .kept_beside()
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
                        unrecovered.push(Unrecovered::SetAside(set_aside));
                    }
                    KeptBesideError::Left { path, .. } => {
                        unrecovered.push(Unrecovered::Left(path));
                    }
                }
                None
            }
        })
        .collect()
}

fn recover_spooled(
    serving: &Serving,
    host: &HostId,
    scope: &str,
    unrecovered: &mut Vec<Unrecovered>,
) -> Option<(KeptDraft, Vec<PendingUpload>, Vec<KeptLostImage>)> {
    match serving.take() {
        Ok(Some(spooled)) => {
            let draft = unspool(host, scope, spooled);
            tracing::info!(
                host = %host,
                %scope,
                images = draft.1.len(),
                damaged = draft.2.len(),
                "recovered a draft kept locally at shutdown"
            );
            Some(draft)
        }
        Ok(None) => None,
        Err(error) => {
            tracing::error!(
                host = %host,
                %scope,
                error = %::diagnostics::chain(&error),
                "a draft kept locally at shutdown is unreadable"
            );
            match error {
                TakeSpooledDraftError::List { .. } => {}
                TakeSpooledDraftError::SetAside { set_aside, .. } => {
                    unrecovered.push(Unrecovered::SetAside(set_aside));
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
                    unrecovered.push(Unrecovered::Left(path));
                }
            }
            None
        }
    }
}

fn read_images(host: &HostId, snapshot: Snapshot) -> (KeptDraft, Vec<Arc<[u8]>>) {
    let Snapshot {
        scope,
        mut draft,
        images,
    } = snapshot;
    let images = images
        .into_iter()
        .filter_map(|(name, image)| match image {
            AdoptedImage::Read(bytes) => Some(bytes),
            AdoptedImage::Unread(upload) => match upload.read() {
                Ok(bytes) => Some(Arc::from(bytes)),
                Err(source) => {
                    let error = KeepLocallyError::Image(source);
                    tracing::error!(
                        host = %host,
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
        .collect();
    (draft, images)
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

    fn commit(
        &self,
        serving: &Serving,
        (draft, images): (KeptDraft, Vec<Arc<[u8]>>),
    ) -> Result<(), KeepLocallyError> {
        let kept = *self.0.kept();
        let scope = &self.0.scope;
        let holder = kept.slot().map_or(Holder::Newcomer, Holder::Owner);
        let kept_in = |slot: Slot| match kept {
            Kept::Nothing => Kept::Written(slot),
            kept @ (Kept::Recovered(_) | Kept::Written(_) | Kept::Unforgotten(_)) => kept,
        };
        match serving.keep(draft, &images, holder) {
            Ok(slot) => {
                *self.0.kept() = kept_in(slot);
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
                Ok(())
            }
            Err(source) => {
                if let Some(slot) = source.published() {
                    *self.0.kept() = kept_in(slot);
                }
                let error = KeepLocallyError::Spool(source);
                tracing::error!(
                    host = %self.0.host,
                    %scope,
                    error = %::diagnostics::chain(&error),
                    "couldn't keep an unsaved draft on this device; holding it in memory"
                );
                Err(error)
            }
        }
    }

    fn rekeep(&self) {
        if *self.0.kept() == Kept::Nothing {
            return;
        }
        let Some(keeping) = Keeping::of(&self.0, &self.0.shared()) else {
            return;
        };
        let Keeping { turn, snapshot } = keeping;
        let kept = self.on_device(move |session| {
            let images = read_images(&session.0.host, snapshot);
            let serving = turn.wait();
            (session.commit(&serving, images), serving)
        });
        let Some(holdings) = self.0.held_by.holdings.upgrade() else {
            tracing::warn!(
                host = %self.0.host,
                scope = %self.0.scope,
                "keeping a draft on this device after its drafts closed; a failure is only logged"
            );
            return;
        };
        lock_holdings(&holdings).hold(self, Hold::Keeping(kept), Standing::Open);
    }

    fn on_device(
        &self,
        keep: impl FnOnce(&Self) -> (Result<(), KeepLocallyError>, Serving) + Send + 'static,
    ) -> std::sync::mpsc::Receiver<Result<(), KeepLocallyError>> {
        let (sender, kept) = std::sync::mpsc::sync_channel(1);
        let session = self.clone();
        runtime().spawn_blocking(move || {
            let (result, serving) = keep(&session);
            if sender.send(result).is_err() {
                tracing::info!(
                    host = %session.0.host,
                    scope = %session.0.scope,
                    "a draft finished on this device after its drafts were closed"
                );
            }
            drop(serving);
            match session.0.held_by.wake.send(()) {
                Ok(())
                | Err(
                    aqua::mailbox::SendError::Full(()) | aqua::mailbox::SendError::Closed(()),
                ) => {}
            }
        });
        kept
    }

    fn forget_recovered(&self, shared: &mut Shared) {
        let kept = *self.0.kept();
        if shared.forgetting
            || !matches!(kept, Kept::Recovered(_) | Kept::Written(_))
            || !shared.state.settled_with_forge()
        {
            return;
        }
        shared.forgetting = true;
        let turn = self.0.spool.turn(&self.0.host, &self.0.scope);
        let session = self.clone();
        runtime().spawn_blocking(move || {
            session.forget_acknowledged(&turn.wait());
            session.mutate(|shared| shared.forgetting = false);
        });
    }

    fn forget_acknowledged(&self, serving: &Serving) {
        let Some(slot) = self.0.kept().slot() else {
            return;
        };
        let scope = &self.0.scope;
        let kept = match serving.forget(slot) {
            Ok(()) => Kept::Nothing,
            Err(error) if error.slot_freed() => {
                tracing::error!(
                    host = %self.0.host,
                    %scope,
                    error = %::diagnostics::chain(&error),
                    "set aside a recovered draft that Forge has saved but couldn't remove it; removing it when this draft next opens"
                );
                Kept::Nothing
            }
            Err(error) => {
                tracing::error!(
                    host = %self.0.host,
                    %scope,
                    error = %::diagnostics::chain(&error),
                    "couldn't remove a recovered draft that Forge has saved; trying again when the Editor closes"
                );
                Kept::Unforgotten(slot)
            }
        };
        *self.0.kept() = kept;
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

    fn recover(&self, turn: Turn, holdings: &Mutex<Holdings>) {
        let inner = &self.0;
        let serving = turn.wait();
        let held = lock_holdings(holdings).take_back(&inner.host, &inner.scope);
        let recovery = Recovery::read(
            &serving,
            &inner.host,
            &inner.scope,
            held.and_then(|held| held.hand_over()),
        );
        if let Err(error) = serving.sweep_tombstones() {
            tracing::error!(
                host = %inner.host,
                scope = %inner.scope,
                error = %::diagnostics::chain(&error),
                "couldn't remove drafts set aside on this device; trying again when this draft next opens"
            );
        }
        *inner.kept() = recovery.kept;
        self.mutate(|shared| recovery.apply(&inner.host, &inner.scope, &mut shared.state));
        drop(serving);
        self.pump();
    }

    fn retiring(&self) -> Turn {
        self.mutate(|shared| {
            if let Some(timer) = shared.timer.take() {
                timer.abort();
            }
        });
        self.0.cancelled.cancel();
        self.0.spool.turn(&self.0.host, &self.0.scope)
    }

    fn retire(&self) -> std::sync::mpsc::Receiver<Result<(), KeepLocallyError>> {
        let turn = self.retiring();
        self.on_device(move |session| {
            let serving = turn.wait();
            (session.retire_within(&serving), serving)
        })
    }

    fn retire_within(&self, serving: &Serving) -> Result<(), KeepLocallyError> {
        loop {
            let version = self.0.shared().version;
            self.settle_within(serving)?;
            let shared = self.0.shared();
            if shared.version == version {
                self.publish(shared, |shared| shared.state.remove());
                return Ok(());
            }
        }
    }

    fn unsettled_on_device(&self) -> bool {
        self.read(DraftSession::keepable) || *self.0.kept() != Kept::Nothing
    }

    fn settle_on_device(&self) -> tokio::task::JoinHandle<Result<bool, KeepLocallyError>> {
        let turn = self.0.spool.turn(&self.0.host, &self.0.scope);
        let session = self.clone();
        runtime().spawn_blocking(move || session.settle_within(&turn.wait()))
    }

    fn settle_within(&self, serving: &Serving) -> Result<bool, KeepLocallyError> {
        let (snapshot, settled) = {
            let shared = self.0.shared();
            (
                shared.state.keepable().then(|| Snapshot::of(&shared.state)),
                shared.state.settled_with_forge(),
            )
        };
        let Some(snapshot) = snapshot else {
            if settled {
                self.forget_acknowledged(serving);
            }
            return Ok(false);
        };
        self.commit(serving, read_images(&self.0.host, snapshot))
            .map(|()| true)
    }

    fn hand_over(&self) -> Option<(KeptDraft, Vec<PendingUpload>, Kept)> {
        let shared = self.0.shared();
        let snapshot = shared
            .state
            .keepable()
            .then(|| Snapshot::of(&shared.state))?;
        let kept = *self.0.kept();
        self.publish(shared, |shared| shared.state.remove());
        let images = snapshot
            .images
            .into_iter()
            .map(|(_, image)| match image {
                AdoptedImage::Unread(upload) => upload,
                AdoptedImage::Read(bytes) => PendingUpload::kept(bytes.to_vec()),
            })
            .collect();
        Some((snapshot.draft, images, kept))
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

    pub fn submit(&self, submission: Submission) -> Result<(), SendRefusal> {
        let mut submitted = Ok(());
        self.mutate(|shared| submitted = shared.state.submit(submission));
        submitted?;
        self.flush();
        Ok(())
    }

    pub fn retry(&self) {
        self.mutate(|shared| shared.state.retry());
        self.flush();
    }

    pub(crate) fn keep_local(&self) {
        let mut rejected = None;
        self.mutate(|shared| rejected = shared.state.keep_local());
        if let Some(slot) = rejected {
            self.carry_out(Choice::Discard(slot));
        }
        self.flush();
    }

    pub(crate) fn use_remote(&self) {
        let mut chosen = None;
        self.mutate(|shared| chosen = shared.state.use_remote());
        if let Some(slot) = chosen {
            self.carry_out(Choice::Adopt(slot));
            self.flush();
        }
    }

    fn carry_out(&self, choice: Choice) {
        let (turn, snapshot) = {
            let shared = self.0.shared();
            (
                self.0.spool.turn(&self.0.host, &self.0.scope),
                shared
                    .state
                    .keeping_set_aside()
                    .then(|| Snapshot::of(&shared.state)),
            )
        };
        let session = self.clone();
        runtime().spawn_blocking(move || {
            let images = snapshot.map(|snapshot| read_images(&session.0.host, snapshot));
            let serving = turn.wait();
            let rejected = session.choose(choice);
            let kept = images.map(|images| session.commit(&serving, images));
            match (kept, rejected) {
                (Some(Err(error)), _) => tracing::error!(
                    host = %session.0.host,
                    scope = %session.0.scope,
                    which = choice.which(),
                    error = %::diagnostics::chain(&error),
                    "couldn't keep the messages sent from a draft the user set aside on this device; sending them anyway and leaving that draft where it is"
                ),
                (Some(Ok(())) | None, Some(slot)) => session.discard(&serving, slot, choice),
                (Some(Ok(())) | None, None) => {}
            }
            drop(serving);
            session.mutate(|shared| shared.state.set_aside_kept());
            session.pump();
        });
    }

    fn choose(&self, choice: Choice) -> Option<Slot> {
        let mut kept = self.0.kept();
        match choice {
            Choice::Discard(slot) => Some(Slot::Beside(slot)),
            Choice::Adopt(slot) => {
                let replaced = kept.slot();
                *kept = Kept::Recovered(Slot::Beside(slot));
                replaced
            }
        }
    }

    fn discard(&self, serving: &Serving, slot: Slot, choice: Choice) {
        let inner = &self.0;
        let which = choice.which();
        match serving.forget(slot) {
            Ok(()) => {}
            Err(error) if error.slot_freed() => tracing::error!(
                host = %inner.host,
                scope = %inner.scope,
                which,
                error = %::diagnostics::chain(&error),
                "set aside a draft the user chose against but couldn't remove it; removing it when this draft next opens"
            ),
            Err(error) => tracing::error!(
                host = %inner.host,
                scope = %inner.scope,
                which,
                error = %::diagnostics::chain(&error),
                "couldn't set aside a draft the user chose against; it is offered again when this draft next opens"
            ),
        }
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
                let mut handed_back = false;
                session.mutate(|shared| handed_back = shared.state.posted_set_aside(result));
                if handed_back {
                    session.rekeep();
                }
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
