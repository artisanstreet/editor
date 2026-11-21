use super::*;
use artisan_api::tls::{Identity, fingerprint};
use artisan_api::{DraftSnapshot, DraftWriteResult};

fn pending_write() -> (DraftHandle, DraftBody, tempfile::TempDir) {
    let spool = tempfile::tempdir().unwrap();
    let identity = Identity::generate("deadline-test").unwrap();
    let pin = fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = ForgeClient::new("127.0.0.1:1", &pin, &identity).unwrap();
    let mut state = DraftSession::new("new".into());
    assert!(matches!(state.next_operation(), Some(Operation::Load)));
    state.loaded(Ok(DraftSnapshot::default()));
    state.queue(
        DraftBody {
            text: "pending".to_owned(),
            ..DraftBody::default()
        },
        0,
        false,
    );
    let Some(Operation::Write { body, .. }) = state.next_operation() else {
        panic!("expected a pending write");
    };
    let (changed, _) = watch::channel(0);
    let handle = DraftHandle(Arc::new(Inner {
        host: HostId("deadline-test".into()),
        scope: "new".into(),
        client,
        spool: DraftSpool::new(spool.path()),
        kept: Mutex::new(Kept::Nothing),
        shared: Mutex::new(Shared {
            state,
            viewers: Vec::new(),
            timer: None,
            version: 0,
            forgetting: false,
        }),
        changed,
        cancelled: CancellationToken::new(),
    }));
    (handle, *body, spool)
}

#[test]
fn acknowledgement_before_deadline_recheck_is_saved_without_cancellation() {
    let (handle, written, _spool) = pending_write();
    assert!(handle.read(DraftSession::pending));
    handle.mutate(|shared| {
        shared.state.written(Ok(DraftWriteResult {
            saved: true,
            current: DraftSnapshot {
                revision: 1,
                body: written.to_wire(),
            },
        }));
    });
    assert_eq!(handle.deadline_outcome(), ShutdownOutcome::Saved);
    assert!(!handle.0.cancelled.is_cancelled());
}

#[test]
fn rejection_before_deadline_recheck_is_a_failure_without_cancellation() {
    let (handle, _, _spool) = pending_write();
    assert!(handle.read(DraftSession::pending));
    handle.mutate(|shared| {
        shared
            .state
            .written(Err(test_support::editor::forge::inconsistent(
                "/drafts/new",
                "rejected",
            )));
    });
    assert_eq!(
        handle.deadline_outcome(),
        ShutdownOutcome::Incomplete {
            failed: 1,
            timed_out: 0,
            kept_locally: 0,
        }
    );
    assert!(!handle.0.cancelled.is_cancelled());
}

#[test]
fn unacknowledged_write_at_deadline_is_cancelled_and_counted_once() {
    let (handle, _, _spool) = pending_write();
    assert_eq!(
        handle.deadline_outcome(),
        ShutdownOutcome::Incomplete {
            failed: 0,
            timed_out: 1,
            kept_locally: 0,
        }
    );
    assert!(handle.0.cancelled.is_cancelled());
    assert!(handle.read(DraftSession::pending));
}
