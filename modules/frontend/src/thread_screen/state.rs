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

/// One owned checklist entry for the inspector checklist card.
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

/// The native thread screen: header, transcript column, inspector cards, and
/// composer dock.
///
/// State arrives through the small setters below; every render projects the
/// retained facts through the existing policies, so this view owns no
/// presentation logic of its own beyond element structure.
pub struct ThreadScreen {
    pub(super) host: Entity<ConversationHost>,
    pub(super) composer: Entity<NativeComposer>,
    /// Live host observation: re-renders the transcript column (including the
    /// empty overlay) the moment turns arrive, so `No messages yet` can never
    /// go stale while data exists.
    _host_observation: Subscription,
    pub(super) retry_focus: FocusHandle,
    pub(super) theme_mode: ThemeMode,
    pub(super) gate: ThreadScreenGate,
    pub(super) on_retry: Option<ThreadScreenRetry>,
    /// Latest content width (window minus desktop sidebar, logical pixels)
    /// published by the route integrator; `None` until the first publish.
    /// Drives inspector visibility and width.
    content_width_px: Option<f32>,
    pub(super) environment: ThreadEnvironmentInput,
    pub(super) terminals: Vec<TerminalSession>,
    pub(super) terminals_loading: bool,
    pub(super) checklist: Vec<ThreadChecklistEntry>,
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
            environment: ThreadEnvironmentInput::default(),
            terminals: Vec::new(),
            terminals_loading: false,
            checklist: Vec::new(),
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

    /// Publishes which legacy gate branch the screen renders.
    pub fn set_gate(&mut self, gate: ThreadScreenGate) {
        self.gate = gate;
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
    /// inspector appears, disappears, and resizes across the content-width
    /// threshold in both directions with no reserved space while hidden.
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

    /// Returns whether the inspector column renders at the published width.
    ///
    /// `None` (no publish yet) keeps the legacy always-show behavior.
    pub(super) fn inspector_visible(&self) -> bool {
        self.content_width_px.is_none_or(thread_inspector_visible)
    }

    /// Resolves the live inspector column width.
    pub(super) fn inspector_width(&self) -> f32 {
        self.content_width_px
            .map_or(INSPECTOR_WIDTH_PX, thread_inspector_width)
    }

    /// Replaces the owned environment-card input.
    pub fn set_environment(&mut self, environment: ThreadEnvironmentInput) {
        self.environment = environment;
    }

    /// Replaces the owned terminal sessions.
    pub fn set_terminals(&mut self, terminals: Vec<TerminalSession>) {
        self.terminals = terminals;
    }

    /// Publishes whether the terminal list itself is still loading.
    pub fn set_terminals_loading(&mut self, terminals_loading: bool) {
        self.terminals_loading = terminals_loading;
    }

    /// Replaces the owned checklist entries.
    pub fn set_checklist(&mut self, checklist: Vec<ThreadChecklistEntry>) {
        self.checklist = checklist;
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
