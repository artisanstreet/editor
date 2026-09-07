//! Dependency-free projection and handoff policy for the thread panel.
//!
//! This is the native value boundary for
//! `routes/components/thread-panel.svelte`. The panel host owns the
//! controllers, Effect execution, browser object URLs, route navigation, and
//! rendering. This leaf only projects an optional plan, records picker state,
//! derives the exact draft keys and workspace route for a selected project,
//! and emits typed adapter actions in their required order.
//!
//! A project handoff is intentionally staged. [`ProjectSelection::move_action`]
//! is handed to the composer-draft adapter first. Once that adapter returns a
//! [`ComposerDraftMoveResult`], [`ProjectSelection::actions_after_move`]
//! produces one release action per orphaned preview URL followed by exactly
//! one navigation action. Release failures are not represented in this leaf:
//! the host attempts every returned release and ignores each release result,
//! so navigation remains last even when one or more releases fail.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// The exact task-tone utility class used for an active checklist entry.
pub const ACTIVE_TASK_TONE_CLASS: &str = "font-medium text-foreground marker:text-foreground";

/// The exact task-tone utility class used for a completed checklist entry.
pub const COMPLETED_TASK_TONE_CLASS: &str =
    "text-muted-foreground line-through marker:text-muted-foreground/60";

/// The exact task-tone utility class used for a pending checklist entry.
pub const PENDING_TASK_TONE_CLASS: &str = "text-muted-foreground marker:text-muted-foreground/60";

/// The exact task-tone utility class used for a skipped checklist entry.
pub const SKIPPED_TASK_TONE_CLASS: &str =
    "text-muted-foreground/70 line-through marker:text-muted-foreground/40";

/// The reserved route segment used when no workspace is present.
pub const DETACHED_WORKSPACE_ROUTE_ID: &str = "_";

/// The four states accepted by the conversation-plan projection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChecklistEntryState {
    /// The entry is currently being worked on.
    Active,
    /// The entry has been completed.
    Completed,
    /// The entry has not started yet.
    Pending,
    /// The entry was intentionally skipped.
    Skipped,
}

impl ChecklistEntryState {
    /// The states in the same order as the Svelte task-tone table.
    pub const ALL: [Self; 4] = [Self::Active, Self::Completed, Self::Pending, Self::Skipped];

    /// Returns the exact lower-case state value used by the protocol and DOM.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Pending => "pending",
            Self::Skipped => "skipped",
        }
    }

    /// Returns the exact task-tone class from the legacy panel.
    #[must_use]
    pub const fn task_tone_class(self) -> &'static str {
        match self {
            Self::Active => ACTIVE_TASK_TONE_CLASS,
            Self::Completed => COMPLETED_TASK_TONE_CLASS,
            Self::Pending => PENDING_TASK_TONE_CLASS,
            Self::Skipped => SKIPPED_TASK_TONE_CLASS,
        }
    }

    /// Returns the exact visually-hidden accessibility prefix from the panel.
    #[must_use]
    pub const fn accessibility_prefix(self) -> &'static str {
        match self {
            Self::Active => "active: ",
            Self::Completed => "completed: ",
            Self::Pending => "pending: ",
            Self::Skipped => "skipped: ",
        }
    }
}

/// One already-decoded conversation-plan entry needed by the panel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ChecklistEntry<'a> {
    /// Stable entry identity, retained exactly for the host's list key.
    pub id: &'a str,
    /// The protocol state used for tone and accessibility projection.
    pub state: ChecklistEntryState,
    /// Reader-facing entry text, retained exactly.
    pub text: &'a str,
}

impl<'a> ChecklistEntry<'a> {
    /// Creates one borrowed checklist entry without changing any input text.
    #[must_use]
    pub const fn new(id: &'a str, state: ChecklistEntryState, text: &'a str) -> Self {
        Self { id, state, text }
    }
}

/// The plan shape read by the panel's checklist projection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ConversationPlan<'a> {
    /// Entries in their authoritative conversation order.
    pub entries: &'a [ChecklistEntry<'a>],
}

impl<'a> ConversationPlan<'a> {
    /// Creates a plan over an already-decoded ordered entry slice.
    #[must_use]
    pub const fn new(entries: &'a [ChecklistEntry<'a>]) -> Self {
        Self { entries }
    }
}

/// The plan projection consumed by a later native renderer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ChecklistPlanProjection<'a> {
    /// The original entries, or an empty slice when no plan was supplied.
    pub entries: &'a [ChecklistEntry<'a>],
}

/// Projects the optional plan exactly like `plan?.entries ?? []`.
#[must_use]
pub fn project_plan<'a>(plan: Option<&ConversationPlan<'a>>) -> ChecklistPlanProjection<'a> {
    ChecklistPlanProjection {
        entries: plan.map_or(&[], |value| value.entries),
    }
}

/// Returns the plan entries directly, using an empty slice for an absent plan.
#[must_use]
pub fn plan_entries<'a>(plan: Option<&ConversationPlan<'a>>) -> &'a [ChecklistEntry<'a>] {
    project_plan(plan).entries
}

/// The exact checklist facts a renderer needs for one entry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ChecklistEntryPresentation<'a> {
    /// The entry's stable identity.
    pub id: &'a str,
    /// The entry's state.
    pub state: ChecklistEntryState,
    /// The exact task-tone utility class.
    pub task_tone_class: &'static str,
    /// The exact visually-hidden state prefix, including its trailing space.
    pub accessibility_prefix: &'static str,
    /// The entry's reader-facing text.
    pub text: &'a str,
}

/// Projects one decoded entry without rendering or normalizing it.
#[must_use]
pub const fn present_checklist_entry(entry: ChecklistEntry<'_>) -> ChecklistEntryPresentation<'_> {
    ChecklistEntryPresentation {
        id: entry.id,
        state: entry.state,
        task_tone_class: entry.state.task_tone_class(),
        accessibility_prefix: entry.state.accessibility_prefix(),
        text: entry.text,
    }
}

/// Returns the exact task-tone class for a protocol checklist state.
#[must_use]
pub const fn task_tone_class(state: ChecklistEntryState) -> &'static str {
    state.task_tone_class()
}

/// Returns the exact screen-reader state prefix for a protocol checklist state.
#[must_use]
pub const fn state_accessibility_prefix(state: ChecklistEntryState) -> &'static str {
    state.accessibility_prefix()
}

/// Returns the stable composer slot for a new-thread surface.
///
/// An absent workspace uses the shared root slot. A present workspace is
/// inserted verbatim, including an empty or Unicode value; draft keys are not
/// URLs and therefore are not percent-encoded here.
#[must_use]
pub fn new_thread_draft_key(workspace_id: Option<&str>) -> String {
    match workspace_id {
        None => "draft:new-thread".to_owned(),
        Some(workspace_id) => format!("draft:{workspace_id}"),
    }
}

/// Percent-encodes one value with the exact unescaped set of JavaScript
/// `encodeURIComponent`.
///
/// Rust strings are valid UTF-8 scalar-value sequences, so each scalar is
/// encoded using its UTF-8 bytes. The host route adapter can therefore pass
/// the returned path directly to the same route surface without normalizing
/// IDs or treating a slash as a path separator.
#[must_use]
pub fn encode_uri_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut encoded = String::with_capacity(value.len());
    for character in value.chars() {
        let mut bytes = [0; 4];
        for byte in character.encode_utf8(&mut bytes).bytes() {
            if is_uri_component_unescaped(byte) {
                encoded.push(byte as char);
            } else {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    encoded
}

/// Builds the canonical workspace route used after selecting a project.
///
/// `None` maps to the reserved detached segment `_`; present IDs, including
/// empty IDs, are encoded as one route segment exactly as in
/// `WorkspaceRoutePath`.
#[must_use]
pub fn workspace_route_path(workspace_id: Option<&str>) -> String {
    let route_id = workspace_id.unwrap_or(DETACHED_WORKSPACE_ROUTE_ID);
    format!("/t/{}", encode_uri_component(route_id))
}

const fn is_uri_component_unescaped(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')'
    )
}

/// The thread and workspace identities available when the panel selects a
/// project.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadPanelContext<'a> {
    /// The current thread identity, when the panel is inside a thread route.
    pub thread_id: Option<&'a str>,
    /// The current workspace identity, when the panel is on a workspace route.
    pub workspace_id: Option<&'a str>,
}

impl<'a> ThreadPanelContext<'a> {
    /// Creates a context while preserving optional identity distinctions.
    #[must_use]
    pub const fn new(thread_id: Option<&'a str>, workspace_id: Option<&'a str>) -> Self {
        Self {
            thread_id,
            workspace_id,
        }
    }

    /// Selects the current draft key, preferring the thread identity exactly.
    #[must_use]
    pub fn current_draft_key(self) -> String {
        self.thread_id
            .map_or_else(|| new_thread_draft_key(self.workspace_id), str::to_owned)
    }
}

/// One attachment displaced by the destination draft.
///
/// The panel only needs `preview_url`; attachment bytes and ownership remain
/// with the composer and browser adapters.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OrphanedAttachment {
    /// The exact browser object URL the host must attempt to release.
    pub preview_url: String,
}

impl OrphanedAttachment {
    /// Creates an orphan record while retaining its URL byte-for-byte.
    #[must_use]
    pub fn new(preview_url: impl Into<String>) -> Self {
        Self {
            preview_url: preview_url.into(),
        }
    }
}

/// The already-decoded result of moving one composer draft.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ComposerDraftMoveResult {
    /// Whether the composer store moved a draft into the destination slot.
    ///
    /// The legacy panel does not branch on this field; once the Move effect
    /// succeeds, it releases returned orphans and navigates even when the
    /// result says `moved: false`.
    pub moved: bool,
    /// Attachments displaced by the destination draft, in release order.
    pub orphaned: Vec<OrphanedAttachment>,
}

impl ComposerDraftMoveResult {
    /// Creates a Move result from its decoded status and ordered orphans.
    #[must_use]
    pub fn new(moved: bool, orphaned: Vec<OrphanedAttachment>) -> Self {
        Self { moved, orphaned }
    }
}

/// One operation a thread-panel host adapter must perform.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ThreadPanelAdapterAction {
    /// Set the controlled project-picker state to open.
    OpenProjectPicker,
    /// Move the current composer draft to the selected project's slot.
    MoveComposerDraft {
        /// The exact source draft key.
        from_draft_key: String,
        /// The exact destination draft key.
        to_draft_key: String,
    },
    /// Attempt to release one orphaned browser object URL.
    ///
    /// The host must ignore this operation's failure and continue with later
    /// release actions.
    ReleaseBrowserObjectUrl {
        /// The exact preview URL to pass to the browser adapter.
        preview_url: String,
    },
    /// Request navigation to the selected project's workspace route.
    Navigate {
        /// The exact path produced by [`workspace_route_path`].
        path: String,
    },
}

/// The exact derived keys and navigation target for one selected project.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProjectSelection {
    /// The selected project's exact identity.
    pub project_id: String,
    /// The current draft key used as the Move source.
    pub current_draft_key: String,
    /// The selected project's new-thread draft key used as the Move target.
    pub destination_draft_key: String,
    /// The selected project's workspace route.
    pub navigation_path: String,
}

impl ProjectSelection {
    /// Derives one project handoff from the current panel context.
    #[must_use]
    pub fn new(context: ThreadPanelContext<'_>, project_id: impl Into<String>) -> Self {
        let project_id = project_id.into();
        let current_draft_key = context.current_draft_key();
        let destination_draft_key = new_thread_draft_key(Some(&project_id));
        let navigation_path = workspace_route_path(Some(&project_id));

        Self {
            project_id,
            current_draft_key,
            destination_draft_key,
            navigation_path,
        }
    }

    /// Returns the first typed action in the handoff sequence.
    pub fn move_action(&self) -> ThreadPanelAdapterAction {
        ThreadPanelAdapterAction::MoveComposerDraft {
            from_draft_key: self.current_draft_key.clone(),
            to_draft_key: self.destination_draft_key.clone(),
        }
    }

    /// Returns releases in Move-result order followed by the final navigation.
    ///
    /// The `moved` flag is intentionally not inspected: the source component
    /// branches only on whether `Move` itself failed, not on this result field.
    /// Release failures are likewise absent from the returned action values,
    /// which makes the host's ignore-and-continue responsibility explicit.
    #[must_use]
    pub fn actions_after_move(
        &self,
        move_result: &ComposerDraftMoveResult,
    ) -> Vec<ThreadPanelAdapterAction> {
        let mut actions = Vec::with_capacity(move_result.orphaned.len().saturating_add(1));
        for attachment in &move_result.orphaned {
            actions.push(ThreadPanelAdapterAction::ReleaseBrowserObjectUrl {
                preview_url: attachment.preview_url.clone(),
            });
        }
        actions.push(ThreadPanelAdapterAction::Navigate {
            path: self.navigation_path.clone(),
        });
        actions
    }

    /// Returns the complete observable sequence: Move, releases, then Navigate.
    #[must_use]
    pub fn adapter_actions(
        &self,
        move_result: &ComposerDraftMoveResult,
    ) -> Vec<ThreadPanelAdapterAction> {
        let mut actions = Vec::with_capacity(move_result.orphaned.len().saturating_add(2));
        actions.push(self.move_action());
        actions.extend(self.actions_after_move(move_result));
        actions
    }
}

/// Builds a project selection from explicit IDs without owning panel state.
#[must_use]
pub fn project_selection(
    context: ThreadPanelContext<'_>,
    project_id: impl Into<String>,
) -> ProjectSelection {
    ProjectSelection::new(context, project_id)
}

/// The local state and action boundary owned by one thread panel instance.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ThreadPanelPolicy {
    thread_id: Option<String>,
    workspace_id: Option<String>,
    project_picker_open: bool,
}

impl ThreadPanelPolicy {
    /// Creates a closed panel policy with its current route identities.
    #[must_use]
    pub fn new(thread_id: Option<String>, workspace_id: Option<String>) -> Self {
        Self {
            thread_id,
            workspace_id,
            project_picker_open: false,
        }
    }

    /// Creates a closed policy from borrowed route identities.
    #[must_use]
    pub fn from_ids(thread_id: Option<&str>, workspace_id: Option<&str>) -> Self {
        Self::new(
            thread_id.map(str::to_owned),
            workspace_id.map(str::to_owned),
        )
    }

    /// Returns the current thread identity, preserving absence and emptiness.
    #[must_use]
    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    /// Returns the current workspace identity, preserving absence and emptiness.
    #[must_use]
    pub fn workspace_id(&self) -> Option<&str> {
        self.workspace_id.as_deref()
    }

    /// Returns the route context used for project selection.
    #[must_use]
    pub fn context(&self) -> ThreadPanelContext<'_> {
        ThreadPanelContext::new(self.thread_id(), self.workspace_id())
    }

    /// Returns whether the controlled project picker is open.
    #[must_use]
    pub const fn is_project_picker_open(&self) -> bool {
        self.project_picker_open
    }

    /// Opens the project picker and emits its typed state action.
    ///
    /// Repeating this operation is idempotent: the state remains `true` and
    /// the same state action is returned without toggling or closing it.
    pub fn open_project_picker(&mut self) -> ThreadPanelAdapterAction {
        self.project_picker_open = true;
        ThreadPanelAdapterAction::OpenProjectPicker
    }

    /// Derives the handoff for a selected project using this panel's context.
    #[must_use]
    pub fn select_project(&self, project_id: impl Into<String>) -> ProjectSelection {
        ProjectSelection::new(self.context(), project_id)
    }

    /// Returns the complete typed handoff sequence for a decoded Move result.
    #[must_use]
    pub fn select_project_actions(
        &self,
        project_id: impl Into<String>,
        move_result: &ComposerDraftMoveResult,
    ) -> Vec<ThreadPanelAdapterAction> {
        self.select_project(project_id).adapter_actions(move_result)
    }
}
