use super::draft_session::{DraftSession, Operation, SAVE_DELAY, Submission};
use crate::core::{ForgeClient, HostId, HostInstance, runtime::runtime};
use artisan_api::{ItemId, ThreadId};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    num::NonZeroUsize,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::watch;

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
        }));
        self.sessions.insert(key, session.clone());
        let lease = DraftLease::new(session.clone());
        session.pump();
        Some(lease)
    }

    pub fn remove_host(&mut self, host: &HostId) {
        self.sessions.retain(|(id, _, _), session| {
            if id != host {
                return true;
            }
            session.mutate(|shared| {
                if let Some(timer) = shared.timer.take() {
                    timer.abort();
                }
                shared.state.remove();
                shared.emit(SessionEvent::Reloaded);
            });
            false
        });
    }
}

impl DraftHandle {
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
        });
    }
}
