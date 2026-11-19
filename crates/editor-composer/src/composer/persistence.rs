use super::attachment::PendingUpload;
use super::draft_session::{
    AdoptImageError, DraftSession, Operation, PostMessageError, SAVE_DELAY, Submission,
};
use super::spool::DraftSpool;
use artisan_api::{BlobHash, ItemId, ThreadId};
use editor_core::{Core, ForgeClient, HostId, HostInstance, runtime::runtime};
use serde_json::Value;
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
    Reloaded,
    Accepted { thread: ThreadId, item: ItemId },
    ImageAdopted(BlobHash),
}

struct Shared {
    state: DraftSession,
    viewers: Vec<Weak<Viewer>>,
    timer: Option<tokio::task::AbortHandle>,
    version: u64,
    recovered: bool,
}

struct Inner {
    host: HostId,
    scope: String,
    client: ForgeClient,
    spool: Option<DraftSpool>,
    shared: Mutex<Shared>,
    changed: watch::Sender<u64>,
    cancelled: CancellationToken,
}

pub const MAX_OPEN_DRAFTS: usize = 64;

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
        self.handle.flush();
    }
}

#[derive(Debug, Default)]
pub struct DraftSessions {
    sessions: HashMap<(HostId, HostInstance, String), DraftHandle>,
    spool: Option<DraftSpool>,
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
        let mut changed = session.subscribe();
        loop {
            let (pending, error) = session.read(|state| (state.pending(), state.halted()));
            if !pending {
                break;
            }
            if error {
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
        if session.keep_locally() {
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
        pending.retain(|session| session.read(DraftSession::pending));
        for session in self.sessions.values() {
            session.flush();
            if session.read(DraftSession::pending) {
                pending.push(session.clone());
            }
        }
    }
}

impl DraftSessions {
    #[must_use]
    pub fn with_spool(spool: DraftSpool) -> Self {
        Self {
            sessions: HashMap::new(),
            spool: Some(spool),
        }
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
        let recovered = match self.spool.as_ref().map(|spool| spool.take(&host, &scope)) {
            Some(Ok(Some(body))) => {
                tracing::info!(host = %host, %scope, "recovered a draft kept locally at shutdown");
                state.recover(body);
                true
            }
            Some(Ok(None)) | None => false,
            Some(Err(error)) => {
                tracing::error!(
                    host = %host,
                    %scope,
                    error = %::diagnostics::chain(&error),
                    "a draft kept locally at shutdown is unreadable; leaving it on disk"
                );
                false
            }
        };
        let (changed, _) = watch::channel(0);
        let session = DraftHandle(Arc::new(Inner {
            host,
            scope,
            client,
            spool: self.spool.clone(),
            shared: Mutex::new(Shared {
                state,
                viewers: Vec::new(),
                timer: None,
                version: 0,
                recovered,
            }),
            changed,
            cancelled: CancellationToken::new(),
        }));
        self.sessions.insert(key, session.clone());
        let lease = DraftLease::new(session.clone());
        session.pump();
        Ok(lease)
    }

    pub fn retain_hosts(&mut self, core: &Core) {
        self.sessions.retain(|(id, instance, _), session| {
            if core.host_if_instance(id, *instance).is_some() {
                return true;
            }
            session.remove();
            false
        });
    }
}

impl DraftHandle {
    fn deadline_outcome(&self) -> ShutdownOutcome {
        self.read(|state| match (state.pending(), state.halted()) {
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

    fn keep_locally(&self) -> bool {
        let (pending, scope, body) = self.read(|state| {
            (
                state.pending(),
                state.scope().to_owned(),
                state.body().clone(),
            )
        });
        let Some(spool) = self.0.spool.as_ref().filter(|_| pending) else {
            return false;
        };
        match spool.keep(&self.0.host, &scope, &body) {
            Ok(()) => {
                tracing::warn!(host = %self.0.host, %scope, "kept an unsaved draft on this device until Forge can take it");
                true
            }
            Err(error) => {
                tracing::error!(
                    host = %self.0.host,
                    %scope,
                    error = %::diagnostics::chain(&error),
                    "couldn't keep an unsaved draft on this device"
                );
                false
            }
        }
    }

    fn forget_recovered(&self, shared: &mut Shared) {
        if !shared.recovered || shared.state.pending() {
            return;
        }
        shared.recovered = false;
        let scope = shared.state.scope();
        let Some(spool) = &self.0.spool else {
            return;
        };
        if let Err(error) = spool.forget(&self.0.host, scope) {
            tracing::error!(
                host = %self.0.host,
                %scope,
                error = %::diagnostics::chain(&error),
                "couldn't remove a recovered draft that Forge has saved"
            );
        }
    }

    pub fn adopt_image(&self, upload: PendingUpload) {
        self.mutate(|shared| shared.state.adopting());
        let session = self.clone();
        let span = tracing::info_span!("adopt_image", host = %session.0.host);
        runtime().spawn(
            async move {
                let cancelled = session.0.cancelled.clone();
                let result = tokio::select! {
                    biased;
                    () = cancelled.cancelled() => Err(AdoptImageError::Closed),
                    result = upload.upload() => result.map_err(AdoptImageError::from),
                };
                session.mutate(|shared| {
                    if let Some(hash) = shared.state.adopted(result) {
                        shared.emit(&SessionEvent::ImageAdopted(hash));
                    }
                });
                session.flush();
            }
            .instrument(span),
        );
    }

    fn remove(&self) {
        self.mutate(|shared| {
            if let Some(timer) = shared.timer.take() {
                timer.abort();
            }
            shared.state.remove();
            self.0.cancelled.cancel();
            shared.emit(&SessionEvent::Reloaded);
        });
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.changed.subscribe()
    }

    pub fn read<T>(&self, read: impl FnOnce(&DraftSession) -> T) -> T {
        read(&self.0.shared().state)
    }

    fn mutate(&self, update: impl FnOnce(&mut Shared)) {
        let mut shared = self.0.shared();
        update(&mut shared);
        shared.emit(&SessionEvent::Changed);
        shared.version = shared.version.wrapping_add(1);
        let version = shared.version;
        drop(shared);
        self.0.changed.send_replace(version);
    }

    pub fn queue(&self, body: Value) {
        let mut shared = self.0.shared();
        if !shared.state.queue(body) {
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
        self.mutate(|shared| shared.state.keep_local());
        self.flush();
    }

    pub(crate) fn use_remote(&self) {
        self.mutate(|shared| {
            shared.state.use_remote();
            shared.emit(&SessionEvent::Reloaded);
            self.forget_recovered(shared);
        });
    }

    fn pump(&self) {
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
            Operation::Write { request, edited } => {
                let result = client.set_draft(&session.0.scope, &request).await;
                session.mutate(|shared| {
                    shared.state.written(&request, edited, result);
                    session.forget_recovered(shared);
                });
            }
            Operation::Post(request) => {
                let span = tracing::info_span!(
                    "send_message",
                    thread = %request.thread,
                    message = %request.message.id
                );
                let result = async {
                    if let Some(create) = &request.create {
                        client.create_thread(create).await.map_err(|source| {
                            PostMessageError::CreateThread {
                                thread: request.thread.clone(),
                                source,
                            }
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
                .instrument(span)
                .await;
                let accepted = result.is_ok();
                session.mutate(|shared| {
                    shared.state.posted(result, request.thread.clone());
                    if accepted && !shared.state.removed() {
                        shared.emit(&SessionEvent::Accepted {
                            thread: request.thread,
                            item: request.message.id,
                        });
                    }
                    session.forget_recovered(shared);
                });
            }
        }
        session.pump();
    }
}

#[cfg(test)]
#[path = "../../tests/composer/persistence/deadline.rs"]
mod deadline_tests;
