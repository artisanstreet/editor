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
    assert!(state.halted());
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
