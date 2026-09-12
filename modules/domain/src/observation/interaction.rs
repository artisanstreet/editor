//! Approval and question interaction observations.

use super::{
    OBSERVATION_ANSWER_MAX_BYTES, OBSERVATION_ANSWERS_MAX, OBSERVATION_COMMAND_MAX_BYTES,
    OBSERVATION_DESCRIPTION_MAX_BYTES, OBSERVATION_LABEL_MAX_BYTES, OBSERVATION_PATH_MAX_BYTES,
    OBSERVATION_QUESTION_MAX_OPTIONS, OBSERVATION_REASON_MAX_BYTES, OBSERVATION_TEXT_MAX_BYTES,
    ObservationError, ObservationId, ObservationSequence, check_nonempty_text, check_optional_text,
};

// ---------------------------------------------------------------------------
// Approval / question (per the S0 audit shapes)
// ---------------------------------------------------------------------------

/// Lifecycle state of one approval observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApprovalState {
    /// The approval was requested and awaits a decision.
    Requested,
    /// The approval was decided.
    Resolved,
}

impl ApprovalState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Resolved => "resolved",
        }
    }

    /// Parses a provider-disclosed approval state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "requested" => Ok(Self::Requested),
            "resolved" => Ok(Self::Resolved),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// Provider-neutral action bound to one approval request.
///
/// Mirrors TypeScript `EngineApprovalRequest`: a command approval carries the
/// command text under review, while file-change and generic action approvals
/// carry only an optional reason.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ApprovalRequest {
    kind: ApprovalKind,
    command: Option<String>,
    cwd: Option<String>,
    reason: Option<String>,
}

/// Kind of action bound to one approval request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApprovalKind {
    /// A shell command awaiting approval.
    Command,
    /// A file change awaiting approval.
    FileChange,
    /// A generic provider action awaiting approval.
    Action,
}

impl ApprovalKind {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::FileChange => "file_change",
            Self::Action => "action",
        }
    }

    /// Parses a provider-disclosed approval kind.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "command" => Ok(Self::Command),
            "file_change" => Ok(Self::FileChange),
            "action" => Ok(Self::Action),
            _ => Err(ObservationError::UnknownValue { field: "kind" }),
        }
    }
}

impl ApprovalRequest {
    /// Creates a command approval request after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the command is empty or exceeds
    /// [`OBSERVATION_COMMAND_MAX_BYTES`] bytes, or when a present working
    /// directory or reason value is empty or exceeds its ceiling.
    pub fn command(
        command: String,
        cwd: Option<String>,
        reason: Option<String>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&command, "command", OBSERVATION_COMMAND_MAX_BYTES)?;
        check_optional_text(cwd.as_deref(), "cwd", OBSERVATION_PATH_MAX_BYTES)?;
        check_optional_text(reason.as_deref(), "reason", OBSERVATION_REASON_MAX_BYTES)?;
        Ok(Self {
            kind: ApprovalKind::Command,
            command: Some(command),
            cwd,
            reason,
        })
    }

    /// Creates a file-change approval request after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a present reason is empty or exceeds
    /// [`OBSERVATION_REASON_MAX_BYTES`] bytes.
    pub fn file_change(reason: Option<String>) -> Result<Self, ObservationError> {
        check_optional_text(reason.as_deref(), "reason", OBSERVATION_REASON_MAX_BYTES)?;
        Ok(Self {
            kind: ApprovalKind::FileChange,
            command: None,
            cwd: None,
            reason,
        })
    }

    /// Creates a generic action approval request after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a present reason is empty or exceeds
    /// [`OBSERVATION_REASON_MAX_BYTES`] bytes.
    pub fn action(reason: Option<String>) -> Result<Self, ObservationError> {
        check_optional_text(reason.as_deref(), "reason", OBSERVATION_REASON_MAX_BYTES)?;
        Ok(Self {
            kind: ApprovalKind::Action,
            command: None,
            cwd: None,
            reason,
        })
    }

    /// Returns the approval kind.
    #[must_use]
    pub const fn kind(&self) -> ApprovalKind {
        self.kind
    }

    /// Returns the command text under review for command approvals.
    #[must_use]
    pub fn command_text(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// Returns the working directory for command approvals, when disclosed.
    #[must_use]
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// Returns the provider reason, when disclosed.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// One approval request or its resolution.
///
/// A requested observation never carries a decision; a resolved observation
/// always does. The constructor pair enforces that relationship.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ApprovalObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    approval_id: ObservationId,
    state: ApprovalState,
    description: String,
    request: ApprovalRequest,
    approved: Option<bool>,
}

impl ApprovalObservation {
    /// Creates a requested approval with no decision attached.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the
    /// description is empty or exceeds
    /// [`OBSERVATION_DESCRIPTION_MAX_BYTES`] bytes.
    pub fn requested(
        id: ObservationId,
        sequence: ObservationSequence,
        approval_id: ObservationId,
        description: String,
        request: ApprovalRequest,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(
            &description,
            "description",
            OBSERVATION_DESCRIPTION_MAX_BYTES,
        )?;
        Ok(Self {
            id,
            sequence,
            approval_id,
            state: ApprovalState::Requested,
            description,
            request,
            approved: None,
        })
    }

    /// Creates a resolved approval carrying its decision.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the
    /// description is empty or exceeds
    /// [`OBSERVATION_DESCRIPTION_MAX_BYTES`] bytes.
    pub fn resolved(
        id: ObservationId,
        sequence: ObservationSequence,
        approval_id: ObservationId,
        description: String,
        request: ApprovalRequest,
        approved: bool,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(
            &description,
            "description",
            OBSERVATION_DESCRIPTION_MAX_BYTES,
        )?;
        Ok(Self {
            id,
            sequence,
            approval_id,
            state: ApprovalState::Resolved,
            description,
            request,
            approved: Some(approved),
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

    /// Returns the provider approval identity.
    #[must_use]
    pub const fn approval_id(&self) -> &ObservationId {
        &self.approval_id
    }

    /// Returns the approval lifecycle state.
    #[must_use]
    pub const fn state(&self) -> ApprovalState {
        self.state
    }

    /// Returns the human-readable approval description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the provider-neutral action under review.
    #[must_use]
    pub const fn request(&self) -> &ApprovalRequest {
        &self.request
    }

    /// Returns the decision for resolved approvals, [`None`] while requested.
    #[must_use]
    pub const fn approved(&self) -> Option<bool> {
        self.approved
    }
}

/// Lifecycle state of one question observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum QuestionState {
    /// The question was asked and awaits answers.
    Requested,
    /// The question was answered.
    Resolved,
}

impl QuestionState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Resolved => "resolved",
        }
    }

    /// Parses a provider-disclosed question state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "requested" => Ok(Self::Requested),
            "resolved" => Ok(Self::Resolved),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// One provider-offered answer to a question.
///
/// Mirrors TypeScript `EngineQuestionOption`: the label names the choice and
/// the optional description explains what choosing it means.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QuestionOption {
    label: String,
    description: Option<String>,
}

impl QuestionOption {
    /// Creates a question option after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the label is empty or exceeds
    /// [`OBSERVATION_LABEL_MAX_BYTES`] bytes, or when a present description
    /// is empty or exceeds [`OBSERVATION_DESCRIPTION_MAX_BYTES`] bytes.
    pub fn new(label: String, description: Option<String>) -> Result<Self, ObservationError> {
        check_nonempty_text(&label, "label", OBSERVATION_LABEL_MAX_BYTES)?;
        check_optional_text(
            description.as_deref(),
            "description",
            OBSERVATION_DESCRIPTION_MAX_BYTES,
        )?;
        Ok(Self { label, description })
    }

    /// Returns the option label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns what choosing this option means, when the provider explained it.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

/// Validated values shared by a question request and its resolution.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QuestionInput {
    /// Provider question identity.
    pub question_id: ObservationId,
    /// The question itself.
    pub text: String,
    /// Short category label shown beside the question, when disclosed.
    pub header: Option<String>,
    /// Whether more than one option may be chosen at once.
    pub multi_select: bool,
    /// Offered answers; [`None`] marks a free-form question answered with
    /// typed text rather than a selection.
    pub options: Option<Vec<QuestionOption>>,
}

/// One question request or its resolution.
///
/// A requested observation never carries answers; a resolved observation
/// always carries the answer list (possibly empty for an explicitly skipped
/// question).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QuestionObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    question_id: ObservationId,
    state: QuestionState,
    text: String,
    header: Option<String>,
    multi_select: bool,
    options: Option<Vec<QuestionOption>>,
    answers: Option<Vec<String>>,
}

impl QuestionObservation {
    fn validate_input(input: &QuestionInput) -> Result<(), ObservationError> {
        check_nonempty_text(&input.text, "text", OBSERVATION_TEXT_MAX_BYTES)?;
        check_optional_text(
            input.header.as_deref(),
            "header",
            OBSERVATION_LABEL_MAX_BYTES,
        )?;
        if let Some(options) = input.options.as_deref() {
            if options.is_empty() {
                return Err(ObservationError::Empty { field: "options" });
            }
            if options.len() > OBSERVATION_QUESTION_MAX_OPTIONS {
                return Err(ObservationError::TooMany {
                    field: "options",
                    count: options.len(),
                    maximum: OBSERVATION_QUESTION_MAX_OPTIONS,
                });
            }
        }
        Ok(())
    }

    fn validate_answers(answers: &[String]) -> Result<(), ObservationError> {
        if answers.len() > OBSERVATION_ANSWERS_MAX {
            return Err(ObservationError::TooMany {
                field: "answers",
                count: answers.len(),
                maximum: OBSERVATION_ANSWERS_MAX,
            });
        }
        for answer in answers {
            check_nonempty_text(answer, "answers", OBSERVATION_ANSWER_MAX_BYTES)?;
        }
        Ok(())
    }

    /// Creates a requested question with no answers attached.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the text is
    /// empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes, the header or an
    /// option violates its ceiling, or the option list is empty or exceeds
    /// [`OBSERVATION_QUESTION_MAX_OPTIONS`] entries.
    pub fn requested(
        id: ObservationId,
        sequence: ObservationSequence,
        input: QuestionInput,
    ) -> Result<Self, ObservationError> {
        Self::validate_input(&input)?;
        Ok(Self {
            id,
            sequence,
            question_id: input.question_id,
            state: QuestionState::Requested,
            text: input.text,
            header: input.header,
            multi_select: input.multi_select,
            options: input.options,
            answers: None,
        })
    }

    /// Creates a resolved question carrying its answers.
    ///
    /// # Errors
    ///
    /// Returns the same [`ObservationError`] cases as
    /// [`QuestionObservation::requested`], plus answer bound violations when
    /// an answer is empty or exceeds [`OBSERVATION_ANSWER_MAX_BYTES`] bytes or
    /// the list exceeds [`OBSERVATION_ANSWERS_MAX`] entries.
    pub fn resolved(
        id: ObservationId,
        sequence: ObservationSequence,
        input: QuestionInput,
        answers: Vec<String>,
    ) -> Result<Self, ObservationError> {
        Self::validate_input(&input)?;
        Self::validate_answers(&answers)?;
        Ok(Self {
            id,
            sequence,
            question_id: input.question_id,
            state: QuestionState::Resolved,
            text: input.text,
            header: input.header,
            multi_select: input.multi_select,
            options: input.options,
            answers: Some(answers),
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

    /// Returns the provider question identity.
    #[must_use]
    pub const fn question_id(&self) -> &ObservationId {
        &self.question_id
    }

    /// Returns the question lifecycle state.
    #[must_use]
    pub const fn state(&self) -> QuestionState {
        self.state
    }

    /// Returns the question itself.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the short category label, when disclosed.
    #[must_use]
    pub fn header(&self) -> Option<&str> {
        self.header.as_deref()
    }

    /// Returns whether more than one option may be chosen at once.
    #[must_use]
    pub const fn multi_select(&self) -> bool {
        self.multi_select
    }

    /// Returns the offered answers, or [`None`] for a free-form question.
    #[must_use]
    pub const fn options(&self) -> Option<&Vec<QuestionOption>> {
        self.options.as_ref()
    }

    /// Returns the answers for resolved questions, [`None`] while requested.
    #[must_use]
    pub const fn answers(&self) -> Option<&Vec<String>> {
        self.answers.as_ref()
    }
}
