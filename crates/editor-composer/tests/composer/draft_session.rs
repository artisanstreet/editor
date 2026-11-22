use super::*;
use crate::composer::controller::SendRefusal;
use artisan_api::{DraftSnapshot, DraftWriteResult, RunFailure};

fn typed(text: &str) -> DraftBody {
    DraftBody {
        text: text.to_owned(),
        ..DraftBody::default()
    }
}

fn snapshot(revision: u64, body: &DraftBody) -> DraftSnapshot {
    DraftSnapshot {
        revision,
        body: body.to_wire(),
    }
}

fn load(state: &mut DraftSession, revision: u64, body: &DraftBody) {
    assert!(
        matches!(state.next_operation(), Some(Operation::Load)),
        "expected the draft to load"
    );
    state.loaded(Ok(snapshot(revision, body)));
}

fn write(state: &mut DraftSession) -> (u64, DraftBody) {
    let Some(Operation::Write {
        expected_revision,
        body,
    }) = state.next_operation()
    else {
        panic!("expected the draft to be written");
    };
    (expected_revision, *body)
}

fn saved(state: &mut DraftSession, revision: u64, body: &DraftBody) {
    state.written(Ok(DraftWriteResult {
        saved: true,
        current: snapshot(revision, body),
    }));
}

fn refused_write(state: &mut DraftSession, revision: u64, current: &DraftBody) {
    state.written(Ok(DraftWriteResult {
        saved: false,
        current: snapshot(revision, current),
    }));
}

fn type_in(state: &mut DraftSession, body: DraftBody) {
    let shown = state.epoch();
    state.queue(body, shown, false);
}

fn post(state: &mut DraftSession) -> Submission {
    let Some(Operation::Post(submission)) = state.next_operation() else {
        panic!("expected the message to be posted");
    };
    *submission
}

fn refused(failure: Failure) -> CallError {
    use editor_core::client::{CallFailure, ResponseError};
    let message = failure.to_string();
    CallError {
        method: reqwest::Method::PUT,
        path: "/blobs".to_owned(),
        failure: Box::new(CallFailure::Answer(ResponseError::Refused {
            status: reqwest::StatusCode::BAD_REQUEST,
            error: artisan_api::ApiError::new(failure, message),
        })),
    }
}

fn not_sent(submission: &Submission) -> PostMessageError {
    PostMessageError::Send {
        thread: submission.thread.clone(),
        message: submission.message.id.clone(),
        source: refused(Failure::Internal),
    }
}

fn submission(id: &str, text: &str) -> Submission {
    Submission {
        message: SendMessage {
            id: ItemId::from(id),
            text: text.to_owned(),
            images: Vec::new(),
            config: None,
            draft: None,
        },
        create: None,
        thread: ThreadId::from("thread"),
    }
}

fn thread() -> ThreadId {
    ThreadId::from("thread")
}

fn loaded(revision: u64, body: &DraftBody) -> DraftSession {
    let mut state = DraftSession::new("thread:thread".into());
    load(&mut state, revision, body);
    state
}

fn reopened(kept: KeptDraft) -> DraftSession {
    let mut state = DraftSession::new("thread:thread".into());
    state.recover(kept, Vec::new(), Vec::new());
    state
}

fn submitted(text: &str) -> DraftSession {
    let mut state = loaded(1, &DraftBody::default());
    type_in(&mut state, typed(text));
    state.submit(submission("message", text)).unwrap();
    state
}

fn in_conflict() -> DraftSession {
    let mut state = loaded(0, &DraftBody::default());
    type_in(&mut state, typed("mine"));
    write(&mut state);
    refused_write(&mut state, 3, &typed("theirs"));
    assert!(state.conflict(), "another client's draft won the write");
    state
}

#[test]
fn a_kept_draft_is_written_over_the_revision_it_was_based_on() {
    let mut closing = loaded(4, &typed("first"));
    type_in(&mut closing, typed("first, then more"));
    let mut state = reopened(closing.kept());
    assert_eq!(
        state.shown(),
        Some(&typed("first, then more")),
        "the kept draft shows before Forge answers"
    );
    load(&mut state, 4, &typed("first"));
    assert!(!state.conflict(), "nobody else changed the draft");
    assert_eq!(write(&mut state), (4, typed("first, then more")));
}

#[test]
fn a_draft_cleared_offline_stays_cleared_after_reopening() {
    let mut closing = loaded(1, &typed("old text"));
    type_in(&mut closing, DraftBody::default());
    let mut state = reopened(closing.kept());
    load(&mut state, 1, &typed("old text"));
    assert_eq!(
        state.body(),
        &DraftBody::default(),
        "the old text stays gone"
    );
    assert_eq!(
        write(&mut state),
        (1, DraftBody::default()),
        "clearing the draft reaches Forge"
    );
}

#[test]
fn a_draft_cleared_offline_gives_way_to_a_newer_draft_from_another_client() {
    let mut closing = loaded(1, &typed("old text"));
    type_in(&mut closing, DraftBody::default());
    let mut state = reopened(closing.kept());
    load(&mut state, 2, &typed("typed elsewhere"));
    assert!(
        !state.conflict(),
        "an empty draft has nothing to choose between"
    );
    assert_eq!(state.shown(), Some(&typed("typed elsewhere")));
    assert!(
        !state.pending(),
        "nothing is written over the other client's draft"
    );
}

#[test]
fn a_kept_draft_another_client_changed_asks_which_to_keep() {
    let mut closing = loaded(1, &typed("old text"));
    type_in(&mut closing, typed("mine"));
    let mut state = reopened(closing.kept());
    load(&mut state, 2, &typed("theirs"));
    assert!(state.conflict(), "both drafts hold text typed by someone");
    assert_eq!(state.body(), &typed("mine"));
    assert!(
        state.next_operation().is_none(),
        "nothing is saved until the user chooses"
    );
}

#[test]
fn a_write_cut_off_at_closing_is_recognised_as_this_clients_own() {
    let mut closing = loaded(1, &typed("first"));
    type_in(&mut closing, typed("second"));
    write(&mut closing);
    type_in(&mut closing, typed("second, then more"));
    let mut state = reopened(closing.kept());
    load(&mut state, 2, &typed("second"));
    assert!(
        !state.conflict(),
        "the draft Forge holds is the write this client sent before closing"
    );
    assert_eq!(write(&mut state), (2, typed("second, then more")));
}

#[test]
fn a_write_that_failed_at_closing_is_recognised_as_this_clients_own() {
    let mut closing = loaded(1, &typed("first"));
    type_in(&mut closing, typed("second"));
    write(&mut closing);
    closing.written(Err(refused(Failure::Internal)));
    type_in(&mut closing, typed("second, then more"));
    let mut state = reopened(closing.kept());
    load(&mut state, 2, &typed("second"));
    assert!(!state.conflict(), "the failed write had in fact landed");
    assert_eq!(write(&mut state), (2, typed("second, then more")));
}

#[test]
fn text_typed_after_an_unanswered_send_shows_on_reopening_and_stays_after_the_send() {
    let mut closing = submitted("send this");
    write(&mut closing);
    saved(&mut closing, 2, &typed("send this"));
    let unanswered = post(&mut closing);
    type_in(&mut closing, typed("typed while it sent"));
    let mut state = reopened(closing.kept());
    assert_eq!(
        state.shown(),
        Some(&typed("typed while it sent")),
        "the text typed after sending shows while the message is still on its way"
    );
    load(&mut state, 2, &typed("send this"));
    let resent = post(&mut state);
    assert_eq!(
        resent.message.id, unanswered.message.id,
        "it is the same message, so Forge doesn't take it twice"
    );
    assert_eq!(
        resent.message.draft.map(|draft| draft.revision),
        Some(2),
        "Forge clears the draft that held the message"
    );
    state.posted(Ok(()), thread());
    assert_eq!(state.body(), &typed("typed while it sent"));
    load(&mut state, 3, &DraftBody::default());
    assert_eq!(
        write(&mut state),
        (3, typed("typed while it sent")),
        "the later text is saved once the message is in"
    );
}

#[test]
fn an_unanswered_send_never_clears_a_draft_another_client_saved_since() {
    let mut closing = submitted("send this");
    write(&mut closing);
    saved(&mut closing, 2, &typed("send this"));
    post(&mut closing);
    let mut state = reopened(closing.kept());
    assert_eq!(
        state.shown(),
        None,
        "the message on its way is not offered as a draft"
    );
    load(&mut state, 3, &typed("typed elsewhere"));
    assert!(!state.conflict(), "nothing but the sent message was local");
    let resent = post(&mut state);
    assert_eq!(
        resent.message.draft, None,
        "the send doesn't clear the other client's draft"
    );
    state.posted(Ok(()), thread());
    assert_eq!(state.shown(), Some(&typed("typed elsewhere")));
    assert!(
        !state.pending(),
        "the other client's draft is not rewritten"
    );
}

#[test]
fn an_unanswered_send_forge_already_took_is_finished_without_coming_back_as_a_draft() {
    let mut closing = submitted("send this");
    write(&mut closing);
    saved(&mut closing, 2, &typed("send this"));
    post(&mut closing);
    let mut state = reopened(closing.kept());
    load(&mut state, 3, &DraftBody::default());
    let resent = post(&mut state);
    assert_eq!(resent.message.draft, None);
    state.posted(Ok(()), thread());
    assert_eq!(state.body(), &DraftBody::default());
    assert!(
        !state.pending(),
        "the sent text isn't saved back as a draft"
    );
}

#[test]
fn an_unanswered_send_over_an_unchanged_draft_clears_it_when_accepted() {
    let mut closing = loaded(1, &typed("send th"));
    type_in(&mut closing, typed("send this"));
    closing.submit(submission("message", "send this")).unwrap();
    let mut state = reopened(closing.kept());
    load(&mut state, 1, &typed("send th"));
    assert_eq!(write(&mut state), (1, typed("send this")));
    saved(&mut state, 2, &typed("send this"));
    let resent = post(&mut state);
    assert_eq!(
        resent.message.draft.map(|draft| draft.revision),
        Some(2),
        "Forge clears exactly the draft revision that held the message"
    );
}

#[test]
fn a_message_whose_draft_another_client_replaced_is_sent_without_clearing_theirs() {
    let mut state = submitted("send this");
    write(&mut state);
    refused_write(&mut state, 2, &typed("typed elsewhere"));
    assert!(!state.conflict(), "nothing but the sent message was local");
    assert_eq!(post(&mut state).message.draft, None);
    state.posted(Ok(()), thread());
    assert_eq!(state.body(), &typed("typed elsewhere"));
}

#[test]
fn a_draft_another_client_cleared_while_saving_is_saved_again() {
    let mut state = loaded(1, &typed("old"));
    type_in(&mut state, typed("mine"));
    write(&mut state);
    refused_write(&mut state, 2, &DraftBody::default());
    assert!(
        !state.conflict(),
        "an empty draft has nothing to choose between"
    );
    assert_eq!(write(&mut state), (2, typed("mine")));
}

#[test]
fn a_message_sent_during_a_conflict_is_posted_and_the_choice_stays_open() {
    let mut state = in_conflict();
    state.submit(submission("message", "mine")).unwrap();
    let posted = post(&mut state);
    assert_eq!(
        posted.message.draft, None,
        "the send doesn't clear the other client's draft"
    );
    state.posted(Ok(()), thread());
    assert!(
        !state.conflict(),
        "once the message is in, only the other client's draft is left"
    );
    assert_eq!(state.shown(), Some(&typed("theirs")));
}

#[test]
fn keeping_this_clients_draft_writes_it_over_the_other() {
    let mut state = in_conflict();
    state.keep_local();
    assert_eq!(write(&mut state), (3, typed("mine")));
}

#[test]
fn choosing_the_other_clients_draft_shows_it_and_keeps_a_message_on_its_way() {
    let mut state = in_conflict();
    let epoch = state.epoch();
    state.submit(submission("message", "mine")).unwrap();
    state.use_remote();
    assert_ne!(state.epoch(), epoch, "the composer shows the chosen draft");
    assert_eq!(state.shown(), Some(&typed("theirs")));
    assert_eq!(post(&mut state).message.id, ItemId::from("message"));
    state.posted(Ok(()), thread());
    assert_eq!(state.body(), &typed("theirs"));
}

#[test]
fn choosing_the_other_clients_draft_drops_images_still_uploading_for_mine() {
    let mut state = in_conflict();
    state.adopt(PendingUpload::kept(vec![1, 2, 3]));
    let uploads = state.next_uploads();
    let [(key, _)] = uploads.as_slice() else {
        panic!("expected the pasted image to start uploading");
    };
    state.use_remote();
    assert_eq!(
        state.adopted(*key, Ok(BlobHash("d".repeat(64)))),
        None,
        "the image belongs to the discarded draft"
    );
    assert_eq!(state.body(), &typed("theirs"));
    assert!(
        !state.pending(),
        "nothing from the discarded draft is saved over the one the user chose"
    );
}

#[test]
fn typing_into_a_view_from_before_another_clients_draft_arrived_asks_which_to_keep() {
    let mut state = submitted("send this");
    write(&mut state);
    saved(&mut state, 2, &typed("send this"));
    post(&mut state);
    state.posted(Ok(()), thread());
    let shown = state.epoch();
    load(&mut state, 4, &typed("typed elsewhere"));
    assert_ne!(
        state.epoch(),
        shown,
        "the other client's draft replaced the view"
    );
    state.queue(typed("typed here"), shown, false);
    assert!(
        state.conflict(),
        "the keystroke was typed over text the user never saw replaced"
    );
}

#[test]
fn a_send_that_fails_keeps_its_identity_for_an_identical_resend() {
    let mut state = submitted("send this");
    write(&mut state);
    saved(&mut state, 2, &typed("send this"));
    let first = post(&mut state);
    state.posted(Err(not_sent(&first)), thread());
    assert_eq!(state.posting(), Posting::Failed);
    type_in(&mut state, typed("send this, edited"));
    type_in(&mut state, typed("send this"));
    state
        .submit(submission("second-attempt", "send this"))
        .unwrap();
    write(&mut state);
    saved(&mut state, 3, &typed("send this"));
    assert_eq!(
        post(&mut state).message.id,
        first.message.id,
        "Forge takes an identical resend only once"
    );
}

#[test]
fn an_edited_resend_is_a_new_message() {
    let mut state = submitted("send this");
    write(&mut state);
    saved(&mut state, 2, &typed("send this"));
    let first = post(&mut state);
    state.posted(Err(not_sent(&first)), thread());
    type_in(&mut state, typed("send this instead"));
    state
        .submit(submission("second-attempt", "send this instead"))
        .unwrap();
    write(&mut state);
    saved(&mut state, 3, &typed("send this instead"));
    assert_eq!(
        post(&mut state).message.id,
        ItemId::from("second-attempt"),
        "a changed message is not passed off as the failed one"
    );
}

#[test]
fn try_again_after_a_failed_send_never_resends_on_its_own() {
    let mut state = submitted("send this");
    write(&mut state);
    saved(&mut state, 2, &typed("send this"));
    let first = post(&mut state);
    state.posted(Err(not_sent(&first)), thread());
    state.retry();
    assert!(
        state.next_operation().is_none(),
        "the failed message comes back to the composer instead of going out unseen"
    );
}

#[test]
fn a_failed_load_hands_the_queued_message_back_with_its_identity() {
    let mut state = DraftSession::new("thread:thread".into());
    type_in(&mut state, typed("send this"));
    state.submit(submission("message", "send this")).unwrap();
    assert!(matches!(state.next_operation(), Some(Operation::Load)));
    state.loaded(Err(refused(Failure::Internal)));
    assert_eq!(state.posting(), Posting::Failed);
    assert!(state.sendable(), "the message can be sent again");
    state
        .submit(submission("second-attempt", "send this"))
        .unwrap();
    load(&mut state, 0, &DraftBody::default());
    write(&mut state);
    saved(&mut state, 1, &typed("send this"));
    assert_eq!(post(&mut state).message.id, ItemId::from("message"));
}

fn sent_and_typed_over(sent: &str, typed_meanwhile: &str) -> (DraftSession, Submission) {
    let mut state = submitted(sent);
    write(&mut state);
    saved(&mut state, 2, &typed(sent));
    let first = post(&mut state);
    type_in(&mut state, typed(typed_meanwhile));
    (state, first)
}

fn handed_back(sent: &str, parked: &[&str]) -> DraftBody {
    DraftBody {
        follow_ups: parked.iter().map(|text| typed(text)).collect(),
        ..typed(sent)
    }
}

#[test]
fn a_send_that_fails_while_no_composer_shows_it_comes_back_with_the_later_text_beside_it() {
    let (mut state, first) = sent_and_typed_over("send this", "typed meanwhile");
    let shown = state.epoch();
    state.posted(Err(not_sent(&first)), thread());
    assert_ne!(
        state.epoch(),
        shown,
        "a composer opened before or after the failure shows the message again"
    );
    assert_eq!(
        state.shown(),
        Some(&handed_back("send this", &["typed meanwhile"])),
        "the failed message is back, with the later text parked beside it"
    );
    state
        .submit(submission("second-attempt", "send this"))
        .unwrap();
    write(&mut state);
    saved(
        &mut state,
        3,
        &handed_back("send this", &["typed meanwhile"]),
    );
    assert_eq!(
        post(&mut state).message.id,
        first.message.id,
        "trying again sends the same message, so Forge takes it only once"
    );
    state.posted(Ok(()), thread());
    assert_eq!(
        state.body(),
        &typed("typed meanwhile"),
        "the parked text becomes the draft once the message is in"
    );
}

fn retried_with_text_parked() -> (DraftSession, Submission) {
    let (mut state, first) = sent_and_typed_over("send this", "typed meanwhile");
    state.posted(Err(not_sent(&first)), thread());
    state
        .submit(submission("second-attempt", "send this"))
        .unwrap();
    write(&mut state);
    saved(
        &mut state,
        3,
        &handed_back("send this", &["typed meanwhile"]),
    );
    let retried = post(&mut state);
    assert_eq!(
        state.shown(),
        None,
        "a composer opened while the retry is on its way starts empty"
    );
    (state, retried)
}

#[test]
fn text_parked_beside_a_retried_send_stays_when_the_retry_fails_too() {
    let (mut state, retried) = retried_with_text_parked();
    type_in(&mut state, typed("typed in a reopened composer"));
    state.posted(Err(not_sent(&retried)), thread());
    assert_eq!(
        state.shown(),
        Some(&handed_back(
            "send this",
            &["typed meanwhile", "typed in a reopened composer"]
        )),
        "both texts typed while sending stay beside the message"
    );
}

#[test]
fn text_parked_beside_a_retried_send_comes_back_when_the_retry_is_in() {
    let (mut state, _) = retried_with_text_parked();
    type_in(&mut state, typed("typed in a reopened composer"));
    let epoch = state.epoch();
    state.posted(Ok(()), thread());
    assert_ne!(state.epoch(), epoch, "the composer shows the parked text");
    assert_eq!(
        state.body(),
        &handed_back("typed in a reopened composer", &["typed meanwhile"]),
        "the text parked beside the message is still in the draft"
    );
    state.queue(
        typed("typed in a reopened composer, and more"),
        epoch,
        false,
    );
    assert_eq!(
        state.body(),
        &handed_back(
            "typed in a reopened composer, and more",
            &["typed meanwhile"]
        ),
        "typing before the composer shows the parked text doesn't drop it"
    );
}

#[test]
fn text_parked_beside_a_retried_send_survives_reopening_the_editor_while_it_is_on_its_way() {
    let (closing, _) = retried_with_text_parked();
    let mut state = reopened(closing.kept());
    assert_eq!(state.shown(), None);
    type_in(&mut state, typed("typed after reopening"));
    load(
        &mut state,
        3,
        &handed_back("send this", &["typed meanwhile"]),
    );
    let resent = post(&mut state);
    state.posted(Err(not_sent(&resent)), thread());
    assert_eq!(
        state.shown(),
        Some(&handed_back(
            "send this",
            &["typed meanwhile", "typed after reopening"]
        ))
    );
}

#[test]
fn typing_into_a_composer_that_has_not_shown_the_failed_send_yet_keeps_the_message() {
    let (mut state, first) = sent_and_typed_over("send this", "typed meanwhile");
    let shown_before_failing = state.epoch();
    state.posted(Err(not_sent(&first)), thread());
    state.queue(
        typed("typed meanwhile, and more"),
        shown_before_failing,
        false,
    );
    assert_eq!(
        state.shown(),
        Some(&handed_back("send this", &["typed meanwhile, and more"])),
        "the message stays in the draft, with the newest text parked beside it"
    );
    assert!(!state.conflict(), "nothing changed on another client");
}

#[test]
fn a_send_that_failed_before_closing_comes_back_on_reopening_with_its_identity() {
    let (mut closing, first) = sent_and_typed_over("send this", "typed meanwhile");
    closing.posted(Err(not_sent(&first)), thread());
    let mut state = reopened(closing.kept());
    assert_eq!(
        state.shown(),
        Some(&handed_back("send this", &["typed meanwhile"])),
        "both the failed message and the later text show before Forge answers"
    );
    assert!(
        state.send_failed(),
        "the composer still says the send failed"
    );
    load(&mut state, 2, &typed("send this"));
    state
        .submit(submission("after-reopening", "send this"))
        .unwrap();
    write(&mut state);
    saved(
        &mut state,
        3,
        &handed_back("send this", &["typed meanwhile"]),
    );
    assert_eq!(post(&mut state).message.id, first.message.id);
}

#[test]
fn a_draft_holding_only_a_failed_send_is_kept_open_and_kept_on_this_device() {
    let mut state = submitted("send this");
    write(&mut state);
    saved(&mut state, 2, &typed("send this"));
    let first = post(&mut state);
    state.posted(Err(not_sent(&first)), thread());
    state.retry();
    assert!(
        !state.evictable(),
        "closing other drafts never drops the failed message's identity"
    );
    assert!(
        state.keepable(),
        "closing the Editor keeps the failed message's identity on this device"
    );
}

#[test]
fn an_image_still_uploading_when_another_clients_draft_arrives_asks_which_to_keep() {
    let mut state = DraftSession::new("thread:thread".into());
    let shown = state.epoch();
    state.queue(DraftBody::default(), shown, true);
    load(&mut state, 3, &typed("theirs"));
    assert_eq!(
        state.epoch(),
        shown,
        "the composer keeps the pasted image instead of showing the other draft"
    );
    assert!(state.conflict(), "the user chooses which draft to keep");
}

#[test]
fn a_second_send_waits_for_the_first() {
    let mut state = submitted("first");
    assert!(!state.sendable(), "one message is sent at a time");
    type_in(&mut state, typed("second"));
    assert_eq!(
        state.submit(submission("second", "second")),
        Err(SendRefusal::AlreadySending)
    );
    write(&mut state);
    saved(&mut state, 2, &typed("second"));
    assert_eq!(
        post(&mut state).message.id,
        ItemId::from("message"),
        "the first message is still the one being sent"
    );
}

#[test]
fn a_follow_up_parked_behind_a_sent_message_becomes_the_draft_once_it_is_in() {
    let mut state = loaded(1, &DraftBody::default());
    let body = DraftBody {
        follow_ups: vec![typed("next")],
        ..typed("first")
    };
    type_in(&mut state, body.clone());
    state.submit(submission("message", "first")).unwrap();
    write(&mut state);
    saved(&mut state, 2, &body);
    let epoch = state.epoch();
    post(&mut state);
    state.posted(Ok(()), thread());
    assert_ne!(state.epoch(), epoch, "the composer shows the follow-up");
    assert_eq!(state.shown(), Some(&typed("next")));
    load(&mut state, 3, &DraftBody::default());
    assert_eq!(write(&mut state), (3, typed("next")));
}

#[test]
fn an_accepted_message_reloads_the_draft_it_cleared() {
    let mut state = submitted("send this");
    write(&mut state);
    saved(&mut state, 2, &typed("send this"));
    post(&mut state);
    state.posted(Ok(()), thread());
    load(&mut state, 4, &typed("typed elsewhere"));
    assert_eq!(
        state.shown(),
        Some(&typed("typed elsewhere")),
        "a draft saved elsewhere after the send shows"
    );
    assert_ne!(state.epoch(), 0, "the composer replaces its empty view");
}

#[test]
fn a_failed_upload_restarts_only_once_its_failure_is_cleared() {
    let mut state = loaded(0, &DraftBody::default());
    state.adopt(PendingUpload::kept(vec![1, 2, 3]));
    let uploads = state.next_uploads();
    let [(key, _)] = uploads.as_slice() else {
        panic!("expected the pasted image to start uploading");
    };
    assert_eq!(state.adopted(*key, Err(AdoptImageError::Closed)), None);
    assert!(
        !state.halted(),
        "an image from an earlier visit never holds up the draft's own saves"
    );
    let error = state.error().expect("the failed upload is shown");
    assert_eq!(
        error.headline(),
        "Couldn't upload an image pasted before leaving this draft."
    );
    assert_eq!(
        error.detail(),
        Some("the draft closed before the image finished uploading")
    );
    assert!(
        state.next_uploads().is_empty(),
        "a failed upload waits for the user instead of retrying on its own"
    );
    state.retry();
    assert_eq!(
        state.next_uploads().len(),
        1,
        "Try again uploads the kept image again"
    );
    assert!(state.pending(), "the image still waits for Forge");
}

fn refused_upload(state: &mut DraftSession, failure: Failure) {
    state.adopt(PendingUpload::kept(vec![1, 2, 3]));
    let uploads = state.next_uploads();
    let [(key, _)] = uploads.as_slice() else {
        panic!("expected the pasted image to start uploading");
    };
    let refusal = AdoptImageError::Upload(UploadImageError::Put(refused(failure)));
    assert_eq!(state.adopted(*key, Err(refusal)), None);
}

#[test]
fn an_image_forge_refuses_for_good_is_handed_back_without_holding_up_a_queued_send() {
    let mut state = submitted("send this");
    refused_upload(
        &mut state,
        Failure::Resource(ResourceFailure::TooManyPixels {
            width: 40_000,
            height: 40_000,
            limit: 100_000_000,
        }),
    );
    assert!(!state.halted(), "the refused image doesn't stop the draft");
    assert_eq!(state.error(), None);
    write(&mut state);
    saved(&mut state, 2, &typed("send this"));
    assert!(
        matches!(state.next_operation(), Some(Operation::Post(_))),
        "the message queued before the refusal is still sent"
    );
    state.posted(Ok(()), thread());
    state.retry();
    assert!(
        state.next_uploads().is_empty(),
        "an image Forge refused is not uploaded again"
    );
    let [lost] = state.lost() else {
        panic!("expected the refused image to be shown as failed");
    };
    assert_eq!(lost.name, "Pasted image");
}

#[test]
fn an_older_forge_refusing_an_image_as_a_bad_request_is_not_retried_forever() {
    let mut state = loaded(0, &DraftBody::default());
    refused_upload(
        &mut state,
        Failure::Legacy {
            code: ErrorCode::BadRequest,
        },
    );
    type_in(&mut state, typed("still typing"));
    write(&mut state);
    saved(&mut state, 1, &typed("still typing"));
    assert!(
        state.next_uploads().is_empty(),
        "the refused image isn't uploaded after every answer from Forge"
    );
    assert_eq!(state.lost().len(), 1, "it shows as a failed image instead");
}

#[test]
fn a_refused_image_is_kept_with_the_draft_until_the_user_removes_it() {
    let mut state = loaded(0, &DraftBody::default());
    refused_upload(&mut state, Failure::Resource(ResourceFailure::NotWebp));
    assert!(
        state.keepable(),
        "closing the Editor keeps the failed image on this device"
    );
    let mut state = reopened(state.kept());
    let [lost] = state.lost() else {
        panic!("expected the failed image to come back after reopening");
    };
    assert_eq!(lost.name, "Pasted image");
    let key = lost.key;
    state.dismiss_lost(key);
    load(&mut state, 0, &DraftBody::default());
    assert!(
        !state.keepable(),
        "a removed failed image is not kept any more"
    );
}

#[test]
fn removing_the_pairing_empties_the_draft_view() {
    let mut state = loaded(1, &typed("draft"));
    let epoch = state.epoch();
    state.remove();
    assert_ne!(state.epoch(), epoch, "the composer clears what it shows");
    assert_eq!(state.body(), &DraftBody::default());
    assert!(
        !state.queue(typed("more"), state.epoch(), false),
        "nothing is saved"
    );
}

fn set_aside_recovered(text: &str) -> (DraftSession, Submission) {
    let refused_message = submission("set-aside", text);
    let kept: KeptDraft = serde_json::from_value(serde_json::json!({
        "body": {"text": "typed since"},
        "set_aside": [refused_message],
    }))
    .unwrap();
    let mut state = DraftSession::new("new".into());
    state.recover(kept, Vec::new(), Vec::new());
    assert!(matches!(
        state.next_operation(),
        Some(Operation::PostSetAside(_))
    ));
    (state, refused_message)
}

fn set_aside_refused(failure: Failure, submission: &Submission) -> Result<(), PostMessageError> {
    Err(PostMessageError::Send {
        thread: submission.thread.clone(),
        message: submission.message.id.clone(),
        source: refused(failure),
    })
}

#[test]
fn a_set_aside_message_forge_refuses_for_good_comes_back_as_a_failed_message() {
    let (mut state, refused_message) = set_aside_recovered("its image was collected");

    assert!(state.posted_set_aside(set_aside_refused(
        Failure::Run(RunFailure::ImageMissing {
            blob: BlobHash("0".repeat(64)),
        }),
        &refused_message,
    )));

    let kept = state.kept();
    assert!(
        kept.set_aside.is_empty(),
        "the device record no longer queues it"
    );
    assert_eq!(state.posting(), Posting::Failed);
    assert_eq!(state.body().text, "its image was collected");
    assert_eq!(
        state
            .body()
            .follow_ups
            .iter()
            .map(|body| body.text.as_str())
            .collect::<Vec<_>>(),
        ["typed since"],
        "what was typed since stays beside it"
    );
    assert!(state.error().is_some(), "the refusal is shown");

    state.queue(
        DraftBody {
            text: "sent instead".to_owned(),
            ..DraftBody::default()
        },
        state.epoch(),
        false,
    );
    assert!(matches!(state.next_operation(), Some(Operation::Load)));
    state.loaded(Ok(artisan_api::DraftSnapshot::default()));
    state.submit(submission("next", "sent instead")).unwrap();
    let posted = loop {
        match state.next_operation() {
            Some(Operation::Write { body, .. }) => saved(&mut state, 1, &body),
            operation => break operation,
        }
    };
    assert!(
        matches!(posted, Some(Operation::Post(_))),
        "the next message isn't held behind the refused one: {posted:?}"
    );
}

#[test]
fn a_set_aside_message_that_fails_for_now_waits_to_be_sent_again() {
    let (mut state, refused_message) = set_aside_recovered("forge was busy");

    assert!(!state.posted_set_aside(set_aside_refused(Failure::Internal, &refused_message)));

    assert_eq!(state.kept().set_aside, [refused_message]);
    state.retry();
    assert!(matches!(
        state.next_operation(),
        Some(Operation::PostSetAside(_))
    ));
}

#[test]
fn a_message_sent_from_a_removed_draft_is_refused_instead_of_dropped() {
    let mut state = loaded(1, &typed("hello"));
    state.remove();

    assert_eq!(
        state.submit(submission("message", "hello")),
        Err(SendRefusal::NoDraft)
    );
}
