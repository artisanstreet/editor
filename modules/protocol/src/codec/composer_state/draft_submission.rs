//! Draft submission: sending a thread's or a project's composer draft at one
//! revision.
//!
//! The scope travels as two text fields, exactly one of them nonempty:
//! `threadId` (the original field) for a thread's draft, `projectId`
//! (appended) for a project's new-task draft.

#![forbid(unsafe_code)]

use artisan_domain::{
    ComposerDraftRevision, ComposerDraftScope, ComposerDraftSubmitted, DraftSubmissionOutcome,
    EngineConfigRevision, ProjectId, SubmitComposerDraft,
};

use super::helpers::*;
use super::*;

/// The (threadId, projectId) texts naming `scope`.
fn scope_texts(scope: &ComposerDraftScope) -> (&str, &str) {
    match scope {
        ComposerDraftScope::Thread(thread) => (thread.as_str(), ""),
        ComposerDraftScope::Project(project) => ("", project.as_str()),
    }
}

/// The scope named by exactly one nonempty text of (threadId, projectId).
fn decode_scope_texts(
    thread: String,
    project: String,
    field: &'static str,
) -> Result<ComposerDraftScope, ComposerStateCodecError> {
    match (thread.is_empty(), project.is_empty()) {
        (false, true) => Ok(ComposerDraftScope::Thread(parse_thread_id(thread, field)?)),
        (true, false) => ProjectId::parse(project)
            .map(ComposerDraftScope::Project)
            .map_err(|source| ComposerStateCodecError::Identifier { field, source }),
        _ => Err(ComposerStateCodecError::StateValue { field }),
    }
}

/// Encodes one draft submission; the parent envelope carries its id.
pub fn encode_submit_composer_draft_request(
    mut builder: composer_state_capnp::submit_composer_draft_request::Builder<'_>,
    value: &SubmitComposerDraft,
) {
    let (thread, project) = scope_texts(&value.scope);
    builder.set_thread_id(thread);
    builder.set_project_id(project);
    builder.set_draft_revision(value.draft_revision.get());
    if let Some(selection) = &value.selection {
        encode_catalog_selection(builder.init_selection(), selection);
    }
}

/// Decodes one draft submission using the parent envelope id.
pub fn decode_submit_composer_draft_request(
    value: composer_state_capnp::submit_composer_draft_request::Reader<'_>,
    request_id: RequestId,
) -> Result<SubmitComposerDraft, ComposerStateCodecError> {
    // The Forge decides steering; the retired field must stay empty.
    let field = "request.submitComposerDraft.steerRunId";
    if !read_text(value.get_steer_run_id(), field)?.is_empty() {
        return Err(ComposerStateCodecError::StateValue { field });
    }
    let selection = if value.has_selection() {
        Some(decode_catalog_selection(
            value.get_selection()?,
            "request.submitComposerDraft.selection",
        )?)
    } else {
        None
    };
    let field = "request.submitComposerDraft.scope";
    let scope = decode_scope_texts(
        read_text(value.get_thread_id(), field)?,
        read_text(value.get_project_id(), field)?,
        field,
    )?;
    Ok(SubmitComposerDraft {
        request_id,
        scope,
        draft_revision: revision(
            value.get_draft_revision(),
            "request.submitComposerDraft.draftRevision",
        )?,
        selection,
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
    let (thread, project) = scope_texts(&value.scope);
    builder.set_thread_id(thread);
    builder.set_project_id(project);
    builder.set_draft_revision(value.draft_revision.get());
    match &value.outcome {
        DraftSubmissionOutcome::Queued {
            thread_id,
            message_id,
            disposition,
            cleared_revision,
            engine_config_revision,
        } => {
            let mut queued = builder.init_queued();
            queued.set_thread_id(thread_id.as_str());
            queued.set_message_id(message_id.as_str());
            queued.set_disposition(encode_disposition(*disposition));
            queued.set_cleared_revision(cleared_revision.get());
            queued.set_engine_config_revision(engine_config_revision.get());
        }
        DraftSubmissionOutcome::Stale { current_revision } => {
            builder.set_stale(current_revision.map_or(0, ComposerDraftRevision::get));
        }
        DraftSubmissionOutcome::Refused(refusal) => {
            encode_submission_refusal(builder.init_refused(), refusal);
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
    let field = "response.composerDraftSubmitted.scope";
    let scope = decode_scope_texts(
        read_text(value.get_thread_id(), field)?,
        read_text(value.get_project_id(), field)?,
        field,
    )?;
    let outcome = match value
        .which()
        .map_err(|_| ComposerStateCodecError::StateValue {
            field: "response.composerDraftSubmitted.outcome",
        })? {
        composer_state_capnp::composer_draft_submitted::Which::Queued(queued) => {
            decode_queued(queued?, &scope)?
        }
        composer_state_capnp::composer_draft_submitted::Which::Refused(refusal) => {
            DraftSubmissionOutcome::Refused(decode_submission_refusal(
                refusal?,
                "response.composerDraftSubmitted.refused",
            )?)
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
        scope,
        draft_revision: revision(
            value.get_draft_revision(),
            "response.composerDraftSubmitted.draftRevision",
        )?,
        outcome,
    })
}

/// Decodes a queued outcome. A thread draft's message is queued in that
/// thread, so its answer may leave the queued thread empty; a project
/// draft's answer must name the thread it created.
fn decode_queued(
    queued: composer_state_capnp::draft_submission_queued::Reader<'_>,
    scope: &ComposerDraftScope,
) -> Result<DraftSubmissionOutcome, ComposerStateCodecError> {
    let field = "response.composerDraftSubmitted.queued.threadId";
    let named = read_text(queued.get_thread_id(), field)?;
    let thread_id = match (scope, named.is_empty()) {
        (ComposerDraftScope::Thread(thread), true) => thread.clone(),
        (ComposerDraftScope::Thread(thread), false) => {
            let named = parse_thread_id(named, field)?;
            if &named != thread {
                return Err(ComposerStateCodecError::StateValue { field });
            }
            named
        }
        (ComposerDraftScope::Project(_), false) => parse_thread_id(named, field)?,
        (ComposerDraftScope::Project(_), true) => {
            return Err(ComposerStateCodecError::StateValue { field });
        }
    };
    Ok(DraftSubmissionOutcome::Queued {
        thread_id,
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
        engine_config_revision: EngineConfigRevision::new(queued.get_engine_config_revision())
            .map_err(|_| ComposerStateCodecError::StateValue {
                field: "response.composerDraftSubmitted.engineConfigRevision",
            })?,
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
