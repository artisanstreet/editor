//! Screen state, gate vocabulary, and mounting for [`ThreadScreen`].
//!
//! Extracted verbatim from `thread_screen.rs` during the module split;
//! visibility was widened to `pub(super)` for cross-module render callers and
//! the fields the render module reads.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Which legacy gate branch the screen renders.
///
/// These are the `thread-route-gate.svelte` branches. Render precedence
/// itself stays in [`thread_route_gate_render`]; [`ThreadScreenGate::presence`]
/// projects the presence triple that function consumes, so the policy keeps
/// sole ownership of the branch order.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ThreadScreenGate {
    /// Cold load in flight; renders the centered `FadeArc` mark.
    ///
    /// A fresh mount has no thread-open snapshot yet, so the gate opens on
    /// its cold-load branch exactly like `thread-route-gate.svelte` with
    /// `thread_open === undefined`.
    #[default]
    Loading,
    /// Thread-open snapshot arrived; renders the route frame.
    Open,
    /// Load failed; renders the message with the retry control.
    Failed {
        /// Exact reader-facing failure message.
        message: String,
    },
}

impl ThreadScreenGate {
    /// Projects the `(has_thread_open, loading, has_failure)` presence triple
    /// consumed by [`thread_route_gate_render`].
    #[must_use]
    pub const fn presence(&self) -> (bool, bool, bool) {
        match self {
            Self::Loading => (false, true, false),
            Self::Open => (true, false, false),
            Self::Failed { .. } => (false, false, true),
        }
    }

    /// Returns the failure message for the retry branch, if this is it.
    #[must_use]
    pub fn failure_message(&self) -> Option<&str> {
        match self {
            Self::Failed { message } => Some(message),
            Self::Loading | Self::Open => None,
        }
    }
}
///
/// Activation callback for the gate failure retry control.
///
/// The transport-owned retry itself lives outside this view; the orchestrator
/// installs a callback that starts it. No callback means retry is
/// unavailable, and the control renders disabled rather than faking a retry.
pub type ThreadScreenRetry = Rc<dyn Fn(&mut Window, &mut App)>;

/// One owned checklist entry for the inspector's Checklist section.
///
/// [`ChecklistEntry`] borrows, so entries are stored owned and projected per
/// render through [`crate::thread_panel_policy::present_checklist_entry`].
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadChecklistEntry {
    /// Stable entry identity, retained exactly for the list key.
    pub id: String,
    /// Protocol state used for tone projection.
    pub state: ChecklistEntryState,
    /// Reader-facing entry text, retained exactly.
    pub text: String,
}

impl ThreadChecklistEntry {
    /// Copies one engine plan entry, keeping its identity and text exactly
    /// and mapping its status through [`checklist_entry_state`].
    #[must_use]
    pub fn from_plan_entry(entry: &PlanEntry) -> Self {
        Self {
            id: entry.id().as_str().to_owned(),
            state: checklist_entry_state(entry.status()),
            text: entry.text().to_owned(),
        }
    }

    /// Returns whether this entry is exactly what
    /// [`Self::from_plan_entry`] would copy from `entry`, without copying.
    #[must_use]
    pub fn presents(&self, entry: &PlanEntry) -> bool {
        self.id == entry.id().as_str()
            && self.state == checklist_entry_state(entry.status())
            && self.text == entry.text()
    }
}

/// Maps a provider-neutral plan status onto the checklist policy state.
///
/// The protocol carries three statuses (`pending`, `inProgress`,
/// `completed`); in-progress is the panel's active entry. The policy's
/// fourth state, [`ChecklistEntryState::Skipped`], has no protocol status
/// behind it, so a live plan never renders one.
#[must_use]
pub const fn checklist_entry_state(status: PlanEntryStatus) -> ChecklistEntryState {
    match status {
        PlanEntryStatus::Pending => ChecklistEntryState::Pending,
        PlanEntryStatus::InProgress => ChecklistEntryState::Active,
        PlanEntryStatus::Completed => ChecklistEntryState::Completed,
    }
}

/// The native thread screen: transcript column, the ruled inspector column
/// (Context, Checklist, and Terminals sections), and composer dock.
///
/// State arrives through the small setters below; every render projects the
/// retained facts through the existing policies, so this view owns no
/// presentation logic of its own beyond element structure.
pub struct ThreadScreen {
    pub(super) host: Entity<ConversationHost>,
    pub(super) composer: Entity<NativeComposer>,
    /// Live host observation: re-renders the transcript column the moment
    /// turns arrive.
    _host_observation: Subscription,
    pub(super) retry_focus: FocusHandle,
    pub(super) theme_mode: ThemeMode,
    pub(super) gate: ThreadScreenGate,
    pub(super) on_retry: Option<ThreadScreenRetry>,
    /// Latest content width (window minus desktop sidebar, logical pixels)
    /// published by the route integrator; `None` until the first publish.
    /// Drives inspector visibility; the column width itself is fixed.
    content_width_px: Option<f32>,
    /// Display name of the thread's project for the Context section's
    /// Project row; `None` omits the row.
    pub(super) project_label: Option<String>,
    pub(super) environment: ThreadEnvironmentInput,
    pub(super) terminals: Vec<TerminalSession>,
    pub(super) terminals_loading: bool,
    pub(super) checklist: Vec<ThreadChecklistEntry>,
    pub(super) agents: Vec<ThreadAgentEntry>,
}

impl ThreadScreen {
    /// Builds the screen around an already-mounted conversation host and
    /// composer. Both children stay live for the screen lifetime; the host
    /// carries the real controller-owned transcript.
    ///
    /// crate-internal because [`NativeComposer`](crate::native_composer) is a
    /// packet-2 surface: the crate root [`ThreadScreen::mount`] is the public
    /// factory, and in-crate hosts may also assemble the screen directly.
    pub(crate) fn new(
        host: Entity<ConversationHost>,
        composer: Entity<NativeComposer>,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Self {
        let host_observation = cx.observe(&host, |_, _, cx| {
            cx.notify();
        });
        Self {
            host,
            composer,
            _host_observation: host_observation,
            retry_focus: cx.focus_handle(),
            theme_mode,
            gate: ThreadScreenGate::default(),
            on_retry: None,
            content_width_px: None,
            project_label: None,
            environment: ThreadEnvironmentInput::default(),
            terminals: Vec::new(),
            terminals_loading: false,
            checklist: Vec::new(),
            agents: Vec::new(),
        }
    }

    /// Mounts host, composer, and screen in one application-context step.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationHostError::SceneProjection`] when the fresh
    /// controller cannot produce its empty initial scene.
    pub fn mount(
        thread_id: ThreadId,
        theme_mode: ThemeMode,
        cx: &mut App,
    ) -> Result<Entity<Self>, ConversationHostError> {
        let host = ConversationHost::mount(thread_id, theme_mode, cx)?;
        let composer = cx.new(NativeComposer::new);
        Ok(cx.new(|screen_cx| Self::new(host, composer, theme_mode, screen_cx)))
    }

    /// Mounts a presentation-complete thread screen for visual-proof harnesses.
    ///
    /// Opens the gate, publishes the content width (window minus desktop
    /// sidebar, logical pixels), and names the header from an already-resolved
    /// listing title, so a proof worker can wrap the result in the public
    /// [`desktop_shell`](crate::desktop_shell::desktop_shell) and capture the
    /// actual app background, chrome, and containment with no transport.
    /// Registration of any proof route stays with the root.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationHostError::SceneProjection`] when the fresh
    /// controller cannot produce its empty initial scene.
    pub fn mount_proof(
        thread_id: ThreadId,
        content_width_px: f32,
        cx: &mut App,
    ) -> Result<Entity<Self>, ConversationHostError> {
        let screen = Self::mount(thread_id, ThemeMode::Dark, cx)?;
        screen.update(cx, |screen, _| {
            screen.set_gate(ThreadScreenGate::Open);
            screen.set_content_width(content_width_px);
        });
        Ok(screen)
    }

    /// Returns the mounted conversation host entity.
    #[must_use]
    pub fn host(&self) -> &Entity<ConversationHost> {
        &self.host
    }

    /// Returns the mounted composer entity (packet 2 surface).
    ///
    /// crate-internal for the same reason as [`ThreadScreen::new`]. Reserved
    /// for the route integrator's composer forwarding; not yet called.
    #[allow(dead_code)]
    pub(crate) fn composer(&self) -> &Entity<NativeComposer> {
        &self.composer
    }

    /// Publishes which legacy gate branch the screen renders, returning
    /// whether it changed so a per-render caller notifies only on a change.
    pub fn set_gate(&mut self, gate: ThreadScreenGate) -> bool {
        let changed = self.gate != gate;
        self.gate = gate;
        changed
    }

    /// Installs the gate retry callback (or clears it when `None`).
    pub fn set_retry_handler(&mut self, on_retry: Option<ThreadScreenRetry>) {
        self.on_retry = on_retry;
    }

    /// Publishes the content width the inspector fit is measured from.
    ///
    /// The route integrator calls this every render from live window bounds
    /// minus the live [`DesktopShellStyle::sidebar_width`](crate::desktop_shell::DesktopShellStyle)
    /// (both in logical pixels, never a scaled screenshot reading); the
    /// inspector resizes, appears, and disappears with the content width in
    /// both directions with no reserved space while hidden.
    /// Returns whether the width changed; the caller notifies only then, so
    /// GPUI cannot retain a stale child across a resize yet never repaints
    /// when nothing moved.
    pub fn set_content_width(&mut self, content_width_px: f32) -> bool {
        if self.content_width_px == Some(content_width_px) {
            false
        } else {
            self.content_width_px = Some(content_width_px);
            true
        }
    }

    /// Returns the inspector column width at the published content width.
    ///
    /// `None` (no publish yet) keeps the legacy always-show behavior at the
    /// full column width.
    pub(super) fn inspector_width(&self) -> Option<f32> {
        match self.content_width_px {
            None => Some(THREAD_INSPECTOR_WIDTH_PX),
            Some(content_width_px) => thread_inspector_width(content_width_px),
        }
    }

    /// Returns the inspector column's width while it is on screen, `None`
    /// while it is not.
    ///
    /// On screen means the opened-route branch renders and the published
    /// content width fits the column. The application feeds this to
    /// [`desktop_shell`](crate::desktop_shell::desktop_shell) each frame,
    /// after the route body has published this frame's gate and content
    /// width, so the shell's right junction crosshair appears and disappears
    /// with the column's left rule.
    #[must_use]
    pub fn visible_inspector_width(&self) -> Option<Pixels> {
        if self.gate_branch() == ThreadRouteGateRender::OpenedRoute {
            self.inspector_width().map(px)
        } else {
            None
        }
    }

    /// Publishes the display name of the thread's project for the Context
    /// section's Project row (`None` omits the row), returning whether it
    /// changed so a per-render caller notifies only on a change.
    pub fn set_project_label(&mut self, project_label: Option<String>) -> bool {
        let changed = self.project_label != project_label;
        self.project_label = project_label;
        changed
    }

    /// Returns the published project label behind the Project row.
    #[must_use]
    pub fn project_label(&self) -> Option<&str> {
        self.project_label.as_deref()
    }

    /// Replaces the owned environment input behind the Context section's
    /// Machine, Changes, Branch, and Worktree rows, returning whether it
    /// changed so a per-render caller notifies only on a change.
    pub fn set_environment(&mut self, environment: ThreadEnvironmentInput) -> bool {
        let changed = self.environment != environment;
        self.environment = environment;
        changed
    }

    /// Returns the retained environment input the Context rows project from.
    pub fn environment(&self) -> &ThreadEnvironmentInput {
        &self.environment
    }

    /// Replaces the owned terminal sessions.
    pub fn set_terminals(&mut self, terminals: Vec<TerminalSession>) {
        self.terminals = terminals;
    }

    /// Publishes whether the terminal list itself is still loading.
    pub fn set_terminals_loading(&mut self, terminals_loading: bool) {
        self.terminals_loading = terminals_loading;
    }

    /// Replaces the owned checklist entries, returning whether they changed
    /// so a per-render caller notifies only on a change.
    pub fn set_checklist(&mut self, checklist: Vec<ThreadChecklistEntry>) -> bool {
        let changed = self.checklist != checklist;
        self.checklist = checklist;
        changed
    }

    /// Replaces the owned agent entries, returning whether they changed so
    /// a per-render caller notifies only on a change.
    pub fn set_agents(&mut self, agents: Vec<ThreadAgentEntry>) -> bool {
        let changed = self.agents != agents;
        self.agents = agents;
        changed
    }

    /// Returns the retained agent entries in the order they started.
    #[must_use]
    pub fn agents(&self) -> &[ThreadAgentEntry] {
        &self.agents
    }

    /// Returns whether the retained agents already present exactly `rows`,
    /// compared borrowed so an unchanged frame copies no entry.
    #[must_use]
    pub fn agents_present(&self, rows: &[&crate::engine_observation_state::ToolRow]) -> bool {
        self.agents.len() == rows.len()
            && self
                .agents
                .iter()
                .zip(rows)
                .all(|(entry, row)| entry.presents(row))
    }

    /// Returns the retained checklist entries in plan order.
    #[must_use]
    pub fn checklist(&self) -> &[ThreadChecklistEntry] {
        &self.checklist
    }

    /// Returns whether the retained checklist already presents exactly
    /// `plan`, entry for entry and in order.
    ///
    /// A per-render caller asks this before building owned entries, so an
    /// unchanged plan costs a borrowed comparison and no allocation.
    #[must_use]
    pub fn checklist_presents(&self, plan: &[PlanEntry]) -> bool {
        self.checklist.len() == plan.len()
            && self
                .checklist
                .iter()
                .zip(plan)
                .all(|(entry, plan_entry)| entry.presents(plan_entry))
    }

    /// Forwards a theme-mode change into the transcript surface.
    pub fn set_theme_mode(&mut self, theme_mode: ThemeMode, cx: &mut App) {
        self.theme_mode = theme_mode;
        self.host.update(cx, |host, host_cx| {
            host.surface().update(host_cx, |surface, surface_cx| {
                surface.set_theme_mode(theme_mode, surface_cx);
            });
        });
    }

    /// Projects the gate render branch from the retained gate state.
    ///
    /// This is the same ordered projection the legacy gate uses
    /// (`thread-route-gate.svelte` branches), reused rather than restated.
    pub(super) fn gate_branch(&self) -> ThreadRouteGateRender {
        let (has_thread_open, loading, has_failure) = self.gate.presence();
        thread_route_gate_render(has_thread_open, loading, has_failure)
    }
}
