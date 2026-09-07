//! Dependency-free provider-neutral intake assessment policy.
//!
//! The current local policy deliberately does not try to interpret request
//! prose.  It preserves the durable assessment vocabulary and validation
//! boundary while every local assessment remains low risk and proceeds.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{fmt, str::FromStr};

/// Risk classifications understood by the orchestration intake contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntakeRisk {
    /// The request has no assessed material risk.
    Low,
    /// The request may have a meaningful but non-high risk.
    Material,
    /// The request has assessed high risk.
    High,
    /// The request is not sufficiently specified for a more precise risk.
    Underspecified,
}

impl IntakeRisk {
    /// All risk variants in their durable vocabulary order.
    pub const ALL: [Self; 4] = [Self::Low, Self::Material, Self::High, Self::Underspecified];

    /// Returns the exact wire spelling of this risk classification.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Material => "material",
            Self::High => "high",
            Self::Underspecified => "underspecified",
        }
    }

    /// Parses one exact risk spelling from the provider-neutral contract.
    ///
    /// Matching is intentionally case-sensitive and does not trim or
    /// otherwise rewrite externally supplied values.
    ///
    /// # Errors
    ///
    /// Returns [`IntakeRiskParseError`] when `value` is not one of the exact
    /// risk spellings in this contract.
    #[must_use = "handle invalid intake risk input"]
    pub fn parse(value: &str) -> Result<Self, IntakeRiskParseError> {
        match value {
            "low" => Ok(Self::Low),
            "material" => Ok(Self::Material),
            "high" => Ok(Self::High),
            "underspecified" => Ok(Self::Underspecified),
            _ => Err(IntakeRiskParseError),
        }
    }
}

impl fmt::Display for IntakeRisk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for IntakeRisk {
    type Err = IntakeRiskParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Error returned when an input is not an exact [`IntakeRisk`] spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntakeRiskParseError;

impl fmt::Display for IntakeRiskParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid intake risk")
    }
}

impl std::error::Error for IntakeRiskParseError {}

/// Resolutions understood by the orchestration intake contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntakeResolution {
    /// The request may continue without a clarification question.
    Proceed,
    /// The request must pause for a clarification question.
    Question,
}

impl IntakeResolution {
    /// All resolution variants in their durable vocabulary order.
    pub const ALL: [Self; 2] = [Self::Proceed, Self::Question];

    /// Returns the exact wire spelling of this resolution.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proceed => "proceed",
            Self::Question => "question",
        }
    }

    /// Parses one exact resolution spelling from the provider-neutral
    /// contract.
    ///
    /// Matching is intentionally case-sensitive and does not trim or
    /// otherwise rewrite externally supplied values.
    ///
    /// # Errors
    ///
    /// Returns [`IntakeResolutionParseError`] when `value` is not one of the
    /// exact resolution spellings in this contract.
    #[must_use = "handle invalid intake resolution input"]
    pub fn parse(value: &str) -> Result<Self, IntakeResolutionParseError> {
        match value {
            "proceed" => Ok(Self::Proceed),
            "question" => Ok(Self::Question),
            _ => Err(IntakeResolutionParseError),
        }
    }
}

impl fmt::Display for IntakeResolution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for IntakeResolution {
    type Err = IntakeResolutionParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Error returned when an input is not an exact [`IntakeResolution`]
/// spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntakeResolutionParseError;

impl fmt::Display for IntakeResolutionParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid intake resolution")
    }
}

impl std::error::Error for IntakeResolutionParseError {}

/// Validation failures for externally supplied intake assessments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntakeAssessmentValidationError {
    /// An assumption at this zero-based position was empty.
    EmptyAssumption { index: usize },
    /// A question resolution did not supply a question.
    QuestionRequired,
    /// A supplied question was empty.
    EmptyQuestion,
    /// A proceed resolution retained a question.
    QuestionNotAllowed,
}

impl fmt::Display for IntakeAssessmentValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyAssumption { index } => {
                write!(formatter, "assumption at index {index} must not be empty")
            }
            Self::QuestionRequired => {
                formatter.write_str("question resolution requires a non-empty question")
            }
            Self::EmptyQuestion => formatter.write_str("question must not be empty"),
            Self::QuestionNotAllowed => {
                formatter.write_str("proceed resolution cannot retain a question")
            }
        }
    }
}

impl std::error::Error for IntakeAssessmentValidationError {}

/// A provider-neutral intake assessment.
///
/// The fields mirror the reached TypeScript contract. Values created through
/// [`Self::try_new`] are validated. Because the fields are public for direct
/// transport/projection use, callers receiving an externally assembled value
/// should call [`Self::validate`] before accepting it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntakeAssessment {
    /// The assessed risk vocabulary value.
    pub risk: IntakeRisk,
    /// Whether execution proceeds or waits for a question.
    pub resolution: IntakeResolution,
    /// Ordered assumptions; every entry must be nonempty.
    pub assumptions: Vec<String>,
    /// The clarification question, present only for [`IntakeResolution::Question`].
    pub question: Option<String>,
}

impl Default for IntakeAssessment {
    fn default() -> Self {
        Self {
            risk: IntakeRisk::Low,
            resolution: IntakeResolution::Proceed,
            assumptions: Vec::new(),
            question: None,
        }
    }
}

impl IntakeAssessment {
    /// Constructs and validates an externally supplied assessment.
    ///
    /// Assumption order and string contents are preserved exactly. In
    /// particular, an empty assumption is rejected rather than discarded or
    /// rewritten.
    ///
    /// # Errors
    ///
    /// Returns [`IntakeAssessmentValidationError::EmptyAssumption`] for an
    /// empty assumption, [`IntakeAssessmentValidationError::QuestionRequired`]
    /// when a question resolution has no question,
    /// [`IntakeAssessmentValidationError::EmptyQuestion`] for an empty
    /// question, or [`IntakeAssessmentValidationError::QuestionNotAllowed`]
    /// when a proceed resolution retains a question.
    #[must_use = "handle invalid intake assessment input"]
    pub fn try_new(
        risk: IntakeRisk,
        resolution: IntakeResolution,
        assumptions: Vec<String>,
        question: Option<String>,
    ) -> Result<Self, IntakeAssessmentValidationError> {
        let assessment = Self {
            risk,
            resolution,
            assumptions,
            question,
        };
        assessment.validate()?;
        Ok(assessment)
    }

    /// Constructs a validated proceed assessment without a question.
    ///
    /// # Errors
    ///
    /// Returns [`IntakeAssessmentValidationError::EmptyAssumption`] when an
    /// assumption is empty.
    #[must_use = "handle invalid proceed assessment input"]
    pub fn proceed(
        risk: IntakeRisk,
        assumptions: Vec<String>,
    ) -> Result<Self, IntakeAssessmentValidationError> {
        Self::try_new(risk, IntakeResolution::Proceed, assumptions, None)
    }

    /// Constructs a validated question assessment.
    ///
    /// # Errors
    ///
    /// Returns [`IntakeAssessmentValidationError::EmptyAssumption`] when an
    /// assumption is empty or [`IntakeAssessmentValidationError::EmptyQuestion`]
    /// when `question` is empty.
    #[must_use = "handle invalid question assessment input"]
    pub fn question(
        risk: IntakeRisk,
        assumptions: Vec<String>,
        question: String,
    ) -> Result<Self, IntakeAssessmentValidationError> {
        Self::try_new(
            risk,
            IntakeResolution::Question,
            assumptions,
            Some(question),
        )
    }

    /// Validates this assessment without changing it.
    ///
    /// # Errors
    ///
    /// Returns the first encountered empty assumption or question/resolution
    /// invariant failure. The assessment and all of its strings remain
    /// untouched on every result.
    #[must_use = "handle an invalid intake assessment"]
    pub fn validate(&self) -> Result<(), IntakeAssessmentValidationError> {
        for (index, assumption) in self.assumptions.iter().enumerate() {
            if assumption.is_empty() {
                return Err(IntakeAssessmentValidationError::EmptyAssumption { index });
            }
        }

        match self.resolution {
            IntakeResolution::Proceed if self.question.is_some() => {
                Err(IntakeAssessmentValidationError::QuestionNotAllowed)
            }
            IntakeResolution::Question => match self.question.as_deref() {
                None => Err(IntakeAssessmentValidationError::QuestionRequired),
                Some("") => Err(IntakeAssessmentValidationError::EmptyQuestion),
                Some(_) => Ok(()),
            },
            IntakeResolution::Proceed => Ok(()),
        }
    }

    /// Returns this assessment's risk classification.
    #[must_use]
    pub const fn risk(&self) -> IntakeRisk {
        self.risk
    }

    /// Returns this assessment's resolution.
    #[must_use]
    pub const fn resolution(&self) -> IntakeResolution {
        self.resolution
    }

    /// Returns ordered assumptions without copying them.
    #[must_use]
    pub fn assumptions(&self) -> &[String] {
        &self.assumptions
    }

    /// Returns the optional question without copying it.
    #[must_use]
    pub fn question_text(&self) -> Option<&str> {
        self.question.as_deref()
    }
}

/// Validates a supplied assessment at an explicit policy boundary.
///
/// This facade is useful when a caller receives an assessment from a
/// transport or persistence adapter and wants to make the validation step
/// visible without constructing a second value.
///
/// # Errors
///
/// Returns the first validation failure reported by [`IntakeAssessment::validate`].
#[must_use = "handle an invalid intake assessment"]
pub fn validate_intake_assessment(
    assessment: &IntakeAssessment,
) -> Result<(), IntakeAssessmentValidationError> {
    assessment.validate()
}

/// Stateless local policy for evaluating intake text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IntakePolicy;

impl IntakePolicy {
    /// Creates the stateless intake policy.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Assesses text using the current local policy.
    ///
    /// The text is deliberately ignored: empty input, ordinary requests, and
    /// alarming prose all proceed as low risk with no assumptions and no
    /// question. Keyword sniffing and semantic risk claims do not belong in
    /// this provider-neutral local policy.
    #[must_use]
    pub fn assess(text: &str) -> IntakeAssessment {
        let _ = text;
        IntakeAssessment::default()
    }
}

/// Assesses text using the current stateless local intake policy.
#[must_use]
pub fn assess(text: &str) -> IntakeAssessment {
    IntakePolicy::assess(text)
}
