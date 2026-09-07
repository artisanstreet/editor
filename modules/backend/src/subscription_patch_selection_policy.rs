//! Dependency-free selection policy for subscription projection patches.
//!
//! The TypeScript subscription boundary receives protocol event envelopes and
//! decides which projection readers should wake. This native leaf keeps that
//! deterministic selection separate from transport, persistence, clocks,
//! serialization, and protocol crates. The local event and projection values
//! are deliberately small typed equivalents of the fields this policy reads
//! or returns; callers retain ownership of decoding and durable I/O.

#![forbid(unsafe_code)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::struct_excessive_bools)]

/// An opaque protocol identifier retained without normalization.
pub type Identifier = String;

/// An ISO-8601 timestamp retained without parsing or clock access.
pub type IsoDateTime = String;

/// A non-negative stream version used by a thread projection.
pub type StreamSequence = u64;

/// Identifies the provider and provider-side reference behind a raw event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawOrigin {
    /// Provider that emitted the raw event.
    pub provider: Identifier,
    /// Provider-side event or observation reference.
    pub reference: Identifier,
}

impl RawOrigin {
    /// Creates a raw origin while preserving both supplied identifiers.
    #[must_use]
    pub fn new(provider: impl Into<Identifier>, reference: impl Into<Identifier>) -> Self {
        Self {
            provider: provider.into(),
            reference: reference.into(),
        }
    }
}

/// A project reference carried by a thread-list projection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectRef {
    /// Human-readable project label.
    pub display_name: String,
    /// Stable project identity.
    pub project_id: Identifier,
    /// Canonical project root path.
    pub root_path: String,
}

/// A category of content-free project-affinity evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectAffinityEvidenceKind {
    /// The active working directory contributed evidence.
    ActiveWorkingDirectory,
    /// A file artifact contributed evidence.
    FileArtifact,
    /// A file mutation contributed evidence.
    FileMutation,
    /// A Git branch contributed evidence.
    GitBranch,
    /// A Git diff contributed evidence.
    GitDiff,
    /// A Git root contributed evidence.
    GitRoot,
    /// A Git worktree contributed evidence.
    GitWorktree,
    /// A historical working directory contributed evidence.
    HistoricalWorkingDirectory,
    /// A process owner contributed evidence.
    ProcessOwner,
    /// A project mention contributed evidence.
    ProjectMention,
    /// A terminal working directory contributed evidence.
    TerminalWorkingDirectory,
    /// Thread metadata contributed evidence.
    ThreadMetadata,
}

/// Counts one kind of project-affinity evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectAffinityEvidenceCount {
    /// Number of unique durable facts in this category.
    pub count: u64,
    /// Category represented by the count.
    pub kind: ProjectAffinityEvidenceKind,
}

/// A scored project candidate in a thread-list projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectAffinityScore {
    /// Evidence counts that produced the score.
    pub evidence: Vec<ProjectAffinityEvidenceCount>,
    /// Candidate project.
    pub project: ProjectRef,
    /// Deterministic score in the protocol's 0..=100 range.
    pub score: u64,
}

/// A suggested project move against an exact affinity projection version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadProjectRehomeSuggestion {
    /// Affinity version used to produce the suggestion.
    pub basis_affinity_version: StreamSequence,
    /// Suggested destination project.
    pub project: ProjectRef,
    /// Deterministic suggestion score in the protocol's 0..=100 range.
    pub score: u64,
}

/// Identifies how a thread-list title was established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadTitleSource {
    /// The title came from initial thread creation.
    Initial,
    /// The title was generated automatically.
    Automatic,
    /// The title was chosen manually.
    Manual,
}

/// The complete thread-list projection value carried by upsert patches.
///
/// Optional fields mirror optional protocol projection fields. The
/// `initialized` constructor is the exact `thread.created` projection from
/// the source policy, including every default and every timestamp copied from
/// the event's `sent_at` value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadListItem {
    /// Monotonic meaningful-activity version.
    pub activity_version: StreamSequence,
    /// Monotonic project-affinity version.
    pub affinity_version: StreamSequence,
    /// Archive timestamp, when the thread is archived.
    pub archived_at: Option<IsoDateTime>,
    /// Thread creation timestamp.
    pub created_at: IsoDateTime,
    /// Current goal text, when known.
    pub current_goal: Option<String>,
    /// Selected engine identity, when a coordinator exists.
    pub engine_id: Option<Identifier>,
    /// Selected model identity, when a coordinator exists.
    pub model_id: Option<String>,
    /// Newest thread activity timestamp.
    pub last_activity_at: IsoDateTime,
    /// Newest user or assistant message timestamp.
    pub last_message_at: Option<IsoDateTime>,
    /// Bounded latest assistant-message preview.
    pub last_assistant_message: Option<String>,
    /// Current live status shown in the thread list.
    pub live_status: String,
    /// Monotonic metadata version.
    pub metadata_version: StreamSequence,
    /// Whether the thread is pinned.
    pub pinned: bool,
    /// Newest root-visible activity timestamp.
    pub reader_activity_at: Option<IsoDateTime>,
    /// Newest reader-acknowledged activity timestamp.
    pub reader_acknowledged_activity_at: Option<IsoDateTime>,
    /// Explicitly selected primary project, when any.
    pub primary_project: Option<ProjectRef>,
    /// Scored project-affinity candidates.
    pub project_affinity_scores: Vec<ProjectAffinityScore>,
    /// Whether automatic project changes are locked.
    pub project_locked: bool,
    /// Suggested automatic rename, when any.
    pub rename_suggestion: Option<String>,
    /// Suggested project rehome, when any.
    pub rehome_suggestion: Option<ThreadProjectRehomeSuggestion>,
    /// Projects linked to the thread.
    pub linked_projects: Vec<ProjectRef>,
    /// Engine-generated summary title, when any.
    pub summary_title: Option<String>,
    /// Stable thread identity.
    pub thread_id: Identifier,
    /// Display title.
    pub title: String,
    /// Whether the title is manually locked.
    pub title_locked: bool,
    /// How the title was established.
    pub title_source: ThreadTitleSource,
    /// Last projection update timestamp.
    pub updated_at: IsoDateTime,
}

impl ThreadListItem {
    /// Builds the initialized projection emitted for a `thread.created` event.
    ///
    /// The title becomes `current_goal` as well as `title`; all three activity
    /// timestamps and both creation/update timestamps use the exact supplied
    /// `sent_at` spelling. Optional fields omitted by the source event remain
    /// absent, while the source's explicit `last_message_at` and
    /// `reader_activity_at` values are represented as `Some`.
    #[must_use]
    pub fn initialized(
        thread_id: impl Into<Identifier>,
        title: impl Into<String>,
        sent_at: impl Into<IsoDateTime>,
    ) -> Self {
        let thread_id = thread_id.into();
        let title = title.into();
        let sent_at = sent_at.into();

        Self {
            activity_version: 0,
            affinity_version: 0,
            archived_at: None,
            created_at: sent_at.clone(),
            current_goal: Some(title.clone()),
            engine_id: None,
            model_id: None,
            last_activity_at: sent_at.clone(),
            last_message_at: Some(sent_at.clone()),
            last_assistant_message: None,
            live_status: "Idle".to_owned(),
            metadata_version: 0,
            pinned: false,
            reader_activity_at: Some(sent_at.clone()),
            reader_acknowledged_activity_at: None,
            primary_project: None,
            project_affinity_scores: Vec::new(),
            project_locked: false,
            rename_suggestion: None,
            rehome_suggestion: None,
            linked_projects: Vec::new(),
            summary_title: None,
            thread_id,
            title,
            title_locked: false,
            title_source: ThreadTitleSource::Initial,
            updated_at: sent_at,
        }
    }

    /// Alias for [`Self::initialized`] for callers constructing a fixture.
    #[must_use]
    pub fn new(
        thread_id: impl Into<Identifier>,
        title: impl Into<String>,
        sent_at: impl Into<IsoDateTime>,
    ) -> Self {
        Self::initialized(thread_id, title, sent_at)
    }
}

/// The event payload forms needed by the subscription-selection boundary.
///
/// Unit variants retain the complete durable event vocabulary even though the
/// predicates only inspect event type for them. Projection-bearing variants
/// retain their full typed thread projection. `Other` is an explicit escape
/// hatch for an event introduced after this leaf; it never fabricates a
/// projection or graph-group identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventPayload {
    /// Creates a thread with its initial title.
    ThreadCreated { title: String },
    /// Replaces erased historical content.
    ThreadContentErased,
    /// Marks the terminal point of an erased thread stream.
    ThreadErased,
    /// Replays a reader-attention acknowledgement.
    ThreadAttentionAcknowledged,
    /// Carries a complete replacement thread-list projection.
    ThreadMetadataUpdated { thread: ThreadListItem },
    /// Records a refinement that lost its version race.
    ThreadRefinementIgnored,
    /// Carries a complete replacement project-affinity projection.
    ThreadProjectAffinityUpdated { thread: ThreadListItem },
    /// Records an affinity input that did not change the projection.
    ThreadProjectAffinityIgnored,
    /// Updates the global thread-retention policy.
    ThreadRetentionPolicyUpdated,
    /// Updates model favorites.
    ModelFavoritesUpdated,
    /// Updates session defaults.
    SessionDefaultsUpdated,
    /// Updates a usage-interruption projection.
    UsageInterruptionUpdated,
    /// Updates canonical global guidance.
    GlobalGuidanceCanonicalUpdated,
    /// Requires global-guidance source selection.
    GlobalGuidanceSelectionRequired,
    /// Reconciles a global-guidance provider.
    GlobalGuidanceProviderReconciled,
    /// Updates a model-behaviour setting.
    ModelBehaviourSettingUpdated,
    /// Reconciles a model-behaviour provider.
    ModelBehaviourProviderReconciled,
    /// Updates a Marketplace ledger item.
    MarketplaceLedger,
    /// Updates a workspace change projection.
    WorkspaceChangeUpdated,
    /// Updates a workspace conflict projection.
    WorkspaceConflictUpdated,
    /// Records a queued user message.
    ThreadMessageQueued,
    /// Records a steering message.
    ThreadMessageSteering,
    /// Records a final message-routing decision.
    ThreadMessageRouted,
    /// Records a run lifecycle state.
    RunLifecycle,
    /// Records a completed assistant message.
    AssistantMessageCompleted,
    /// Records an approval interaction.
    InteractionApproval,
    /// Records a question interaction.
    InteractionQuestion,
    /// Records an intake assessment.
    IntakeAssessed,
    /// Records an intake assumption.
    IntakeAssumptionRecorded,
    /// Updates automatic steering preference.
    ThreadAutoSteerUpdated,
    /// Updates the thread session policy.
    ThreadSessionPolicyUpdated,
    /// Records a filesystem mutation.
    FilesystemMutation,
    /// Records process ownership.
    ProcessOwnership,
    /// Records an observed Git workspace.
    GitWorkspaceObserved,
    /// Updates a Git workspace projection.
    GitWorkspaceUpdated,
    /// Updates a Git mutation projection.
    GitMutationUpdated,
    /// Records a terminal lifecycle transition.
    TerminalLifecycle,
    /// Records a graph-node lifecycle transition.
    OrchestrationGraphLifecycle {
        group_id: Identifier,
        action: String,
    },
    /// Records a visible assignment heartbeat.
    AssignmentHeartbeat { group_id: Identifier },
    /// Records an agent-instance rename.
    AgentInstanceRenamed { group_id: Identifier },
    /// Records an assignment-control outcome.
    AssignmentControl { group_id: Identifier },
    /// Records a graph result artifact.
    ArtifactRecorded { group_id: Identifier },
    /// Records an Artisan tool-invocation update.
    ArtisanToolInvocationUpdated,
    /// Records an Artisan approval update.
    ArtisanApprovalUpdated,
    /// Records an Artisan assumption.
    ArtisanAssumptionRecorded,
    /// Records an engine-native action.
    ArtisanNativeAction,
    /// Updates a preview target.
    PreviewTargetUpdated,
    /// Updates a preview inspection session.
    PreviewInspectionSessionUpdated,
    /// Records a thread model transition.
    ThreadModelTransition,
    /// Carries an event type not yet modeled by this leaf.
    Other { event_type: String },
}

impl EventPayload {
    /// Creates a `thread.created` payload.
    #[must_use]
    pub fn thread_created(title: impl Into<String>) -> Self {
        Self::ThreadCreated {
            title: title.into(),
        }
    }

    /// Creates a metadata projection payload.
    #[must_use]
    pub fn thread_metadata_updated(thread: ThreadListItem) -> Self {
        Self::ThreadMetadataUpdated { thread }
    }

    /// Creates a project-affinity projection payload.
    #[must_use]
    pub fn thread_project_affinity_updated(thread: ThreadListItem) -> Self {
        Self::ThreadProjectAffinityUpdated { thread }
    }

    /// Creates a graph lifecycle payload while preserving its action spelling.
    #[must_use]
    pub fn orchestration_graph_lifecycle(
        group_id: impl Into<Identifier>,
        action: impl Into<String>,
    ) -> Self {
        Self::OrchestrationGraphLifecycle {
            group_id: group_id.into(),
            action: action.into(),
        }
    }

    /// Creates an opaque event payload.
    #[must_use]
    pub fn other(event_type: impl Into<String>) -> Self {
        Self::Other {
            event_type: event_type.into(),
        }
    }

    /// Returns the exact protocol event-type spelling represented by this payload.
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Self::ThreadCreated { .. } => "thread.created",
            Self::ThreadContentErased => "thread.content_erased",
            Self::ThreadErased => "thread.erased",
            Self::ThreadAttentionAcknowledged => "thread.attention.acknowledged",
            Self::ThreadMetadataUpdated { .. } => "thread.metadata.updated",
            Self::ThreadRefinementIgnored => "thread.refinement.ignored",
            Self::ThreadProjectAffinityUpdated { .. } => "thread.project_affinity.updated",
            Self::ThreadProjectAffinityIgnored => "thread.project_affinity.ignored",
            Self::ThreadRetentionPolicyUpdated => "thread.retention.updated",
            Self::ModelFavoritesUpdated => "model.favorites.updated",
            Self::SessionDefaultsUpdated => "session.defaults.updated",
            Self::UsageInterruptionUpdated => "usage.interruption.updated",
            Self::GlobalGuidanceCanonicalUpdated => "guidance.canonical.updated",
            Self::GlobalGuidanceSelectionRequired => "guidance.selection.required",
            Self::GlobalGuidanceProviderReconciled => "guidance.provider.reconciled",
            Self::ModelBehaviourSettingUpdated => "model_behaviour.setting.updated",
            Self::ModelBehaviourProviderReconciled => "model_behaviour.provider.reconciled",
            Self::MarketplaceLedger => "marketplace.lifecycle",
            Self::WorkspaceChangeUpdated => "workspace.change.updated",
            Self::WorkspaceConflictUpdated => "workspace.conflict.updated",
            Self::ThreadMessageQueued => "thread.message_queued",
            Self::ThreadMessageSteering => "thread.message_steering",
            Self::ThreadMessageRouted => "thread.message_routed",
            Self::RunLifecycle => "run.lifecycle",
            Self::AssistantMessageCompleted => "assistant.message_completed",
            Self::InteractionApproval => "interaction.approval",
            Self::InteractionQuestion => "interaction.question",
            Self::IntakeAssessed => "intake.assessed",
            Self::IntakeAssumptionRecorded => "intake.assumption_recorded",
            Self::ThreadAutoSteerUpdated => "thread.auto_steer.updated",
            Self::ThreadSessionPolicyUpdated => "thread.session_policy.updated",
            Self::FilesystemMutation => "filesystem.mutation",
            Self::ProcessOwnership => "process.ownership",
            Self::GitWorkspaceObserved => "git.workspace.observed",
            Self::GitWorkspaceUpdated => "git.workspace.updated",
            Self::GitMutationUpdated => "git.mutation.updated",
            Self::TerminalLifecycle => "terminal.lifecycle",
            Self::OrchestrationGraphLifecycle { .. } => "orchestration.graph.lifecycle",
            Self::AssignmentHeartbeat { .. } => "assignment.heartbeat",
            Self::AgentInstanceRenamed { .. } => "agent_instance.renamed",
            Self::AssignmentControl { .. } => "assignment.control",
            Self::ArtifactRecorded { .. } => "artifact.recorded",
            Self::ArtisanToolInvocationUpdated => "artisan.tool.invocation.updated",
            Self::ArtisanApprovalUpdated => "artisan.approval.updated",
            Self::ArtisanAssumptionRecorded => "artisan.assumption.recorded",
            Self::ArtisanNativeAction => "engine.native_action",
            Self::PreviewTargetUpdated => "preview.target.updated",
            Self::PreviewInspectionSessionUpdated => "preview.inspection.updated",
            Self::ThreadModelTransition => "thread.model_transition",
            Self::Other { event_type } => event_type,
        }
    }
}

/// The event envelope fields consumed by this selection policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventEnvelope {
    /// Thread stream to which the event belongs.
    pub thread_id: Identifier,
    /// Backend send timestamp used by initialized projections.
    pub sent_at: IsoDateTime,
    /// Optional raw provider attribution.
    pub raw_origin: Option<RawOrigin>,
    /// Typed durable payload.
    pub payload: EventPayload,
}

impl EventEnvelope {
    /// Creates an event without raw provider attribution.
    #[must_use]
    pub fn new(
        thread_id: impl Into<Identifier>,
        sent_at: impl Into<IsoDateTime>,
        payload: EventPayload,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            sent_at: sent_at.into(),
            raw_origin: None,
            payload,
        }
    }

    /// Adds raw provider attribution to an event.
    #[must_use]
    pub fn with_raw_origin(mut self, raw_origin: RawOrigin) -> Self {
        self.raw_origin = Some(raw_origin);
        self
    }
}

/// A direct thread-list projection patch selected from one event.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThreadListProjectionPatch {
    /// Removes the exact erased thread identity.
    Remove { thread_id: Identifier },
    /// Upserts the exact projection supplied or initialized from the event.
    Upsert { thread: ThreadListItem },
}

impl ThreadListProjectionPatch {
    /// Returns the thread identity carried by either patch form.
    #[must_use]
    pub fn thread_id(&self) -> &str {
        match self {
            Self::Remove { thread_id } => thread_id,
            Self::Upsert { thread } => &thread.thread_id,
        }
    }

    /// Returns whether this patch removes a thread.
    #[must_use]
    pub const fn is_remove(&self) -> bool {
        matches!(self, Self::Remove { .. })
    }

    /// Returns whether this patch upserts a projection.
    #[must_use]
    pub const fn is_upsert(&self) -> bool {
        matches!(self, Self::Upsert { .. })
    }
}

/// Exact event types that wake the transcript projection.
pub const TRANSCRIPT_EVENT_TYPES: &[&str] = &[
    "thread.message_queued",
    "thread.message_steering",
    "assistant.message_completed",
    "interaction.approval",
    "interaction.question",
    "intake.assessed",
    "intake.assumption_recorded",
    "thread.erased",
];

/// Exact event types that carry a graph group identity to graph subscribers.
pub const GRAPH_GROUP_EVENT_TYPES: &[&str] = &[
    "orchestration.graph.lifecycle",
    "assignment.heartbeat",
    "agent_instance.renamed",
    "assignment.control",
    "artifact.recorded",
];

/// Exact event types that wake the surface projection before provider forms.
pub const SURFACE_EVENT_TYPES: &[&str] = &[
    "assistant.message_completed",
    "interaction.approval",
    "interaction.question",
    "run.lifecycle",
    "usage.interruption.updated",
    "thread.erased",
];

/// Exact event types that wake the workspace-conflict projection.
pub const WORKSPACE_CONFLICT_EVENT_TYPES: &[&str] =
    &["workspace.conflict.updated", "thread.erased"];

/// Exact provider-attributed graph actions that update surface projections.
pub const PROVIDER_PROJECTION_GRAPH_ACTIONS: &[&str] = &[
    "attempt_finished",
    "finished",
    "provider_state",
    "summary_recorded",
];

/// Returns a direct thread-list remove/upsert patch for an event, if any.
///
/// Metadata and project-affinity events carry a complete projection and are
/// copied exactly. A creation event receives the complete initialized default
/// projection. Erasure wins first and produces only a remove patch.
#[must_use]
pub fn direct_thread_list_patch(event: &EventEnvelope) -> Option<ThreadListProjectionPatch> {
    match &event.payload {
        EventPayload::ThreadErased => Some(ThreadListProjectionPatch::Remove {
            thread_id: event.thread_id.clone(),
        }),
        EventPayload::ThreadCreated { title } => Some(ThreadListProjectionPatch::Upsert {
            thread: ThreadListItem::initialized(
                event.thread_id.clone(),
                title.clone(),
                event.sent_at.clone(),
            ),
        }),
        EventPayload::ThreadMetadataUpdated { thread }
        | EventPayload::ThreadProjectAffinityUpdated { thread } => {
            Some(ThreadListProjectionPatch::Upsert {
                thread: thread.clone(),
            })
        }
        _ => None,
    }
}

/// Returns the graph group identity for the exact group-event allowlist.
///
/// The event type alone selects all five graph forms; graph action and raw
/// origin are intentionally irrelevant here. Every other event form returns
/// `None`, even if its payload could carry a group-like string elsewhere.
#[must_use]
pub fn graph_group_id(event: &EventEnvelope) -> Option<&str> {
    match &event.payload {
        EventPayload::OrchestrationGraphLifecycle { group_id, .. }
        | EventPayload::AssignmentHeartbeat { group_id }
        | EventPayload::AgentInstanceRenamed { group_id }
        | EventPayload::AssignmentControl { group_id }
        | EventPayload::ArtifactRecorded { group_id } => Some(group_id),
        _ => None,
    }
}

/// Returns whether an event can change the durable transcript projection.
#[must_use]
pub fn event_affects_transcript(event: &EventEnvelope) -> bool {
    TRANSCRIPT_EVENT_TYPES.contains(&event.payload.type_name())
}

/// Returns whether an event can change the surface projection.
///
/// The six direct surface forms are always accepted. Graph lifecycle events
/// and artifacts are accepted only when [`RawOrigin`] is present and graph
/// lifecycle actions use the exact provider allowlist.
#[must_use]
pub fn event_affects_surface(event: &EventEnvelope) -> bool {
    SURFACE_EVENT_TYPES.contains(&event.payload.type_name()) || is_provider_projection_event(event)
}

/// Returns whether an event can change the workspace-conflict projection.
#[must_use]
pub fn event_affects_workspace_conflicts(event: &EventEnvelope) -> bool {
    WORKSPACE_CONFLICT_EVENT_TYPES.contains(&event.payload.type_name())
}

/// Returns whether a raw provider event is eligible for surface projection.
///
/// Raw attribution is a hard prerequisite. A raw artifact is accepted as a
/// provider projection; a raw graph lifecycle is accepted only for
/// `attempt_finished`, `finished`, `provider_state`, or `summary_recorded`.
#[must_use]
pub fn is_provider_projection_event(event: &EventEnvelope) -> bool {
    if event.raw_origin.is_none() {
        return false;
    }

    match &event.payload {
        EventPayload::ArtifactRecorded { .. } => true,
        EventPayload::OrchestrationGraphLifecycle { action, .. } => {
            PROVIDER_PROJECTION_GRAPH_ACTIONS.contains(&action.as_str())
        }
        _ => false,
    }
}

/// Stateless facade for callers that keep policy operations under one name.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SubscriptionPatchSelectionPolicy;

impl SubscriptionPatchSelectionPolicy {
    /// Creates the stateless selection policy value.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Selects a direct thread-list patch.
    #[must_use]
    pub fn direct_thread_list_patch(event: &EventEnvelope) -> Option<ThreadListProjectionPatch> {
        direct_thread_list_patch(event)
    }

    /// Selects an allowlisted graph group identity.
    #[must_use]
    pub fn graph_group_id(event: &EventEnvelope) -> Option<&str> {
        graph_group_id(event)
    }

    /// Evaluates transcript impact.
    #[must_use]
    pub fn event_affects_transcript(event: &EventEnvelope) -> bool {
        event_affects_transcript(event)
    }

    /// Evaluates surface impact.
    #[must_use]
    pub fn event_affects_surface(event: &EventEnvelope) -> bool {
        event_affects_surface(event)
    }

    /// Evaluates workspace-conflict impact.
    #[must_use]
    pub fn event_affects_workspace_conflicts(event: &EventEnvelope) -> bool {
        event_affects_workspace_conflicts(event)
    }

    /// Evaluates the raw-origin/provider-action rule.
    #[must_use]
    pub fn is_provider_projection_event(event: &EventEnvelope) -> bool {
        is_provider_projection_event(event)
    }
}
