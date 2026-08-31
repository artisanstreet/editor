//! Focused checks for the explicit configured-run scheduler boundary.

use std::time::Duration;

use artisan_native_engine::NativeOpenCode2Authority;
use std::num::NonZeroUsize;

use super::{
    NativeRunDispatcherConfig, NativeRunDispatcherConfigError,
    conversation_commit_notifier::ConversationCommitNotifier,
};

fn config(
    prompt_delivery: &str,
) -> Result<NativeRunDispatcherConfig, NativeRunDispatcherConfigError> {
    NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        Duration::from_millis(10),
        Duration::from_millis(10),
        Duration::from_millis(10),
        Duration::from_millis(10),
        NonZeroUsize::new(1).expect("one queue slot is nonzero"),
        NonZeroUsize::new(1).expect("one retry is nonzero"),
        prompt_delivery,
        0,
    )
}

#[test]
fn scheduler_requires_positive_injected_durations() {
    let error = NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        Duration::ZERO,
        Duration::from_millis(10),
        Duration::from_millis(10),
        Duration::from_millis(10),
        NonZeroUsize::new(1).expect("one queue slot is nonzero"),
        NonZeroUsize::new(1).expect("one retry is nonzero"),
        "queue",
        0,
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
