use artisan_api::{Run, RunId, RunStatus, Timestamp};

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
        if !run.status.is_live() {
            return None;
        }
        run.status = RunStatus::Cancelled;
        run.ended_at = Some(at);
        Some(Self {
            run: run.id.clone(),
            at,
        })
    }

    #[must_use]
    pub fn hold(&self, run: Run) -> Held {
        match (run.id == self.run, run.status.is_live()) {
            (false, _) => Held::Other(run),
            (true, false) => Held::Settled(run),
            (true, true) => Held::Stopping(Run {
                status: RunStatus::Cancelled,
                ended_at: Some(self.at),
                ..run
            }),
        }
    }

    pub fn revert(&self, run: &mut Run) {
        if run.id == self.run && run.status == RunStatus::Cancelled && run.ended_at == Some(self.at)
        {
            run.status = RunStatus::Running;
            run.ended_at = None;
        }
    }
}
