//! Native values are resolved by opaque choice identity, never displayed labels.
use artisan_api::{Collection, Question, QuestionAnswer};

pub fn native_answers(
    questions: &[Question],
    answers: &[Option<QuestionAnswer>],
) -> Result<Vec<Option<Vec<String>>>, String> {
    if answers.len() != questions.len() {
        return Err("question answer count differs from the request".into());
    }
    questions
        .iter()
        .zip(answers)
        .map(|(question, answer)| {
            question
                .validate_answer_structure(answer)
                .map_err(str::to_owned)?;
            match answer {
                None => Ok(None),
                Some(QuestionAnswer::Text { text }) => Ok(Some(vec![text.clone()])),
                Some(QuestionAnswer::Choices { ids }) => {
                    let ids = ids.inline().ok_or("submitted choices must be inline")?;
                    let options = question
                        .options
                        .inline()
                        .ok_or("native choices must be inline")?;
                    let mut labels = std::collections::HashMap::with_capacity(options.len());
                    for option in options {
                        if labels
                            .insert(option.id.as_str(), option.label.as_str())
                            .is_some()
                        {
                            return Err("native question has duplicate option IDs".into());
                        }
                    }
                    ids.iter()
                        .map(|id| {
                            labels
                                .get(id.as_str())
                                .map(|label| (*label).to_owned())
                                .ok_or_else(|| {
                                    format!("unknown choice {id} for question {}", question.id)
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map(Some)
                }
            }
        })
        .collect()
}

/// Adapters retain native pending prompts until they are answered. A descriptor
/// cannot stand in for the native question records used to build a reply.
pub fn native_collection_answers(
    questions: &Collection<Question>,
    answers: &[Option<QuestionAnswer>],
) -> Result<Vec<Option<Vec<String>>>, String> {
    native_answers(
        questions
            .inline()
            .ok_or("native questions must be inline")?,
        answers,
    )
}

pub fn invalid_answer(key: &str, message: String) -> crate::harness::EngineEvent {
    crate::harness::EngineEvent::Item {
        key: format!("answer-error:{key}"),
        parent: None,
        status: artisan_api::Status::Done,
        kind: artisan_api::ItemKind::Error {
            kind: artisan_api::ErrorKind::Execution,
            message: message.into(),
            retryable: true,
        },
    }
}
