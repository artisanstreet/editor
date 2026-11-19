use crate::LiveRun;
use artisan_api::ThreadId;
use std::collections::HashMap;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub(crate) enum Turn {
    Live(LiveRun),
    Unsettled {
        settlement: Box<forge_store::Settlement>,
        then: AfterStop,
    },
    Owed,
    Deferred {
        until: Instant,
        retries: u32,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Phase {
    Working,
    Stopping { through: u64, then: AfterStop },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterStop {
    Settle,
    Continue,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct StopSignal {
    pub(crate) requested: CancellationToken,
    pub(crate) saved: CancellationToken,
}

pub(crate) enum Stopped {
    Live(CancellationToken),
    Idle,
}

pub(crate) enum Continued {
    Queued,
    Busy,
    NothingStopped,
    Idle,
}

#[derive(Debug, Default)]
pub(crate) struct Turns(HashMap<ThreadId, Turn>);

impl Turns {
    pub(crate) fn get(&self, thread: &ThreadId) -> Option<&LiveRun> {
        match self.0.get(thread)? {
            Turn::Live(run) => Some(run),
            Turn::Unsettled { .. } | Turn::Owed | Turn::Deferred { .. } => None,
        }
    }

    pub(crate) fn get_mut(&mut self, thread: &ThreadId) -> Option<&mut LiveRun> {
        match self.0.get_mut(thread)? {
            Turn::Live(run) => Some(run),
            Turn::Unsettled { .. } | Turn::Owed | Turn::Deferred { .. } => None,
        }
    }

    pub(crate) fn contains_key(&self, thread: &ThreadId) -> bool {
        self.get(thread).is_some()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&ThreadId, &LiveRun)> {
        self.0.iter().filter_map(|(thread, turn)| match turn {
            Turn::Live(run) => Some((thread, run)),
            Turn::Unsettled { .. } | Turn::Owed | Turn::Deferred { .. } => None,
        })
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &ThreadId> {
        self.iter().map(|(thread, _)| thread)
    }

    pub(crate) fn held(&self) -> impl Iterator<Item = &ThreadId> {
        self.0.iter().filter_map(|(thread, turn)| match turn {
            Turn::Live(_) | Turn::Unsettled { .. } | Turn::Owed => Some(thread),
            Turn::Deferred { .. } => None,
        })
    }

    pub(crate) fn holds(&self, thread: &ThreadId) -> bool {
        match self.0.get(thread) {
            Some(Turn::Live(_) | Turn::Unsettled { .. } | Turn::Owed) => true,
            Some(Turn::Deferred { .. }) | None => false,
        }
    }

    pub(crate) fn waiting(&self, now: Instant) -> impl Iterator<Item = &ThreadId> {
        self.0.iter().filter_map(move |(thread, turn)| match turn {
            Turn::Deferred { until, .. } if *until > now => Some(thread),
            Turn::Deferred { .. } | Turn::Live(_) | Turn::Unsettled { .. } | Turn::Owed => None,
        })
    }

    pub(crate) fn next_due(&self) -> Option<Instant> {
        self.0
            .values()
            .filter_map(|turn| match turn {
                Turn::Deferred { until, .. } => Some(*until),
                Turn::Live(_) | Turn::Unsettled { .. } | Turn::Owed => None,
            })
            .min()
    }

    pub(crate) fn release_due(&mut self, now: Instant) {
        self.0.retain(|_, turn| match turn {
            Turn::Deferred { until, .. } => *until > now,
            Turn::Live(_) | Turn::Unsettled { .. } | Turn::Owed => true,
        });
    }

    pub(crate) fn undefer(&mut self, thread: &ThreadId) {
        if let Some(Turn::Deferred { .. }) = self.0.get(thread) {
            self.0.remove(thread);
        }
    }

    pub(crate) fn claim(&mut self, thread: ThreadId, start: impl FnOnce(u32) -> LiveRun) {
        let retries = match self.0.remove(&thread) {
            Some(Turn::Deferred { retries, .. }) => retries,
            None => 0,
            Some(held @ (Turn::Live(_) | Turn::Unsettled { .. } | Turn::Owed)) => {
                unreachable!("invariant: thread {thread} is claimed while it is held: {held:?}")
            }
        };
        self.0.insert(thread, Turn::Live(start(retries)));
    }

    pub(crate) fn stop(&mut self, thread: &ThreadId, through: u64) -> Stopped {
        match self.0.get_mut(thread) {
            Some(Turn::Live(run)) => {
                run.phase = Phase::Stopping {
                    through,
                    then: AfterStop::Settle,
                };
                run.stop.requested.cancel();
                run.cancel.cancel();
                Stopped::Live(run.stop.saved.clone())
            }
            Some(Turn::Unsettled { then, .. }) => {
                *then = AfterStop::Settle;
                Stopped::Idle
            }
            Some(Turn::Owed) => {
                self.0.remove(thread);
                Stopped::Idle
            }
            Some(Turn::Deferred { .. }) | None => Stopped::Idle,
        }
    }

    pub(crate) fn carry_on(&mut self, thread: &ThreadId) -> Continued {
        match self.0.get_mut(thread) {
            Some(Turn::Live(run)) => match &mut run.phase {
                Phase::Working => Continued::Busy,
                Phase::Stopping { then, .. } => {
                    *then = AfterStop::Continue;
                    Continued::Queued
                }
            },
            Some(Turn::Unsettled { settlement, then }) => {
                if settlement.run.status().is_stopped() {
                    *then = AfterStop::Continue;
                    Continued::Queued
                } else {
                    Continued::NothingStopped
                }
            }
            Some(Turn::Owed) => Continued::Queued,
            Some(Turn::Deferred { .. }) => {
                self.0.remove(thread);
                Continued::Idle
            }
            None => Continued::Idle,
        }
    }

    pub(crate) fn unsettle(&mut self, settlement: forge_store::Settlement) {
        let thread = settlement.run.thread_id.clone();
        let then = match self.0.remove(&thread) {
            Some(Turn::Live(LiveRun {
                phase: Phase::Stopping { then, .. },
                ..
            })) => then,
            Some(Turn::Live(LiveRun {
                phase: Phase::Working,
                ..
            })) => AfterStop::Settle,
            other => unreachable!(
                "invariant: run {} of thread {thread} settles while it is live, not {other:?}",
                settlement.run.id
            ),
        };
        self.0.insert(
            thread,
            Turn::Unsettled {
                settlement: Box::new(settlement),
                then,
            },
        );
    }

    pub(crate) fn unsettled(&self) -> Vec<forge_store::Settlement> {
        self.0
            .values()
            .filter_map(|turn| match turn {
                Turn::Unsettled { settlement, .. } => Some(settlement.as_ref().clone()),
                Turn::Live(_) | Turn::Owed | Turn::Deferred { .. } => None,
            })
            .collect()
    }

    pub(crate) fn settled(&mut self, thread: &ThreadId) {
        match self.0.remove(thread) {
            Some(Turn::Unsettled {
                then: AfterStop::Continue,
                ..
            }) => {
                self.0.insert(thread.clone(), Turn::Owed);
            }
            Some(Turn::Unsettled {
                then: AfterStop::Settle,
                ..
            }) => {}
            other => unreachable!(
                "invariant: only an unsettled run of thread {thread} settles later, not {other:?}"
            ),
        }
    }

    pub(crate) fn continues_after_stop(&self, thread: &ThreadId) -> bool {
        self.get(thread).is_some_and(|run| {
            matches!(
                run.phase,
                Phase::Stopping {
                    then: AfterStop::Continue,
                    ..
                }
            )
        })
    }

    pub(crate) fn end(&mut self, thread: &ThreadId, retry_later: bool, owed: bool, now: Instant) {
        match self.0.remove(thread) {
            Some(Turn::Live(_)) if owed => {
                self.0.insert(thread.clone(), Turn::Owed);
            }
            Some(Turn::Live(run)) if retry_later => {
                let retries = run.retries.saturating_add(1);
                self.0.insert(
                    thread.clone(),
                    Turn::Deferred {
                        until: now + crate::retry_delay(retries),
                        retries,
                    },
                );
            }
            Some(Turn::Live(_)) => {}
            Some(kept @ Turn::Unsettled { .. }) => {
                self.0.insert(thread.clone(), kept);
            }
            other => unreachable!(
                "invariant: the run of thread {thread} ends while it is live, not {other:?}"
            ),
        }
    }

    pub(crate) fn owed(&self) -> Vec<ThreadId> {
        self.0
            .iter()
            .filter_map(|(thread, turn)| match turn {
                Turn::Owed => Some(thread.clone()),
                Turn::Live(_) | Turn::Unsettled { .. } | Turn::Deferred { .. } => None,
            })
            .collect()
    }

    pub(crate) fn is_owed(&self, thread: &ThreadId) -> bool {
        matches!(self.0.get(thread), Some(Turn::Owed))
    }

    pub(crate) fn fulfilled(&mut self, thread: &ThreadId) {
        if self.is_owed(thread) {
            self.0.remove(thread);
        }
    }
}
