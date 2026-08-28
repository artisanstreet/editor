//! Dependency-free projection and evidence sanitization for usage interruptions.
//!
//! The persistence adapter owns JSON decoding and supplies the alternatives as
//! an explicit typed outcome. This module only projects that outcome into the
//! renderer-facing snapshot and sanitizes provider evidence; it does not parse
//! JSON, validate a schema, access a database, or retain state.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::fmt;

/// Maximum number of Unicode scalar values retained in provider evidence.
pub const MAX_USAGE_EVIDENCE_TEXT_SCALARS: usize = 256;

/// One provider-verified alternative model supplied by the persistence
/// decoder.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageInterruptionAlternative {
    /// Display name supplied by the provider catalog.
    pub display_name: String,
    /// Engine identity supplied by the provider catalog.
    pub engine_id: String,
    /// Native model identity supplied by the provider catalog.
    pub model_id: String,
    /// Verification timestamp supplied by the provider usage read.
    pub verified_at: String,
}

impl UsageInterruptionAlternative {
    /// Creates an alternative while preserving each supplied field exactly.
    #[must_use = "use the constructed alternative"]
    pub fn new(
        display_name: impl Into<String>,
        engine_id: impl Into<String>,
        model_id: impl Into<String>,
        verified_at: impl Into<String>,
    ) -> Self {
        Self {
            display_name: display_name.into(),
            engine_id: engine_id.into(),
            model_id: model_id.into(),
            verified_at: verified_at.into(),
        }
    }
}

/// The explicit failure used when the persistence adapter cannot decode the
/// stored alternatives payload.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageInterruptionDecodeError {
    /// The stored alternatives were not a usable decoded alternative list.
    MalformedAlternatives,
}

impl fmt::Display for UsageInterruptionDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedAlternatives => {
                formatter.write_str("usage interruption alternatives failed to decode")
            }
        }
    }
}

impl std::error::Error for UsageInterruptionDecodeError {}

/// Already-decoded alternatives or the explicit failure produced by their
/// persistence decoder.
pub type DecodedUsageInterruptionAlternatives =
    Result<Vec<UsageInterruptionAlternative>, UsageInterruptionDecodeError>;

/// A typed representation of the durable columns consumed by the source
/// usage-interruption model decoder.
///
/// The alternatives field stands in for the source row's serialized
/// `alternatives_json` column after an adapter-owned decode attempt. Keeping
/// that attempt as a Result prevents malformed data from being projected as
/// an empty, invented list.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageInterruptionRow {
    /// Model identified by the provider's usage evidence, when present.
    pub affected_model_id: Option<String>,
    /// Alternatives decoded from the durable serialized column.
    pub alternatives: DecodedUsageInterruptionAlternatives,
    /// Whether the interruption may resume automatically.
    pub auto_continue: bool,
    /// Time at which the interruption was cancelled, when present.
    pub cancelled_at: Option<String>,
    /// Command that claimed continuation, when present.
    pub continuation_command_id: Option<String>,
    /// Time at which the target run continued, when present.
    pub continued_at: Option<String>,
    /// Durable creation timestamp.
    pub created_at: String,
    /// Time at which the target run failed, when present.
    pub failed_at: Option<String>,
    /// Stable interruption identity.
    pub interruption_id: String,
    /// Provider allowance identity, when present.
    pub limit_id: Option<String>,
    /// Provider allowance label, when present.
    pub limit_label: Option<String>,
    /// Provider allowance scope.
    pub limit_scope: String,
    /// Provider error code, when present.
    pub provider_code: Option<String>,
    /// Provider reset timestamp, when present.
    pub resets_at: Option<String>,
    /// Earliest permitted continuation timestamp, when present.
    pub resume_not_before: Option<String>,
    /// Optimistic-concurrency revision.
    pub revision: u64,
    /// Agent that owns the interrupted source run.
    pub source_agent_id: String,
    /// Engine that owns the interrupted source run.
    pub source_engine_id: String,
    /// Model used by the interrupted source run, when present.
    pub source_model_id: Option<String>,
    /// Interrupted source-run identity.
    pub source_run_id: String,
    /// Durable lifecycle state.
    pub state: String,
    /// Engine selected for continuation, when present.
    pub target_engine_id: Option<String>,
    /// Model selected for continuation, when present.
    pub target_model_id: Option<String>,
    /// Target continuation-run identity, when present.
    pub target_run_id: Option<String>,
    /// Thread that owns the interruption.
    pub thread_id: String,
    /// Durable last-updated timestamp.
    pub updated_at: String,
}

/// The renderer-facing snapshot projected from one durable row.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageInterruptionSnapshot {
    /// Model identified by the provider's usage evidence, when present.
    pub affected_model_id: Option<String>,
    /// Provider-verified alternatives.
    pub alternatives: Vec<UsageInterruptionAlternative>,
    /// Whether the interruption may resume automatically.
    pub auto_continue: bool,
    /// Time at which the interruption was cancelled, when present.
    pub cancelled_at: Option<String>,
    /// Command that claimed continuation, when present.
    pub continuation_command_id: Option<String>,
    /// Time at which the target run continued, when present.
    pub continued_at: Option<String>,
    /// Durable creation timestamp.
    pub created_at: String,
    /// Time at which the target run failed, when present.
    pub failed_at: Option<String>,
    /// Stable interruption identity.
    pub interruption_id: String,
    /// Provider allowance identity, when present.
    pub limit_id: Option<String>,
    /// Provider allowance label, when present.
    pub limit_label: Option<String>,
    /// Provider allowance scope.
    pub limit_scope: String,
    /// Provider error code, when present.
    pub provider_code: Option<String>,
    /// Provider reset timestamp, when present.
    pub resets_at: Option<String>,
    /// Earliest permitted continuation timestamp, when present.
    pub resume_not_before: Option<String>,
    /// Optimistic-concurrency revision.
    pub revision: u64,
    /// Agent that owns the interrupted source run.
    pub source_agent_id: String,
    /// Engine that owns the interrupted source run.
    pub source_engine_id: String,
    /// Model used by the interrupted source run, when present.
    pub source_model_id: Option<String>,
    /// Interrupted source-run identity.
    pub source_run_id: String,
    /// Durable lifecycle state.
    pub state: String,
    /// Engine selected for continuation, when present.
    pub target_engine_id: Option<String>,
    /// Model selected for continuation, when present.
    pub target_model_id: Option<String>,
    /// Target continuation-run identity, when present.
    pub target_run_id: Option<String>,
    /// Thread that owns the interruption.
    pub thread_id: String,
    /// Durable last-updated timestamp.
    pub updated_at: String,
}

/// Compatibility alias for callers that use the protocol snapshot name.
pub type UsageInterruption = UsageInterruptionSnapshot;

/// Stateless entry point for usage-interruption row projection.
#[must_use]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UsageInterruptionModelPolicy;

impl UsageInterruptionModelPolicy {
    /// Creates the stateless projection policy.
    #[must_use = "use the constructed projection policy"]
    pub const fn new() -> Self {
        Self
    }

    /// Projects one typed durable row into its renderer-facing snapshot.
    ///
    /// The row is borrowed so a caller may evaluate it repeatedly without
    /// changing the supplied data. Every required field is copied exactly and
    /// every nullable database field remains absent when it is None.
    ///
    /// # Errors
    ///
    /// Returns `UsageInterruptionDecodeError::MalformedAlternatives` when the
    /// supplied alternative decode outcome is an error.
    #[must_use = "handle usage interruption decode failures"]
    pub fn project(
        row: &UsageInterruptionRow,
    ) -> Result<UsageInterruptionSnapshot, UsageInterruptionDecodeError> {
        decode_usage_interruption_row(row)
    }

    /// Sanitizes optional provider evidence using the policy's bounded text
    /// rules.
    #[must_use = "use the sanitized evidence text"]
    pub fn sanitise_evidence_text(value: Option<&str>) -> Option<String> {
        sanitise_usage_evidence_text(value)
    }
}

/// Projects one typed durable usage-interruption row into a public snapshot.
///
/// This is the dependency-free counterpart of the source
/// `DecodeUsageInterruptionRow` boundary. The persistence adapter supplies the
/// alternatives after attempting to decode `alternatives_json`; this function
/// never parses that serialized value itself.
///
/// # Errors
///
/// Returns `UsageInterruptionDecodeError::MalformedAlternatives` when the
/// supplied alternative decode outcome is an error.
#[must_use = "handle usage interruption decode failures"]
pub fn decode_usage_interruption_row(
    row: &UsageInterruptionRow,
) -> Result<UsageInterruptionSnapshot, UsageInterruptionDecodeError> {
    let alternatives = row.alternatives.clone()?;

    Ok(UsageInterruptionSnapshot {
        affected_model_id: row.affected_model_id.clone(),
        alternatives,
        auto_continue: row.auto_continue,
        cancelled_at: row.cancelled_at.clone(),
        continuation_command_id: row.continuation_command_id.clone(),
        continued_at: row.continued_at.clone(),
        created_at: row.created_at.clone(),
        failed_at: row.failed_at.clone(),
        interruption_id: row.interruption_id.clone(),
        limit_id: row.limit_id.clone(),
        limit_label: row.limit_label.clone(),
        limit_scope: row.limit_scope.clone(),
        provider_code: row.provider_code.clone(),
        resets_at: row.resets_at.clone(),
        resume_not_before: row.resume_not_before.clone(),
        revision: row.revision,
        source_agent_id: row.source_agent_id.clone(),
        source_engine_id: row.source_engine_id.clone(),
        source_model_id: row.source_model_id.clone(),
        source_run_id: row.source_run_id.clone(),
        state: row.state.clone(),
        target_engine_id: row.target_engine_id.clone(),
        target_model_id: row.target_model_id.clone(),
        target_run_id: row.target_run_id.clone(),
        thread_id: row.thread_id.clone(),
        updated_at: row.updated_at.clone(),
    })
}

/// Keeps optional provider evidence bounded, printable, and free of raw
/// diagnostics.
///
/// Every C0 control scalar (U+0000 through U+001F) and DEL (U+007F)
/// becomes one ordinary space. The result is then trimmed and bounded to the
/// first `MAX_USAGE_EVIDENCE_TEXT_SCALARS` Unicode scalar values. Missing or
/// empty cleaned text is returned as None.
#[must_use = "use the sanitized evidence text"]
pub fn sanitise_usage_evidence_text(value: Option<&str>) -> Option<String> {
    let value = value?;
    let cleaned = value
        .chars()
        .map(|character| {
            if character <= '\u{001F}' || character == '\u{007F}' {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let trimmed = cleaned.trim();
    let bounded = trimmed
        .chars()
        .take(MAX_USAGE_EVIDENCE_TEXT_SCALARS)
        .collect::<String>();

    (!bounded.is_empty()).then_some(bounded)
}

/// US-spelling alias for `sanitise_usage_evidence_text`.
#[must_use = "use the sanitized evidence text"]
pub fn sanitize_usage_evidence_text(value: Option<&str>) -> Option<String> {
    sanitise_usage_evidence_text(value)
}
