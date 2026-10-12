//! System notifications when a thread finishes, fails or asks for an
//! answer, unless the user is already looking at it. Derived by comparing
//! each thread's state before and after a host change; nothing is stored.

use artisan_api::{RunStatus, ThreadId};
use editor_core::{HostData, HostId};
use gpui::{SharedString, SystemNotification};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
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
    /// What changed on `host` since the last call. `watching` is the thread
    /// on screen while the window is focused.
    pub fn observe(
        &mut self,
        host: &HostData,
        watching: Option<&ThreadId>,
    ) -> Vec<SystemNotification> {
        let current: HashMap<ThreadId, Mark> = host
            .threads
            .keys()
            .map(|thread| {
                (
                    thread.clone(),
                    Mark {
                        working: host.is_working(thread),
                        waiting: host.is_waiting(thread),
                    },
                )
            })
            .collect();
        let previous = self
            .marks
            .insert(host.id.clone(), host.is_live().then(|| current.clone()))
            .flatten();
        let Some(previous) = previous else {
            return Vec::new();
        };
        let mut posted = Vec::new();
        for (thread, now) in &current {
            let Some(before) = previous.get(thread) else {
                continue;
            };
            if Some(thread) == watching {
                continue;
            }
            let Some(info) = host.threads.get(thread) else {
                continue;
            };
            if info.parent_thread_id.is_some() {
                continue;
            }
            let title = info.title.clone().unwrap_or_else(|| "A thread".to_owned());
            let body = if now.waiting && !before.waiting {
                "needs your answer"
            } else if before.working && !now.working {
                match host.runs.get(thread).map(|run| run.status) {
                    Some(RunStatus::Done) => "finished",
                    Some(RunStatus::Failed | RunStatus::Interrupted | RunStatus::Refused) => {
                        "failed"
                    }
                    _ => continue,
                }
            } else {
                continue;
            };
            posted.push(SystemNotification {
                tag: tag(&host.id, thread),
                title: SharedString::from(title),
                body: body.into(),
                actions: Vec::new(),
            });
        }
        posted
    }
}

pub fn tag(host: &HostId, thread: &ThreadId) -> SharedString {
    format!("{host}|{}", thread.0).into()
}

pub fn parse_tag(tag: &str) -> Option<(HostId, ThreadId)> {
    let (host, thread) = tag.split_once('|')?;
    Some((HostId(host.to_owned()), ThreadId(thread.to_owned())))
}
