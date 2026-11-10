use super::*;

#[tokio::test]
async fn a_failed_cancellation_stop_is_reported_as_failed_without_retry() {
    let work = tempfile::tempdir().unwrap();
    let backend = script(vec![Reply::Waits(vec![start("m1")])]);
    *backend.stop_error.lock().unwrap() = Some(Refusal::Overloaded("stop refused".to_owned()));
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    let mut user: User = Box::new(move |event| {
        if matches!(event, EngineEvent::Model { .. }) {
            stopper.cancel();
        }
        Vec::new()
    });
    let (outcome, _, _) = run_with(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        &mut user,
        cancel,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.retry, None);
    assert!(
        outcome
            .error
            .unwrap()
            .contains("Could not stop claude.ai completion")
    );
    assert_eq!(backend.sent.lock().unwrap().len(), 1);
}
