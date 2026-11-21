use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_mins(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket(u64);

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Started(Ticket),
    Cancelled,
    Unchanged,
}

#[derive(Debug)]
pub struct Renewal<Task> {
    issued: u64,
    phase: Phase<Task>,
}

#[derive(Debug)]
enum Phase<Task> {
    Idle {
        last: Option<Instant>,
    },
    Running {
        ticket: Ticket,
        stage: Stage,
        since: Instant,
        _task: Task,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Capturing,
    Handing,
}

impl<Task> Default for Renewal<Task> {
    fn default() -> Self {
        Self {
            issued: 0,
            phase: Phase::Idle { last: None },
        }
    }
}

impl<Task> Renewal<Task> {
    pub fn observe(
        &mut self,
        wanted: bool,
        now: Instant,
        start: impl FnOnce(Ticket) -> Task,
    ) -> Step {
        match (&self.phase, wanted) {
            (Phase::Running { since, .. }, false) => {
                self.phase = Phase::Idle { last: Some(*since) };
                Step::Cancelled
            }
            (Phase::Idle { last }, true)
                if last.is_none_or(|at| now.saturating_duration_since(at) >= EVERY) =>
            {
                self.issued += 1;
                let ticket = Ticket(self.issued);
                self.phase = Phase::Running {
                    ticket,
                    stage: Stage::Capturing,
                    since: now,
                    _task: start(ticket),
                };
                Step::Started(ticket)
            }
            (Phase::Running { .. }, true) | (Phase::Idle { .. }, true | false) => Step::Unchanged,
        }
    }

    pub fn captured(&mut self, ticket: Ticket, hand: impl FnOnce() -> Task) -> bool {
        let Phase::Running {
            ticket: current,
            stage: Stage::Capturing,
            since,
            ..
        } = &self.phase
        else {
            return false;
        };
        if *current != ticket {
            return false;
        }
        self.phase = Phase::Running {
            ticket,
            stage: Stage::Handing,
            since: *since,
            _task: hand(),
        };
        true
    }

    pub fn finished(&mut self, ticket: Ticket) {
        if let Phase::Running {
            ticket: current,
            since,
            ..
        } = &self.phase
            && *current == ticket
        {
            self.phase = Phase::Idle { last: Some(*since) };
        }
    }
}

#[cfg(test)]
#[path = "../tests/renewal.rs"]
mod tests;
