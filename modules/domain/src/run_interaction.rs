//! Owner-delivered answers to pending engine approval and question requests.
//!
//! The S0 audit found approval *presentation* only: no command, event, or
//! observation type carried a decision back to the owning run. This module
//! owns the finite A-approve backend vocabulary: two idempotent domain
//! commands (`RespondApproval`, `RespondQuestion`) plus the target outcome
//! reported back in their result receipts.
//!
//! Every command carries a client-minted [`RequestId`] so retries correlate
//! to a receipt instead of a second effect. Reusing a request id with a
//! different target or decision is an idempotency conflict, never a silent
//! second resolution: [`RespondApproval::intent_key`] and
//! [`RespondQuestion::intent_key`] fingerprint the exact intent for that
//! comparison.
//!
//! Target identities reuse [`ObservationId`]: an approval or question id is
//! the provider identity carried by the matching requested observation, under
//! the shared wire identifier rule. Answers reuse the observation bounds
//! ([`OBSERVATION_ANSWERS_MAX`], [`OBSERVATION_ANSWER_MAX_BYTES`]) so a
//! decision accepted here always fits the resolved observation committed
//! later. An empty answer list is valid: it records an explicitly skipped
//! question, mirroring [`QuestionObservation::resolved`](crate::QuestionObservation::resolved).
//!
//! There is deliberately no default decision. Every resolution is an explicit
//! authenticated choice carried by one of these commands; silence never
//! approves and never denies.

use thiserror::Error;

use crate::identifiers::{RequestId, RunId, ThreadId};
use crate::observation::{OBSERVATION_ANSWER_MAX_BYTES, OBSERVATION_ANSWERS_MAX, ObservationId};

/// Which pending provider request a response answers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InteractionKind {
    /// A pending approval request.
    Approval,
    /// A pending question.
    Question,
}

impl InteractionKind {
    /// Returns the stable target spelling shared with the engine errors.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Question => "question",
        }
    }

    /// Parses a stored target spelling.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError::UnknownKind`] for any other spelling.
    /// Unknown kinds never default to another kind.
    pub fn parse(value: &str) -> Result<Self, RunInteractionError> {
        match value {
            "approval" => Ok(Self::Approval),
            "question" => Ok(Self::Question),
            _ => Err(RunInteractionError::UnknownKind),
        }
    }
}

/// How Forge settled one response, reported in its result receipt.
///
/// These are per-target routing results for a well-formed, authenticated
/// request, not protocol failures: the request itself was valid, but its
/// target may be absent, already settled, or owned by another run.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InteractionOutcome {
    /// The decision was recorded and delivered to the owning run.
    Applied,
    /// No pending request carries this target id on the live run.
    UnknownTarget,
    /// The target was already resolved by an earlier response.
    AlreadyResolved,
    /// The named thread/run pair is not the live owning run.
    WrongRun,
}

impl InteractionOutcome {
    /// Returns the stable outcome spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::UnknownTarget => "unknown_target",
            Self::AlreadyResolved => "already_resolved",
            Self::WrongRun => "wrong_run",
        }
    }

    /// Parses a stored outcome spelling.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError::UnknownOutcome`] for any other
    /// spelling. Unknown outcomes never default to another outcome.
    pub fn parse(value: &str) -> Result<Self, RunInteractionError> {
        match value {
            "applied" => Ok(Self::Applied),
            "unknown_target" => Ok(Self::UnknownTarget),
            "already_resolved" => Ok(Self::AlreadyResolved),
            "wrong_run" => Ok(Self::WrongRun),
            _ => Err(RunInteractionError::UnknownOutcome),
        }
    }
}

/// Validation failure for one externally supplied interaction value.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum RunInteractionError {
    /// A target spelling is not a modeled interaction kind.
    #[error("run interaction kind is unknown")]
    UnknownKind,
    /// An outcome spelling is not a modeled interaction outcome.
    #[error("run interaction outcome is unknown")]
    UnknownOutcome,
    /// An answer list exceeded its documented entry ceiling.
    #[error("question answers hold {count} entries; the maximum is {maximum}")]
    TooManyAnswers {
        /// Offending answer count.
        count: usize,
        /// The documented entry ceiling.
        maximum: usize,
    },
    /// One answer was empty.
    #[error("question answer {index} must not be empty")]
    EmptyAnswer {
        /// Zero-based position of the rejected answer.
        index: usize,
    },
    /// One answer exceeded its documented UTF-8 byte ceiling.
    #[error("question answer {index} is {length} UTF-8 bytes; the maximum is {maximum}")]
    AnswerTooLong {
        /// Zero-based position of the rejected answer.
        index: usize,
        /// Offending UTF-8 byte length.
        length: usize,
        /// The documented ceiling in UTF-8 bytes.
        maximum: usize,
    },
    /// A questionnaire answer named no question.
    #[error("a questionnaire answer must name at least one question")]
    NoQuestions,
    /// A questionnaire answer named more questions than one group may hold.
    #[error("a questionnaire answer names {count} questions; the maximum is {maximum}")]
    TooManyQuestions {
        /// Offending question count.
        count: usize,
        /// The documented ceiling.
        maximum: usize,
    },
    /// A questionnaire answer named the same question twice.
    #[error("questionnaire answer {index} repeats an earlier question")]
    DuplicateQuestion {
        /// Zero-based position of the repeated question.
        index: usize,
    },
}

/// Answers one pending approval request with an explicit decision.
///
/// This is a live routing request authenticated by its exact thread/run
/// ownership, not a durable completion receipt. The backend answers from
/// its live run registry and the owning run settles the durable resolution
/// separately. There is no default: `approved` is always an explicit choice.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RespondApproval {
    /// Client-minted stable request identity for this response.
    pub request_id: RequestId,
    /// Thread that must own the exact target run.
    pub thread_id: ThreadId,
    /// Exact native run identity owning the pending approval.
    pub run_id: RunId,
    /// Provider approval identity under review.
    pub approval_id: ObservationId,
    /// The explicit decision: true allows, false denies.
    pub approved: bool,
}

impl RespondApproval {
    /// Creates an explicit approval response over already validated values.
    #[must_use]
    pub const fn new(
        request_id: RequestId,
        thread_id: ThreadId,
        run_id: RunId,
        approval_id: ObservationId,
        approved: bool,
    ) -> Self {
        Self {
            request_id,
            thread_id,
            run_id,
            approval_id,
            approved,
        }
    }

    /// Returns the stable request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns the authenticated owning thread.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the exact target run.
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Returns the provider approval identity under review.
    #[must_use]
    pub const fn approval_id(&self) -> &ObservationId {
        &self.approval_id
    }

    /// Returns the explicit decision.
    #[must_use]
    pub const fn approved(&self) -> bool {
        self.approved
    }

    /// Returns the interaction kind answered by this command.
    #[must_use]
    pub const fn kind(&self) -> InteractionKind {
        InteractionKind::Approval
    }

    /// Fingerprints the exact intent for idempotency-conflict comparison.
    ///
    /// Two commands share an intent only when thread, run, target, and
    /// decision all agree. A reused request id with a different fingerprint
    /// is a conflict, never a second resolution.
    #[must_use]
    pub fn intent_key(&self) -> String {
        format!(
            "respond_approval\x00{}\x00{}\x00{}\x00{}",
            self.thread_id.as_str(),
            self.run_id.as_str(),
            self.approval_id.as_str(),
            self.approved,
        )
    }
}

/// Answers one pending question with explicit answers.
///
/// An empty answer list records an explicitly skipped question. Answers are
/// validated against the observation bounds so the later resolved
/// observation always fits.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RespondQuestion {
    request_id: RequestId,
    thread_id: ThreadId,
    run_id: RunId,
    question_id: ObservationId,
    answers: Vec<String>,
}

impl RespondQuestion {
    /// Creates an explicit question response after validating answer bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] when the answer list exceeds
    /// [`OBSERVATION_ANSWERS_MAX`] entries or an answer is empty or exceeds
    /// [`OBSERVATION_ANSWER_MAX_BYTES`] UTF-8 bytes.
    pub fn new(
        request_id: RequestId,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
        answers: Vec<String>,
    ) -> Result<Self, RunInteractionError> {
        validate_answers(&answers)?;
        Ok(Self {
            request_id,
            thread_id,
            run_id,
            question_id,
            answers,
        })
    }

    /// Returns the stable request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns the authenticated owning thread.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the exact target run.
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Returns the provider question identity under review.
    #[must_use]
    pub const fn question_id(&self) -> &ObservationId {
        &self.question_id
    }

    /// Returns the explicit answers, possibly empty for a skipped question.
    #[must_use]
    pub const fn answers(&self) -> &Vec<String> {
        &self.answers
    }

    /// Returns the interaction kind answered by this command.
    #[must_use]
    pub const fn kind(&self) -> InteractionKind {
        InteractionKind::Question
    }

    /// Fingerprints the exact intent for idempotency-conflict comparison.
    ///
    /// Answers render with debug escaping so separator bytes inside an
    /// answer can never alias a different answer list.
    #[must_use]
    pub fn intent_key(&self) -> String {
        format!(
            "respond_question\x00{}\x00{}\x00{}\x00{:?}",
            self.thread_id.as_str(),
            self.run_id.as_str(),
            self.question_id.as_str(),
            self.answers,
        )
    }
}

/// Validates one answer list against the shared observation bounds.
/// Maximum questions one questionnaire (one provider question request) may
/// hold, and so the most one [`AnswerQuestions`] may answer at once.
pub const QUESTIONNAIRE_MAX_QUESTIONS: usize = 16;

/// The answers to one question inside a questionnaire.
///
/// An empty list skips that question.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QuestionAnswer {
    /// Provider question identity being answered.
    pub question_id: ObservationId,
    /// Chosen or typed answers; empty for a skipped question.
    pub answers: Vec<String>,
}

/// Answers one open questionnaire on a thread.
///
/// Unlike [`RespondQuestion`], this names the questionnaire rather than a
/// run: the Forge routes each named question to the run that asked it, which
/// resolves it durably and hands the answer back to the provider request
/// still waiting on it. A question skipped here (an empty answer list)
/// resolves with no answer; skipping every question dismisses the
/// questionnaire and the provider learns the user declined.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AnswerQuestions {
    request_id: RequestId,
    thread_id: ThreadId,
    group_id: ObservationId,
    answers: Vec<QuestionAnswer>,
}

impl AnswerQuestions {
    /// Creates a questionnaire answer after validating its bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RunInteractionError`] when no question is named, more than
    /// [`QUESTIONNAIRE_MAX_QUESTIONS`] are, one is named twice, or any
    /// answer list violates the observation answer bounds.
    pub fn new(
        request_id: RequestId,
        thread_id: ThreadId,
        group_id: ObservationId,
        answers: Vec<QuestionAnswer>,
    ) -> Result<Self, RunInteractionError> {
        if answers.is_empty() {
            return Err(RunInteractionError::NoQuestions);
        }
        if answers.len() > QUESTIONNAIRE_MAX_QUESTIONS {
            return Err(RunInteractionError::TooManyQuestions {
                count: answers.len(),
                maximum: QUESTIONNAIRE_MAX_QUESTIONS,
            });
        }
        for (index, answer) in answers.iter().enumerate() {
            if answers[..index]
                .iter()
                .any(|earlier| earlier.question_id == answer.question_id)
            {
                return Err(RunInteractionError::DuplicateQuestion { index });
            }
            validate_answers(&answer.answers)?;
        }
        Ok(Self {
            request_id,
            thread_id,
            group_id,
            answers,
        })
    }

    /// Returns the stable request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns the thread the questionnaire was asked on.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the questionnaire identity.
    #[must_use]
    pub const fn group_id(&self) -> &ObservationId {
        &self.group_id
    }

    /// Returns the per-question answers in display order.
    #[must_use]
    pub fn answers(&self) -> &[QuestionAnswer] {
        &self.answers
    }
}

fn validate_answers(answers: &[String]) -> Result<(), RunInteractionError> {
    if answers.len() > OBSERVATION_ANSWERS_MAX {
        return Err(RunInteractionError::TooManyAnswers {
            count: answers.len(),
            maximum: OBSERVATION_ANSWERS_MAX,
        });
    }
    for (index, answer) in answers.iter().enumerate() {
        if answer.is_empty() {
            return Err(RunInteractionError::EmptyAnswer { index });
        }
        let length = answer.len();
        if length > OBSERVATION_ANSWER_MAX_BYTES {
            return Err(RunInteractionError::AnswerTooLong {
                index,
                length,
                maximum: OBSERVATION_ANSWER_MAX_BYTES,
            });
        }
    }
    Ok(())
}
