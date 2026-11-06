use super::draft_session::{DraftSession, Operation, SAVE_DELAY, Submission};
use crate::core::{Core, ForgeClient, HostId, HostInstance, runtime::runtime};
use artisan_api::{ItemId, ThreadId};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    num::NonZeroUsize,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub enum SessionEvent {
    Changed,
    Reloaded,
    Accepted { thread: ThreadId, item: ItemId },
}

struct Shared {
    state: DraftSession,
    viewers: Vec<Weak<Viewer>>,
    timer: Option<tokio::task::AbortHandle>,
    version: u64,
}

struct Inner {
    client: ForgeClient,
    shared: Mutex<Shared>,
    changed: watch::Sender<u64>,
    cancelled: CancellationToken,
}

#[derive(Clone)]
pub struct DraftHandle(Arc<Inner>);

struct Viewer {
    events: Mutex<VecDeque<SessionEvent>>,
    wake: aqua::mailbox::Sender<()>,
}

pub struct DraftLease {
    handle: DraftHandle,
    viewer: Arc<Viewer>,
    wake: aqua::mailbox::Mailbox<()>,
}

impl DraftLease {
    fn new(handle: DraftHandle) -> Self {
        let wake = aqua::mailbox::Mailbox::new(NonZeroUsize::MIN);
        let viewer = Arc::new(Viewer {
            events: Mutex::new(VecDeque::new()),
            wake: wake.sender(),
        });
        handle
            .0
            .shared
            .lock()
            .unwrap()
            .viewers
            .push(Arc::downgrade(&viewer));
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
        self.viewer.events.lock().unwrap().drain(..).collect()
    }
}

impl Shared {
    fn emit(&mut self, event: SessionEvent) {
        self.viewers.retain(|viewer| {
            let Some(viewer) = viewer.upgrade() else {
                return false;
            };
            viewer.events.lock().unwrap().push_back(event.clone());
            let _ = viewer.wake.send(());
            true
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

#[derive(Default)]
pub struct DraftSessions {
    sessions: HashMap<(HostId, HostInstance, String), DraftHandle>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Saved,
    Incomplete { failed: usize, timed_out: usize },
}

fn pending_shutdown() -> &'static Mutex<Vec<DraftHandle>> {
    static PENDING: OnceLock<Mutex<Vec<DraftHandle>>> = OnceLock::new();
    PENDING.get_or_init(Mutex::default)
}

pub async fn drain_pending(timeout: Duration) -> ShutdownOutcome {
    let sessions = std::mem::take(&mut *pending_shutdown().lock().unwrap());
    let deadline = tokio::time::Instant::now() + timeout;
    let mut failed = 0;
    let mut timed_out = 0;
    for session in sessions {
        let mut changed = session.subscribe();
        loop {
            let (pending, error) =
                session.read(|state| (state.pending(), state.error.is_some() || state.conflict));
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
                    } => {
                        failed += failures;
                        timed_out += timeouts;
                    }
                }
                break;
            }
        }
    }
    match (failed, timed_out) {
        (0, 0) => ShutdownOutcome::Saved,
        _ => ShutdownOutcome::Incomplete { failed, timed_out },
    }
}

impl Drop for DraftSessions {
    fn drop(&mut self) {
        let mut pending = pending_shutdown().lock().unwrap();
        pending.retain(|session| session.read(|state| state.pending()));
        for session in self.sessions.values() {
            session.flush();
            if session.read(|state| state.pending()) {
                pending.push(session.clone());
            }
        }
    }
}

impl DraftSessions {
    pub fn open(
        &mut self,
        host: HostId,
        instance: HostInstance,
        scope: String,
        client: ForgeClient,
    ) -> Option<DraftLease> {
        let key = (host, instance, scope.clone());
        if let Some(session) = self.sessions.get(&key) {
            return Some(DraftLease::new(session.clone()));
        }
        if self.sessions.len() >= 64 {
            let removable = self
                .sessions
                .iter()
                .find(|(_, session)| {
                    Arc::strong_count(&session.0) == 1
                        && session.0.shared.lock().unwrap().state.evictable()
                })
                .map(|(key, _)| key.clone());
            if let Some(key) = removable {
                self.sessions.remove(&key);
            }
        }
        if self.sessions.len() >= 64 {
            return None;
        }
        let (changed, _) = watch::channel(0);
        let session = DraftHandle(Arc::new(Inner {
            client,
            shared: Mutex::new(Shared {
                state: DraftSession::new(scope),
                viewers: Vec::new(),
                timer: None,
                version: 0,
            }),
            changed,
            cancelled: CancellationToken::new(),
        }));
        self.sessions.insert(key, session.clone());
        let lease = DraftLease::new(session.clone());
        session.pump();
        Some(lease)
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
        self.read(
            |state| match (state.pending(), state.error.is_some() || state.conflict) {
                (false, _) => ShutdownOutcome::Saved,
                (true, true) => ShutdownOutcome::Incomplete {
                    failed: 1,
                    timed_out: 0,
                },
                (true, false) => {
                    self.0.cancelled.cancel();
                    ShutdownOutcome::Incomplete {
                        failed: 0,
                        timed_out: 1,
                    }
                }
            },
        )
    }

    fn remove(&self) {
        self.mutate(|shared| {
            if let Some(timer) = shared.timer.take() {
                timer.abort();
            }
            shared.state.remove();
            self.0.cancelled.cancel();
            shared.emit(SessionEvent::Reloaded);
        });
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.changed.subscribe()
    }

    pub fn read<T>(&self, read: impl FnOnce(&DraftSession) -> T) -> T {
        read(&self.0.shared.lock().unwrap().state)
    }

    fn mutate(&self, update: impl FnOnce(&mut Shared)) {
        let mut shared = self.0.shared.lock().unwrap();
        update(&mut shared);
        shared.emit(SessionEvent::Changed);
        shared.version = shared.version.wrapping_add(1);
        let version = shared.version;
        drop(shared);
        self.0.changed.send_replace(version);
    }

    pub fn queue(&self, body: Value) {
        let mut shared = self.0.shared.lock().unwrap();
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
        shared.emit(SessionEvent::Changed);
        shared.version = shared.version.wrapping_add(1);
        let version = shared.version;
        drop(shared);
        self.0.changed.send_replace(version);
    }

    pub fn flush(&self) {
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

    pub fn keep_local(&self) {
        self.mutate(|shared| shared.state.keep_local());
        self.flush();
    }

    pub fn use_remote(&self) {
        self.mutate(|shared| {
            shared.state.use_remote();
            shared.emit(SessionEvent::Reloaded);
        });
    }

    fn pump(&self) {
        let operation = self.0.shared.lock().unwrap().state.next();
        let Some(operation) = operation else {
            return;
        };
        let session = self.clone();
        runtime().spawn(async move {
            let cancelled = session.0.cancelled.clone();
            tokio::select! {
                biased;
                () = cancelled.cancelled() => {}
                () = session.perform(operation) => {}
            }
        });
    }

    async fn perform(self, operation: Operation) {
        let session = self;
        let client = &session.0.client;
        match operation {
            Operation::Load => {
                let scope = session.read(|state| state.scope.clone());
                let result = client
                    .draft(&scope)
                    .await
                    .map_err(|error| format!("{error:#}"));
                session.mutate(|shared| shared.state.loaded(result));
            }
            Operation::Write { request, edited } => {
                let scope = session.read(|state| state.scope.clone());
                let result = client
                    .set_draft(&scope, &request)
                    .await
                    .map_err(|error| format!("{error:#}"));
                session.mutate(|shared| shared.state.written(&request, edited, result));
            }
            Operation::Post(request) => {
                let result = async {
                    if let Some(create) = &request.create {
                        client.create_thread(create).await?;
                    }
                    client
                        .send_message(&request.thread, &request.message)
                        .await?;
                    Ok::<(), anyhow::Error>(())
                }
                .await
                .map_err(|error| format!("{error:#}"));
                let accepted = result.is_ok();
                session.mutate(|shared| {
                    shared.state.posted(result);
                    if accepted && !shared.state.removed() {
                        shared.emit(SessionEvent::Accepted {
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

#[cfg(test)]
#[path = "../../tests/composer/persistence/deadline.rs"]
mod deadline_tests;
