use crate::core::HostId;
use aqua::prelude::Notification;
use artisan_api::{Run, RunStatus, Thread, ThreadId};
use std::collections::{HashMap, HashSet};

pub trait NotificationHost {
    fn id(&self) -> &HostId;
    fn is_live(&self) -> bool;
    fn threads(&self) -> &HashMap<ThreadId, Thread>;
    fn run(&self, thread: &ThreadId) -> Option<&Run>;
    fn is_working(&self, thread: &ThreadId) -> bool;
    fn is_waiting(&self, thread: &ThreadId) -> bool;
    fn setting(&self, key: &str) -> Option<&serde_json::Value>;
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Mark {
    working: bool,
    waiting: bool,
}

#[derive(Default)]
pub struct Notifications {
    marks: HashMap<HostId, Option<HashMap<ThreadId, Mark>>>,
    posted: HashSet<(HostId, ThreadId)>,
}

impl Notifications {
    pub fn reset(&mut self, host: &impl NotificationHost) {
        let baseline = host.is_live().then(|| {
            host.threads()
                .keys()
                .filter_map(|id| {
                    let mark = mark(host, id);
                    (mark.working || mark.waiting).then(|| (id.clone(), mark))
                })
                .collect()
        });
        self.marks.insert(host.id().clone(), baseline);
    }

    pub fn sync_connection(&mut self, host: &impl NotificationHost) {
        if !host.is_live() || !matches!(self.marks.get(host.id()), Some(Some(_))) {
            self.reset(host);
        }
    }

    pub fn retain_hosts(&mut self, live: &[HostId]) -> Vec<String> {
        self.marks.retain(|id, _| live.contains(id));
        let mut gone = Vec::new();
        self.posted.retain(|(host, thread)| {
            let keep = live.contains(host);
            if !keep {
                gone.push(tag(host, thread));
            }
            keep
        });
        gone
    }

    pub fn withdraw(&mut self, host: &impl NotificationHost) -> Vec<String> {
        let mut settled = Vec::new();
        self.posted.retain(|(id, thread)| {
            let wanted = id != host.id()
                || host.is_waiting(thread)
                || host
                    .threads()
                    .get(thread)
                    .is_none_or(|info| !info.archived && info.is_unread());
            if !wanted {
                settled.push(tag(id, thread));
            }
            wanted
        });
        settled
    }

    pub fn observe(
        &mut self,
        host: &impl NotificationHost,
        thread: &ThreadId,
        watching: Option<&ThreadId>,
    ) -> Option<Notification> {
        if !host.is_live() {
            self.marks.insert(host.id().clone(), None);
            return None;
        }
        let now = mark(host, thread);
        let marks = self
            .marks
            .entry(host.id().clone())
            .or_insert_with(|| Some(HashMap::new()))
            .as_mut()?;
        let previous = if now.working || now.waiting {
            marks.insert(thread.clone(), now)
        } else {
            marks.remove(thread)
        }
        .unwrap_or_default();
        if watching == Some(thread) {
            return None;
        }
        let info = host.threads().get(thread)?;
        if info.parent_thread_id.is_some() {
            return None;
        }
        let engine = info.config.engine.display_name();
        let body = if now.waiting && !previous.waiting {
            format!("{engine} asked you a question.")
        } else if previous.working && !now.working {
            match host.run(thread).map(|run| run.status) {
                Some(RunStatus::Done) => "Finished.".to_owned(),
                Some(RunStatus::Failed) if info.parked_until.is_some() => return None,
                Some(RunStatus::Failed) => format!("{engine} ran into an error."),
                Some(RunStatus::Interrupted)
                    if host
                        .setting("continue_after_restart")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true) =>
                {
                    return None;
                }
                Some(RunStatus::Interrupted) => "Stopped because Forge shut down.".to_owned(),
                Some(RunStatus::Refused) => format!("{engine} declined to continue."),
                _ => return None,
            }
        } else {
            return None;
        };
        self.posted.insert((host.id().clone(), thread.clone()));
        Some(Notification {
            tag: tag(host.id(), thread),
            title: info.title.clone().unwrap_or_else(|| "A thread".into()),
            body,
            actions: Vec::new(),
        })
    }
}

fn mark(host: &impl NotificationHost, thread: &ThreadId) -> Mark {
    Mark {
        working: host.is_working(thread),
        waiting: host.is_waiting(thread),
    }
}

pub fn tag(host: &HostId, thread: &ThreadId) -> String {
    format!("{host}|{}", thread.0)
}

pub fn parse_tag(tag: &str) -> Option<(HostId, ThreadId)> {
    let (host, thread) = tag.split_once('|')?;
    Some((HostId(host.to_owned()), ThreadId(thread.to_owned())))
}

pub struct NotificationChanges {
    pub withdraw: Vec<String>,
    pub show: Option<Notification>,
}

impl Notifications {
    pub fn thread_changed(
        &mut self,
        host: &impl NotificationHost,
        thread: &ThreadId,
        watching: Option<&ThreadId>,
        enabled: bool,
    ) -> NotificationChanges {
        let withdraw = self.withdraw(host);
        let notification = self.observe(host, thread, watching);
        NotificationChanges {
            withdraw,
            show: notification.filter(|_| enabled),
        }
    }

    pub fn host_reset(&mut self, host: &impl NotificationHost) -> Vec<String> {
        self.reset(host);
        self.withdraw(host)
    }
}

impl NotificationHost for crate::core::HostData {
    fn id(&self) -> &HostId {
        &self.id
    }
    fn is_live(&self) -> bool {
        self.is_live()
    }
    fn threads(&self) -> &HashMap<ThreadId, Thread> {
        &self.threads
    }
    fn run(&self, thread: &ThreadId) -> Option<&Run> {
        self.runs.get(thread)
    }
    fn is_working(&self, thread: &ThreadId) -> bool {
        self.is_working(thread)
    }
    fn is_waiting(&self, thread: &ThreadId) -> bool {
        self.is_waiting(thread)
    }
    fn setting(&self, key: &str) -> Option<&serde_json::Value> {
        self.setting(key)
    }
}
