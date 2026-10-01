//! Answering a thread's open questionnaires.
//!
//! Every engine's questions reach the thread as questionnaires (see
//! `QuestionObservation`): the questions one provider request asked
//! together. The request stays open on the engine while the run waits for
//! the user, so an answer goes back to that request through the run that
//! owns it, exactly like an approval: the run resolves each question durably
//! and hands the answer to its provider pump, which replies to the provider
//! once every question of the request has one. The answer never becomes a
//! user message. When the run has already ended, its questions were closed
//! with it, so a late answer finds them resolved.
//!
//! Each question resolves under a request identity derived from the
//! answer's, so a retry after a partial failure finds the resolved ones as
//! replays instead of answering them twice.

use artisan_database::LedgerQuestion;
use artisan_domain::{AnswerQuestions, QuestionAnswer, RequestId, RespondQuestion, RunId};
use artisan_protocol::{
    AnswerQuestionsOutcome, AnswerQuestionsReceipt, ErrorCode, ProtocolFailure, ResponsePayload,
    RunInteractionOutcome, ServerResponse,
};

use super::RequestHandler;
use super::failures::{outcome, repository_failure, typed_failure};

impl RequestHandler {
    /// Answers one open questionnaire through the run that asked it.
    pub(super) async fn answer_questions_outcome(
        &self,
        request_id: &RequestId,
        command: &AnswerQuestions,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let questions = self
            .repository
            .read_thread_questionnaire(command.thread_id(), command.group_id())
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        let named: Vec<(&LedgerQuestion, &QuestionAnswer)> = command
            .answers()
            .iter()
            .filter_map(|answer| {
                questions
                    .iter()
                    .find(|question| question.question.question_id() == &answer.question_id)
                    .map(|question| (question, answer))
            })
            .collect();
        if named.is_empty() {
            return Ok(receipt(
                request_id,
                command,
                AnswerQuestionsOutcome::UnknownTarget,
            ));
        }
        let mut applied = false;
        for (index, (question, answer)) in named
            .iter()
            .filter(|(question, _)| !question.resolved)
            .enumerate()
        {
            applied |= self
                .resolve_through_run(request_id, command, &question.run_id, index, answer)
                .await?;
        }
        Ok(receipt(
            request_id,
            command,
            if applied {
                AnswerQuestionsOutcome::Applied
            } else {
                AnswerQuestionsOutcome::AlreadyResolved
            },
        ))
    }

    /// Resolves one question through the run that asked it. `true` means
    /// the answer reached the run; `false` means the run no longer owns the
    /// question (it ended, closing the question with it, or answered it
    /// already), so there is nothing left to deliver.
    async fn resolve_through_run(
        &self,
        request_id: &RequestId,
        command: &AnswerQuestions,
        run_id: &RunId,
        index: usize,
        answer: &QuestionAnswer,
    ) -> Result<bool, ProtocolFailure> {
        let live_request = derived_request_id(request_id, index)?;
        let respond = RespondQuestion::new(
            live_request.clone(),
            command.thread_id().clone(),
            run_id.clone(),
            answer.question_id.clone(),
            answer.answers.clone(),
        )
        .map_err(|_| {
            typed_failure(
                ErrorCode::InvalidInput,
                "the answer is out of bounds",
                false,
                request_id,
            )
        })?;
        let response = self
            .respond_question_outcome(&live_request, &respond)
            .await?;
        Ok(match response.payload {
            ResponsePayload::QuestionResponse(receipt) => {
                matches!(receipt.outcome, RunInteractionOutcome::Applied)
            }
            _ => false,
        })
    }
}

/// A request identity for one question of an answer, derived so a retry of
/// the same answer replays the same resolution.
fn derived_request_id(request_id: &RequestId, index: usize) -> Result<RequestId, ProtocolFailure> {
    RequestId::parse(format!("{}.q{index}", request_id.as_str())).map_err(|_| {
        typed_failure(
            ErrorCode::InvalidInput,
            "the answer request id is too long to derive its effects",
            false,
            request_id,
        )
    })
}

fn receipt(
    request_id: &RequestId,
    command: &AnswerQuestions,
    result: AnswerQuestionsOutcome,
) -> ServerResponse {
    outcome(
        request_id,
        ResponsePayload::QuestionsAnswered(AnswerQuestionsReceipt {
            request_id: request_id.clone(),
            thread_id: command.thread_id().clone(),
            group_id: command.group_id().clone(),
            outcome: result,
        }),
    )
}
