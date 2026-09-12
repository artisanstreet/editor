//! Bounded provider-token usage attributed to one durable assistant run.
//!
//! `OpenCode2`'s usage envelope carries a provider session and durable source
//! sequence, but no Artisan run, thread, or model identity. The engine owner
//! supplies those immutable launch-context values; this module validates and
//! stores the resulting typed report without retaining the provider payload.

use thiserror::Error;

use crate::bounds::IDENTIFIER_MAX_BYTES;
use crate::{EngineModelId, EngineRouteId, EngineVariantId, RunId, ThreadId, UnixMillis};

/// Maximum UTF-8 byte length of a provider session identity.
pub const RUN_USAGE_PROVIDER_SESSION_MAX_BYTES: usize = IDENTIFIER_MAX_BYTES;

/// Maximum UTF-8 byte length of a provider turn/message identity.
pub const RUN_USAGE_PROVIDER_TURN_MAX_BYTES: usize = IDENTIFIER_MAX_BYTES;

/// Largest source sequence representable by the SQLite signed integer column.
pub const RUN_USAGE_MAX_SOURCE_SEQUENCE: u64 = i64::MAX as u64;

/// Largest token count representable by the SQLite signed integer columns.
pub const RUN_USAGE_MAX_TOKEN_COUNT: u64 = i64::MAX as u64;

/// Provider usage accounting basis.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunUsageBasis {
    /// `OpenCode2` reports the usage for the completed step as a delta.
    Delta,
    /// A future provider may report a cumulative total.
    Cumulative,
    /// The source did not identify its accounting basis.
    Unknown,
}

impl RunUsageBasis {
    /// Returns the stable database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delta => "delta",
            Self::Cumulative => "cumulative",
            Self::Unknown => "unknown",
        }
    }

    /// Parses the stable database spelling.
    #[expect(
        clippy::should_implement_trait,
        reason = "database callers depend on the Option-returning spelling parser; `FromStr` requires a `Result` error type for a value that is expected to be absent"
    )]
    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "delta" => Some(Self::Delta),
            "cumulative" => Some(Self::Cumulative),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Validated values used to construct one provider usage report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunUsageReportInput {
    /// Durable Artisan run attributed by the owner.
    pub run_id: RunId,
    /// Native thread containing the run.
    pub thread_id: ThreadId,
    /// Provider session from the authenticated event.
    pub provider_session_id: String,
    /// Durable provider sequence from `durable.seq`.
    pub source_sequence: u64,
    /// Immutable model origin from the launched run configuration.
    pub model_id: EngineModelId,
    /// Immutable provider route origin from the launched run configuration.
    pub provider_route_id: EngineRouteId,
    /// Optional immutable provider variant origin.
    pub variant_id: Option<EngineVariantId>,
    /// Accounting basis reported by the source adapter.
    pub basis: RunUsageBasis,
    /// Optional provider assistant message/turn identity.
    pub provider_turn_id: Option<String>,
    /// Provider-reported input token count.
    pub input_tokens: Option<u64>,
    /// Provider-reported cached input/read token count.
    pub cached_input_tokens: Option<u64>,
    /// Provider-reported output token count.
    pub output_tokens: Option<u64>,
    /// Provider-reported context token count, when a source supplies one.
    pub context_tokens: Option<u64>,
    /// Provider-reported context-window capacity, when a source supplies one.
    pub context_window_tokens: Option<u64>,
    /// Local observation time supplied by the owner.
    pub observed_at: UnixMillis,
}

/// One validated, bounded provider usage report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunUsageReport {
    run_id: RunId,
    thread_id: ThreadId,
    provider_session_id: String,
    source_sequence: u64,
    model_id: EngineModelId,
    provider_route_id: EngineRouteId,
    variant_id: Option<EngineVariantId>,
    basis: RunUsageBasis,
    provider_turn_id: Option<String>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    context_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    observed_at: UnixMillis,
}

impl RunUsageReport {
    /// Builds a report after validating provider identities and bounded
    /// integer fields. `Some(0)` is deliberately distinct from `None`.
    ///
    /// # Errors
    ///
    /// Returns [`RunUsageReportError`] when the provider session or turn
    /// identity is empty, contains whitespace or control scalars, or exceeds
    /// its byte bound; when the source sequence or a token count leaves the
    /// SQLite integer range; or when the context window is zero.
    pub fn new(input: RunUsageReportInput) -> Result<Self, RunUsageReportError> {
        validate_provider_text(
            &input.provider_session_id,
            "provider session id",
            RUN_USAGE_PROVIDER_SESSION_MAX_BYTES,
        )?;
        if let Some(provider_turn_id) = &input.provider_turn_id {
            validate_provider_text(
                provider_turn_id,
                "provider turn id",
                RUN_USAGE_PROVIDER_TURN_MAX_BYTES,
            )?;
        }
        if input.source_sequence > RUN_USAGE_MAX_SOURCE_SEQUENCE {
            return Err(RunUsageReportError::SourceSequenceTooLarge);
        }
        validate_token(input.input_tokens, "input_tokens")?;
        validate_token(input.cached_input_tokens, "cached_input_tokens")?;
        validate_token(input.output_tokens, "output_tokens")?;
        validate_token(input.context_tokens, "context_tokens")?;
        if let Some(context_window_tokens) = input.context_window_tokens {
            if context_window_tokens == 0 {
                return Err(RunUsageReportError::ContextWindowMustBePositive);
            }
            validate_token(Some(context_window_tokens), "context_window_tokens")?;
        }

        Ok(Self {
            run_id: input.run_id,
            thread_id: input.thread_id,
            provider_session_id: input.provider_session_id,
            source_sequence: input.source_sequence,
            model_id: input.model_id,
            provider_route_id: input.provider_route_id,
            variant_id: input.variant_id,
            basis: input.basis,
            provider_turn_id: input.provider_turn_id,
            input_tokens: input.input_tokens,
            cached_input_tokens: input.cached_input_tokens,
            output_tokens: input.output_tokens,
            context_tokens: input.context_tokens,
            context_window_tokens: input.context_window_tokens,
            observed_at: input.observed_at,
        })
    }

    /// Returns the Artisan run identity.
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Returns the native thread identity.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the exact provider session identity.
    #[must_use]
    pub fn provider_session_id(&self) -> &str {
        &self.provider_session_id
    }

    /// Returns the exact provider durable source sequence.
    #[must_use]
    pub const fn source_sequence(&self) -> u64 {
        self.source_sequence
    }

    /// Returns the immutable model origin.
    #[must_use]
    pub const fn model_id(&self) -> &EngineModelId {
        &self.model_id
    }

    /// Returns the immutable provider route origin.
    #[must_use]
    pub const fn provider_route_id(&self) -> &EngineRouteId {
        &self.provider_route_id
    }

    /// Returns the optional immutable variant origin.
    #[must_use]
    pub const fn variant_id(&self) -> Option<&EngineVariantId> {
        self.variant_id.as_ref()
    }

    /// Returns the accounting basis.
    #[must_use]
    pub const fn basis(&self) -> RunUsageBasis {
        self.basis
    }

    /// Returns the optional provider assistant message/turn identity.
    #[must_use]
    pub fn provider_turn_id(&self) -> Option<&str> {
        self.provider_turn_id.as_deref()
    }

    /// Returns the provider-reported input token count.
    #[must_use]
    pub const fn input_tokens(&self) -> Option<u64> {
        self.input_tokens
    }

    /// Returns the provider-reported cached input/read token count.
    #[must_use]
    pub const fn cached_input_tokens(&self) -> Option<u64> {
        self.cached_input_tokens
    }

    /// Returns the provider-reported output token count.
    #[must_use]
    pub const fn output_tokens(&self) -> Option<u64> {
        self.output_tokens
    }

    /// Returns the provider-reported context token count, if present.
    #[must_use]
    pub const fn context_tokens(&self) -> Option<u64> {
        self.context_tokens
    }

    /// Returns the provider-reported context-window capacity, if present.
    #[must_use]
    pub const fn context_window_tokens(&self) -> Option<u64> {
        self.context_window_tokens
    }

    /// Returns the owner observation timestamp.
    #[must_use]
    pub const fn observed_at(&self) -> UnixMillis {
        self.observed_at
    }

    /// Compares provider content for retry idempotence, ignoring only the
    /// local observation timestamp.
    #[must_use]
    pub fn same_provider_measurement(&self, other: &Self) -> bool {
        self.run_id == other.run_id
            && self.thread_id == other.thread_id
            && self.provider_session_id == other.provider_session_id
            && self.source_sequence == other.source_sequence
            && self.model_id == other.model_id
            && self.provider_route_id == other.provider_route_id
            && self.variant_id == other.variant_id
            && self.basis == other.basis
            && self.provider_turn_id == other.provider_turn_id
            && self.input_tokens == other.input_tokens
            && self.cached_input_tokens == other.cached_input_tokens
            && self.output_tokens == other.output_tokens
            && self.context_tokens == other.context_tokens
            && self.context_window_tokens == other.context_window_tokens
    }
}

/// Failure while validating a provider usage report.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum RunUsageReportError {
    /// Provider session identity was empty.
    #[error("provider session id must not be empty")]
    ProviderSessionEmpty,
    /// Provider session identity contained whitespace or a control scalar.
    #[error("provider session id contains whitespace or a control character")]
    ProviderSessionForbiddenCharacter,
    /// Provider session identity exceeded its byte bound.
    #[error("provider session id exceeds its bounded length")]
    ProviderSessionTooLong,
    /// Provider turn identity was empty.
    #[error("provider turn id must not be empty")]
    ProviderTurnEmpty,
    /// Provider turn identity contained whitespace or a control scalar.
    #[error("provider turn id contains whitespace or a control character")]
    ProviderTurnForbiddenCharacter,
    /// Provider turn identity exceeded its byte bound.
    #[error("provider turn id exceeds its bounded length")]
    ProviderTurnTooLong,
    /// Source sequence cannot be represented by SQLite.
    #[error("provider source sequence exceeds its bounded integer range")]
    SourceSequenceTooLarge,
    /// A token count cannot be represented by SQLite.
    #[error("provider token count for `{field}` exceeds its bounded integer range")]
    TokenCountTooLarge { field: &'static str },
    /// A context-window capacity of zero is not meaningful.
    #[error("context window token count must be positive")]
    ContextWindowMustBePositive,
}

fn validate_provider_text(
    value: &str,
    field: &'static str,
    maximum: usize,
) -> Result<(), RunUsageReportError> {
    if value.is_empty() {
        return Err(match field {
            "provider session id" => RunUsageReportError::ProviderSessionEmpty,
            _ => RunUsageReportError::ProviderTurnEmpty,
        });
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(match field {
            "provider session id" => RunUsageReportError::ProviderSessionForbiddenCharacter,
            _ => RunUsageReportError::ProviderTurnForbiddenCharacter,
        });
    }
    if value.len() > maximum {
        return Err(match field {
            "provider session id" => RunUsageReportError::ProviderSessionTooLong,
            _ => RunUsageReportError::ProviderTurnTooLong,
        });
    }
    Ok(())
}

fn validate_token(value: Option<u64>, field: &'static str) -> Result<(), RunUsageReportError> {
    if value.is_some_and(|value| value > RUN_USAGE_MAX_TOKEN_COUNT) {
        return Err(RunUsageReportError::TokenCountTooLarge { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(provider_session_id: &str, source_sequence: u64, observed_at: i64) -> RunUsageReport {
        RunUsageReport::new(RunUsageReportInput {
            run_id: RunId::parse("run-1").expect("run id"),
            thread_id: ThreadId::parse("thread-1").expect("thread id"),
            provider_session_id: provider_session_id.to_owned(),
            source_sequence,
            model_id: EngineModelId::parse("model-1").expect("model id"),
            provider_route_id: EngineRouteId::parse("route-1").expect("route id"),
            variant_id: None,
            basis: RunUsageBasis::Delta,
            provider_turn_id: Some("assistant-1".to_owned()),
            input_tokens: Some(0),
            cached_input_tokens: None,
            output_tokens: Some(4),
            context_tokens: None,
            context_window_tokens: None,
            observed_at: UnixMillis::from_millis(observed_at),
        })
        .expect("report")
    }

    #[test]
    fn preserves_zero_and_absent_token_fields() {
        let value = report("session-1", 0, 1);
        assert_eq!(value.input_tokens(), Some(0));
        assert_eq!(value.cached_input_tokens(), None);
    }

    #[test]
    fn measurement_equality_ignores_only_local_observation_time() {
        let first = report("session-1", 4, 1);
        let second = report("session-1", 4, 2);
        assert!(first.same_provider_measurement(&second));
    }

    #[test]
    fn rejects_oversized_numeric_values_without_clamping() {
        let error = RunUsageReport::new(RunUsageReportInput {
            run_id: RunId::parse("run-1").expect("run id"),
            thread_id: ThreadId::parse("thread-1").expect("thread id"),
            provider_session_id: "session-1".to_owned(),
            source_sequence: i64::MAX as u64 + 1,
            model_id: EngineModelId::parse("model-1").expect("model id"),
            provider_route_id: EngineRouteId::parse("route-1").expect("route id"),
            variant_id: None,
            basis: RunUsageBasis::Delta,
            provider_turn_id: None,
            input_tokens: Some(0),
            cached_input_tokens: None,
            output_tokens: None,
            context_tokens: None,
            context_window_tokens: None,
            observed_at: UnixMillis::EPOCH,
        })
        .expect_err("sequence must stay within the SQLite bound");
        assert_eq!(error, RunUsageReportError::SourceSequenceTooLarge);

        let error = RunUsageReport::new(RunUsageReportInput {
            run_id: RunId::parse("run-1").expect("run id"),
            thread_id: ThreadId::parse("thread-1").expect("thread id"),
            provider_session_id: "session-1".to_owned(),
            source_sequence: 1,
            model_id: EngineModelId::parse("model-1").expect("model id"),
            provider_route_id: EngineRouteId::parse("route-1").expect("route id"),
            variant_id: None,
            basis: RunUsageBasis::Delta,
            provider_turn_id: None,
            input_tokens: Some(i64::MAX as u64 + 1),
            cached_input_tokens: None,
            output_tokens: None,
            context_tokens: None,
            context_window_tokens: None,
            observed_at: UnixMillis::EPOCH,
        })
        .expect_err("token counts must stay within the SQLite bound");
        assert_eq!(
            error,
            RunUsageReportError::TokenCountTooLarge {
                field: "input_tokens"
            }
        );
    }
}
