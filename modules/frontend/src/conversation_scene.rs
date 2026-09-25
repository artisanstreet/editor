//! Pure immutable render scene consumed by the later GPUI renderer.
//!
//! This module performs deterministic validation, ordering, grouping, and
//! state-to-block projection only. It does no I/O, Markdown parsing, timers,
//! scrolling, mutation, `Statig` dispatch, or GPUI element creation. A later
//! aggregate state machine will feed it authoritative delivery, turn,
//! steering, and disclosure views.
//!
//! # Bounds
//!
//! Every bound is measured in UTF-8 bytes unless it is explicitly marked as a
//! count. Bounds are checked before the scene takes ownership of caller data;
//! oversize input is refused rather than truncated.
//!
//! - [`SCENE_MAX_TURNS`] — turn descriptors per scene (count)
//! - [`SCENE_MAX_ITEMS`] — input items per scene (count)
//! - [`SCENE_MAX_NARRATIONS`] — narration entries per scene (count)
//! - [`SCENE_MAX_STEERING_PLACEMENTS`] — steering placements per scene (count)
//! - [`SCENE_MAX_WORK_GROUP_ITEMS`] — items coalesced into one work group
//!   (count)
//! - [`SCENE_MAX_PLAN_ENTRIES`] — entries in one plan (count)
//! - [`SCENE_MAX_CHANGED_FILES_PER_CARD`] — files in one change-set card
//!   (count)
//! - [`SCENE_MAX_NATIVE_FACT_BYTES`] — native-event/fallback fact text
//! - [`SCENE_MAX_DISPLAY_PATH_BYTES`] — filesystem display path text
//! - [`SCENE_ID_MAX_BYTES`] — render-only opaque scene identity
//! - [`SCENE_MAX_STEERING_LABEL_BYTES`] — steering label text
//! - [`SCENE_MAX_TEXT_BYTES`] — general renderer-safe text
//! - [`SCENE_MAX_MESSAGE_BODY_BYTES`] — complete user/assistant message text
//!
//! # Deterministic terminal order
//!
//! Within each turn, ordinary blocks retain canonical item ordinal order.
//! The settled change-set card, when present, is appended after those blocks,
//! followed by exactly one status row (unless streaming suppression applies)
//! and exactly one turn footer.

#![allow(clippy::module_name_repetitions)]

use std::collections::{HashMap, HashSet};

use artisan_domain::{ConversationLifecycle, EngineId, ItemId, RunId, TurnId};
use thiserror::Error;

/// Maximum turn descriptors per scene (count).
pub const SCENE_MAX_TURNS: usize = 512;

/// Maximum scene input items per build (count).
pub const SCENE_MAX_ITEMS: usize = 512;

/// Maximum per-turn narration entries per build (count).
pub const SCENE_MAX_NARRATIONS: usize = 512;

/// Maximum steering placements per build (count).
pub const SCENE_MAX_STEERING_PLACEMENTS: usize = 512;

/// Maximum items coalesced into one work group (count).
pub const SCENE_MAX_WORK_GROUP_ITEMS: usize = 32;

/// Maximum checklist entries in one plan (count).
pub const SCENE_MAX_PLAN_ENTRIES: usize = 256;

/// Maximum changed files per change-set card (count).
pub const SCENE_MAX_CHANGED_FILES_PER_CARD: usize = 128;

/// Maximum UTF-8 bytes for a native-event/fallback fact text.
pub const SCENE_MAX_NATIVE_FACT_BYTES: usize = 4_096;

/// Maximum UTF-8 bytes for a safe display path.
pub const SCENE_MAX_DISPLAY_PATH_BYTES: usize = 1_024;

/// Maximum UTF-8 bytes for the render-only opaque scene identity.
pub const SCENE_ID_MAX_BYTES: usize = 128;

/// Maximum UTF-8 bytes for a steering label.
pub const SCENE_MAX_STEERING_LABEL_BYTES: usize = 1_024;

/// Maximum UTF-8 bytes for general renderer-safe text.
pub const SCENE_MAX_TEXT_BYTES: usize = 8_192;

/// Maximum UTF-8 bytes for a complete user or assistant message body.
///
/// This deliberately follows the frozen domain ceiling. A full message is
/// not a streamed fragment and therefore must not inherit the smaller general
/// renderer-text bound.
pub const SCENE_MAX_MESSAGE_BODY_BYTES: usize = artisan_domain::MESSAGE_BODY_MAX_BYTES;

#[cfg(test)]
mod multimodal_scene_tests {
    use super::{SceneBuildError, SceneItemKind, validate_item_kind};
    use artisan_domain::{ImageAttachmentRef, MessageId, ThreadId};

    fn image(index: u32, thread: &str) -> ImageAttachmentRef {
        ImageAttachmentRef::new(
            MessageId::parse("message-images").expect("message"),
            ThreadId::parse(thread).expect("thread"),
            index,
            "image/png",
            "capture.png",
            128,
            [7; 32],
        )
        .expect("image reference")
    }

    #[test]
    fn image_only_scene_preserves_empty_text_and_ordered_references() {
        let kind = SceneItemKind::MultimodalUserMessage {
            body: String::new(),
            attachments: vec![image(0, "thread-images"), image(1, "thread-images")],
        };
        assert_eq!(validate_item_kind(&kind), Ok(()));
        let SceneItemKind::MultimodalUserMessage { body, attachments } = kind else {
            unreachable!()
        };
        assert!(body.is_empty());
        assert_eq!(attachments[1].index, 1);
    }

    #[test]
    fn scene_rejects_reordered_or_cross_thread_image_references() {
        for attachments in [
            vec![image(1, "thread-images"), image(0, "thread-images")],
            vec![image(0, "thread-images"), image(1, "other-thread")],
            Vec::new(),
        ] {
            let kind = SceneItemKind::MultimodalUserMessage {
                body: String::new(),
                attachments,
            };
            assert_eq!(
                validate_item_kind(&kind),
                Err(SceneBuildError::InvalidImageAttachments)
            );
        }
    }
}

#[cfg(test)]
mod activity_category_tests {
    use super::{ActivityCategory, activity_category, activity_presentation_label};
    use artisan_domain::ConversationLifecycle;

    #[test]
    fn classifies_the_kinds_the_native_rows_carry() {
        // Tool names, the terminal kind, and the timeline tags.
        assert_eq!(
            activity_category("terminal_activity"),
            ActivityCategory::Command
        );
        assert_eq!(activity_category("bash"), ActivityCategory::Command);
        assert_eq!(
            activity_category("command_execution"),
            ActivityCategory::Command
        );
        assert_eq!(activity_category("read"), ActivityCategory::FileRead);
        assert_eq!(activity_category("file_read"), ActivityCategory::FileRead);
        assert_eq!(
            activity_category("workspace.read"),
            ActivityCategory::FileRead
        );
        assert_eq!(activity_category("write"), ActivityCategory::FileEdit);
        assert_eq!(activity_category("apply_patch"), ActivityCategory::FileEdit);
        assert_eq!(
            activity_category("file.delete"),
            ActivityCategory::FileDelete
        );
        assert_eq!(activity_category("grep"), ActivityCategory::FileSearch);
        assert_eq!(activity_category("glob"), ActivityCategory::FileSearch);
        assert_eq!(activity_category("search"), ActivityCategory::WebSearch);
        assert_eq!(activity_category("webfetch"), ActivityCategory::WebSearch);
        assert_eq!(activity_category("test"), ActivityCategory::Test);
        assert_eq!(activity_category("typecheck"), ActivityCategory::Typecheck);
        assert_eq!(activity_category("git_status"), ActivityCategory::GitStatus);
        assert_eq!(activity_category("diff"), ActivityCategory::Diff);
        assert_eq!(activity_category("database"), ActivityCategory::Database);
        assert_eq!(activity_category("browser"), ActivityCategory::AppInspect);
        assert_eq!(activity_category("subagent"), ActivityCategory::Subagent);
        assert_eq!(activity_category("mcp"), ActivityCategory::Integration);
        assert_eq!(activity_category("plugin"), ActivityCategory::Tool);
        assert_eq!(activity_category("something-new"), ActivityCategory::Other);
    }

    #[test]
    fn labels_and_counted_clauses_match_the_reference() {
        assert_eq!(ActivityCategory::Command.label(), "Command");
        assert_eq!(ActivityCategory::FileRead.label(), "Files");
        assert_eq!(ActivityCategory::Typecheck.label(), "Types");
        assert_eq!(ActivityCategory::Command.count_label(1), "ran a command");
        assert_eq!(ActivityCategory::Command.count_label(4), "ran 4 commands");
        assert_eq!(ActivityCategory::FileRead.count_label(1), "read a file");
        assert_eq!(ActivityCategory::FileRead.count_label(2), "read 2 files");
        assert_eq!(ActivityCategory::Test.count_label(3), "ran 3 test runs");
        assert_eq!(ActivityCategory::Other.count_label(2), "used 2 tools");
    }

    #[test]
    fn presentation_labels_follow_kind_and_lifecycle() {
        assert_eq!(
            activity_presentation_label("read", Some(ConversationLifecycle::Completed)),
            Some("Read a file".to_owned())
        );
        assert_eq!(
            activity_presentation_label("read", Some(ConversationLifecycle::Active)),
            Some("Reading a file".to_owned())
        );
        assert_eq!(
            activity_presentation_label("read", Some(ConversationLifecycle::Failed)),
            Some("File read failed".to_owned())
        );
        // Unknown lifecycle settles rather than claiming live work.
        assert_eq!(
            activity_presentation_label("read", None),
            Some("Read a file".to_owned())
        );
        assert_eq!(
            activity_presentation_label("mcp", Some(ConversationLifecycle::Completed)),
            Some("Used an integration".to_owned())
        );
        // The generic tool bucket keeps its provider name, normalized.
        assert_eq!(
            activity_presentation_label("custom_tool", Some(ConversationLifecycle::Completed)),
            Some("Used custom tool".to_owned())
        );
        // Unrecognised kinds keep their own label; blank ones say nothing.
        assert_eq!(
            activity_presentation_label("something-new", Some(ConversationLifecycle::Completed)),
            Some("something-new".to_owned())
        );
        assert_eq!(activity_presentation_label("", None), None);
    }
}

#[path = "conversation_scene/validation.rs"]
mod validation;
#[allow(clippy::wildcard_imports)]
use self::validation::*;
pub use self::validation::{SceneBuildError, validate_engine_label};

#[path = "conversation_scene/build.rs"]
mod build;
pub use self::build::session_anchor_id;

#[path = "conversation_scene/blocks.rs"]
mod blocks;
pub use self::blocks::*;

#[path = "conversation_scene/types.rs"]
mod types;
pub use self::types::*;
