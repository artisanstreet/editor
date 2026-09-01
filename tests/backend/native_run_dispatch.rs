//! Focused checks for the explicit configured-run scheduler boundary.

use std::{num::NonZeroUsize, path::PathBuf, sync::Arc, time::Duration};

use artisan_database::{RepositoryError, RunLaunchError, SqliteConfig, connect};
use artisan_domain::MessageId;
use artisan_migrations::migrate_to_current;
use artisan_native_engine::NativeOpenCode2Authority;
use artisan_transport::CancelHandle;

use super::{
    NativeRunDispatcherConfig, NativeRunDispatcherConfigError, NativeRunDispatcherConfigInput,
    conversation_commit_notifier::ConversationCommitNotifier,
};
use crate::native_run_dispatch::{
    LaunchAuthority, NativeRunDispatcher, NativeRunDispatcherShutdown, PromptAuthorization,
    SettingsLoadDecision, classify_launch_result, classify_settings_load, notify_after_commit,
    prompt_authorization_after_binding,
};

fn config(
    prompt_delivery: &str,
) -> Result<NativeRunDispatcherConfig, NativeRunDispatcherConfigError> {
    NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::from_millis(10),
            poll_interval: Duration::from_millis(10),
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_millis(10),
            queue_capacity: NonZeroUsize::new(1).expect("one queue slot is nonzero"),
            max_command_retries: NonZeroUsize::new(1).expect("one retry is nonzero"),
            prompt_delivery: prompt_delivery.to_owned(),
            stream_after: 0,
        },
    )
}

#[test]
fn scheduler_requires_positive_injected_durations() {
    let error = NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::ZERO,
            poll_interval: Duration::from_millis(10),
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_millis(10),
            queue_capacity: NonZeroUsize::new(1).expect("one queue slot is nonzero"),
            max_command_retries: NonZeroUsize::new(1).expect("one retry is nonzero"),
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
    )
    .expect_err("zero claim lease must be rejected");
    assert_eq!(error, NativeRunDispatcherConfigError::ZeroDuration);
}

#[test]
fn scheduler_rejects_unstructured_prompt_selector() {
    let error = config("queue\n").expect_err("line breaks must not enter the selector");
    assert_eq!(
        error,
        NativeRunDispatcherConfigError::InvalidPromptDeliveryCharacter
    );
}

#[test]
fn scheduler_debug_contains_policy_shape_without_prompt_bytes() {
    let config = config("prompt-selector-sentinel").expect("complete scheduler policy");
    let debug = format!("{config:?}");
    assert!(debug.contains("claim_lease"));
    assert!(debug.contains("prompt_delivery_bytes"));
    assert!(!debug.contains("prompt-selector-sentinel"));
    assert!(!debug.contains("profiles.json"));
}

#[test]
fn unconfigured_settings_requeue_before_provider_launch() {
    let decision = classify_settings_load(Ok(None));
    assert_eq!(
        decision,
        SettingsLoadDecision::Requeue("engine unconfigured")
    );

    let decision = classify_settings_load(Err(RepositoryError::Database {
        operation: "read settings",
        source: sea_orm::DbErr::Custom("temporary".to_owned()),
    }));
    assert_eq!(
        decision,
        SettingsLoadDecision::Requeue("engine settings unavailable")
    );
}

#[test]
fn launch_snapshot_mismatch_is_requeued_without_provider_authority() {
    let mismatch = Err(RunLaunchError::SnapshotMismatch {
        message_id: MessageId::parse("message-snapshot").expect("bounded message id"),
    });
    assert_eq!(classify_launch_result(&mismatch), LaunchAuthority::Requeue);
}

#[test]
fn provider_binding_controls_prompt_authorization() {
    assert_eq!(
        prompt_authorization_after_binding(false),
        PromptAuthorization::Authorize
    );
    assert_eq!(
        prompt_authorization_after_binding(true),
        PromptAuthorization::DoNotAuthorize
    );
}

#[test]
fn observations_notify_only_after_sqlite_commit() {
    let mut notifications = Vec::new();
    assert!(!notify_after_commit(false, || notifications.push("not committed")));
    assert!(notifications.is_empty());
    assert!(notify_after_commit(true, || notifications.push("committed")));
    assert_eq!(notifications, ["committed"]);
}

#[tokio::test(flavor = "current_thread")]
async fn dispatcher_shutdown_joins_owner_custody() {
    let database = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("memory database should open");
    migrate_to_current(&database)
        .await
        .expect("memory database should migrate");
    let repository = artisan_database::Repository::new(database);
    let process_cancel = Arc::new(CancelHandle::new());
    let mut dispatcher = NativeRunDispatcher::start(
        repository,
        PathBuf::from("C:/forge/database.sqlite3"),
        config("queue").expect("complete dispatcher policy"),
        Arc::clone(&process_cancel),
        &tokio::runtime::Handle::current(),
    );

    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
}
