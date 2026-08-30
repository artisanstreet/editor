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

use artisan_domain::{ConversationLifecycle, ItemId, TurnId};
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

// ---------------------------------------------------------------------------
// Scene identity
// ---------------------------------------------------------------------------

/// Validation failure for [`SceneId`].
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum SceneIdError {
    /// The supplied value was empty.
    #[error("scene id must not be empty")]
    Empty,
    /// The supplied value contained whitespace or a control character.
    #[error("scene id must not contain whitespace or control characters; found {character:?}")]
    ForbiddenCharacter { character: char },
    /// The supplied value exceeded [`SCENE_ID_MAX_BYTES`] UTF-8 bytes.
    #[error("scene id is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong { length: usize, maximum: usize },
}

fn validate_scene_id(value: &str) -> Result<(), SceneIdError> {
    if value.is_empty() {
        return Err(SceneIdError::Empty);
    }
    if let Some(character) = value
        .chars()
        .find(|character| character.is_whitespace() || character.is_control())
    {
        return Err(SceneIdError::ForbiddenCharacter { character });
    }
    let length = value.len();
    if length > SCENE_ID_MAX_BYTES {
        return Err(SceneIdError::TooLong {
            length,
            maximum: SCENE_ID_MAX_BYTES,
        });
    }
    Ok(())
}

/// Bounded validated opaque scene identity for render-only records.
///
/// Synthetic work, change, and command records use this identity until a
/// later aggregate supplies a domain identity. Real domain [`ItemId`] values
/// can be converted losslessly when they are used as scene item identities.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SceneId(String);

impl SceneId {
    /// Validates and creates a scene identity.
    ///
    /// # Errors
    ///
    /// Returns [`SceneIdError`] on empty, forbidden-character, or overlong
    /// input.
    pub fn parse(value: impl Into<String>) -> Result<Self, SceneIdError> {
        let value = value.into();
        validate_scene_id(&value)?;
        Ok(Self(value))
    }

    /// Converts a validated domain item identity without changing its text.
    #[must_use]
    pub fn from_item_id(item_id: &ItemId) -> Self {
        Self(item_id.as_str().to_owned())
    }

    /// Returns the validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<ItemId> for SceneId {
    fn from(item_id: ItemId) -> Self {
        Self::from_item_id(&item_id)
    }
}

impl std::fmt::Display for SceneId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Disclosure and narration enums (closed, not booleans)
// ---------------------------------------------------------------------------

/// Explicit disclosure value copied into the exact owning group or card.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SceneDisclosure {
    /// The disclosure is open/expanded.
    Open,
    /// The disclosure is closed/collapsed.
    Closed,
}

/// Closed per-turn narration vocabulary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TurnNarration {
    /// Quiet status (no active work).
    Quiet,
    /// Waiting for a provider to respond.
    ProviderWait,
    /// Compaction is in progress.
    Compacting,
    /// Thinking.
    Thinking,
    /// Working.
    Working,
    /// An assistant reply is streaming; quiet status can be suppressed.
    StreamingSuppression,
    /// Waiting for background agents.
    BackgroundWait,
    /// Completed work with duration in milliseconds.
    WorkedFor { millis: u64 },
    /// Completed thought with duration in milliseconds.
    ThoughtFor { millis: u64 },
    /// Turn failed.
    Failed,
    /// Turn interrupted.
    Interrupted,
    /// Turn cancelled.
    Cancelled,
}

impl TurnNarration {
    fn terminal_label(self) -> Option<WorkGroupLabel> {
        match self {
            Self::WorkedFor { millis } => Some(WorkGroupLabel::WorkedFor { millis }),
            Self::ThoughtFor { millis } => Some(WorkGroupLabel::ThoughtFor { millis }),
            _ => None,
        }
    }
}

/// Per-turn narration entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnNarrationEntry {
    /// Owning turn.
    pub turn_id: TurnId,
    /// Closed narration value.
    pub narration: TurnNarration,
}

impl TurnNarrationEntry {
    /// Creates a narration entry.
    #[must_use]
    pub fn new(turn_id: TurnId, narration: TurnNarration) -> Self {
        Self { turn_id, narration }
    }
}

/// Steering placement anchored to an exact user-message item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringPlacement {
    /// Steering identity.
    pub id: SceneId,
    /// Anchor must be a user-message [`ItemId`].
    pub anchor: ItemId,
    /// Renderer-safe label.
    pub label: String,
}

impl SteeringPlacement {
    /// Creates a steering placement after validating its label.
    ///
    /// # Errors
    ///
    /// Returns [`SceneBuildError`] if the label is empty or exceeds
    /// [`SCENE_MAX_STEERING_LABEL_BYTES`] UTF-8 bytes.
    pub fn new(
        id: SceneId,
        anchor: ItemId,
        label: impl Into<String>,
    ) -> Result<Self, SceneBuildError> {
        let label = label.into();
        validate_steering_label(&label)?;
        Ok(Self { id, anchor, label })
    }
}

// ---------------------------------------------------------------------------
// Input vocabulary
// ---------------------------------------------------------------------------

/// Assistant message display phase supplied by the caller, not inferred.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AssistantPhase {
    /// Streaming/incremental text.
    Streaming,
    /// Final settled text.
    Final,
}

/// File change status for display.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FileChangeStatus {
    /// A file was added.
    Added,
    /// A file was modified.
    Modified,
    /// A file was removed.
    Removed,
    /// A file was renamed.
    Renamed,
}

/// One file fact for a change-set card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneFileChange {
    /// Safe display path (never read from disk).
    pub path: String,
    /// Change status.
    pub status: FileChangeStatus,
}

impl SceneFileChange {
    /// Creates a file change fact after validating its path.
    ///
    /// # Errors
    ///
    /// Returns [`SceneBuildError`] if the path is empty or exceeds
    /// [`SCENE_MAX_DISPLAY_PATH_BYTES`] UTF-8 bytes.
    pub fn new(path: impl Into<String>, status: FileChangeStatus) -> Result<Self, SceneBuildError> {
        let path = path.into();
        validate_display_path(&path)?;
        Ok(Self { path, status })
    }
}

/// Closed renderer-input enum covering every current conversation family.
///
/// Variant payloads carry only renderer-safe bounded text and metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SceneItemKind {
    /// Canonical user message.
    UserMessage { body: String },
    /// Assistant message with caller-supplied phase.
    AssistantMessage { body: String, phase: AssistantPhase },
    /// Settled reasoning summary.
    ReasoningSummary { body: String },
    /// Activity or tool-result summary.
    Activity { body: String },
    /// Bounded work-session title.
    WorkSession { title: String },
    /// Compaction summary card.
    Compaction { summary: String },
    /// One change-set fact containing zero or more files.
    ChangeSet { files: Vec<SceneFileChange> },
    /// One individual file-change fact.
    FileChange { file: SceneFileChange },
    /// Plan/checklist card.
    Plan { title: String, entries: Vec<String> },
    /// Approval request card.
    Approval { prompt: String },
    /// Question card.
    Question { prompt: String },
    /// Error card.
    Error { message: String },
    /// Usage or provider interruption card.
    UsageInterruption { detail: String },
    /// Model transition fact.
    ModelTransition {
        from_model: String,
        to_model: String,
    },
    /// Bounded native event/fallback fact.
    NativeFact { text: String },
}

/// One renderer input record with stable identity, owning turn, and ordinal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneItem {
    /// Stable scene identity. Real domain item identities can be converted to
    /// [`SceneId`] with [`SceneId::from_item_id`].
    pub id: SceneId,
    /// Owning turn.
    pub turn_id: TurnId,
    /// Stable ordinal (global ordering key).
    pub ordinal: u64,
    /// Family variant.
    pub kind: SceneItemKind,
    /// Explicit disclosure for the owning group/card, if any.
    pub disclosure: Option<SceneDisclosure>,
}

impl SceneItem {
    /// Creates a scene item after validating all bounded variant payloads.
    ///
    /// # Errors
    ///
    /// Returns [`SceneBuildError`] for an overlong body, title, prompt,
    /// collection, native fact, or path. Validation happens before the item
    /// is returned, so no invalid item is constructed by this constructor.
    pub fn new(
        id: impl Into<SceneId>,
        turn_id: TurnId,
        ordinal: u64,
        kind: SceneItemKind,
        disclosure: Option<SceneDisclosure>,
    ) -> Result<Self, SceneBuildError> {
        validate_item_kind(&kind)?;
        Ok(Self {
            id: id.into(),
            turn_id,
            ordinal,
            kind,
            disclosure,
        })
    }
}

/// One turn descriptor for the scene build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneTurn {
    /// Forge-minted turn identity.
    pub turn_id: TurnId,
    /// Stable ordinal for canonical ordering.
    pub ordinal: u64,
    /// Authoritative lifecycle.
    pub lifecycle: ConversationLifecycle,
}

impl SceneTurn {
    /// Creates a scene turn. The domain [`TurnId`] is already validated by its
    /// owning domain constructor; scene-level collection bounds are checked
    /// by [`ConversationScene::build`].
    #[must_use]
    pub fn new(turn_id: TurnId, ordinal: u64, lifecycle: ConversationLifecycle) -> Self {
        Self {
            turn_id,
            ordinal,
            lifecycle,
        }
    }
}

// ---------------------------------------------------------------------------
// Scene output vocabulary
// ---------------------------------------------------------------------------

/// Label for a work group header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorkGroupLabel {
    /// Completed reasoning-only work.
    ThoughtFor { millis: u64 },
    /// Completed ordinary work.
    WorkedFor { millis: u64 },
}

impl WorkGroupLabel {
    /// Returns the display label text for diagnostics (not a snapshot).
    #[must_use]
    pub fn display(self) -> String {
        match self {
            Self::ThoughtFor { millis } => format!("Thought for {millis}ms"),
            Self::WorkedFor { millis } => format!("Worked for {millis}ms"),
        }
    }
}

/// One work item inside a grouped work block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkItem {
    /// A reasoning summary.
    Reasoning {
        id: SceneId,
        body: String,
        disclosure: Option<SceneDisclosure>,
    },
    /// An activity/tool-result summary.
    Activity {
        id: SceneId,
        body: String,
        disclosure: Option<SceneDisclosure>,
    },
    /// A work-session title.
    WorkSession {
        id: SceneId,
        title: String,
        disclosure: Option<SceneDisclosure>,
    },
}

/// Ordered blocks for one turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnBlock {
    /// User message block.
    UserMessage(UserMessageBlock),
    /// Assistant message block.
    AssistantMessage(AssistantMessageBlock),
    /// Contiguous work group.
    WorkGroup(WorkGroupBlock),
    /// Compaction card.
    Compaction(CompactionBlock),
    /// Changed-files card.
    ChangeSet(ChangeSetBlock),
    /// Plan/checklist card.
    Plan(PlanBlock),
    /// Approval card.
    Approval(ApprovalBlock),
    /// Question card.
    Question(QuestionBlock),
    /// Error card.
    Error(ErrorBlock),
    /// Usage interruption card.
    UsageInterruption(UsageInterruptionBlock),
    /// Model transition card.
    ModelTransition(ModelTransitionBlock),
    /// Native fact card.
    NativeFact(NativeFactBlock),
    /// Steering label anchored to a user message.
    SteeringLabel(SteeringBlock),
    /// One per-turn status row unless streaming suppression applies.
    TurnStatus(TurnStatusBlock),
    /// One per-turn footer.
    TurnFooter(TurnFooterBlock),
}

/// User message block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserMessageBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Complete bounded body.
    pub body: String,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Assistant message block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssistantMessageBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Complete bounded body.
    pub body: String,
    /// Caller-supplied text phase.
    pub phase: AssistantPhase,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Group of contiguous reasoning/activity/work-session items.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkGroupBlock {
    /// Ordered group members.
    pub items: Vec<WorkItem>,
    /// At most one terminal duration label for this turn.
    pub label: Option<WorkGroupLabel>,
    /// Disclosure owned by the group.
    pub disclosure: Option<SceneDisclosure>,
}

/// Compaction card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Bounded summary.
    pub summary: String,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Changed-files card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeSetBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Ordered file facts.
    pub files: Vec<SceneFileChange>,
    /// Disclosure owned by the card.
    pub disclosure: Option<SceneDisclosure>,
}

/// Plan/checklist card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Bounded title.
    pub title: String,
    /// Ordered checklist entries.
    pub entries: Vec<String>,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Approval card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Bounded prompt.
    pub prompt: String,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Question card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuestionBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Bounded prompt.
    pub prompt: String,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Error card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Bounded message.
    pub message: String,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Usage interruption card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageInterruptionBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Bounded detail.
    pub detail: String,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Model transition card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelTransitionBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Previous model label.
    pub from_model: String,
    /// New model label.
    pub to_model: String,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Native fact card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeFactBlock {
    /// Scene identity.
    pub id: SceneId,
    /// Bounded native fact text.
    pub text: String,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Steering label block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringBlock {
    /// Steering identity.
    pub id: SceneId,
    /// Exact domain item anchor.
    pub anchor: ItemId,
    /// Bounded label.
    pub label: String,
}

/// Turn status row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnStatusBlock {
    /// Closed narration value.
    pub narration: TurnNarration,
}

/// Turn footer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnFooterBlock {
    /// Owning turn.
    pub turn_id: TurnId,
}

/// One turn's rendered scene.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnScene {
    /// Owning turn.
    pub turn_id: TurnId,
    /// Canonical turn ordinal.
    pub ordinal: u64,
    /// Authoritative lifecycle.
    pub lifecycle: ConversationLifecycle,
    /// Ordered blocks.
    pub blocks: Vec<TurnBlock>,
}

impl TurnScene {
    /// Returns the turn identity.
    #[must_use]
    pub fn turn_id(&self) -> &TurnId {
        &self.turn_id
    }

    /// Returns ordered blocks.
    #[must_use]
    pub fn blocks(&self) -> &[TurnBlock] {
        &self.blocks
    }
}

/// One deferred change-set retained when owning work is not terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredChangeSet {
    /// Owning turn.
    pub turn_id: TurnId,
    /// Deferred card.
    pub card: ChangeSetBlock,
}

/// Whole conversation render scene.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationScene {
    turn_scenes: Vec<TurnScene>,
    deferred: Vec<DeferredChangeSet>,
}

#[derive(Clone, Copy)]
enum AnchorKind {
    UserMessage,
    Other,
}

impl ConversationScene {
    /// Returns ordered turn scenes.
    #[must_use]
    pub fn turn_scenes(&self) -> &[TurnScene] {
        &self.turn_scenes
    }

    /// Returns typed deferred change-set cards.
    #[must_use]
    pub fn deferred_change_sets(&self) -> &[DeferredChangeSet] {
        &self.deferred
    }

    /// Returns the turn scene for `turn_id`, if present.
    #[must_use]
    pub fn turn_scene(&self, turn_id: &TurnId) -> Option<&TurnScene> {
        self.turn_scenes
            .iter()
            .find(|scene| &scene.turn_id == turn_id)
    }

    /// Builds a deterministic scene from authoritative inputs.
    ///
    /// Frozen rules:
    ///
    /// - turns and items are ordered by their globally unique stable ordinal;
    /// - duplicate identity/ordinal and unknown ownership are typed errors;
    /// - contiguous reasoning/activity/work-session items coalesce into one
    ///   work group, while every other item is a barrier;
    /// - a compaction card cannot be paired with a generic thinking/working
    ///   narration;
    /// - a streaming assistant suppresses the status only when the supplied
    ///   narration is [`TurnNarration::StreamingSuppression`];
    /// - change cards render only for a terminal domain lifecycle and are
    ///   retained in [`Self::deferred_change_sets`] before that point;
    /// - ordinary blocks are followed by a terminal change card, one status,
    ///   and one footer;
    /// - each steering placement appears once immediately after its exact
    ///   user-message anchor.
    ///
    /// The build is atomic: any validation failure returns only a typed error
    /// and no partial scene.
    ///
    /// # Errors
    ///
    /// Returns [`SceneBuildError`] when collection bounds, payload bounds,
    /// identities, ordinals, ownership, or steering placement rules fail.
    #[allow(clippy::too_many_lines)]
    pub fn build(
        turns: Vec<SceneTurn>,
        items: Vec<SceneItem>,
        narrations: Vec<TurnNarrationEntry>,
        steerings: Vec<SteeringPlacement>,
    ) -> Result<Self, SceneBuildError> {
        // Count bounds are checked before ownership is rearranged or any
        // large text payload is cloned into output blocks.
        if turns.len() > SCENE_MAX_TURNS {
            return Err(SceneBuildError::TooManyTurns {
                count: turns.len(),
                maximum: SCENE_MAX_TURNS,
            });
        }
        if items.len() > SCENE_MAX_ITEMS {
            return Err(SceneBuildError::TooManyItems {
                count: items.len(),
                maximum: SCENE_MAX_ITEMS,
            });
        }
        if narrations.len() > SCENE_MAX_NARRATIONS {
            return Err(SceneBuildError::TooManyNarrations {
                count: narrations.len(),
                maximum: SCENE_MAX_NARRATIONS,
            });
        }
        if steerings.len() > SCENE_MAX_STEERING_PLACEMENTS {
            return Err(SceneBuildError::TooManySteeringPlacements {
                count: steerings.len(),
                maximum: SCENE_MAX_STEERING_PLACEMENTS,
            });
        }

        // Validate every owned payload while it is still borrowed from the
        // caller's vectors. This also covers public struct literals that did
        // not use `SceneItem::new` or `SceneFileChange::new`.
        for item in &items {
            validate_item_kind(&item.kind)?;
        }
        for steering in &steerings {
            validate_steering_label(&steering.label)?;
        }

        // Domain snapshot invariants use globally unique turn/item ordinals;
        // the scene repeats that contract before sorting.
        let mut turn_ids: HashSet<TurnId> = HashSet::with_capacity(turns.len());
        let mut all_ordinals: HashSet<u64> =
            HashSet::with_capacity(turns.len().saturating_add(items.len()));
        for turn in &turns {
            if !turn_ids.insert(turn.turn_id.clone()) {
                return Err(SceneBuildError::DuplicateTurnId {
                    turn_id: turn.turn_id.clone(),
                });
            }
            if !all_ordinals.insert(turn.ordinal) {
                return Err(SceneBuildError::DuplicateOrdinal {
                    ordinal: turn.ordinal,
                });
            }
        }

        let mut narration_map: HashMap<TurnId, TurnNarration> =
            HashMap::with_capacity(narrations.len());
        for entry in narrations {
            if !turn_ids.contains(&entry.turn_id) {
                return Err(SceneBuildError::UnknownNarrationTurn {
                    turn_id: entry.turn_id.clone(),
                });
            }
            if narration_map
                .insert(entry.turn_id.clone(), entry.narration)
                .is_some()
            {
                return Err(SceneBuildError::DuplicateNarration {
                    turn_id: entry.turn_id.clone(),
                });
            }
        }

        let mut item_ids: HashSet<SceneId> = HashSet::with_capacity(items.len());
        for item in &items {
            if !turn_ids.contains(&item.turn_id) {
                return Err(SceneBuildError::UnknownTurn {
                    item_id: item.id.clone(),
                    turn_id: item.turn_id.clone(),
                });
            }
            if !item_ids.insert(item.id.clone()) {
                return Err(SceneBuildError::DuplicateItemId {
                    id: item.id.clone(),
                });
            }
            if !all_ordinals.insert(item.ordinal) {
                return Err(SceneBuildError::DuplicateOrdinal {
                    ordinal: item.ordinal,
                });
            }
        }

        // Keep the exact ItemId in the placement, but use the validated text
        // only as the lookup bridge to a scene identity. The bridge cannot
        // relocate a label: it is checked against the one exact user item.
        let mut item_anchor_kinds: HashMap<&str, AnchorKind> = HashMap::with_capacity(items.len());
        for item in &items {
            item_anchor_kinds.insert(
                item.id.as_str(),
                if matches!(&item.kind, SceneItemKind::UserMessage { .. }) {
                    AnchorKind::UserMessage
                } else {
                    AnchorKind::Other
                },
            );
        }

        let mut steering_ids: HashSet<SceneId> = HashSet::with_capacity(steerings.len());
        for steering in &steerings {
            if !steering_ids.insert(steering.id.clone()) {
                return Err(SceneBuildError::DuplicateSteeringId {
                    id: steering.id.clone(),
                });
            }
            match item_anchor_kinds.get(steering.anchor.as_str()) {
                None => {
                    return Err(SceneBuildError::UnknownSteeringAnchor {
                        anchor: steering.anchor.clone(),
                    });
                }
                Some(AnchorKind::Other) => {
                    return Err(SceneBuildError::NonUserSteeringAnchor {
                        anchor: steering.anchor.clone(),
                    });
                }
                Some(AnchorKind::UserMessage) => {}
            }
        }
        drop(item_anchor_kinds);

        let mut sorted_turns = turns;
        sorted_turns.sort_by_key(|turn| turn.ordinal);

        let mut sorted_items = items;
        sorted_items.sort_by_key(|item| item.ordinal);

        let mut items_by_turn: HashMap<TurnId, Vec<SceneItem>> =
            HashMap::with_capacity(sorted_turns.len());
        for item in sorted_items {
            items_by_turn
                .entry(item.turn_id.clone())
                .or_default()
                .push(item);
        }

        // Preserve caller order when multiple legal placements share one
        // exact anchor. Different anchors remain independently addressable.
        let mut steerings_by_anchor: HashMap<String, Vec<SteeringPlacement>> =
            HashMap::with_capacity(steerings.len());
        for steering in steerings {
            steerings_by_anchor
                .entry(steering.anchor.as_str().to_owned())
                .or_default()
                .push(steering);
        }

        let mut turn_scenes = Vec::with_capacity(sorted_turns.len());
        let mut deferred = Vec::new();

        for turn in &sorted_turns {
            let narration = narration_map
                .get(&turn.turn_id)
                .copied()
                .unwrap_or(TurnNarration::Quiet);
            let turn_items = items_by_turn.remove(&turn.turn_id).unwrap_or_default();

            let mut blocks = Vec::new();
            let mut work_buffer = Vec::new();
            let mut work_disclosure = None;
            let mut change_id = None;
            let mut change_disclosure = None;
            let mut change_files = Vec::new();
            let mut has_compaction = false;
            let mut has_streaming_assistant = false;

            for item in turn_items {
                // Change facts are a barrier even though the card is rendered
                // at the terminal position. This preserves *contiguous* work
                // grouping around a deferred or settled change event.
                if matches!(
                    &item.kind,
                    SceneItemKind::ChangeSet { .. } | SceneItemKind::FileChange { .. }
                ) {
                    flush_work(&mut work_buffer, &mut work_disclosure, &mut blocks)?;
                    match item.kind {
                        SceneItemKind::ChangeSet { files } => {
                            if change_id.is_none() {
                                change_id = Some(item.id);
                                change_disclosure = item.disclosure;
                            } else if change_disclosure.is_none() {
                                change_disclosure = item.disclosure;
                            }
                            change_files.extend(files);
                        }
                        SceneItemKind::FileChange { file } => {
                            if change_id.is_none() {
                                change_id = Some(item.id);
                                change_disclosure = item.disclosure;
                            } else if change_disclosure.is_none() {
                                change_disclosure = item.disclosure;
                            }
                            change_files.push(file);
                        }
                        _ => unreachable!("change barrier matched only change variants"),
                    }
                    if change_files.len() > SCENE_MAX_CHANGED_FILES_PER_CARD {
                        return Err(SceneBuildError::TooManyChangedFiles {
                            count: change_files.len(),
                            maximum: SCENE_MAX_CHANGED_FILES_PER_CARD,
                        });
                    }
                    continue;
                }

                let is_work_like = matches!(
                    &item.kind,
                    SceneItemKind::ReasoningSummary { .. }
                        | SceneItemKind::Activity { .. }
                        | SceneItemKind::WorkSession { .. }
                );
                if is_work_like {
                    if work_buffer.len() >= SCENE_MAX_WORK_GROUP_ITEMS {
                        return Err(SceneBuildError::TooManyWorkItems {
                            count: work_buffer.len() + 1,
                            maximum: SCENE_MAX_WORK_GROUP_ITEMS,
                        });
                    }
                    let work_item = match item.kind {
                        SceneItemKind::ReasoningSummary { body } => WorkItem::Reasoning {
                            id: item.id,
                            body,
                            disclosure: item.disclosure,
                        },
                        SceneItemKind::Activity { body } => WorkItem::Activity {
                            id: item.id,
                            body,
                            disclosure: item.disclosure,
                        },
                        SceneItemKind::WorkSession { title } => WorkItem::WorkSession {
                            id: item.id,
                            title,
                            disclosure: item.disclosure,
                        },
                        _ => unreachable!("work-like match covered every work variant"),
                    };
                    if work_disclosure.is_none() {
                        work_disclosure = work_item_disclosure(&work_item);
                    }
                    work_buffer.push(work_item);
                    continue;
                }

                // Messages, cards, and facts are all work-group barriers.
                flush_work(&mut work_buffer, &mut work_disclosure, &mut blocks)?;

                match item.kind {
                    SceneItemKind::UserMessage { body } => {
                        let anchor_key = item.id.as_str().to_owned();
                        blocks.push(TurnBlock::UserMessage(UserMessageBlock {
                            id: item.id,
                            body,
                            disclosure: item.disclosure,
                        }));
                        if let Some(placements) = steerings_by_anchor.remove(&anchor_key) {
                            for placement in placements {
                                blocks.push(TurnBlock::SteeringLabel(SteeringBlock {
                                    id: placement.id,
                                    anchor: placement.anchor,
                                    label: placement.label,
                                }));
                            }
                        }
                    }
                    SceneItemKind::AssistantMessage { body, phase } => {
                        has_streaming_assistant |= phase == AssistantPhase::Streaming;
                        blocks.push(TurnBlock::AssistantMessage(AssistantMessageBlock {
                            id: item.id,
                            body,
                            phase,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::Compaction { summary } => {
                        has_compaction = true;
                        blocks.push(TurnBlock::Compaction(CompactionBlock {
                            id: item.id,
                            summary,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::Plan { title, entries } => {
                        blocks.push(TurnBlock::Plan(PlanBlock {
                            id: item.id,
                            title,
                            entries,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::Approval { prompt } => {
                        blocks.push(TurnBlock::Approval(ApprovalBlock {
                            id: item.id,
                            prompt,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::Question { prompt } => {
                        blocks.push(TurnBlock::Question(QuestionBlock {
                            id: item.id,
                            prompt,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::Error { message } => {
                        blocks.push(TurnBlock::Error(ErrorBlock {
                            id: item.id,
                            message,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::UsageInterruption { detail } => {
                        blocks.push(TurnBlock::UsageInterruption(UsageInterruptionBlock {
                            id: item.id,
                            detail,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::ModelTransition {
                        from_model,
                        to_model,
                    } => {
                        blocks.push(TurnBlock::ModelTransition(ModelTransitionBlock {
                            id: item.id,
                            from_model,
                            to_model,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::NativeFact { text } => {
                        blocks.push(TurnBlock::NativeFact(NativeFactBlock {
                            id: item.id,
                            text,
                            disclosure: item.disclosure,
                        }));
                    }
                    SceneItemKind::ReasoningSummary { .. }
                    | SceneItemKind::Activity { .. }
                    | SceneItemKind::WorkSession { .. }
                    | SceneItemKind::ChangeSet { .. }
                    | SceneItemKind::FileChange { .. } => {
                        unreachable!("all work and change variants were handled above")
                    }
                }
            }

            flush_work(&mut work_buffer, &mut work_disclosure, &mut blocks)?;

            // A duration narration is a terminal label, not a label on every
            // historical work fragment. Put exactly one label on the latest
            // group when a turn contains work.
            if let Some(label) = narration.terminal_label()
                && let Some(group) = blocks.iter_mut().rev().find_map(|block| match block {
                    TurnBlock::WorkGroup(group) => Some(group),
                    _ => None,
                })
            {
                group.label = Some(label);
            }

            if let Some(change_id) = change_id
                && !change_files.is_empty()
            {
                let card = ChangeSetBlock {
                    id: change_id,
                    files: change_files,
                    disclosure: change_disclosure,
                };
                if turn.lifecycle.is_terminal() {
                    // Terminal cards are deliberately appended before the
                    // status/footer pair, regardless of item ordinal.
                    blocks.push(TurnBlock::ChangeSet(card));
                } else {
                    deferred.push(DeferredChangeSet {
                        turn_id: turn.turn_id.clone(),
                        card,
                    });
                }
            }

            if has_compaction
                && matches!(narration, TurnNarration::Thinking | TurnNarration::Working)
            {
                return Err(SceneBuildError::CompactionNarrationConflict { narration });
            }

            let suppress_status =
                has_streaming_assistant && narration == TurnNarration::StreamingSuppression;
            if !suppress_status {
                blocks.push(TurnBlock::TurnStatus(TurnStatusBlock { narration }));
            }
            blocks.push(TurnBlock::TurnFooter(TurnFooterBlock {
                turn_id: turn.turn_id.clone(),
            }));

            turn_scenes.push(TurnScene {
                turn_id: turn.turn_id.clone(),
                ordinal: turn.ordinal,
                lifecycle: turn.lifecycle,
                blocks,
            });
        }

        if let Some(placement) = steerings_by_anchor
            .values()
            .next()
            .and_then(|placements| placements.first())
        {
            return Err(SceneBuildError::SteeringAnchorNotPlaced {
                anchor: placement.anchor.clone(),
            });
        }

        // Turns were already sorted, but retain the final sort as a local
        // invariant if construction changes later.
        turn_scenes.sort_by_key(|scene| scene.ordinal);

        Ok(Self {
            turn_scenes,
            deferred,
        })
    }
}

fn flush_work(
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
    }));
    Ok(())
}

fn work_item_disclosure(item: &WorkItem) -> Option<SceneDisclosure> {
    match item {
        WorkItem::Reasoning { disclosure, .. }
        | WorkItem::Activity { disclosure, .. }
        | WorkItem::WorkSession { disclosure, .. } => *disclosure,
    }
}

fn validate_steering_label(label: &str) -> Result<(), SceneBuildError> {
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

fn validate_display_path(path: &str) -> Result<(), SceneBuildError> {
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

fn validate_general_text(text: &str) -> Result<(), SceneBuildError> {
    if text.len() > SCENE_MAX_TEXT_BYTES {
        return Err(SceneBuildError::TextTooLong {
            length: text.len(),
            maximum: SCENE_MAX_TEXT_BYTES,
        });
    }
    Ok(())
}

fn validate_message_body(text: &str) -> Result<(), SceneBuildError> {
    if text.len() > SCENE_MAX_MESSAGE_BODY_BYTES {
        return Err(SceneBuildError::MessageBodyTooLong {
            length: text.len(),
            maximum: SCENE_MAX_MESSAGE_BODY_BYTES,
        });
    }
    Ok(())
}

fn validate_native_fact(text: &str) -> Result<(), SceneBuildError> {
    if text.len() > SCENE_MAX_NATIVE_FACT_BYTES {
        return Err(SceneBuildError::NativeFactTooLong {
            length: text.len(),
            maximum: SCENE_MAX_NATIVE_FACT_BYTES,
        });
    }
    Ok(())
}

fn validate_file_change(file: &SceneFileChange) -> Result<(), SceneBuildError> {
    validate_display_path(&file.path)
}

fn validate_item_kind(kind: &SceneItemKind) -> Result<(), SceneBuildError> {
    match kind {
        SceneItemKind::UserMessage { body } | SceneItemKind::AssistantMessage { body, .. } => {
            validate_message_body(body)
        }
        SceneItemKind::ReasoningSummary { body } | SceneItemKind::Activity { body } => {
            validate_general_text(body)
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
    /// A compaction card was paired with a generic active-work narration.
    #[error("compaction card cannot coexist with {narration:?} narration")]
    CompactionNarrationConflict { narration: TurnNarration },
}
