use artisan_api::{Run, RunId, RunOutcome, RunState, Timestamp};

#[derive(Clone, Debug, PartialEq)]
pub enum Held {
    Stopping(Run),
    Settled(Run),
    Other(Run),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stopping {
    run: RunId,
    at: Timestamp,
}

impl Stopping {
    pub fn begin(run: &mut Run, at: Timestamp) -> Option<Self> {
        if !run.is_live() {
            return None;
        }
        run.state = cancelled_at(at);
        Some(Self {
            run: run.id.clone(),
            at,
        })
    }

    #[must_use]
    pub fn hold(&self, run: Run) -> Held {
        match (run.id == self.run, run.is_live()) {
            (false, _) => Held::Other(run),
            (true, false) => Held::Settled(run),
            (true, true) => Held::Stopping(Run {
                state: cancelled_at(self.at),
                ..run
            }),
        }
    }

    pub fn revert(&self, run: &mut Run) {
        if run.id == self.run && run.state == cancelled_at(self.at) {
            run.state = RunState::Running;
        }
    }
}

const fn cancelled_at(at: Timestamp) -> RunState {
    RunState::Finished {
        outcome: RunOutcome::Cancelled,
        error: None,
        ended_at: at,
    }
}
