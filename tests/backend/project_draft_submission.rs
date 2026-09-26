//! Private handler tests for sending a project's new-task draft: the
//! submission creates exactly one thread and its first message, a repeat of
//! the revision answers them, and every refusal creates nothing.
//!
//! Path-linked from `request_handler/project_draft_submission.rs`; the
//! storage and handler fixtures are the failed-message tests'.

use artisan_database::Repository;
use artisan_domain::{
    AuthoredText, CatalogSelection, ComposerDraftRevision, ComposerDraftScope,
    ComposerDraftSubmitted, DraftSubmissionOutcome, ListQueuedMessages, ModelFavoriteId, ProjectId,
    QueuedMessageListOrder, ReceiptDisposition, SaveComposerDraft, SubmissionRefusalKind,
    SubmitComposerDraft, ThreadId,
};
use artisan_protocol::ResponsePayload;

use super::super::failed_messages_tests::{engine_config, handler, request, seed, storage};
use crate::RequestHandler;

fn project() -> ProjectId {
    ProjectId::parse("project-failed").unwrap()
}

fn scope() -> ComposerDraftScope {
    ComposerDraftScope::Project(project())
}

async fn save_project_draft(handler: &RequestHandler, text: &str) -> ComposerDraftRevision {
    let save = SaveComposerDraft::new(
        request(&format!("save-{}", text.len())),
        scope(),
        if text.is_empty() {
            AuthoredText::empty()
        } else {
            AuthoredText::parse(text).unwrap()
        },
        Vec::new(),
    )
    .unwrap();
    let ResponsePayload::ComposerDraftSaved(saved) = handler
        .save_composer_draft(save.request_id(), &save)
        .await
        .unwrap()
        .payload
    else {
        panic!("expected a save answer");
    };
    saved.revision
}

async fn submit(
    handler: &RequestHandler,
    request_id: &str,
    draft_revision: ComposerDraftRevision,
    selection: Option<CatalogSelection>,
) -> ComposerDraftSubmitted {
    let command = SubmitComposerDraft {
        request_id: request(request_id),
        scope: scope(),
        draft_revision,
        selection,
    };
    let ResponsePayload::ComposerDraftSubmitted(answer) = handler
        .submit_composer_draft_outcome(&command.request_id, &command)
        .await
        .unwrap()
        .payload
    else {
        panic!("expected a submission answer");
    };
    assert_eq!(answer.request_id, command.request_id);
    assert_eq!(answer.scope, scope());
    answer
}

async fn thread_count(repository: &Repository) -> usize {
    repository
        .list_threads(&project())
        .await
        .unwrap()
        .threads()
        .len()
}

async fn queued_texts(repository: &Repository, thread: &ThreadId) -> Vec<String> {
    repository
        .read_queued_messages(
            ListQueuedMessages::new(thread.clone(), QueuedMessageListOrder::OldestFirst, 32)
                .unwrap(),
        )
        .await
        .unwrap()
        .messages()
        .iter()
        .map(|message| {
            message
                .text
                .as_ref()
                .map_or_else(String::new, |text| text.as_str().to_owned())
        })
        .collect()
}

#[tokio::test]
async fn a_project_draft_creates_one_configured_thread_with_its_first_message() {
    let (_temporary, storage) = storage("project-submit").await;
    let repository = storage.repository();
    seed(repository).await;
    repository
        .remember_default_engine_config(&engine_config())
        .await
        .unwrap();
    let handler = handler(&storage);
    let revision = save_project_draft(&handler, "build the thing").await;
    let before = thread_count(repository).await;

    let answer = submit(&handler, "submit-new-1", revision, None).await;
    let DraftSubmissionOutcome::Queued {
        thread_id,
        message_id,
        disposition: ReceiptDisposition::Accepted,
        cleared_revision,
        ..
    } = answer.outcome
    else {
        panic!("the draft is queued in a new thread: {:?}", answer.outcome);
    };
    assert_eq!(thread_count(repository).await, before + 1);
    assert_eq!(
        repository.read_thread_project(&thread_id).await.unwrap(),
        project()
    );
    assert_eq!(
        queued_texts(repository, &thread_id).await,
        ["build the thing"]
    );
    let settings = repository
        .read_thread_engine_settings(&thread_id)
        .await
        .unwrap()
        .expect("the new thread starts from the default configuration");
    assert_eq!(settings.config(), &engine_config());
    let draft = repository
        .read_composer_draft(&scope())
        .await
        .unwrap()
        .expect("the project draft keeps its row");
    assert!(draft.text().as_str().is_empty(), "the draft is emptied");
    assert_eq!(draft.revision(), cleared_revision);
    assert!(cleared_revision > revision);

    // The answer was lost: the same revision under another request id
    // answers the same thread and message and creates nothing.
    let replay = submit(&handler, "submit-new-2", revision, None).await;
    let DraftSubmissionOutcome::Queued {
        thread_id: replayed_thread,
        message_id: replayed_message,
        disposition: ReceiptDisposition::Duplicate,
        cleared_revision: replayed_cleared,
        ..
    } = replay.outcome
    else {
        panic!("a repeat is a duplicate: {:?}", replay.outcome);
    };
    assert_eq!(replayed_thread, thread_id);
    assert_eq!(replayed_message, message_id);
    assert_eq!(replayed_cleared, cleared_revision);
    assert_eq!(thread_count(repository).await, before + 1);
    assert_eq!(queued_texts(repository, &thread_id).await.len(), 1);

    // A revision the Forge never gave the draft is stale.
    let stale = submit(
        &handler,
        "submit-new-3",
        ComposerDraftRevision::new(40).unwrap(),
        None,
    )
    .await;
    assert_eq!(
        stale.outcome,
        DraftSubmissionOutcome::Stale {
            current_revision: Some(cleared_revision)
        }
    );
    assert_eq!(thread_count(repository).await, before + 1);
}

#[tokio::test]
async fn a_refused_project_draft_creates_nothing_and_keeps_the_draft() {
    let (_temporary, storage) = storage("project-refused").await;
    let repository = storage.repository();
    seed(repository).await;
    let handler = handler(&storage);
    let revision = save_project_draft(&handler, "which model?").await;
    let before = thread_count(repository).await;

    // No selection and no default configuration.
    let answer = submit(&handler, "submit-refused-1", revision, None).await;
    let DraftSubmissionOutcome::Refused(refusal) = answer.outcome else {
        panic!("refused as data: {:?}", answer.outcome);
    };
    assert_eq!(refusal.kind(), SubmissionRefusalKind::NoSelection);

    // A selection the Forge cannot resolve (this handler serves no catalog).
    let selection = CatalogSelection {
        model_id: ModelFavoriteId::parse("codex-sol").unwrap(),
        profile_id: None,
        reasoning_effort: None,
        speed: None,
        context_window: None,
        permission: None,
    };
    let answer = submit(&handler, "submit-refused-2", revision, Some(selection)).await;
    let DraftSubmissionOutcome::Refused(refusal) = answer.outcome else {
        panic!("refused as data: {:?}", answer.outcome);
    };
    assert_eq!(refusal.kind(), SubmissionRefusalKind::InvalidSelection);

    assert_eq!(thread_count(repository).await, before);
    let draft = repository
        .read_composer_draft(&scope())
        .await
        .unwrap()
        .expect("the draft is kept");
    assert_eq!(draft.text().as_str(), "which model?");
    assert_eq!(draft.revision(), revision);
}

#[tokio::test]
async fn an_empty_project_draft_is_refused() {
    let (_temporary, storage) = storage("project-empty").await;
    let repository = storage.repository();
    seed(repository).await;
    repository
        .remember_default_engine_config(&engine_config())
        .await
        .unwrap();
    let handler = handler(&storage);
    let revision = save_project_draft(&handler, "").await;
    let before = thread_count(repository).await;
    let command = SubmitComposerDraft {
        request_id: request("submit-empty"),
        scope: scope(),
        draft_revision: revision,
        selection: None,
    };
    assert!(
        handler
            .submit_composer_draft_outcome(&command.request_id, &command)
            .await
            .is_err()
    );
    assert_eq!(thread_count(repository).await, before);
}
