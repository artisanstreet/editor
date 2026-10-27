//! Late-enrollment activation: one live waiter observes the confirm file
//! landing, shutdown wakes it, unconfirmed bindings never resolve.
//!
//! Path-included into `src/account_service.rs`, so `super::` reaches the
//! private helper. Not an integration target (`autotests = false`).

use super::*;
use std::time::Duration;

const POLL: Duration = Duration::from_millis(10);

fn write_identity(dir: &std::path::Path, account_id: Option<&str>) {
    use base64::Engine as _;
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0x21_u8; 32]);
    let file = serde_json::json!({
        "secret_b64u": secret,
        "machine_id": "mch_11111111111111111111111111",
        "account_id": account_id,
    });
    std::fs::write(dir.join("ast_account.json"), serde_json::to_vec(&file).unwrap()).unwrap();
}

/// The same waiter stays pending while accountless, then resolves once
/// the confirm file lands: no restart, no second future.
#[tokio::test]
async fn late_enrollment_activates_without_restart() {
    let dir = tempfile::tempdir().unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let waiter = await_enrollment(dir.path(), &shutdown, POLL);
    tokio::pin!(waiter);
    let parked = tokio::time::timeout(Duration::from_millis(50), &mut waiter).await;
    assert!(
        parked.is_err(),
        "an accountless Forge must keep waiting, not resolve"
    );
    write_identity(dir.path(), Some("acct_00000000000000000000000000"));
    let identity = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        identity.machine_id, "mch_11111111111111111111111111",
        "the live waiter must return the enrolled machine"
    );
    assert_eq!(
        identity.account_id.as_deref(),
        Some("acct_00000000000000000000000000"),
        "the live waiter must return the confirmed binding"
    );
}

/// A waiter parked in a long sleep still wakes promptly on cancel, with
/// no identity.
#[tokio::test]
async fn shutdown_cancels_enrollment_wait() {
    let dir = tempfile::tempdir().unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let waiter = await_enrollment(dir.path(), &shutdown, Duration::from_secs(3600));
    tokio::pin!(waiter);
    let parked = tokio::time::timeout(Duration::from_millis(50), &mut waiter).await;
    assert!(
        parked.is_err(),
        "the waiter must park in its sleep, not resolve"
    );
    shutdown.cancel();
    let done = tokio::time::timeout(Duration::from_secs(5), waiter).await;
    assert!(
        done.is_ok(),
        "cancel must wake the waiter despite the long sleep"
    );
    assert!(
        done.unwrap().is_none(),
        "a cancelled wait must not yield an identity"
    );
}

/// The same waiter ignores an unconfirmed enrollment, then resolves once
/// the binding completes.
#[tokio::test]
async fn incomplete_binding_keeps_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    write_identity(dir.path(), None);
    let waiter = await_enrollment(dir.path(), &shutdown, POLL);
    tokio::pin!(waiter);
    let parked = tokio::time::timeout(Duration::from_millis(50), &mut waiter).await;
    assert!(
        parked.is_err(),
        "an unconfirmed enrollment must not resolve"
    );
    write_identity(dir.path(), Some("acct_00000000000000000000000000"));
    let identity = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        identity.account_id.as_deref(),
        Some("acct_00000000000000000000000000"),
        "completing the binding must resolve the live waiter"
    );
}
