//! System notifications when a thread finishes, fails or asks for an
//! answer, unless the user is already looking at it. Each Forge metadata
//! event updates one transient thread mark.

use artisan_api::{RunStatus, ThreadId};
use editor_core::{HostData, HostId};
use gpui::{SharedString, SystemNotification};
use std::collections::HashMap;

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

    pub fn retain_hosts(&mut self, live: &[HostId]) {
        self.marks.retain(|id, _| live.contains(id));
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
        let body = if now.waiting && !previous.waiting {
            "needs your answer"
        } else if previous.working && !now.working {
            match host.runs.get(thread).map(|run| run.status) {
                Some(RunStatus::Done) => "finished",
                Some(RunStatus::Failed | RunStatus::Interrupted | RunStatus::Refused) => "failed",
                _ => return None,
            }
        } else {
            return None;
        };
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
