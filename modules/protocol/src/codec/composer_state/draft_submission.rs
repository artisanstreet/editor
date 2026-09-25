//! Draft submission: sending a thread's composer draft at one revision.

#![forbid(unsafe_code)]

use artisan_domain::{
    ComposerDraftRevision, ComposerDraftSubmitted, DraftSubmissionOutcome, SteerTarget,
    SubmitComposerDraft,
};

use super::helpers::*;
use super::*;

/// Encodes one draft submission; the parent envelope carries its id.
pub fn encode_submit_composer_draft_request(
    mut builder: composer_state_capnp::submit_composer_draft_request::Builder<'_>,
    value: &SubmitComposerDraft,
) {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_draft_revision(value.draft_revision.get());
    builder.set_steer_run_id(
        value
            .steer_target
            .as_ref()
            .map_or("", |target| target.run_id().as_str()),
    );
}

/// Decodes one draft submission using the parent envelope id.
pub fn decode_submit_composer_draft_request(
    value: composer_state_capnp::submit_composer_draft_request::Reader<'_>,
    request_id: RequestId,
) -> Result<SubmitComposerDraft, ComposerStateCodecError> {
    let field = "request.submitComposerDraft.steerRunId";
    let steer = read_text(value.get_steer_run_id(), field)?;
    let steer_target = if steer.is_empty() {
        None
    } else {
        Some(SteerTarget::new(parse_run_id(steer, field)?))
    };
    Ok(SubmitComposerDraft {
        request_id,
        thread_id: parse_thread_id(
            read_text(
                value.get_thread_id(),
                "request.submitComposerDraft.threadId",
            )?,
            "request.submitComposerDraft.threadId",
        )?,
        draft_revision: revision(
            value.get_draft_revision(),
            "request.submitComposerDraft.draftRevision",
        )?,
        steer_target,
    })
}

/// Encodes a submission answer after checking its correlation.
pub fn encode_composer_draft_submitted(
    mut builder: composer_state_capnp::composer_draft_submitted::Builder<'_>,
    outer_request_id: &RequestId,
    value: &ComposerDraftSubmitted,
) -> Result<(), ComposerStateCodecError> {
    if outer_request_id != &value.request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch {
            field: "response.composerDraftSubmitted.requestId",
        });
    }
    builder.set_request_id(value.request_id.as_str());
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_draft_revision(value.draft_revision.get());
    match &value.outcome {
        DraftSubmissionOutcome::Queued {
            message_id,
            disposition,
            cleared_revision,
        } => {
            let mut queued = builder.init_queued();
            queued.set_message_id(message_id.as_str());
            queued.set_disposition(encode_disposition(*disposition));
            queued.set_cleared_revision(cleared_revision.get());
        }
        DraftSubmissionOutcome::Stale { current_revision } => {
            builder.set_stale(current_revision.map_or(0, ComposerDraftRevision::get));
        }
    }
    Ok(())
}

/// Decodes a submission answer and enforces its correlation.
pub fn decode_composer_draft_submitted(
    value: composer_state_capnp::composer_draft_submitted::Reader<'_>,
    outer_request_id: &RequestId,
) -> Result<ComposerDraftSubmitted, ComposerStateCodecError> {
    let field = "response.composerDraftSubmitted.requestId";
    let request_id = parse_request_id(read_text(value.get_request_id(), field)?, field)?;
    if &request_id != outer_request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch { field });
    }
    let outcome = match value
        .which()
        .map_err(|_| ComposerStateCodecError::StateValue {
            field: "response.composerDraftSubmitted.outcome",
        })? {
        composer_state_capnp::composer_draft_submitted::Which::Queued(queued) => {
            let queued = queued?;
            DraftSubmissionOutcome::Queued {
                message_id: parse_message_id(
                    read_text(
                        queued.get_message_id(),
                        "response.composerDraftSubmitted.messageId",
                    )?,
                    "response.composerDraftSubmitted.messageId",
                )?,
                disposition: decode_disposition(
                    queued.get_disposition(),
                    "response.composerDraftSubmitted.disposition",
                )?,
                cleared_revision: revision(
                    queued.get_cleared_revision(),
                    "response.composerDraftSubmitted.clearedRevision",
                )?,
            }
        }
        composer_state_capnp::composer_draft_submitted::Which::Stale(current) => {
            DraftSubmissionOutcome::Stale {
                current_revision: (current != 0)
                    .then(|| revision(current, "response.composerDraftSubmitted.stale"))
                    .transpose()?,
            }
        }
    };
    Ok(ComposerDraftSubmitted {
        request_id,
        thread_id: parse_thread_id(
            read_text(
                value.get_thread_id(),
                "response.composerDraftSubmitted.threadId",
            )?,
            "response.composerDraftSubmitted.threadId",
        )?,
        draft_revision: revision(
            value.get_draft_revision(),
            "response.composerDraftSubmitted.draftRevision",
        )?,
        outcome,
    })
}

fn revision(
    value: u64,
    field: &'static str,
) -> Result<ComposerDraftRevision, ComposerStateCodecError> {
    ComposerDraftRevision::new(value)
        .ok()
        .filter(|revision| revision.get() > 0)
        .ok_or(ComposerStateCodecError::StateValue { field })
}
