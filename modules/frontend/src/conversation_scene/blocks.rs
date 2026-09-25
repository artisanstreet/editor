//! Scene block, item, turn, and aggregate scene records.
//!
//! Extracted verbatim from `conversation_scene.rs` during the module split;
//! aggregate scene fields were widened to `pub(super)` for the build pipeline.

#![allow(clippy::module_name_repetitions)]

#[allow(clippy::wildcard_imports)]
use super::*;

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
    /// User input with byte-free references to persisted images.
    MultimodalUserMessage {
        body: String,
        attachments: Vec<artisan_domain::ImageAttachmentRef>,
    },
    /// Assistant message with caller-supplied phase.
    AssistantMessage { body: String, phase: AssistantPhase },
    /// Settled reasoning summary.
    ReasoningSummary { body: String },
    /// Activity or tool-result summary.
    Activity {
        /// Bounded body used by legacy flat rows.
        body: String,
        /// Provider activity kind when the row carries one; `None` keeps the
        /// legacy flat-body rendering for callers without provider kinds.
        kind: Option<String>,
        /// Raw provider detail (terminal command, file path, query), when
        /// disclosed. Terminal details normalize at render time.
        detail: Option<String>,
    },
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

/// Caller-supplied durable attribution for one scene item.
///
/// Carries the exact domain lifecycle and run identity the aggregate
/// observed; nothing here is inferred from text. Absent provenance means the
/// legacy positional layout (no session derivation), never a default.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemProvenance {
    /// Owning run for assistant items; `None` for user items and
    /// unattributed facts.
    pub run_id: Option<RunId>,
    /// Renderer-visible lifecycle for assistant items; `None` for
    /// fact-derived cards, which are never treated as live.
    pub lifecycle: Option<ConversationLifecycle>,
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
    /// Durable attribution for session derivation; `None` selects the legacy
    /// positional layout for unattributed inputs.
    pub provenance: Option<ItemProvenance>,
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
            provenance: None,
        })
    }

    /// Attaches durable attribution for session derivation.
    ///
    /// Items without provenance keep the legacy positional layout; provenance
    /// never defaults and is never inferred.
    #[must_use]
    pub fn with_provenance(self, provenance: ItemProvenance) -> Self {
        Self {
            provenance: Some(provenance),
            ..self
        }
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
        /// Provider activity kind, when the source row carried one.
        kind: Option<String>,
        /// Raw provider detail, when disclosed.
        detail: Option<String>,
        /// Durable liveness for the chain's live/failed default-open rule.
        lifecycle: Option<ConversationLifecycle>,
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
    /// Ordered image references; encoded image data is loaded separately.
    pub attachments: Vec<artisan_domain::ImageAttachmentRef>,
    /// Scene identity.
    pub id: SceneId,
    /// Complete bounded body.
    pub body: String,
    /// Explicit disclosure.
    pub disclosure: Option<SceneDisclosure>,
}

/// Newest-phase computation over one turn's content, mirroring the
/// reference progress phase. Commentary counts on neither side.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProgressPhase {
    /// No reply and no work yet.
    None,
    /// The newest visible progress is model prose.
    Reply,
    /// The newest visible progress is work.
    Work,
}

/// One session detail row owned by a session group.
///
/// Commentary, non-final assistant prose, activities, compaction summaries,
/// and native facts join the owning session here instead of rendering
/// top-level, in durable ordinal order (every variant carries its exact
/// ordinal for merge). The renderer matches nothing on this enum yet; it is
/// purely additive data. Session mode fills only this list and leaves
/// `items` empty; legacy positional groups do the reverse — never both.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionDetail {
    /// Assistant prose that is not the promoted reply (commentary or
    /// interim/unspecified text).
    Assistant {
        /// Scene identity.
        id: SceneId,
        /// Complete bounded body.
        body: String,
        /// Preserved disclosed phase, never inferred.
        phase: AssistantPhase,
        /// Stable ordinal for merge order.
        ordinal: u64,
        /// Durable attribution when the caller supplied it.
        provenance: Option<ItemProvenance>,
        /// Explicit disclosure.
        disclosure: Option<SceneDisclosure>,
    },
    /// Activity or tool-result summary folded into the session.
    Activity {
        /// Scene identity.
        id: SceneId,
        /// Bounded body.
        body: String,
        /// Provider activity kind, when the source row carried one.
        kind: Option<String>,
        /// Raw provider detail, when disclosed.
        detail: Option<String>,
        /// Durable liveness for the chain's live/failed default-open rule.
        lifecycle: Option<ConversationLifecycle>,
        /// Stable ordinal for merge order.
        ordinal: u64,
        /// Explicit disclosure.
        disclosure: Option<SceneDisclosure>,
    },
    /// Compaction summary folded into the session.
    Compaction {
        /// Scene identity.
        id: SceneId,
        /// Bounded summary.
        summary: String,
        /// Stable ordinal for merge order.
        ordinal: u64,
        /// Explicit disclosure.
        disclosure: Option<SceneDisclosure>,
    },
    /// Native fact folded into the session.
    NativeFact {
        /// Scene identity.
        id: SceneId,
        /// Bounded native fact text.
        text: String,
        /// Stable ordinal for merge order.
        ordinal: u64,
        /// Explicit disclosure.
        disclosure: Option<SceneDisclosure>,
    },
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
    /// Durable attribution when the caller supplied it; `None` is
    /// settled/unknown, never live.
    pub provenance: Option<ItemProvenance>,
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
    /// Stable session anchor (`session-{turn_id}`) when this group is a
    /// session; `None` for legacy positional groups, which never carry the
    /// fields below.
    pub session: Option<SceneId>,
    /// The session's run, when run evidence identified one.
    pub session_run: Option<RunId>,
    /// Whether later content in the same turn supersedes this session: a
    /// superseded session never narrates live status.
    pub superseded: bool,
    /// Newest non-empty reasoning body for the one live summary line;
    /// never present on settled rows.
    pub reasoning_summary: Option<String>,
    /// Newest-phase computation for reply-phase disclosure folding.
    pub progress: ProgressPhase,
    /// Engine handoff folded into the session header, when the turn carried
    /// one.
    pub transition: Option<ModelTransitionBlock>,
    /// Session-owned detail rows (assistant prose, compaction, native
    /// facts) in durable ordinal order.
    pub session_details: Vec<SessionDetail>,
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
    /// Authoritative active-work clock basis in Unix millis, if any.
    ///
    /// Present only with an active-work narration. The renderer derives a
    /// stable live elapsed value from its own frame clock minus this basis and
    /// stops ticking once the narration settles.
    pub active_started_at_ms: Option<i64>,
    /// Newest non-empty reasoning body for the one live summary line,
    /// shown in place of the verb; never present on settled rows.
    pub reasoning_summary: Option<String>,
    /// Handoff target label from the turn's model transition, for
    /// `Waiting for {engine}` wording and header attribution.
    pub engine_label: Option<String>,
    /// Typed engine from the turn's send-time metadata, when known; the
    /// summary line policy keys off this, never off the display label.
    pub engine: Option<EngineId>,
}

/// Settled response facts for one turn footer.
///
/// Present only when the turn completed with an eligible settled reply (see
/// the aggregate scene projection). The response text is the exact copy
/// payload and the timestamp is the authoritative Forge settlement time; both
/// stay fixed once set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnFooterSettlement {
    response_text: String,
    settled_at_ms: i64,
}

impl TurnFooterSettlement {
    /// Creates settled footer facts after validating the response text bound.
    ///
    /// # Errors
    ///
    /// Returns [`SceneBuildError::MessageBodyTooLong`] when the response text
    /// exceeds [`SCENE_MAX_MESSAGE_BODY_BYTES`] UTF-8 bytes.
    pub fn new(response_text: String, settled_at_ms: i64) -> Result<Self, SceneBuildError> {
        if response_text.len() > SCENE_MAX_MESSAGE_BODY_BYTES {
            return Err(SceneBuildError::MessageBodyTooLong {
                length: response_text.len(),
                maximum: SCENE_MAX_MESSAGE_BODY_BYTES,
            });
        }
        Ok(Self {
            response_text,
            settled_at_ms,
        })
    }

    /// Returns the exact settled response text retained for the copy payload.
    #[must_use]
    pub fn response_text(&self) -> &str {
        &self.response_text
    }

    /// Returns the authoritative Forge settlement time in Unix millis.
    #[must_use]
    pub const fn settled_at_ms(&self) -> i64 {
        self.settled_at_ms
    }
}

/// Turn footer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnFooterBlock {
    /// Owning turn.
    pub turn_id: TurnId,
    /// Settled response facts, present only for a completed turn with an
    /// eligible settled reply. `None` means the renderer shows no footer.
    pub settlement: Option<TurnFooterSettlement>,
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
    pub(super) turn_scenes: Vec<TurnScene>,
    pub(super) deferred: Vec<DeferredChangeSet>,
    pub(super) promoted: HashMap<TurnId, SceneId>,
}
