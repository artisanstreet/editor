//! System notifications when a thread finishes, fails or asks for an
//! answer, unless the user is already looking at it. Each Forge metadata
//! event updates one transient thread mark. A posted notification is
//! withdrawn once its thread is read or answered: Windows keeps it in
//! Notification Center, badging the taskbar button, until then.

use artisan_api::{RunStatus, ThreadId};
use editor_core::{HostData, HostId};
use gpui::{SharedString, SystemNotification};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mark {
    working: bool,
    waiting: bool,
}

#[derive(Default)]
pub struct Notifications {
    /// `None` until a host's first state arrived, so a fresh connection
    /// doesn't announce everything that happened while away.
    marks: HashMap<HostId, Option<HashMap<ThreadId, Mark>>>,
    /// Threads whose notification may still sit in Notification Center.
    posted: HashSet<(HostId, ThreadId)>,
}

impl Notifications {
    /// Establish a connection baseline once; reconnects do not replay old notifications.
    pub fn reset(&mut self, host: &HostData) {
        let baseline = host.is_live().then(|| {
            host.threads
                .keys()
                .filter_map(|id| {
                    let mark = mark(host, id);
                    (mark.working || mark.waiting).then(|| (id.clone(), mark))
                })
                .collect()
        });
        self.marks.insert(host.id.clone(), baseline);
    }

    pub fn sync_connection(&mut self, host: &HostData) {
        if !host.is_live() || !matches!(self.marks.get(&host.id), Some(Some(_))) {
            self.reset(host);
        }
    }

    /// Returns the tags of a removed host's notifications to withdraw.
    pub fn retain_hosts(&mut self, live: &[HostId]) -> Vec<SharedString> {
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

    /// The tags of this host's notifications whose threads no longer want the
    /// user: read and not asking, or archived. A thread evicted from memory
    /// keeps its notification.
    pub fn withdraw(&mut self, host: &HostData) -> Vec<SharedString> {
        let mut settled = Vec::new();
        self.posted.retain(|(id, thread)| {
            let wanted = id != &host.id
                || host.is_waiting(thread)
                || host
                    .threads
                    .get(thread)
                    .is_none_or(|info| !info.archived && info.is_unread());
            if !wanted {
                settled.push(tag(id, thread));
            }
            wanted
        });
        settled
    }

    /// Inspect only the thread named by Forge's metadata event.
    pub fn observe(
        &mut self,
        host: &HostData,
        thread: &ThreadId,
        watching: Option<&ThreadId>,
    ) -> Option<SystemNotification> {
        if !host.is_live() {
            self.marks.insert(host.id.clone(), None);
            return None;
        }
        let now = mark(host, thread);
        let marks = self
            .marks
            .entry(host.id.clone())
            .or_insert_with(|| Some(HashMap::new()))
            .as_mut()?;
        let previous = remember(marks, thread, now);
        if watching == Some(thread) {
            return None;
        }
        let info = host.threads.get(thread)?;
        if info.parent_thread_id.is_some() {
            return None;
        }
        let engine = info.config.engine.display_name();
        let body = if now.waiting && !previous.waiting {
            format!("{engine} asked you a question.")
        } else if previous.working && !now.working {
            match host.runs.get(thread).map(|run| run.status) {
                Some(RunStatus::Done) => "Finished.".to_owned(),
                // Parked: Forge picks it up again by itself.
                Some(RunStatus::Failed) if info.parked_until.is_some() => return None,
                Some(RunStatus::Failed) => format!("{engine} ran into an error."),
                // Forge carries the turn on when it starts again.
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
        self.posted.insert((host.id.clone(), thread.clone()));
        Some(SystemNotification {
            tag: tag(&host.id, thread),
            title: info
                .title
                .clone()
                .unwrap_or_else(|| "A thread".into())
                .into(),
            body: body.into(),
            actions: Vec::new(),
        })
    }
}

/// Completed/idle threads need no retained history to detect the next transition.
fn remember(marks: &mut HashMap<ThreadId, Mark>, thread: &ThreadId, now: Mark) -> Mark {
    if now.working || now.waiting {
        marks.insert(thread.clone(), now)
    } else {
        marks.remove(thread)
    }
    .unwrap_or_default()
}

fn mark(host: &HostData, thread: &ThreadId) -> Mark {
    Mark {
        working: host.is_working(thread),
        waiting: host.is_waiting(thread),
    }
}

pub fn tag(host: &HostId, thread: &ThreadId) -> SharedString {
    format!("{host}|{}", thread.0).into()
}

pub fn parse_tag(tag: &str) -> Option<(HostId, ThreadId)> {
    let (host, thread) = tag.split_once('|')?;
    Some((HostId(host.to_owned()), ThreadId(thread.to_owned())))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_threads_release_their_marks_and_notify_at_most_once() {
        let mut marks = HashMap::new();
        for index in 0..10_000 {
            let id = ThreadId::from(format!("thread-{index}"));
            assert_eq!(
                remember(
                    &mut marks,
                    &id,
                    Mark {
                        working: true,
                        waiting: false
                    }
                ),
                Mark::default()
            );
            assert!(remember(&mut marks, &id, Mark::default()).working);
            assert_eq!(remember(&mut marks, &id, Mark::default()), Mark::default());
        }
        assert!(marks.is_empty());
    }
}
