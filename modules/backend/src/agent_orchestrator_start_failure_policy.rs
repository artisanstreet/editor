//! Dependency-free classification of native engine startup failures.
//!
//! The orchestration boundary owns the effect cause and its pretty diagnostic.
//! This leaf accepts only the observations needed by the TypeScript startup
//! policy and returns an owned, provider-neutral result. It does not inspect an
//! Effect cause, contact an engine, log, or publish a failure.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::fmt;

/// Maximum number of ASCII characters accepted in a tagged failure name.
pub const MAX_TAG_LENGTH: usize = 64;

/// Maximum number of JavaScript UTF-16 code units retained in failure detail.
pub const MAX_DETAIL_UTF16_CODE_UNITS: usize = 256;

/// Artisan code used when an adapter identifies an unavailable engine.
pub const ENGINE_UNAVAILABLE_ARTISAN_CODE: &str = "AE-CLIENT_STATE-104";

/// Artisan code used for an otherwise unclassified startup failure.
pub const ENGINE_START_FAILED_ARTISAN_CODE: &str = "AE-CLIENT_STATE-105";

/// The tag that receives the unavailable-engine fallback code.
pub const ENGINE_UNAVAILABLE_TAG: &str = "EngineUnavailableError";

/// Exact wording for an interruption-only startup cause.
pub const INTERRUPTED_START_FAILURE_MESSAGE: &str =
    "Engine startup was interrupted before the native session became ready.";

/// Exact wording for an unclassified startup cause.
pub const GENERIC_START_FAILURE_MESSAGE: &str =
    "Engine startup failed before the native session became ready.";

const START_FAILURE_MESSAGE_PREFIX: &str =
    "Engine startup failed before the native session became ready";

/// The four startup-failure kinds understood by the orchestration contract.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StartFailureKind {
    /// The command or startup configuration was rejected before opening.
    Configuration,
    /// The engine produced a non-interrupt failure while opening.
    EngineError,
    /// Startup ended because the surrounding operation was interrupted.
    Interrupted,
    /// Startup exceeded its caller-owned deadline.
    Timeout,
}

impl StartFailureKind {
    /// Every contract kind in its durable vocabulary order.
    pub const ALL: [Self; 4] = [
        Self::Configuration,
        Self::EngineError,
        Self::Interrupted,
        Self::Timeout,
    ];

    /// Returns the exact TypeScript union spelling for this kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::EngineError => "engine_error",
            Self::Interrupted => "interrupted",
            Self::Timeout => "timeout",
        }
    }
}

impl fmt::Display for StartFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The typed fields that an Effect error may expose to the startup policy.
///
/// `None` represents a missing or non-string JavaScript property. The policy
/// intentionally does not validate `artisan_code`: adapter classification is
/// handed over verbatim, including an empty string.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaggedFailureObservation {
    /// The candidate value of the JavaScript `_tag` property.
    pub tag: String,
    /// The candidate value of the JavaScript `message` property.
    pub message: Option<String>,
    /// The candidate value of the JavaScript `artisan_code` property.
    pub artisan_code: Option<String>,
}

impl TaggedFailureObservation {
    /// Creates an observation while preserving every supplied field.
    pub fn new(
        tag: impl Into<String>,
        message: Option<String>,
        artisan_code: Option<String>,
    ) -> Self {
        Self {
            tag: tag.into(),
            message,
            artisan_code,
        }
    }

    /// Creates an observation with no optional JavaScript properties.
    pub fn tag_only(tag: impl Into<String>) -> Self {
        Self::new(tag, None, None)
    }

    /// Adds the observed message while preserving the tag and adapter code.
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Adds the observed adapter code verbatim.
    pub fn with_artisan_code(mut self, artisan_code: impl Into<String>) -> Self {
        self.artisan_code = Some(artisan_code.into());
        self
    }
}

/// The pure input snapshot needed to classify one startup cause.
///
/// `diagnostic` is the caller's already-rendered equivalent of
/// `Cause.pretty(cause)`. A caller may retain a tagged failure even when
/// `interrupts_only` is true; the classifier deliberately applies the
/// interruption result first, making that precedence explicit and testable.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartFailureCauseObservation {
    /// Whether the original cause contained interrupts and nothing else.
    pub interrupts_only: bool,
    /// Private diagnostic text retained for every non-interrupt result.
    pub diagnostic: String,
    /// The first typed error observation, if one was available.
    pub tagged_failure: Option<TaggedFailureObservation>,
}

impl StartFailureCauseObservation {
    /// Creates a complete cause observation without rewriting any input.
    pub fn new(
        interrupts_only: bool,
        diagnostic: impl Into<String>,
        tagged_failure: Option<TaggedFailureObservation>,
    ) -> Self {
        Self {
            interrupts_only,
            diagnostic: diagnostic.into(),
            tagged_failure,
        }
    }

    /// Creates an interruption-only observation.
    pub fn interrupted() -> Self {
        Self::new(true, String::new(), None)
    }

    /// Creates a non-interrupt observation with an optional tagged failure.
    pub fn failed(
        diagnostic: impl Into<String>,
        tagged_failure: Option<TaggedFailureObservation>,
    ) -> Self {
        Self::new(false, diagnostic, tagged_failure)
    }
}

impl From<&StartFailureCauseObservation> for StartFailureCauseObservation {
    fn from(observation: &StartFailureCauseObservation) -> Self {
        observation.clone()
    }
}

/// A valid tagged failure after its JavaScript-visible fields were classified.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaggedFailureClassification {
    /// The original valid tag spelling, retained as provider code.
    pub name: String,
    /// The normalized, bounded message detail, when nonempty.
    pub detail: Option<String>,
    /// The adapter's own Artisan code, if it supplied a string.
    pub artisan_code: Option<String>,
}

/// The subset of an engine error reference emitted by startup classification.
///
/// The optional fields mirror the source `EngineErrorRef` fields used by this
/// boundary. No provider-specific type or engine implementation is required.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineErrorRef {
    /// Stable Artisan classification, either adapter-supplied or a fallback.
    pub artisan_code: String,
    /// Bounded provider detail retained for private diagnostics/projection.
    pub detail: Option<String>,
    /// Valid tagged failure name, when the cause was classified.
    pub provider_code: Option<String>,
}

/// The provider-neutral result of one startup-failure classification.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartFailure {
    /// The pretty cause retained for non-interrupt failures only.
    pub diagnostic: Option<String>,
    /// Artisan-owned error identity, absent for interruption-only startup.
    pub error_ref: Option<EngineErrorRef>,
    /// The durable startup-failure kind.
    pub kind: StartFailureKind,
    /// Exact user-facing startup wording.
    pub message: String,
}

impl StartFailure {
    /// Returns whether this result represents an interruption-only cause.
    #[must_use]
    pub const fn is_interrupted(&self) -> bool {
        matches!(self.kind, StartFailureKind::Interrupted)
    }
}

/// Classifies one typed failure observation using the TypeScript tag grammar.
///
/// A tag must match `^[A-Za-z][A-Za-z0-9_.-]{0,63}$`. Message whitespace is
/// collapsed using ECMAScript's `\s` set, trimmed, and then bounded by UTF-16
/// code units. Rust cannot represent an unpaired surrogate, so a supplementary
/// scalar at the cutoff is kept whole rather than producing replacement text.
#[must_use]
pub fn classify_tagged_failure(
    failure: &TaggedFailureObservation,
) -> Option<TaggedFailureClassification> {
    if !is_valid_tag(&failure.tag) {
        return None;
    }

    let detail = failure
        .message
        .as_deref()
        .map(normalize_message_detail)
        .filter(|detail| !detail.is_empty());

    Some(TaggedFailureClassification {
        name: failure.tag.clone(),
        detail,
        artisan_code: failure.artisan_code.clone(),
    })
}

/// Classifies one pure startup-cause observation.
///
/// Interruption-only precedence is evaluated before tag inspection. Every
/// non-interrupt result carries the supplied diagnostic and an error reference;
/// an invalid or absent tag simply receives the generic startup code/message.
#[must_use = "handle the startup-failure result"]
pub fn start_failure_from_cause(
    observation: impl Into<StartFailureCauseObservation>,
) -> StartFailure {
    let observation = observation.into();

    if observation.interrupts_only {
        return StartFailure {
            diagnostic: None,
            error_ref: None,
            kind: StartFailureKind::Interrupted,
            message: INTERRUPTED_START_FAILURE_MESSAGE.to_owned(),
        };
    }

    let classification = observation
        .tagged_failure
        .as_ref()
        .and_then(classify_tagged_failure);

    let (artisan_code, detail, provider_code, message) = match classification.as_ref() {
        None => (
            ENGINE_START_FAILED_ARTISAN_CODE,
            None,
            None,
            GENERIC_START_FAILURE_MESSAGE.to_owned(),
        ),
        Some(failure) => {
            let artisan_code = failure.artisan_code.as_deref().unwrap_or_else(|| {
                if failure.name == ENGINE_UNAVAILABLE_TAG {
                    ENGINE_UNAVAILABLE_ARTISAN_CODE
                } else {
                    ENGINE_START_FAILED_ARTISAN_CODE
                }
            });
            let detail = failure.detail.clone();
            let message = match detail.as_deref() {
                Some(detail) => format!(
                    "{START_FAILURE_MESSAGE_PREFIX} ({}: {detail}).",
                    failure.name
                ),
                None => format!("{START_FAILURE_MESSAGE_PREFIX} ({}).", failure.name),
            };

            (artisan_code, detail, Some(failure.name.clone()), message)
        }
    };

    StartFailure {
        diagnostic: Some(observation.diagnostic),
        error_ref: Some(EngineErrorRef {
            artisan_code: artisan_code.to_owned(),
            detail,
            provider_code,
        }),
        kind: StartFailureKind::EngineError,
        message,
    }
}

fn is_valid_tag(tag: &str) -> bool {
    if tag.is_empty() || tag.len() > MAX_TAG_LENGTH {
        return false;
    }

    let mut bytes = tag.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }

    bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn normalize_message_detail(message: &str) -> String {
    let mut collapsed = String::with_capacity(message.len());
    let mut pending_space = false;

    for character in message.chars() {
        if is_javascript_whitespace(character) {
            pending_space = true;
            continue;
        }

        if pending_space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push(character);
        pending_space = false;
    }

    truncate_utf16_scalars(&collapsed, MAX_DETAIL_UTF16_CODE_UNITS)
}

fn truncate_utf16_scalars(value: &str, maximum: usize) -> String {
    let mut bounded = String::with_capacity(value.len().min(maximum));
    let mut used = 0_usize;

    for character in value.chars() {
        let units = character.len_utf16();
        if units > maximum.saturating_sub(used) {
            break;
        }
        bounded.push(character);
        used += units;
    }

    bounded
}

fn is_javascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000a}'
            | '\u{000b}'
            | '\u{000c}'
            | '\u{000d}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'
            | '\u{2001}'
            | '\u{2002}'
            | '\u{2003}'
            | '\u{2004}'
            | '\u{2005}'
            | '\u{2006}'
            | '\u{2007}'
            | '\u{2008}'
            | '\u{2009}'
            | '\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}
