//! One revisioned writer and submission per Forge draft slot, independent of navigation.
use artisan_api::{
    CreateThread, DraftRevision, DraftSnapshot, ItemId, SendMessage, ThreadId, WriteDraft,
};
use editor_core::{Core, CoreEvent, ForgeClient, HostId};
use gpui::{
    App, AppContext as _, Context, Entity, EntityId, EventEmitter, Global, Subscription, Task,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    rc::{Rc, Weak},
    time::Duration,
};

#[derive(Clone)]
pub(crate) struct Submission {
    pub message: SendMessage,
    pub create: Option<CreateThread>,
    pub thread: ThreadId,
}
#[derive(Clone)]
pub(crate) enum SessionEvent {
    Changed,
    Reloaded,
    Accepted { thread: ThreadId, item: ItemId },
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

pub(crate) struct DraftSession {
    client: ForgeClient,
    removed: bool,
    scope: String,
    viewer: Weak<()>,
    snapshot: DraftSnapshot,
    pub body: Value,
    pub loaded: bool,
    pub error: Option<String>,
    pub conflict: bool,
    edited: u64,
    dirty: bool,
    busy: bool,
    outgoing: Option<Outgoing>,
    timer: Option<Task<()>>,
    task: Task<()>,
}
impl EventEmitter<SessionEvent> for DraftSession {}

type Key = (EntityId, HostId, String);
#[derive(Default)]
struct Sessions(
    HashMap<Key, Entity<DraftSession>>,
    HashMap<EntityId, (Subscription, Subscription)>,
);
impl Global for Sessions {}

/// The bounded cache retains active operations across view destruction. Only idle
/// slots are evicted; unsaved values and request identities are never discarded.
pub(crate) fn session(
    core: &Entity<Core>,
    host: HostId,
    scope: String,
    client: ForgeClient,
    cx: &mut App,
) -> Option<(Entity<DraftSession>, Rc<()>)> {
    if cx.try_global::<Sessions>().is_none() {
        cx.set_global(Sessions::default());
    }
    let core_id = core.entity_id();
    if !cx.global::<Sessions>().1.contains_key(&core_id) {
        let changed = cx.subscribe(core, move |core, event: &CoreEvent, cx| {
            if !matches!(event, CoreEvent::HostsChanged) {
                return;
            }
            let removed: Vec<_> = cx
                .global::<Sessions>()
                .0
                .keys()
                .filter(|(owner, host, _)| *owner == core_id && !core.read(cx).is_paired(host))
                .cloned()
                .collect();
            for key in removed {
                if let Some(session) = cx.global_mut::<Sessions>().0.remove(&key) {
                    session.update(cx, |session, cx| {
                        session.removed = true;
                        session.timer = None;
                        session.task = Task::ready(());
                        session.busy = false;
                        session.dirty = false;
                        session.outgoing = None;
                        session.body = Value::Null;
                        session.snapshot.body = Value::Null;
                        session.error = Some("This Forge pairing was removed.".into());
                        cx.emit(SessionEvent::Reloaded);
                        cx.emit(SessionEvent::Changed);
                        cx.notify();
                    });
                }
            }
        });
        let released = cx.observe_release(core, move |_, cx| {
            let sessions = cx.global_mut::<Sessions>();
            sessions.0.retain(|(owner, _, _), _| *owner != core_id);
            sessions.1.remove(&core_id);
        });
        cx.global_mut::<Sessions>()
            .1
            .insert(core_id, (changed, released));
    }
    let key = (core_id, host, scope.clone());
    if let Some(session) = cx.global::<Sessions>().0.get(&key) {
        let session = session.clone();
        let viewer = session
            .read(cx)
            .viewer
            .upgrade()
            .unwrap_or_else(|| Rc::new(()));
        session.update(cx, |session, _| session.viewer = Rc::downgrade(&viewer));
        return Some((session, viewer));
    }
    let removable = if cx.global::<Sessions>().0.len() >= 64 {
        cx.global::<Sessions>()
            .0
            .iter()
            .find(|(_, session)| {
                let state = session.read(cx);
                !state.busy
                    && !state.dirty
                    && state.outgoing.is_none()
                    && state.viewer.upgrade().is_none()
            })
            .map(|(key, _)| key.clone())
    } else {
        None
    };
    if let Some(key) = removable {
        cx.global_mut::<Sessions>().0.remove(&key);
    }
    if cx.global::<Sessions>().0.len() >= 64 {
        return None;
    }
    let viewer = Rc::new(());
    let session = cx.new(|_| DraftSession {
        viewer: Rc::downgrade(&viewer),
        client,
        removed: false,
        scope,
        snapshot: DraftSnapshot {
            revision: 0,
            body: Value::Null,
        },
        body: Value::Null,
        loaded: false,
        error: None,
        conflict: false,
        edited: 0,
        dirty: false,
        busy: false,
        outgoing: None,
        timer: None,
        task: Task::ready(()),
    });
    cx.global_mut::<Sessions>().0.insert(key, session.clone());
    session.update(cx, |session, cx| session.pump(cx));
    Some((session, viewer))
}

impl DraftSession {
    pub fn sending(&self) -> bool {
        self.outgoing
            .as_ref()
            .is_some_and(|outgoing| outgoing.state != Sending::Failed)
    }
    #[cfg(test)]
    pub fn has_outgoing(&self) -> bool {
        self.outgoing.is_some()
    }
    pub fn queue(&mut self, body: Value, cx: &mut Context<Self>) {
        if self.removed || self.body == body {
            return;
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
        self.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.timer = None;
                this.pump(cx);
            });
        }));
    }
    pub fn flush(&mut self, cx: &mut Context<Self>) {
        self.timer = None;
        self.pump(cx);
    }
    pub fn submit(&mut self, submission: Submission, cx: &mut Context<Self>) {
        if self.removed || self.sending() {
            return;
        }
        // An unchanged failed draft retries the exact request identity and body.
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.state = Sending::Queued;
        } else {
            self.outgoing = Some(Outgoing {
                request: submission,
                state: Sending::Queued,
            });
        }
        if !self.conflict {
            self.error = None;
        }
        self.flush(cx);
        cx.emit(SessionEvent::Changed);
    }
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if self.removed {
            return;
        }
        self.error = None;
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.state = Sending::Queued;
        }
        self.flush(cx);
    }
    pub fn keep_local(&mut self, cx: &mut Context<Self>) {
        if self.removed {
            return;
        }
        self.conflict = false;
        self.error = None;
        self.flush(cx);
    }
    pub fn use_remote(&mut self, cx: &mut Context<Self>) {
        if self.removed {
            return;
        }
        self.body = self.snapshot.body.clone();
        self.dirty = false;
        self.conflict = false;
        self.error = None;
        self.outgoing = None;
        cx.emit(SessionEvent::Reloaded);
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(SessionEvent::Changed);
        cx.notify();
    }
    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.removed || self.busy || self.conflict || self.error.is_some() {
            return;
        }
        if !self.loaded {
            self.load(cx);
            return;
        }
        if self.dirty {
            self.write(cx);
            return;
        }
        if self
            .outgoing
            .as_ref()
            .is_some_and(|outgoing| outgoing.state == Sending::Queued)
        {
            self.post(cx);
        }
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        let client = self.client.clone();
        let scope = self.scope.clone();
        self.task = cx.spawn(async move |this, cx| {
            let result = client.draft(&scope).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(snapshot) => {
                        if !this.dirty {
                            this.body = snapshot.body.clone();
                        }
                        this.snapshot = snapshot;
                        this.loaded = true;
                    }
                    Err(error) => this.error = Some(format!("Couldn't load the draft: {error:#}")),
                }
                this.changed(cx);
                this.pump(cx);
            });
        });
    }
    fn write(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        let client = self.client.clone();
        let scope = self.scope.clone();
        let edited = self.edited;
        let write = WriteDraft {
            expected_revision: self.snapshot.revision,
            body: self.body.clone(),
        };
        self.task = cx.spawn(async move |this, cx| {
            let result = client.set_draft(&scope, &write).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(result) => {
                        let matched = result.saved || result.current.body == write.body;
                        this.snapshot = result.current;
                        if matched { if this.edited == edited { this.dirty = false; } }
                        else { this.conflict = true; this.error = Some("This draft changed on another client. Choose which version to keep.".into()); }
                    }
                    Err(error) => this.error = Some(format!("Couldn't save the draft: {error:#}")),
                }
                this.changed(cx); this.pump(cx);
            });
        });
    }
    fn post(&mut self, cx: &mut Context<Self>) {
        let Some(outgoing) = &mut self.outgoing else {
            return;
        };
        outgoing.state = Sending::Running;
        outgoing.request.message.draft = Some(DraftRevision {
            scope: self.scope.clone(),
            revision: self.snapshot.revision,
        });
        let request = outgoing.request.clone();
        let client = self.client.clone();
        self.busy = true;
        self.task = cx.spawn(async move |this, cx| {
            let result = async {
                if let Some(create) = &request.create {
                    client.create_thread(create).await?;
                }
                client.send_message(&request.thread, &request.message).await
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        // Forge cleared this exact revision in the acceptance transaction.
                        this.snapshot.revision = this.snapshot.revision.saturating_add(1);
                        this.snapshot.body = Value::Null;
                        this.body = Value::Null;
                        this.dirty = false;
                        this.outgoing = None;
                        this.error = None;
                        this.loaded = false;
                        cx.emit(SessionEvent::Accepted {
                            thread: request.thread,
                            item: request.message.id,
                        });
                    }
                    Err(error) => {
                        if let Some(outgoing) = &mut this.outgoing {
                            outgoing.state = Sending::Failed;
                        }
                        this.error = Some(format!("Couldn't send the message: {error:#}"));
                    }
                }
                this.changed(cx);
                this.pump(cx);
            });
        });
    }
}

#[cfg(test)]
mod removal_tests {
    use super::*;
    use editor_core::{EditorFiles, HostConfig, HostKind};
    use gpui::TestAppContext;

    #[gpui::test]
    fn removing_a_pairing_releases_drafts_and_rejects_late_view_actions(cx: &mut TestAppContext) {
        // This fixture starts the owned Forge connection on its real network runtime.
        cx.executor().allow_parking();
        let directory = tempfile::tempdir().unwrap();
        let core = cx.new(|cx| {
            Core::new(
                EditorFiles {
                    dir: directory.path().to_owned(),
                },
                cx,
            )
            .unwrap()
        });
        let fingerprint = core.read_with(cx, |core, _| {
            artisan_api::tls::fingerprint(
                core.identity
                    .as_ref()
                    .unwrap()
                    .certificate_der()
                    .unwrap()
                    .as_ref(),
            )
        });
        let host_id = HostId("removed-host".into());
        core.update(cx, |core, cx| {
            core.add_host(
                HostConfig {
                    id: host_id.clone(),
                    name: "Test".into(),
                    address: "127.0.0.1:1".into(),
                    fingerprint,
                    kind: HostKind::Remote,
                },
                cx,
            )
            .unwrap()
        });
        let host = core.read_with(cx, |core, _| core.host(&host_id).unwrap().clone());
        let client = host.read_with(cx, |host, _| host.client.clone());
        let (session, _view) =
            cx.update(|cx| session(&core, host_id.clone(), "new".into(), client, cx).unwrap());
        let submission = Submission {
            thread: ThreadId::from("thread"),
            create: None,
            message: SendMessage {
                id: ItemId::from("input"),
                text: "Draft".into(),
                images: Vec::new(),
                config: None,
                draft: None,
            },
        };
        session.update(cx, |session, cx| {
            session.queue(serde_json::json!({"text":"Draft"}), cx);
            session.outgoing = Some(Outgoing {
                request: submission.clone(),
                state: Sending::Failed,
            });
        });
        core.update(cx, |core, cx| core.remove_host(&host_id, cx).unwrap());
        cx.run_until_parked();
        session.read_with(cx, |session, _| {
            assert!(session.removed);
            assert_eq!(session.body, Value::Null);
            assert_eq!(session.snapshot.body, Value::Null);
            assert!(session.timer.is_none() && !session.busy && !session.dirty);
            assert!(session.outgoing.is_none());
            assert_eq!(
                session.error.as_deref(),
                Some("This Forge pairing was removed.")
            );
        });
        cx.update(|cx| {
            assert!(
                !cx.global::<Sessions>()
                    .0
                    .keys()
                    .any(|(owner, host, _)| *owner == core.entity_id() && host == &host_id)
            )
        });
        session.update(cx, |session, cx| {
            session.queue(serde_json::json!({"text":"Late view edit"}), cx);
            session.submit(submission, cx);
            session.retry(cx);
            session.keep_local(cx);
            session.use_remote(cx);
            assert_eq!(session.body, Value::Null);
            assert!(session.timer.is_none() && session.outgoing.is_none() && !session.dirty);
            assert_eq!(
                session.error.as_deref(),
                Some("This Forge pairing was removed.")
            );
        });
    }
}
