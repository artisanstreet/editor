//! Agent questionnaires between the thread and the composer.
//!
//! The selected thread's open questionnaires come from its retained
//! observations and are pushed to the composer, which presents them stacked
//! above the editor. A finished answer comes back from the composer and goes
//! to the Forge as one [`AnswerQuestions`]; the Forge routes each question to
//! the run that asked it, which replies to the agent's waiting request. The
//! questionnaire then leaves the composer when its resolution arrives with
//! the thread's observations.

use artisan_domain::{AnswerQuestions, ObservationId, QuestionAnswer};
use artisan_protocol::{AnswerQuestionsOutcome, AnswerQuestionsReceipt};

use super::state::create_message_request_id;
use super::{Context, NativeApplication, NativeTransportCommand};
use crate::native_composer::{ComposerQuestion, ComposerQuestionnaire};
use crate::native_transport_service::AnswerFailure;

impl NativeApplication {
    /// Pushes the selected thread's open questionnaires to the composer.
    ///
    /// Nothing is pushed while the thread's history is still replaying, so a
    /// questionnaire answered long ago never flashes open before its
    /// resolution arrives.
    pub(super) fn sync_composer_questionnaires(&mut self, cx: &mut Context<Self>) {
        let questionnaires = if self.selected_history_current() {
            self.engine_observations
                .as_ref()
                .filter(|state| Some(state.thread_id()) == self.selected_thread.as_ref())
                .map(|state| {
                    state
                        .open_questionnaires()
                        .into_iter()
                        .filter(|open| {
                            !open.questions.iter().any(|question| {
                                question.attribution().is_some_and(|attribution| {
                                    self.run_controls.optimistically_stopped()
                                        == Some(&attribution.run_id)
                                })
                            })
                        })
                        .map(|open| ComposerQuestionnaire {
                            group_id: open.group_id,
                            questions: open
                                .questions
                                .iter()
                                .map(|question| ComposerQuestion {
                                    question_id: question.question_id().to_owned(),
                                    text: question.text().to_owned(),
                                    header: question.header().map(str::to_owned),
                                    multi_select: question.multi_select(),
                                    options: question
                                        .options()
                                        .unwrap_or_default()
                                        .iter()
                                        .map(|option| {
                                            (
                                                option.label().to_owned(),
                                                option.description().map(str::to_owned),
                                            )
                                        })
                                        .collect(),
                                })
                                .collect(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.composer.update(cx, |composer, cx| {
            composer.set_questionnaires(questionnaires, cx);
        });
    }

    /// Sends the questionnaire answer the composer just finished.
    pub(super) fn send_questionnaire_answer(&mut self, cx: &mut Context<Self>) {
        let Some(answer) = self
            .composer
            .update(cx, |composer, _| composer.take_questionnaire_answer())
        else {
            return;
        };
        let group = answer.group_id.clone();
        let command = self.selected_thread.clone().and_then(|thread_id| {
            let request_id = create_message_request_id().ok()?;
            let group_id = ObservationId::parse(answer.group_id).ok()?;
            let answers = answer
                .answers
                .into_iter()
                .map(|(question_id, answers)| {
                    Some(QuestionAnswer {
                        question_id: ObservationId::parse(question_id).ok()?,
                        answers,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            AnswerQuestions::new(request_id, thread_id, group_id, answers).ok()
        });
        let sent = command.is_some_and(|command| {
            self.submit_command(NativeTransportCommand::AnswerQuestions(Box::new(command)))
                .is_ok()
        });
        if !sent {
            self.composer.update(cx, |composer, cx| {
                composer.questionnaire_settled(
                    &group,
                    Some("Couldn't send your answer. Try again.".to_owned()),
                    cx,
                );
            });
        }
    }

    /// Settles a questionnaire answer the Forge recorded.
    pub(super) fn settle_questionnaire_answered(
        &mut self,
        command: &AnswerQuestions,
        receipt: &AnswerQuestionsReceipt,
        cx: &mut Context<Self>,
    ) {
        let error = match receipt.outcome {
            AnswerQuestionsOutcome::Applied | AnswerQuestionsOutcome::AlreadyResolved => None,
            AnswerQuestionsOutcome::UnknownTarget => {
                Some("These questions are no longer open.".to_owned())
            }
        };
        self.composer.update(cx, |composer, cx| {
            composer.questionnaire_settled(command.group_id().as_str(), error, cx);
        });
    }

    /// Reopens a questionnaire whose answer could not be sent.
    pub(super) fn settle_questionnaire_failed(
        &mut self,
        command: &AnswerQuestions,
        _failure: &AnswerFailure,
        cx: &mut Context<Self>,
    ) {
        self.composer.update(cx, |composer, cx| {
            composer.questionnaire_settled(
                command.group_id().as_str(),
                Some("Couldn't send your answer. Try again.".to_owned()),
                cx,
            );
        });
    }
}
