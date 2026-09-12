//! Artisan error codes, engine error references, provider-native actions,
//! and process/protocol diagnostics.

use crate::identifiers::IdentifierError;

use super::{
    OBSERVATION_ARTISAN_CODE_MAX_BYTES, OBSERVATION_LABEL_MAX_BYTES,
    OBSERVATION_LIMIT_LABEL_MAX_BYTES, OBSERVATION_PROVIDER_CODE_MAX_BYTES,
    OBSERVATION_REASON_MAX_BYTES, OBSERVATION_TEXT_MAX_BYTES, OBSERVATION_TIMESTAMP_MAX_BYTES,
    ObservationError, ObservationId, ObservationSequence, check_nonempty_text, check_optional_text,
};

// ---------------------------------------------------------------------------
// Native action + diagnostics + error reference
// ---------------------------------------------------------------------------

/// Stable `AE-*` artisan error code.
///
/// The adapter translates the provider's typed signal into this vocabulary at
/// the boundary; the provider's own code rides along as evidence in
/// [`EngineErrorRef`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArtisanCode(String);

impl ArtisanCode {
    /// Creates an artisan code after validating the `AE-*` shape.
    ///
    /// The value must start with `AE-`, carry a non-empty suffix of ASCII
    /// uppercase letters, digits, or hyphens, and fit
    /// [`OBSERVATION_ARTISAN_CODE_MAX_BYTES`] bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] when the value is not a
    /// modeled `AE-*` code.
    pub fn parse(value: impl Into<String>) -> Result<Self, ObservationError> {
        const FIELD: &str = "artisan_code";
        let value = value.into();
        let valid = value.len() <= OBSERVATION_ARTISAN_CODE_MAX_BYTES
            && value.starts_with("AE-")
            && value.len() > 3
            && value
                .bytes()
                .skip(3)
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(ObservationError::UnknownValue { field: FIELD })
        }
    }

    /// Returns the validated code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Scope of a depleted provider allowance, when disclosed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LimitScope {
    /// The depleted allowance is shared.
    Shared,
    /// The depleted allowance is model-specific.
    Model,
    /// The provider did not disclose the scope.
    Unknown,
}

impl LimitScope {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Model => "model",
            Self::Unknown => "unknown",
        }
    }

    /// Parses a provider-disclosed limit scope.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "shared" => Ok(Self::Shared),
            "model" => Ok(Self::Model),
            "unknown" => Ok(Self::Unknown),
            _ => Err(ObservationError::UnknownValue {
                field: "limit_scope",
            }),
        }
    }
}

/// Validated values used to construct one engine error reference.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineErrorRefInput {
    /// Stable `AE-*` artisan code.
    pub artisan_code: ArtisanCode,
    /// Provider's own error code, when disclosed.
    pub provider_code: Option<String>,
    /// Renderer-safe explanation, when supplied.
    pub detail: Option<String>,
    /// Provider model whose allowance was depleted, when disclosed.
    pub affected_model_id: Option<String>,
    /// Provider quota-bucket identifier, when disclosed.
    pub limit_id: Option<String>,
    /// Provider quota-bucket label, when disclosed.
    pub limit_label: Option<String>,
    /// Whether the depleted allowance is shared, model-specific, or unknown.
    pub limit_scope: Option<LimitScope>,
    /// When a limit-class failure clears, as an ISO timestamp, when disclosed.
    pub resets_at: Option<String>,
}

/// One provider failure transferred into Artisan's custody.
///
/// Everything downstream reasons in the `AE-*` vocabulary while the
/// provider's own code and message ride along as evidence.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineErrorRef {
    artisan_code: ArtisanCode,
    provider_code: Option<String>,
    detail: Option<String>,
    affected_model_id: Option<String>,
    limit_id: Option<String>,
    limit_label: Option<String>,
    limit_scope: Option<LimitScope>,
    resets_at: Option<String>,
}

impl EngineErrorRef {
    /// Creates an error reference after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a present evidence value is empty or
    /// exceeds its ceiling.
    pub fn new(input: EngineErrorRefInput) -> Result<Self, ObservationError> {
        check_optional_text(
            input.provider_code.as_deref(),
            "provider_code",
            OBSERVATION_PROVIDER_CODE_MAX_BYTES,
        )?;
        check_optional_text(
            input.detail.as_deref(),
            "detail",
            OBSERVATION_REASON_MAX_BYTES,
        )?;
        check_optional_text(
            input.affected_model_id.as_deref(),
            "affected_model_id",
            OBSERVATION_PROVIDER_CODE_MAX_BYTES,
        )?;
        check_optional_text(
            input.limit_id.as_deref(),
            "limit_id",
            OBSERVATION_PROVIDER_CODE_MAX_BYTES,
        )?;
        check_optional_text(
            input.limit_label.as_deref(),
            "limit_label",
            OBSERVATION_LIMIT_LABEL_MAX_BYTES,
        )?;
        if let Some(resets_at) = input.resets_at.as_deref() {
            if resets_at.is_empty() {
                return Err(ObservationError::Empty { field: "resets_at" });
            }
            if resets_at.len() > OBSERVATION_TIMESTAMP_MAX_BYTES {
                return Err(ObservationError::TooLong {
                    field: "resets_at",
                    length: resets_at.len(),
                    maximum: OBSERVATION_TIMESTAMP_MAX_BYTES,
                });
            }
            if resets_at
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
            {
                return Err(ObservationError::Identifier(
                    IdentifierError::ForbiddenCharacter {
                        character: resets_at
                            .chars()
                            .find(|character| character.is_whitespace() || character.is_control())
                            .unwrap_or('\0'),
                    },
                ));
            }
        }
        Ok(Self {
            artisan_code: input.artisan_code,
            provider_code: input.provider_code,
            detail: input.detail,
            affected_model_id: input.affected_model_id,
            limit_id: input.limit_id,
            limit_label: input.limit_label,
            limit_scope: input.limit_scope,
            resets_at: input.resets_at,
        })
    }

    /// Returns the stable `AE-*` artisan code.
    #[must_use]
    pub const fn artisan_code(&self) -> &ArtisanCode {
        &self.artisan_code
    }

    /// Returns the provider's own error code, when disclosed.
    #[must_use]
    pub fn provider_code(&self) -> Option<&str> {
        self.provider_code.as_deref()
    }

    /// Returns the renderer-safe explanation, when supplied.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Returns the affected provider model, when disclosed.
    #[must_use]
    pub fn affected_model_id(&self) -> Option<&str> {
        self.affected_model_id.as_deref()
    }

    /// Returns the provider quota-bucket identifier, when disclosed.
    #[must_use]
    pub fn limit_id(&self) -> Option<&str> {
        self.limit_id.as_deref()
    }

    /// Returns the provider quota-bucket label, when disclosed.
    #[must_use]
    pub fn limit_label(&self) -> Option<&str> {
        self.limit_label.as_deref()
    }

    /// Returns the allowance scope, when disclosed.
    #[must_use]
    pub const fn limit_scope(&self) -> Option<LimitScope> {
        self.limit_scope
    }

    /// Returns when a limit-class failure clears, when disclosed.
    #[must_use]
    pub fn resets_at(&self) -> Option<&str> {
        self.resets_at.as_deref()
    }
}

/// One provider-native action with no canonical tool equivalent.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeActionObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    action: String,
    detail: Option<String>,
    diagnostic: bool,
    error_ref: Option<EngineErrorRef>,
}

impl NativeActionObservation {
    /// Creates a native action observation after validating bounds.
    ///
    /// A `diagnostic` action marks a frame the adapter could not interpret
    /// rather than something the provider did; it is ordinary drift, not a
    /// fault.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the action is
    /// empty or exceeds [`OBSERVATION_LABEL_MAX_BYTES`] bytes, or a present
    /// detail is empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        action: String,
        detail: Option<String>,
        diagnostic: bool,
        error_ref: Option<EngineErrorRef>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&action, "action", OBSERVATION_LABEL_MAX_BYTES)?;
        check_optional_text(detail.as_deref(), "detail", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            action,
            detail,
            diagnostic,
            error_ref,
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

    /// Returns the provider-native action name.
    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }

    /// Returns the optional provider detail.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Returns whether this row marks uninterpretable drift rather than
    /// provider activity.
    #[must_use]
    pub const fn diagnostic(&self) -> bool {
        self.diagnostic
    }

    /// Returns the classified failure, when the action reported one.
    #[must_use]
    pub const fn error_ref(&self) -> Option<&EngineErrorRef> {
        self.error_ref.as_ref()
    }
}

/// Severity of one process or protocol diagnostic.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticLevel {
    /// Informational diagnostic.
    Info,
    /// Warning diagnostic.
    Warning,
    /// Error diagnostic.
    Error,
}

impl DiagnosticLevel {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }

    /// Parses a provider-disclosed diagnostic level.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "info" => Ok(Self::Info),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            _ => Err(ObservationError::UnknownValue { field: "level" }),
        }
    }
}

/// One process-level diagnostic from the engine host.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessDiagnosticObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    level: DiagnosticLevel,
    message: String,
    error_ref: Option<EngineErrorRef>,
}

impl ProcessDiagnosticObservation {
    /// Creates a process diagnostic after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the message
    /// is empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        level: DiagnosticLevel,
        message: String,
        error_ref: Option<EngineErrorRef>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&message, "message", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            level,
            message,
            error_ref,
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

    /// Returns the diagnostic severity.
    #[must_use]
    pub const fn level(&self) -> DiagnosticLevel {
        self.level
    }

    /// Returns the diagnostic message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the classified failure, when the diagnostic reported one.
    #[must_use]
    pub const fn error_ref(&self) -> Option<&EngineErrorRef> {
        self.error_ref.as_ref()
    }
}

/// One decoded transport or protocol diagnostic.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProtocolDiagnosticObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    level: DiagnosticLevel,
    message: String,
}

impl ProtocolDiagnosticObservation {
    /// Creates a protocol diagnostic after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the message
    /// is empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        level: DiagnosticLevel,
        message: String,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&message, "message", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            level,
            message,
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

    /// Returns the diagnostic severity.
    #[must_use]
    pub const fn level(&self) -> DiagnosticLevel {
        self.level
    }

    /// Returns the diagnostic message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}
