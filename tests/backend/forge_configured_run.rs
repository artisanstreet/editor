//! Focused checks for attaching the configured dispatcher to Forge startup.

use std::{
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use artisan_native_engine::NativeOpenCode2Authority;
use artisan_transport::CancelHandle;

use super::{ForgeLaunchConfig, ForgeLaunchConfigInput, ListenerLimits};
use crate::{
    NativeRunDispatcherConfig, NativeRunDispatcherConfigInput,
    conversation_commit_notifier::ConversationCommitNotifier,
};

fn path(name: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!("C:\\temp\\{name}.bin"))
    } else {
        PathBuf::from(format!("/tmp/{name}.bin"))
    }
}

fn dispatcher() -> NativeRunDispatcherConfig {
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
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
    )
    .expect("complete dispatcher policy")
}

#[test]
fn forge_debug_records_configured_dispatcher_without_authority_details() {
    let config = ForgeLaunchConfig::new(ForgeLaunchConfigInput {
        database: path("forge"),
        custody: path("custody"),
        certificate_der: vec![path("certificate")],
        private_key_der: path("private-key"),
        bootstrap_capability: path("bootstrap"),
        ready_file: path("ready"),
        limits: ListenerLimits {
            admission: Duration::from_millis(10),
            handshake: Duration::from_millis(10),
            next_request: Duration::from_millis(10),
            drain: Duration::from_millis(10),
        },
        admission_capacity: NonZeroU32::new(1).expect("one admission slot is nonzero"),
        requests_per_connection: NonZeroU32::new(1).expect("one request is nonzero"),
        native_run: dispatcher(),
        cancel: Arc::new(CancelHandle::new()),
    })
    .expect("explicit Forge configuration");
    let debug = format!("{config:?}");

    assert!(debug.contains("native_run: \"configured\""));
    assert!(!debug.contains("prompt-selector"));
    assert!(!debug.contains("profiles.json"));
}
