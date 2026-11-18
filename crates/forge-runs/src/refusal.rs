use forge_store::{
    AnswerFailure, ContentError, CreateProjectFailure, GrepFailure, SendRejected, SettingFailure,
    ThreadsPageFailure, WriteFailure,
};
use std::error::Error;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Refused(pub String);

pub fn refusal(error: &anyhow::Error) -> Option<String> {
    let cause = error.chain().find(|cause| refused(*cause))?;
    let mut message = cause.to_string();
    let mut source = cause.source();
    while let Some(next) = source {
        message.push_str(": ");
        message.push_str(&next.to_string());
        source = next.source();
    }
    Some(message)
}

fn refused(cause: &(dyn Error + 'static)) -> bool {
    cause.is::<Refused>()
        || cause.is::<SendRejected>()
        || cause
            .downcast_ref::<AnswerFailure>()
            .is_some_and(|failure| matches!(failure, AnswerFailure::Refused(_)))
        || cause
            .downcast_ref::<ContentError>()
            .is_some_and(|failure| matches!(failure, ContentError::Refused(_)))
        || cause
            .downcast_ref::<GrepFailure>()
            .is_some_and(|failure| matches!(failure, GrepFailure::Pattern(_)))
        || cause
            .downcast_ref::<ThreadsPageFailure>()
            .is_some_and(|failure| matches!(failure, ThreadsPageFailure::SearchTooLong { .. }))
        || cause
            .downcast_ref::<CreateProjectFailure>()
            .is_some_and(|failure| !matches!(failure, CreateProjectFailure::Storage(_)))
        || cause
            .downcast_ref::<SettingFailure>()
            .is_some_and(|failure| {
                matches!(
                    failure,
                    SettingFailure::Write(WriteFailure::SettingsFull { .. })
                )
            })
        || cause
            .downcast_ref::<WriteFailure>()
            .is_some_and(|failure| matches!(failure, WriteFailure::SettingsFull { .. }))
}
