//! Bounded text/identity validators and the typed scene build error.
//!
//! Extracted verbatim from `conversation_scene.rs` during the module split;
//! private validators were widened to `pub(super)` for the build pipeline.

#![allow(clippy::module_name_repetitions)]

#[allow(clippy::wildcard_imports)]
use super::*;

/// Whether an assistant lifecycle means text may still be arriving.
///
/// Mirrors the reference live-lifecycle set; only these lifecycles combine
/// with a genuine reply phase into a streaming reply.
pub(super) fn is_live_lifecycle(lifecycle: ConversationLifecycle) -> bool {
    matches!(
        lifecycle,
        ConversationLifecycle::Pending
            | ConversationLifecycle::Streaming
            | ConversationLifecycle::Active
            | ConversationLifecycle::Waiting
    )
}

pub(super) fn flush_work(
    buffer: &mut Vec<WorkItem>,
    disclosure: &mut Option<SceneDisclosure>,
    blocks: &mut Vec<TurnBlock>,
) -> Result<(), SceneBuildError> {
    if buffer.is_empty() {
        return Ok(());
    }
    if buffer.len() > SCENE_MAX_WORK_GROUP_ITEMS {
        return Err(SceneBuildError::TooManyWorkItems {
            count: buffer.len(),
            maximum: SCENE_MAX_WORK_GROUP_ITEMS,
        });
    }
    blocks.push(TurnBlock::WorkGroup(WorkGroupBlock {
        items: std::mem::take(buffer),
        label: None,
        disclosure: disclosure.take(),
        session: None,
        session_run: None,
        superseded: false,
        reasoning_summary: None,
        progress: ProgressPhase::None,
        transition: None,
        session_details: Vec::new(),
    }));
    Ok(())
}

pub(super) fn work_item_disclosure(item: &WorkItem) -> Option<SceneDisclosure> {
    match item {
        WorkItem::Reasoning { disclosure, .. }
        | WorkItem::Activity { disclosure, .. }
        | WorkItem::WorkSession { disclosure, .. } => *disclosure,
    }
}

pub(super) fn validate_steering_label(label: &str) -> Result<(), SceneBuildError> {
    if label.is_empty() {
        return Err(SceneBuildError::EmptySteeringLabel);
    }
    if label.len() > SCENE_MAX_STEERING_LABEL_BYTES {
        return Err(SceneBuildError::SteeringLabelTooLong {
            length: label.len(),
            maximum: SCENE_MAX_STEERING_LABEL_BYTES,
        });
    }
    Ok(())
}

/// Validates an engine display label against the existing scene label limits.
///
/// Non-blank text within [`SCENE_MAX_STEERING_LABEL_BYTES`] UTF-8 bytes;
/// display names are short by construction and anything larger is a
/// producer defect, never silently truncated.
///
/// # Errors
///
/// Returns [`SceneBuildError::EmptyEngineLabel`] for blank input (including
/// whitespace-only) or [`SceneBuildError::EngineLabelTooLong`] past the
/// ceiling.
pub fn validate_engine_label(label: &str) -> Result<(), SceneBuildError> {
    if label.trim().is_empty() {
        return Err(SceneBuildError::EmptyEngineLabel);
    }
    if label.len() > SCENE_MAX_STEERING_LABEL_BYTES {
        return Err(SceneBuildError::EngineLabelTooLong {
            length: label.len(),
            maximum: SCENE_MAX_STEERING_LABEL_BYTES,
        });
    }
    Ok(())
}

pub(super) fn validate_display_path(path: &str) -> Result<(), SceneBuildError> {
    if path.is_empty() {
        return Err(SceneBuildError::EmptyDisplayPath);
    }
    if path.len() > SCENE_MAX_DISPLAY_PATH_BYTES {
        return Err(SceneBuildError::DisplayPathTooLong {
            length: path.len(),
            maximum: SCENE_MAX_DISPLAY_PATH_BYTES,
        });
    }
    Ok(())
}

pub(super) fn validate_general_text(text: &str) -> Result<(), SceneBuildError> {
    if text.len() > SCENE_MAX_TEXT_BYTES {
        return Err(SceneBuildError::TextTooLong {
            length: text.len(),
            maximum: SCENE_MAX_TEXT_BYTES,
        });
    }
    Ok(())
}

pub(super) fn validate_message_body(text: &str) -> Result<(), SceneBuildError> {
    if text.len() > SCENE_MAX_MESSAGE_BODY_BYTES {
        return Err(SceneBuildError::MessageBodyTooLong {
            length: text.len(),
            maximum: SCENE_MAX_MESSAGE_BODY_BYTES,
        });
    }
    Ok(())
}

pub(super) fn validate_native_fact(text: &str) -> Result<(), SceneBuildError> {
    if text.len() > SCENE_MAX_NATIVE_FACT_BYTES {
        return Err(SceneBuildError::NativeFactTooLong {
            length: text.len(),
            maximum: SCENE_MAX_NATIVE_FACT_BYTES,
        });
    }
    Ok(())
}

pub(super) fn validate_file_change(file: &SceneFileChange) -> Result<(), SceneBuildError> {
    validate_display_path(&file.path)
}

pub(super) fn validate_item_kind(kind: &SceneItemKind) -> Result<(), SceneBuildError> {
    match kind {
        SceneItemKind::UserMessage { body } | SceneItemKind::AssistantMessage { body, .. } => {
            validate_message_body(body)
        }
        SceneItemKind::MultimodalUserMessage { body, attachments } => {
            validate_message_body(body)?;
            if attachments.is_empty() || attachments.len() > 10 {
                return Err(SceneBuildError::InvalidImageAttachments);
            }
            for (index, attachment) in attachments.iter().enumerate() {
                if attachment.index as usize != index
                    || attachment.thread_id != attachments[0].thread_id
                    || attachment.message_id != attachments[0].message_id
                {
                    return Err(SceneBuildError::InvalidImageAttachments);
                }
            }
            Ok(())
        }
        SceneItemKind::ReasoningSummary { body } => validate_general_text(body),
        SceneItemKind::Activity { body, kind, detail } => {
            validate_general_text(body)?;
            if let Some(kind) = kind {
                validate_general_text(kind)?;
            }
            if let Some(detail) = detail {
                validate_general_text(detail)?;
            }
            Ok(())
        }
        SceneItemKind::WorkSession { title } | SceneItemKind::Compaction { summary: title } => {
            validate_general_text(title)
        }
        SceneItemKind::ChangeSet { files } => {
            if files.len() > SCENE_MAX_CHANGED_FILES_PER_CARD {
                return Err(SceneBuildError::TooManyChangedFiles {
                    count: files.len(),
                    maximum: SCENE_MAX_CHANGED_FILES_PER_CARD,
                });
            }
            for file in files {
                validate_file_change(file)?;
            }
            Ok(())
        }
        SceneItemKind::FileChange { file } => validate_file_change(file),
        SceneItemKind::Plan { title, entries } => {
            if entries.len() > SCENE_MAX_PLAN_ENTRIES {
                return Err(SceneBuildError::TooManyPlanEntries {
                    count: entries.len(),
                    maximum: SCENE_MAX_PLAN_ENTRIES,
                });
            }
            validate_general_text(title)?;
            for entry in entries {
                validate_general_text(entry)?;
            }
            Ok(())
        }
        SceneItemKind::Approval { prompt } | SceneItemKind::Question { prompt } => {
            validate_general_text(prompt)
        }
        SceneItemKind::Error { message } => validate_general_text(message),
        SceneItemKind::UsageInterruption { detail } => validate_general_text(detail),
        SceneItemKind::ModelTransition {
            from_model,
            to_model,
        } => {
            validate_general_text(from_model)?;
            validate_general_text(to_model)
        }
        SceneItemKind::NativeFact { text } => validate_native_fact(text),
    }
}

// ---------------------------------------------------------------------------
// Typed build error
// ---------------------------------------------------------------------------

/// Typed atomic failure for a scene build.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SceneBuildError {
    /// Image references were empty, unordered, mixed-owner, or over count.
    #[error("invalid user image attachment references")]
    InvalidImageAttachments,
    /// Two turns reused one identity.
    #[error("duplicate turn id {turn_id}")]
    DuplicateTurnId { turn_id: TurnId },
    /// Two scene items reused one identity.
    #[error("duplicate item id {id}")]
    DuplicateItemId { id: SceneId },
    /// Two steering placements reused one identity.
    #[error("duplicate steering id {id}")]
    DuplicateSteeringId { id: SceneId },
    /// Two turn/item records reused one global ordinal.
    #[error("duplicate ordinal {ordinal}")]
    DuplicateOrdinal { ordinal: u64 },
    /// Two narration entries targeted one turn.
    #[error("duplicate narration for turn {turn_id}")]
    DuplicateNarration { turn_id: TurnId },
    /// A narration targeted a turn absent from the input.
    #[error("narration references unknown turn {turn_id}")]
    UnknownNarrationTurn { turn_id: TurnId },
    /// An item targeted a turn absent from the input.
    #[error("item {item_id} references unknown turn {turn_id}")]
    UnknownTurn { item_id: SceneId, turn_id: TurnId },
    /// A steering placement targeted no input item.
    #[error("steering anchor {anchor} is unknown")]
    UnknownSteeringAnchor { anchor: ItemId },
    /// A steering placement targeted an item that is not a user message.
    #[error("steering anchor {anchor} is not a user message")]
    NonUserSteeringAnchor { anchor: ItemId },
    /// A validated anchor unexpectedly had no output placement.
    #[error("steering anchor {anchor} was not placed")]
    SteeringAnchorNotPlaced { anchor: ItemId },
    /// The scene contained too many turns.
    #[error("scene has {count} turns; the maximum is {maximum} (count)")]
    TooManyTurns { count: usize, maximum: usize },
    /// The scene contained too many items.
    #[error("scene has {count} items; the maximum is {maximum} (count)")]
    TooManyItems { count: usize, maximum: usize },
    /// The scene contained too many narration entries.
    #[error("scene has {count} narrations; the maximum is {maximum} (count)")]
    TooManyNarrations { count: usize, maximum: usize },
    /// The scene contained too many steering placements.
    #[error("scene has {count} steering placements; the maximum is {maximum} (count)")]
    TooManySteeringPlacements { count: usize, maximum: usize },
    /// One contiguous work group exceeded its item ceiling.
    #[error("work group has {count} items; the maximum is {maximum} (count)")]
    TooManyWorkItems { count: usize, maximum: usize },
    /// One plan exceeded its entry ceiling.
    #[error("plan has {count} entries; the maximum is {maximum} (count)")]
    TooManyPlanEntries { count: usize, maximum: usize },
    /// One merged change card exceeded its file ceiling.
    #[error("change-set card has {count} files; the maximum is {maximum} (count)")]
    TooManyChangedFiles { count: usize, maximum: usize },
    /// The supplied plan/prompt/title/general text was too long.
    #[error("text is {length} UTF-8 bytes; the maximum is {maximum} (bytes)")]
    TextTooLong { length: usize, maximum: usize },
    /// A complete user/assistant body exceeded the domain body ceiling.
    #[error("message body is {length} UTF-8 bytes; the maximum is {maximum} (bytes)")]
    MessageBodyTooLong { length: usize, maximum: usize },
    /// A native fact exceeded its conservative display ceiling.
    #[error("native fact text is {length} UTF-8 bytes; the maximum is {maximum} (bytes)")]
    NativeFactTooLong { length: usize, maximum: usize },
    /// A display path was empty.
    #[error("display path must not be empty")]
    EmptyDisplayPath,
    /// A display path exceeded its conservative ceiling.
    #[error("display path is {length} UTF-8 bytes; the maximum is {maximum} (bytes)")]
    DisplayPathTooLong { length: usize, maximum: usize },
    /// A steering label was empty.
    #[error("steering label must not be empty")]
    EmptySteeringLabel,
    /// A steering label exceeded its conservative ceiling.
    #[error("steering label is {length} UTF-8 bytes; the maximum is {maximum} (bytes)")]
    SteeringLabelTooLong { length: usize, maximum: usize },
    /// An engine display label was empty.
    #[error("engine label must not be empty")]
    EmptyEngineLabel,
    /// An engine display label exceeded its conservative ceiling.
    #[error("engine label is {length} UTF-8 bytes; the maximum is {maximum} (bytes)")]
    EngineLabelTooLong { length: usize, maximum: usize },
    /// A compaction card was paired with a generic active-work narration.
    #[error("compaction card cannot coexist with {narration:?} narration")]
    CompactionNarrationConflict { narration: TurnNarration },
    /// An active-work clock basis accompanied a narration that is not active
    /// work. Only live-work narrations may carry a basis; quiet and terminal
    /// narrations render no ticking row or carry their own settled durations.
    #[error("active clock basis requires an active-work narration, found {narration:?}")]
    ActiveBasisWithoutActiveNarration { narration: TurnNarration },
    /// A derived session anchor exceeded the scene identity ceiling.
    #[error("session anchor is {length} UTF-8 bytes; the maximum is {maximum} (bytes)")]
    SessionAnchorTooLong { length: usize, maximum: usize },
}
