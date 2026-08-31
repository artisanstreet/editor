//! Focused checks for attaching the configured dispatcher to Forge startup.

use std::{
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use artisan_native_engine::NativeOpenCode2Authority;
use artisan_transport::CancelHandle;

use super::{ForgeLaunchConfig, ListenerLimits};
use crate::{NativeRunDispatcherConfig, conversation_commit_notifier::ConversationCommitNotifier};

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
        Duration::from_millis(10),
        Duration::from_millis(10),
        Duration::from_millis(10),
        Duration::from_millis(10),
        NonZeroUsize::new(1).expect("one queue slot is nonzero"),
        NonZeroUsize::new(1).expect("one retry is nonzero"),
        "queue",
        0,
    )
    .expect("complete dispatcher policy");
}

#[test]
fn forge_debug_records_configured_dispatcher_without_authority_details() {
    let config = ForgeLaunchConfig::new(
        path("forge"),
        path("custody"),
        vec![path("certificate")],
        path("private-key"),
        path("bootstrap"),
        path("ready"),
        ListenerLimits {
            admission: Duration::from_millis(10),
            handshake: Duration::from_millis(10),
            next_request: Duration::from_millis(10),
            drain: Duration::from_millis(10),
        },
        NonZeroU32::new(1).expect("one admission slot is nonzero"),
        NonZeroU32::new(1).expect("one request is nonzero"),
        Arc::new(CancelHandle::new()),
    )
    .expect("explicit Forge configuration");
    let config = config.with_native_run_dispatcher(dispatcher());
    let debug = format!("{config:?}");

    assert!(debug.contains("native_run: Some(\"configured\")"));
    assert!(!debug.contains("prompt-selector"));
    assert!(!debug.contains("profiles.json"));
}
