//! Dependency-free transition coverage for the thread-title settings policy.
//!
//! The production module is linked by path so these tests do not require the
//! VP-owned frontend registration, transport, streams, or UI build.

#[path = "../../modules/frontend/src/thread_title_settings_policy.rs"]
mod thread_title_settings_policy;

use thread_title_settings_policy::{
    THREAD_TITLE_SETTINGS_SAVE_FAILURE_MESSAGE, ThreadTitleMode,
    ThreadTitleSettingsAuthoritativeState, ThreadTitleSettingsPersistenceCommand,
    ThreadTitleSettingsSaveOutcome, ThreadTitleSettingsSaveRejection, ThreadTitleSettingsState,
};

fn state(available: bool, mode: ThreadTitleMode) -> ThreadTitleSettingsState {
    ThreadTitleSettingsState::new(ThreadTitleSettingsAuthoritativeState::new(available, mode))
}

fn requested_mode(command: &ThreadTitleSettingsPersistenceCommand) -> &ThreadTitleMode {
    command.requested_mode()
}

#[test]
fn recognized_modes_are_exact_and_summary_is_the_default() {
    assert_eq!(ThreadTitleMode::default(), ThreadTitleMode::Summary);
    assert_eq!(
        ThreadTitleMode::ALL,
        [ThreadTitleMode::Summary, ThreadTitleMode::LatestMessage]
    );
    assert_eq!(ThreadTitleMode::Summary.as_raw(), "summary");
    assert_eq!(ThreadTitleMode::LatestMessage.as_str(), "latest_message");
    assert!(ThreadTitleMode::Summary.is_summary());
    assert!(!ThreadTitleMode::LatestMessage.is_summary());
}

#[test]
fn presentation_derivations_cover_modes_availability_and_save_flight() {
    for (mode, summarized) in [
        (ThreadTitleMode::Summary, true),
        (ThreadTitleMode::LatestMessage, false),
        (ThreadTitleMode::from_raw("future_mode"), false),
    ] {
        for (available, saving, disabled) in [
            (true, false, false),
            (true, true, true),
            (false, false, true),
            (false, true, true),
        ] {
            let mut current = state(available, mode.clone());
            current.saving = saving;

            assert_eq!(current.summarized(), summarized);
            assert_eq!(current.is_summarized(), summarized);
            assert_eq!(current.switch_disabled(), disabled);
            assert_eq!(current.disabled(), disabled);
            assert_eq!(current.can_admit_toggle(), !saving);
        }
    }
}

#[test]
fn both_toggle_directions_clear_message_start_saving_and_keep_mode_authoritative() {
    let mut current = state(true, ThreadTitleMode::LatestMessage);
    current.message = "old failure".to_owned();

    let enable = current.admit_toggle(true).expect("an idle toggle admits");
    assert_eq!(
        enable,
        ThreadTitleSettingsPersistenceCommand::SetThreadTitleMode {
            mode: ThreadTitleMode::Summary,
        }
    );
    assert_eq!(requested_mode(&enable), &ThreadTitleMode::Summary);
    assert_eq!(enable.clone().into_mode(), ThreadTitleMode::Summary);
    assert_eq!(
        ThreadTitleSettingsPersistenceCommand::operation(),
        "session.defaults.update"
    );
    assert!(current.saving);
    assert!(current.message.is_empty());
    assert_eq!(current.mode(), &ThreadTitleMode::LatestMessage);

    current.save_succeeded();
    assert!(!current.saving);

    let disable = current
        .start_toggle(false)
        .expect("the settled toggle admits");
    assert_eq!(
        disable,
        ThreadTitleSettingsPersistenceCommand::SetThreadTitleMode {
            mode: ThreadTitleMode::LatestMessage,
        }
    );
    assert!(current.saving);
    assert_eq!(current.mode(), &ThreadTitleMode::LatestMessage);
    current.finish_save(ThreadTitleSettingsSaveOutcome::Succeeded);
    assert!(!current.saving);
}

#[test]
fn unavailable_state_is_disabled_but_the_source_guard_still_admits_when_idle() {
    let mut current = state(false, ThreadTitleMode::LatestMessage);

    assert!(current.disabled());
    assert!(current.can_admit_toggle());
    let command = current
        .toggle()
        .expect("availability is not the source guard");
    assert_eq!(
        command,
        ThreadTitleSettingsPersistenceCommand::SetThreadTitleMode {
            mode: ThreadTitleMode::Summary,
        }
    );
    assert!(current.saving);
    assert!(current.disabled());
}

#[test]
fn duplicate_toggle_is_rejected_without_disturbing_the_existing_flight() {
    let mut current = state(true, ThreadTitleMode::Summary);
    current.message = "keep only until admission".to_owned();
    let first = current.admit_toggle(false).expect("first toggle admits");
    let snapshot = current.clone();

    assert_eq!(
        current.admit_toggle(true),
        Err(ThreadTitleSettingsSaveRejection::Saving)
    );
    assert_eq!(current, snapshot);
    assert_eq!(
        first,
        ThreadTitleSettingsPersistenceCommand::SetThreadTitleMode {
            mode: ThreadTitleMode::LatestMessage,
        }
    );
}

#[test]
fn streamed_defaults_replace_available_and_mode_while_a_save_is_in_flight() {
    let mut current = state(true, ThreadTitleMode::Summary);
    let _command = current.admit_toggle(false).expect("save starts");
    current.message = "stream must not erase local status".to_owned();

    current.apply_authoritative_update(ThreadTitleSettingsAuthoritativeState::new(
        false,
        ThreadTitleMode::LatestMessage,
    ));

    assert!(!current.available);
    assert_eq!(current.mode(), &ThreadTitleMode::LatestMessage);
    assert!(current.saving);
    assert_eq!(current.message, "stream must not erase local status");
    let authoritative = current.authoritative();
    assert_eq!(authoritative.mode(), &ThreadTitleMode::LatestMessage);
    assert_eq!(
        authoritative,
        ThreadTitleSettingsAuthoritativeState::new(false, ThreadTitleMode::LatestMessage)
    );
}

#[test]
fn failure_always_clears_saving_and_reports_the_exact_message_without_flipping_mode() {
    let mut current = state(true, ThreadTitleMode::Summary);
    let _command = current.admit_toggle(false).expect("save starts");
    current.save_failed();

    assert!(!current.saving);
    assert_eq!(current.message, THREAD_TITLE_SETTINGS_SAVE_FAILURE_MESSAGE);
    assert_eq!(
        current.message,
        "Couldn't verify the new default. Forge did not confirm the change."
    );
    assert_eq!(current.mode(), &ThreadTitleMode::Summary);

    // A new admitted toggle clears the old error before the next completion.
    let retry = current.admit_toggle(false).expect("retry admits");
    assert!(current.message.is_empty());
    assert!(current.saving);
    assert_eq!(
        retry,
        ThreadTitleSettingsPersistenceCommand::SetThreadTitleMode {
            mode: ThreadTitleMode::LatestMessage,
        }
    );
    current.save_succeeded();
    assert!(!current.saving);
    assert!(current.message.is_empty());
    assert_eq!(current.mode(), &ThreadTitleMode::Summary);
}

#[test]
fn success_keeps_a_prior_message_if_no_new_admission_cleared_it() {
    let mut current = state(true, ThreadTitleMode::LatestMessage);
    current.message = "prior status".to_owned();
    current.saving = true;

    current.save_succeeded();

    assert!(!current.saving);
    assert_eq!(current.message, "prior status");
}

#[test]
fn unknown_empty_and_unicode_modes_are_lossless_and_non_summary() {
    for raw in ["", "future_mode", " 未来 🚀 "] {
        let mode = ThreadTitleMode::from_raw(raw);
        assert_eq!(mode.as_raw(), raw);
        assert_eq!(mode.as_str(), raw);
        assert!(!mode.is_summary());

        let mut current = state(true, mode.clone());
        assert!(!current.summarized());
        let command = current.toggle().expect("unknown mode still admits toggle");
        assert_eq!(requested_mode(&command), &ThreadTitleMode::Summary);
        assert_eq!(current.mode(), &mode);
        current.save_succeeded();
    }

    let raw = String::from("未来のタイトルモード 🧵");
    let pointer = raw.as_ptr();
    let mode = ThreadTitleMode::from_owned(raw);
    assert_eq!(
        mode,
        ThreadTitleMode::Unknown(String::from("未来のタイトルモード 🧵"))
    );
    assert_eq!(mode.as_raw().as_ptr(), pointer);
    assert_eq!(mode.clone().into_raw(), "未来のタイトルモード 🧵");
}
