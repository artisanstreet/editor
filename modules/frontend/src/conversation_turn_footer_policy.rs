//! Pure action and state policy for a settled conversation turn footer.
//!
//! The footer owns two independent settled facts: the machine-readable
//! settlement timestamp and the response text copied by its action. Relative
//! age is supplied by the caller in its already-formatted form; this module
//! does not parse timestamps or duplicate relative-time policy. Likewise,
//! clipboard and clock work are represented as host-facing actions rather
//! than performed here.
//!
//! A clock sample is requested only when the actions become visible through
//! the footer's hover/focus inputs. There is no timer or periodic wakeup in
//! this policy. A periodic input, if a host reports one accidentally, is an
//! inert no-op.

#![allow(clippy::module_name_repetitions)]

/// Stable accessible name for the footer action group.
pub const TURN_ACTIONS_LABEL: &str = "Turn actions";

/// Stable accessible name and title for the copy control.
pub const COPY_RESPONSE_LABEL: &str = "Copy response";

/// Exact reader-facing message used when the host clipboard write fails.
pub const COPY_FAILURE_MESSAGE: &str = "Couldn't copy response. Try again.";

/// Input observed by the footer host.
///
/// `Hover` and `Focus` correspond to the legacy `mouseenter` and `focusin`
/// visibility paths. `Copy` starts one clipboard command. `PeriodicTick` is
/// included as an explicit fail-closed input so a host cannot accidentally
/// turn this leaf into a periodic refresher; it never requests a clock sample.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TurnFooterInput {
    /// The action group became visible through pointer hover.
    Hover,
    /// The action group became visible through keyboard or descendant focus.
    Focus,
    /// The copy control was activated.
    Copy,
    /// A host supplied a timer-like wakeup; the footer deliberately ignores it.
    PeriodicTick,
}

/// Result of the clipboard adapter operation started by a copy action.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CopyOutcome {
    /// The exact response text was accepted by the clipboard adapter.
    Succeeded,
    /// The clipboard adapter rejected the write.
    Failed,
}

/// One host operation admitted by a footer input.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TurnFooterAction {
    /// No host operation is needed.
    NoOp,
    /// Ask the host for one current clock sample.
    RequestClockSample,
    /// Ask the host clipboard adapter to write this exact response text.
    CopyResponse {
        /// The response payload, preserved byte-for-byte as supplied to the
        /// footer policy.
        text: String,
    },
}

impl TurnFooterAction {
    /// Returns whether this action admits no host operation.
    #[must_use]
    pub const fn is_no_op(&self) -> bool {
        matches!(self, Self::NoOp)
    }

    /// Returns whether this action requests one clock sample.
    #[must_use]
    pub const fn is_clock_sample_request(&self) -> bool {
        matches!(self, Self::RequestClockSample)
    }

    /// Returns the exact copy payload, if this action starts a copy.
    #[must_use]
    pub fn copy_text(&self) -> Option<&str> {
        match self {
            Self::CopyResponse { text } => Some(text),
            Self::NoOp | Self::RequestClockSample => None,
        }
    }
}

/// Stable accessibility facts and the machine-readable settled timestamp
/// exposed by one footer.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TurnFooterAccessibility<'a> {
    /// The footer's stable accessible name.
    pub actions_label: &'static str,
    /// The copy control's stable accessible name and title.
    pub copy_label: &'static str,
    /// The exact timestamp value intended for the `time[datetime]` attribute.
    pub settled_timestamp: &'a str,
}

/// Deterministic state owner for one settled conversation turn footer.
///
/// The response text is owned separately from `settled_at`, so a copy action
/// can carry the exact response independently of the timestamp used by the
/// age adapter. `relative_age` is also an opaque, adapter-formatted string;
/// assigning it never changes either settled fact.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationTurnFooterPolicy {
    settled_at: String,
    response_text: String,
    relative_age: String,
    copy_message: String,
}

impl ConversationTurnFooterPolicy {
    /// Creates a footer from the settled timestamp, exact response text, and
    /// an already-formatted relative age supplied by an adapter.
    pub fn new(
        settled_at: impl Into<String>,
        response_text: impl Into<String>,
        relative_age: impl Into<String>,
    ) -> Self {
        Self {
            settled_at: settled_at.into(),
            response_text: response_text.into(),
            relative_age: relative_age.into(),
            copy_message: String::new(),
        }
    }

    /// Returns the exact settled timestamp retained for machine-readable
    /// custody. This value is never replaced by relative-age refreshes.
    #[must_use]
    pub fn settled_at(&self) -> &str {
        &self.settled_at
    }

    /// Returns the exact response text retained for the copy command.
    #[must_use]
    pub fn response_text(&self) -> &str {
        &self.response_text
    }

    /// Returns the adapter-formatted relative age currently displayed.
    #[must_use]
    pub fn relative_age(&self) -> &str {
        &self.relative_age
    }

    /// Returns the current reader-facing copy status message, or an empty
    /// string when no failure is being shown.
    #[must_use]
    pub fn copy_message(&self) -> &str {
        &self.copy_message
    }

    /// Returns the stable accessibility facts for this footer.
    pub const fn accessibility(&self) -> TurnFooterAccessibility<'_> {
        TurnFooterAccessibility {
            actions_label: TURN_ACTIONS_LABEL,
            copy_label: COPY_RESPONSE_LABEL,
            settled_timestamp: self.settled_at.as_str(),
        }
    }

    /// Applies one footer input and returns the host-facing action it admits.
    ///
    /// Every hover/focus input requests one fresh clock sample. The returned
    /// action does not read the clock; the host owns sampling and should pass
    /// its adapter-formatted result to [`Self::set_relative_age`]. Timer-like
    /// inputs and all other non-refreshing inputs are deterministic no-ops.
    pub fn observe(&mut self, input: TurnFooterInput) -> TurnFooterAction {
        match input {
            TurnFooterInput::Hover | TurnFooterInput::Focus => TurnFooterAction::RequestClockSample,
            TurnFooterInput::Copy => self.start_copy(),
            TurnFooterInput::PeriodicTick => TurnFooterAction::NoOp,
        }
    }

    /// Starts a copy command, clearing any earlier status message before
    /// returning the exact response payload to the host adapter.
    pub fn start_copy(&mut self) -> TurnFooterAction {
        self.copy_message.clear();
        TurnFooterAction::CopyResponse {
            text: self.response_text.clone(),
        }
    }

    /// Applies the result of the host clipboard operation.
    ///
    /// Success leaves the status empty. Failure uses the one exact stable
    /// message; adapter error details never cross this presentation boundary.
    pub fn settle_copy(&mut self, outcome: CopyOutcome) {
        match outcome {
            CopyOutcome::Succeeded => self.copy_message.clear(),
            CopyOutcome::Failed => self.copy_message = COPY_FAILURE_MESSAGE.to_owned(),
        }
    }

    /// Stores an adapter-formatted relative age verbatim.
    ///
    /// This method deliberately accepts display text rather than a timestamp
    /// or a clock value. Parsing and relative-time thresholds belong to the
    /// existing adapter boundary.
    pub fn set_relative_age(&mut self, relative_age: impl Into<String>) {
        self.relative_age = relative_age.into();
    }
}
