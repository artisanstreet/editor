use super::*;
use artisan_api::DraftSnapshot;

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
