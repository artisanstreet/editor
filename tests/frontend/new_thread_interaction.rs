//! Focused dependency-free coverage for new-thread pre-creation policies.
//!
//! The production module is path-linked deliberately: this packet does not
//! edit the shared frontend module registration, so the harness can run with
//! plain `rustc --test` and no Cargo or Bazel dependencies.

#[path = "../../modules/frontend/src/new_thread_interaction.rs"]
mod new_thread_interaction;

use new_thread_interaction::{
    NewThreadActivation, is_unmodified_primary_activation, new_thread_draft_key,
};

fn activation(button: i32, modifiers: u8) -> NewThreadActivation {
    NewThreadActivation {
        alt_key: modifiers & 0b0001 != 0,
        button,
        ctrl_key: modifiers & 0b0010 != 0,
        meta_key: modifiers & 0b0100 != 0,
        shift_key: modifiers & 0b1000 != 0,
    }
}

#[test]
fn absent_workspace_uses_the_shared_new_thread_slot() {
    assert_eq!(new_thread_draft_key(None), "draft:new-thread");
}

#[test]
fn present_workspace_is_inserted_verbatim() {
    assert_eq!(
        new_thread_draft_key(Some("workspace-123")),
        "draft:workspace-123"
    );
}

#[test]
fn empty_workspace_is_distinct_from_an_absent_workspace() {
    assert_eq!(new_thread_draft_key(Some("")), "draft:");
    assert_ne!(new_thread_draft_key(Some("")), new_thread_draft_key(None));
}

#[test]
fn special_workspace_characters_are_not_normalized() {
    assert_eq!(
        new_thread_draft_key(Some(" Workspace ID/?&=+%#[]:é\t")),
        "draft: Workspace ID/?&=+%#[]:é\t"
    );
}

#[test]
fn every_modifier_combination_rejects_all_but_the_unmodified_primary_click() {
    for modifiers in 0_u8..16_u8 {
        let expected = modifiers == 0;
        assert_eq!(
            is_unmodified_primary_activation(activation(0, modifiers)),
            expected,
            "modifier mask {modifiers:04b}"
        );
    }
}

#[test]
fn every_nonzero_button_including_negative_is_rejected() {
    for button in [-1, 1, 2, 3, i32::MIN, i32::MAX] {
        for modifiers in 0_u8..16_u8 {
            assert!(
                !is_unmodified_primary_activation(activation(button, modifiers)),
                "button {button} with modifier mask {modifiers:04b} must not activate"
            );
        }
    }
}
