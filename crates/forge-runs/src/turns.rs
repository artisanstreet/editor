use crate::LiveRun;
use artisan_api::{RunId, ThreadId};
use forge_store::{SettleRunError, Settlement};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::time::Instant;

#[derive(Debug)]
pub(crate) enum Turn {
    Live(LiveRun),
    Unsettled(Box<Settlement>),
    Deferred { until: Instant, retries: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Working,
    Stopping { through: u64, then: AfterStop },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterStop {
    Settle,
    Continue,
}

#[derive(Debug, Clone)]
pub(crate) enum StopState {
    Running,
    Requested,
    Saved,
    Unsaved(Arc<SettleRunError>),
}

#[derive(Debug, Clone)]
pub(crate) struct StopSignal(Arc<watch::Sender<StopState>>);

impl Default for StopSignal {
    fn default() -> Self {
        Self(Arc::new(watch::Sender::new(StopState::Running)))
    }
}

impl StopSignal {
    fn request(&self) -> StopWait {
        self.0.send_if_modified(|state| match state {
            StopState::Running => {
                *state = StopState::Requested;
                true
            }
            StopState::Requested | StopState::Saved | StopState::Unsaved(_) => false,
        });
        StopWait(self.0.subscribe())
    }

    pub(crate) fn is_requested(&self) -> bool {
        match *self.0.borrow() {
            StopState::Running => false,
            StopState::Requested | StopState::Saved | StopState::Unsaved(_) => true,
        }
    }

    pub(crate) async fn requested(&self) {
        let mut state = self.0.subscribe();
        state
            .wait_for(|state| match state {
                StopState::Running => false,
                StopState::Requested | StopState::Saved | StopState::Unsaved(_) => true,
            })
            .await
            .expect("invariant: a run's stop signal lives as long as the run");
    }

    pub(crate) fn resolve(&self, saved: Result<(), Arc<SettleRunError>>) {
        self.0.send_if_modified(|state| match state {
            StopState::Running | StopState::Requested => {
                *state = match saved {
                    Ok(()) => StopState::Saved,
                    Err(error) => StopState::Unsaved(error),
                };
                true
            }
            StopState::Saved | StopState::Unsaved(_) => false,
        });
    }
}

#[derive(Debug)]
pub(crate) struct StopWait(watch::Receiver<StopState>);

impl StopWait {
    pub(crate) async fn saved(mut self) -> Result<(), Arc<SettleRunError>> {
        let state = self
            .0
            .wait_for(|state| match state {
                StopState::Running | StopState::Requested => false,
                StopState::Saved | StopState::Unsaved(_) => true,
            })
            .await
            .expect("invariant: a live run resolves its stop before it leaves the turns")
            .clone();
        match state {
            StopState::Saved => Ok(()),
            StopState::Unsaved(error) => Err(error),
            StopState::Running | StopState::Requested => {
                unreachable!("invariant: the wait ends only once the stop is resolved")
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum Stopped {
    Live(StopWait),
    Unsettled(Box<Settlement>),
    Idle,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Continued {
    Recorded,
    Record,
    Busy,
    NothingStopped,
    Idle,
}

#[derive(Debug)]
pub(crate) enum Ended {
    Settled,
    Deferred,
    Unsaved {
        settlement: Box<Settlement>,
        error: Arc<SettleRunError>,
    },
}

#[derive(Debug, Default)]
pub(crate) struct Turns(HashMap<ThreadId, Turn>);

impl Turns {
    pub(crate) fn get(&self, thread: &ThreadId) -> Option<&LiveRun> {
        match self.0.get(thread)? {
            Turn::Live(run) => Some(run),
            Turn::Unsettled(_) | Turn::Deferred { .. } => None,
        }
    }

    pub(crate) fn get_mut(&mut self, thread: &ThreadId) -> Option<&mut LiveRun> {
        match self.0.get_mut(thread)? {
            Turn::Live(run) => Some(run),
            Turn::Unsettled(_) | Turn::Deferred { .. } => None,
        }
    }

    pub(crate) fn contains_key(&self, thread: &ThreadId) -> bool {
        self.get(thread).is_some()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&ThreadId, &LiveRun)> {
        self.0.iter().filter_map(|(thread, turn)| match turn {
            Turn::Live(run) => Some((thread, run)),
            Turn::Unsettled(_) | Turn::Deferred { .. } => None,
        })
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &ThreadId> {
        self.iter().map(|(thread, _)| thread)
    }

    pub(crate) fn held(&self) -> impl Iterator<Item = &ThreadId> {
        self.0.iter().filter_map(|(thread, turn)| match turn {
            Turn::Live(_) | Turn::Unsettled(_) => Some(thread),
            Turn::Deferred { .. } => None,
        })
    }

    pub(crate) fn holds(&self, thread: &ThreadId) -> bool {
        match self.0.get(thread) {
            Some(Turn::Live(_) | Turn::Unsettled(_)) => true,
            Some(Turn::Deferred { .. }) | None => false,
        }
    }

    pub(crate) fn waiting(&self, now: Instant) -> impl Iterator<Item = &ThreadId> {
        self.0.iter().filter_map(move |(thread, turn)| match turn {
            Turn::Deferred { until, .. } if *until > now => Some(thread),
            Turn::Deferred { .. } | Turn::Live(_) | Turn::Unsettled(_) => None,
        })
    }

    pub(crate) fn next_due(&self, now: Instant) -> Option<Instant> {
        self.0
            .values()
            .filter_map(|turn| match turn {
                Turn::Deferred { until, .. } if *until > now => Some(*until),
                Turn::Deferred { .. } | Turn::Live(_) | Turn::Unsettled(_) => None,
            })
            .min()
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
            Some(held @ (Turn::Live(_) | Turn::Unsettled(_))) => {
                unreachable!("invariant: thread {thread} is claimed while it is held: {held:?}")
            }
        };
        self.0.insert(thread, Turn::Live(start(retries)));
    }

    pub(crate) fn phase(&self, thread: &ThreadId, run: &RunId) -> Phase {
        match self.0.get(thread) {
            Some(Turn::Live(live)) if live.id == *run => live.phase,
            other => unreachable!(
                "invariant: run {run} of thread {thread} is live while it concludes, not {other:?}"
            ),
        }
    }

    pub(crate) fn stop(&mut self, thread: &ThreadId, through: u64) -> Stopped {
        match self.0.get_mut(thread) {
            Some(Turn::Live(run)) => {
                run.phase = Phase::Stopping {
                    through,
                    then: AfterStop::Settle,
                };
                let wait = run.stop.request();
                run.cancel.cancel();
                Stopped::Live(wait)
            }
            Some(Turn::Unsettled(settlement)) => {
                settlement.park = None;
                Stopped::Unsettled(settlement.clone())
            }
            Some(Turn::Deferred { .. }) | None => Stopped::Idle,
        }
    }

    pub(crate) fn carry_on(&self, thread: &ThreadId) -> Continued {
        match self.0.get(thread) {
            Some(Turn::Live(run)) => match run.phase {
                Phase::Working => Continued::Busy,
                Phase::Stopping {
                    then: AfterStop::Continue,
                    ..
                } => Continued::Recorded,
                Phase::Stopping {
                    then: AfterStop::Settle,
                    ..
                } => Continued::Record,
            },
            Some(Turn::Unsettled(settlement)) => {
                if settlement.run.status().is_stopped() {
                    Continued::Record
                } else {
                    Continued::NothingStopped
                }
            }
            Some(Turn::Deferred { .. }) | None => Continued::Idle,
        }
    }

    pub(crate) fn continued(&mut self, thread: &ThreadId) {
        match self.0.get_mut(thread) {
            Some(Turn::Live(LiveRun {
                phase: Phase::Stopping { then, .. },
                ..
            })) => *then = AfterStop::Continue,
            Some(Turn::Unsettled(settlement)) if settlement.run.status().is_stopped() => {}
            other => unreachable!(
                "invariant: thread {thread} records a Continue only while it is stopped, not {other:?}"
            ),
        }
    }

    pub(crate) fn end(&mut self, thread: &ThreadId, run: &RunId, ended: Ended, now: Instant) {
        let live = match self.0.remove(thread) {
            Some(Turn::Live(live)) if live.id == *run => live,
            other => unreachable!(
                "invariant: run {run} of thread {thread} ends while it is live, not {other:?}"
            ),
        };
        match ended {
            Ended::Settled => live.stop.resolve(Ok(())),
            Ended::Deferred => {
                live.stop.resolve(Ok(()));
                let retries = live.retries.saturating_add(1);
                self.0.insert(
                    thread.clone(),
                    Turn::Deferred {
                        until: now + crate::retry_delay(retries),
                        retries,
                    },
                );
            }
            Ended::Unsaved { settlement, error } => {
                live.stop.resolve(Err(error));
                self.0.insert(thread.clone(), Turn::Unsettled(settlement));
            }
        }
    }

    pub(crate) fn unsettled(&self) -> Vec<Settlement> {
        self.0
            .values()
            .filter_map(|turn| match turn {
                Turn::Unsettled(settlement) => Some(settlement.as_ref().clone()),
                Turn::Live(_) | Turn::Deferred { .. } => None,
            })
            .collect()
    }

    pub(crate) fn settled(&mut self, thread: &ThreadId) {
        match self.0.remove(thread) {
            Some(Turn::Unsettled(_)) => {}
            other => unreachable!(
                "invariant: only an unsettled run of thread {thread} settles later, not {other:?}"
            ),
        }
    }
}

#[cfg(test)]
#[path = "../tests/turns/unit.rs"]
mod tests;
