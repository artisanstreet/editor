//! Provider usage measurement observations.

use crate::run_usage::RunUsageBasis;

use super::{
    OBSERVATION_COUNT_MAX, ObservationError, ObservationId, ObservationSequence, check_count,
};

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

/// Provider usage accounting basis.
///
/// Preserved verbatim from the provider report.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UsageBasis {
    /// Counts add to prior observations.
    Delta,
    /// Counts replace a provider-reported total.
    Cumulative,
    /// The source did not identify its accounting basis.
    Unknown,
}

impl UsageBasis {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delta => "delta",
            Self::Cumulative => "cumulative",
            Self::Unknown => "unknown",
        }
    }

    /// Parses a provider-disclosed usage basis.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    /// Unknown bases never default to another basis.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "delta" => Ok(Self::Delta),
            "cumulative" => Ok(Self::Cumulative),
            "unknown" => Ok(Self::Unknown),
            _ => Err(ObservationError::UnknownValue { field: "basis" }),
        }
    }
}

impl From<RunUsageBasis> for UsageBasis {
    fn from(basis: RunUsageBasis) -> Self {
        match basis {
            RunUsageBasis::Delta => Self::Delta,
            RunUsageBasis::Cumulative => Self::Cumulative,
            RunUsageBasis::Unknown => Self::Unknown,
        }
    }
}

impl From<UsageBasis> for RunUsageBasis {
    fn from(basis: UsageBasis) -> Self {
        match basis {
            UsageBasis::Delta => Self::Delta,
            UsageBasis::Cumulative => Self::Cumulative,
            UsageBasis::Unknown => Self::Unknown,
        }
    }
}

/// Validated values used to construct one usage observation.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageInput {
    /// Whether counts add to prior observations or replace a total.
    pub basis: UsageBasis,
    /// Provider-reported input token count.
    pub input_tokens: Option<u64>,
    /// Provider-reported cached input token count.
    pub cached_input_tokens: Option<u64>,
    /// Provider-reported output token count.
    pub output_tokens: Option<u64>,
    /// Tokens occupying the context window when measured. A point-in-time
    /// gauge, never additive: a newer report replaces an older one regardless
    /// of `basis`.
    pub context_tokens: Option<u64>,
    /// Provider-reported usable context window in tokens, when disclosed.
    pub context_window_tokens: Option<u64>,
    /// Provider-reported cost in US dollars, when available.
    pub cost_usd: Option<f64>,
    /// Route provenance keeping gateway billing distinct, when disclosed.
    pub provider_route_id: Option<ObservationId>,
    /// Provider assistant message or turn identity, when disclosed.
    pub turn_id: Option<ObservationId>,
}

/// One provider usage measurement for the run or one turn.
///
/// `Some(0)` is deliberately distinct from [`None`]: zero is a measured
/// value, absent means the provider did not report the field. Context tokens
/// are a gauge and must never be summed across reports, no matter the basis.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    basis: UsageBasis,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    context_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    cost_usd: Option<f64>,
    provider_route_id: Option<ObservationId>,
    turn_id: Option<ObservationId>,
}

impl UsageObservation {
    /// Creates a usage observation after validating counts and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a token count leaves its finite
    /// range, the context window is zero, or the cost is negative or not
    /// finite.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        input: UsageInput,
    ) -> Result<Self, ObservationError> {
        check_count(input.input_tokens, "input_tokens")?;
        check_count(input.cached_input_tokens, "cached_input_tokens")?;
        check_count(input.output_tokens, "output_tokens")?;
        check_count(input.context_tokens, "context_tokens")?;
        if let Some(window) = input.context_window_tokens
            && (window == 0 || window > OBSERVATION_COUNT_MAX)
        {
            return Err(ObservationError::OutOfRange {
                field: "context_window_tokens",
            });
        }
        if input
            .cost_usd
            .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
        {
            return Err(ObservationError::OutOfRange { field: "cost_usd" });
        }
        Ok(Self {
            id,
            sequence,
            basis: input.basis,
            input_tokens: input.input_tokens,
            cached_input_tokens: input.cached_input_tokens,
            output_tokens: input.output_tokens,
            context_tokens: input.context_tokens,
            context_window_tokens: input.context_window_tokens,
            cost_usd: input.cost_usd,
            provider_route_id: input.provider_route_id,
            turn_id: input.turn_id,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns whether counts add to prior observations or replace a total.
    #[must_use]
    pub const fn basis(&self) -> UsageBasis {
        self.basis
    }

    /// Returns the provider-reported input token count.
    #[must_use]
    pub const fn input_tokens(&self) -> Option<u64> {
        self.input_tokens
    }

    /// Returns the provider-reported cached input token count.
    #[must_use]
    pub const fn cached_input_tokens(&self) -> Option<u64> {
        self.cached_input_tokens
    }

    /// Returns the provider-reported output token count.
    #[must_use]
    pub const fn output_tokens(&self) -> Option<u64> {
        self.output_tokens
    }

    /// Returns the context gauge value. Never additive across reports.
    #[must_use]
    pub const fn context_tokens(&self) -> Option<u64> {
        self.context_tokens
    }

    /// Returns the provider-reported usable context window, when disclosed.
    #[must_use]
    pub const fn context_window_tokens(&self) -> Option<u64> {
        self.context_window_tokens
    }

    /// Returns the provider-reported cost in US dollars, when available.
    #[must_use]
    pub const fn cost_usd(&self) -> Option<f64> {
        self.cost_usd
    }

    /// Returns the route provenance, when disclosed.
    #[must_use]
    pub const fn provider_route_id(&self) -> Option<&ObservationId> {
        self.provider_route_id.as_ref()
    }

    /// Returns the provider turn identity, when disclosed.
    #[must_use]
    pub const fn turn_id(&self) -> Option<&ObservationId> {
        self.turn_id.as_ref()
    }
}
