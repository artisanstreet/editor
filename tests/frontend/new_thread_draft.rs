//! Focused tests for the pure new-thread draft key and activation policies.
//!
//! The implementation is loaded directly so this dependency-free harness does
//! not require shared `lib.rs` or build-file registration.

#[path = "../../modules/frontend/src/new_thread_draft.rs"]
mod new_thread_draft;

use new_thread_draft::{
    NewThreadActivation, is_unmodified_primary_activation, new_thread_draft_key,
};

#[test]
fn draft_keys_use_exact_root_and_workspace_strings() {
    let cases = [
        (None, "draft:new-thread"),
        (Some(""), "draft:"),
        (Some("workspace-42"), "draft:workspace-42"),
        (Some("  workspace/東京 🚀  "), "draft:  workspace/東京 🚀  "),
    ];

    for (workspace_id, expected) in cases {
        assert_eq!(new_thread_draft_key(workspace_id), expected);
    }
}

#[test]
fn workspace_key_derivation_does_not_normalize_unicode_or_empty_ids() {
    let workspace_id = " ́é/プロジェクト\n";
    let key = new_thread_draft_key(Some(workspace_id));

    assert_eq!(key, "draft: ́é/プロジェクト\n");
    assert_eq!(&key["draft:".len()..], workspace_id);
}

#[test]
fn only_the_unmodified_primary_activation_is_accepted() {
    for mask in 0_u8..16 {
        let input = NewThreadActivation::new(
            0,
            mask & 0b0001 != 0,
            mask & 0b0010 != 0,
            mask & 0b0100 != 0,
            mask & 0b1000 != 0,
        );

        assert_eq!(
            is_unmodified_primary_activation(input),
            mask == 0,
            "modifier mask {mask:04b}"
        );
    }
}

#[test]
fn every_non_primary_button_is_rejected_without_modifiers() {
    for button in [1, 2, 3, u8::MAX] {
        assert!(!is_unmodified_primary_activation(NewThreadActivation::new(
            button, false, false, false, false,
        )));
    }
}

#[test]
fn activation_input_fields_are_preserved_as_typed_values() {
    let input = NewThreadActivation {
        alt_key: true,
        button: 2,
        ctrl_key: false,
        meta_key: true,
        shift_key: false,
    };

    assert!(input.alt_key);
    assert_eq!(input.button, 2);
    assert!(!input.ctrl_key);
    assert!(input.meta_key);
    assert!(!input.shift_key);
    assert!(!is_unmodified_primary_activation(input));
}
