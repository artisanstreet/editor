use super::*;
use artisan_api::tls::{Identity, fingerprint};
use artisan_api::{DraftSnapshot, DraftWriteResult};
use serde_json::json;

fn pending_write() -> (DraftHandle, artisan_api::WriteDraft, u64) {
    let identity = Identity::generate("deadline-test").unwrap();
    let pin = fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = ForgeClient::new("127.0.0.1:1", &pin, &identity).unwrap();
    let mut state = DraftSession::new("new".into());
    state.loaded(Ok(DraftSnapshot::default()));
    state.queue(json!({"text": "pending"}));
    let Some(Operation::Write { request, edited }) = state.next() else {
        panic!("expected a pending write");
    };
    let (changed, _) = watch::channel(0);
    let handle = DraftHandle(Arc::new(Inner {
        client,
        shared: Mutex::new(Shared {
            state,
            viewers: Vec::new(),
            timer: None,
            version: 0,
        }),
        changed,
        cancelled: CancellationToken::new(),
    }));
    (handle, request, edited)
}

#[test]
fn acknowledgement_before_deadline_recheck_is_saved_without_cancellation() {
    let (handle, request, edited) = pending_write();
    assert!(handle.read(|state| state.pending()));
    handle.mutate(|shared| {
        shared.state.written(
            &request,
            edited,
            Ok(DraftWriteResult {
                saved: true,
                current: DraftSnapshot {
                    revision: 1,
                    body: request.body.clone(),
                },
            }),
        );
    });
    assert_eq!(handle.deadline_outcome(), ShutdownOutcome::Saved);
    assert!(!handle.0.cancelled.is_cancelled());
}

#[test]
fn rejection_before_deadline_recheck_is_a_failure_without_cancellation() {
    let (handle, request, edited) = pending_write();
    assert!(handle.read(|state| state.pending()));
    handle.mutate(|shared| {
        shared
            .state
            .written(&request, edited, Err("rejected".into()))
    });
    assert_eq!(
        handle.deadline_outcome(),
        ShutdownOutcome::Incomplete {
            failed: 1,
            timed_out: 0
        }
    );
    assert!(!handle.0.cancelled.is_cancelled());
}

#[test]
fn unacknowledged_write_at_deadline_is_cancelled_and_counted_once() {
    let (handle, _, _) = pending_write();
    assert_eq!(
        handle.deadline_outcome(),
        ShutdownOutcome::Incomplete {
            failed: 0,
            timed_out: 1
        }
    );
    assert!(handle.0.cancelled.is_cancelled());
    assert!(handle.read(|state| state.pending()));
}
