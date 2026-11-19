use super::*;
use artisan_api::{DraftSnapshot, DraftWriteResult};
use serde_json::json;

fn typed(text: &str) -> DraftBody {
    DraftBody {
        text: text.to_owned(),
        ..DraftBody::default()
    }
}

fn in_conflict() -> DraftSession {
    let mut state = DraftSession::new("new".into());
    assert!(matches!(state.next_operation(), Some(Operation::Load)));
    state.loaded(Ok(DraftSnapshot::default()));
    state.queue(typed("mine"));
    let Some(Operation::Write { body, edited, .. }) = state.next_operation() else {
        panic!("expected the typed draft to be written");
    };
    state.written(
        &body,
        edited,
        Ok(DraftWriteResult {
            saved: false,
            current: DraftSnapshot {
                revision: 3,
                body: json!({"text": "theirs"}),
            },
        }),
    );
    assert!(state.conflict());
    state
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
fn a_failed_upload_restarts_only_once_its_failure_is_cleared() {
    let mut state = DraftSession::new("new".into());
    assert!(matches!(state.next_operation(), Some(Operation::Load)));
    state.loaded(Ok(DraftSnapshot::default()));
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
    assert_eq!(
        state.error().as_deref(),
        Some(
            "Couldn't upload an image pasted before leaving this draft: the draft closed before the image finished uploading"
        )
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
    assert!(state.pending());
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

fn submission() -> Submission {
    Submission {
        message: SendMessage {
            id: ItemId::from("message"),
            text: "send this".to_owned(),
            images: Vec::new(),
            config: None,
            draft: None,
        },
        create: None,
        thread: ThreadId::from("thread"),
    }
}

#[test]
fn an_image_forge_refuses_for_good_is_handed_back_without_holding_up_a_queued_send() {
    let mut state = DraftSession::new("thread:thread".into());
    assert!(matches!(state.next_operation(), Some(Operation::Load)));
    state.loaded(Ok(DraftSnapshot::default()));
    state.adopt(PendingUpload::kept(vec![1, 2, 3]));
    let uploads = state.next_uploads();
    let [(key, _)] = uploads.as_slice() else {
        panic!("expected the pasted image to start uploading");
    };
    state.queue(typed("send this"));
    let Some(Operation::Write { body, edited, .. }) = state.next_operation() else {
        panic!("expected the typed draft to be written");
    };
    state.written(
        &body,
        edited,
        Ok(DraftWriteResult {
            saved: true,
            current: DraftSnapshot {
                revision: 1,
                body: json!({"text": "send this"}),
            },
        }),
    );
    state.submit(submission());
    let refusal = AdoptImageError::Upload(UploadImageError::Put(refused(Failure::Resource(
        ResourceFailure::TooManyPixels {
            width: 40_000,
            height: 40_000,
            limit: 100_000_000,
        },
    ))));
    assert_eq!(state.adopted(*key, Err(refusal)), None);
    assert!(!state.halted());
    assert_eq!(state.error(), None);
    assert!(
        matches!(state.next_operation(), Some(Operation::Post(_))),
        "the message queued before the refusal is still sent"
    );
    state.posted(Ok(()), ThreadId::from("thread"));
    state.retry();
    assert!(
        state.next_uploads().is_empty(),
        "an image Forge refused is not uploaded again"
    );
    let lost = state.take_lost_images();
    let [lost] = lost.as_slice() else {
        panic!("expected the refused image to be handed back once");
    };
    assert_eq!(lost.name, "Pasted image");
    assert!(state.take_lost_images().is_empty());
    assert!(!state.pending());
}
