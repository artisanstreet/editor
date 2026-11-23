#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Wake {
    #[default]
    Prompted,
    Quiet,
    Jobs,
    Waking,
    Replying,
    Replied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Seen {
    PromptSent,
    PromptEnded,
    Activity,
    StepEnded,
    JobsFinished,
    Lapsed,
    Stopped,
}

impl Wake {
    #[must_use]
    pub(super) const fn next(self, seen: Seen, jobs: bool) -> Self {
        match (self, seen) {
            (_, Seen::PromptSent)
            | (
                Self::Prompted,
                Seen::Activity | Seen::StepEnded | Seen::JobsFinished | Seen::Lapsed,
            ) => Self::Prompted,
            (_, Seen::Stopped) | (Self::Waking, Seen::Lapsed) => Self::Quiet,
            (Self::Prompted, Seen::PromptEnded) | (Self::Replied, Seen::Lapsed) => {
                if jobs {
                    Self::Jobs
                } else {
                    Self::Quiet
                }
            }
            (_, Seen::Activity) => Self::Replying,
            (Self::Replying, Seen::StepEnded) => Self::Replied,
            (Self::Jobs, Seen::JobsFinished) => Self::Waking,
            (
                state @ (Self::Quiet | Self::Jobs | Self::Waking | Self::Replying | Self::Replied),
                Seen::PromptEnded | Seen::StepEnded | Seen::JobsFinished | Seen::Lapsed,
            ) => state,
        }
    }

    #[must_use]
    pub(super) const fn waiting_on_jobs(self) -> bool {
        matches!(self, Self::Jobs)
    }
}
