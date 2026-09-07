//! Pure reconciliation and presentation policy for the editor route gate.
//!
//! This is the native counterpart of
//! `modules/frontend/src/routes/e/[workspace]/[thread]/editor-route-gate.svelte`.
//! The caller supplies the facts that the route, catalog, and identity
//! adapters already derived. This leaf only orders those facts into a typed
//! reconciliation decision and a renderer-facing presentation. It does not
//! read page globals, resolve a catalog, subscribe to streams, execute
//! navigation, or render a surface.

#![allow(clippy::module_name_repetitions)]

use crate::route_navigation::{RouteNavigationIntent, RouteNavigationOptions};

/// The root path used when the resolved thread has disappeared.
pub const ROOT_ROUTE_PATH: &str = "/";

/// The exact loading text rendered while the thread catalog is unavailable.
pub const LOADING_THREAD_TEXT: &str = "Loading thread…";

/// The exact navigation options used by every redirect from this gate.
pub const EDITOR_ROUTE_NAVIGATION_OPTIONS: RouteNavigationOptions =
    RouteNavigationOptions::new(Some(true), Some(true), Some(true));

/// The minimal project identity needed by the gate and editor presentation.
///
/// The adapter owns protocol decoding and supplies this already-resolved
/// identity. The value is borrowed and is never trimmed, normalized, or
/// otherwise interpreted by this policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResolvedProject<'a> {
    /// The authoritative project/workspace identity.
    pub project_id: &'a str,
}

impl<'a> ResolvedProject<'a> {
    /// Creates a project identity from caller-owned text.
    #[must_use]
    pub const fn new(project_id: &'a str) -> Self {
        Self { project_id }
    }
}

/// The minimal resolved thread projection consumed by the route gate.
///
/// `None` for [`Self::primary_project`] represents a detached thread. An
/// explicitly empty project identifier remains a present project identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResolvedThread<'a> {
    /// The authoritative thread identity.
    pub thread_id: &'a str,
    /// The thread's primary project, when it has one.
    pub primary_project: Option<ResolvedProject<'a>>,
}

impl<'a> ResolvedThread<'a> {
    /// Creates a resolved thread without changing either supplied identity.
    #[must_use]
    pub const fn new(thread_id: &'a str, primary_project: Option<ResolvedProject<'a>>) -> Self {
        Self {
            thread_id,
            primary_project,
        }
    }

    /// Creates a detached thread projection with no primary project.
    #[must_use]
    pub const fn detached(thread_id: &'a str) -> Self {
        Self::new(thread_id, None)
    }

    /// Creates a project-backed thread projection.
    #[must_use]
    pub const fn in_project(thread_id: &'a str, project_id: &'a str) -> Self {
        Self::new(thread_id, Some(ResolvedProject::new(project_id)))
    }
}

/// The adapter-derived target selected for a resolved thread.
///
/// A detached thread normally produces [`Self::Thread`], while a
/// project-backed thread normally produces [`Self::Editor`]. The target is
/// accepted as a value from the identity adapter so this leaf does not build
/// or normalize route paths. The route-workspace ownership fact remains a
/// separate input because it is derived by the route/navigation adapter.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditorRouteTarget<'a> {
    /// A normal thread route, which is not an editor target.
    Thread {
        /// The exact path to use if this target must be reached.
        path: &'a str,
    },
    /// An editor route with its adapter-derived workspace identity.
    Editor {
        /// The exact path, including any query string, to use for navigation.
        path: &'a str,
        /// The workspace identity selected by the identity adapter.
        workspace_id: &'a str,
    },
}

impl<'a> EditorRouteTarget<'a> {
    /// Creates a non-editor thread target without changing its path.
    #[must_use]
    pub const fn thread(path: &'a str) -> Self {
        Self::Thread { path }
    }

    /// Creates an editor target without changing its path or workspace.
    #[must_use]
    pub const fn editor(path: &'a str, workspace_id: &'a str) -> Self {
        Self::Editor { path, workspace_id }
    }

    /// Returns the exact target path, including its query string.
    #[must_use]
    pub const fn path(self) -> &'a str {
        match self {
            Self::Thread { path } | Self::Editor { path, .. } => path,
        }
    }

    /// Returns whether this target is the editor branch.
    #[must_use]
    pub const fn is_editor(self) -> bool {
        matches!(self, Self::Editor { .. })
    }

    /// Returns the adapter-selected workspace for an editor target.
    #[must_use]
    pub const fn workspace_id(self) -> Option<&'a str> {
        match self {
            Self::Thread { .. } => None,
            Self::Editor { workspace_id, .. } => Some(workspace_id),
        }
    }
}

/// All adapter-derived facts needed for one editor-route reconciliation.
///
/// `current_url` is the exact pathname-plus-search representation supplied by
/// the URL adapter. It is deliberately compared as text: this policy does not
/// parse, decode, reorder, or normalize query parameters. `route_owns_target`
/// and `route_workspace_owned` are also already-derived facts; their adapters
/// own page/navigation identity and thread workspace comparison respectively.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EditorRouteGateInput<'a> {
    /// Whether this route scope still owns the current or in-flight target.
    pub route_owns_target: bool,
    /// Whether the authoritative thread catalog has finished loading.
    pub catalog_loaded: bool,
    /// The resolved thread, or `None` when the catalog no longer contains it.
    pub resolved_thread: Option<ResolvedThread<'a>>,
    /// The identity adapter's target for the resolved thread, when available.
    pub editor_target: Option<EditorRouteTarget<'a>>,
    /// The exact current pathname plus query string.
    pub current_url: &'a str,
    /// Whether the route workspace owns the resolved thread's workspace.
    pub route_workspace_owned: bool,
}

impl<'a> EditorRouteGateInput<'a> {
    /// Creates one reconciliation input without reading or deriving any
    /// additional route state.
    #[must_use]
    pub const fn new(
        route_owns_target: bool,
        catalog_loaded: bool,
        resolved_thread: Option<ResolvedThread<'a>>,
        editor_target: Option<EditorRouteTarget<'a>>,
        current_url: &'a str,
        route_workspace_owned: bool,
    ) -> Self {
        Self {
            route_owns_target,
            catalog_loaded,
            resolved_thread,
            editor_target,
            current_url,
            route_workspace_owned,
        }
    }
}

/// The ordered side-effect description emitted by reconciliation.
///
/// `NoOp` leaves any caller-owned active thread untouched. The navigation
/// variant explicitly tells the caller to clear that active state before it
/// executes the returned intent. `Activate` is the only branch that installs
/// the resolved thread as active state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorRouteGateDecision<'a> {
    /// The route scope cannot act yet or no longer owns the target.
    NoOp,
    /// Clear active state, then execute the exact navigation intent.
    ClearActiveAndNavigate {
        /// The path navigation request with all three exact flags enabled.
        navigation: RouteNavigationIntent,
    },
    /// Activate the resolved thread after an exact editor/workspace/URL match.
    Activate {
        /// The resolved thread to install as active state.
        thread: ResolvedThread<'a>,
    },
}

impl<'a> EditorRouteGateDecision<'a> {
    /// Returns whether this decision clears the caller's active state.
    #[must_use]
    pub const fn clears_active_state(&self) -> bool {
        matches!(self, Self::ClearActiveAndNavigate { .. })
    }

    /// Returns whether this decision performs no state or navigation action.
    #[must_use]
    pub const fn is_no_op(&self) -> bool {
        matches!(self, Self::NoOp)
    }

    /// Returns the navigation intent when this decision requests one.
    #[must_use]
    pub fn navigation(&self) -> Option<&RouteNavigationIntent> {
        match self {
            Self::ClearActiveAndNavigate { navigation } => Some(navigation),
            Self::NoOp | Self::Activate { .. } => None,
        }
    }

    /// Returns the thread to activate when this is an activation decision.
    #[must_use]
    pub const fn activated_thread(&self) -> Option<ResolvedThread<'a>> {
        match self {
            Self::Activate { thread } => Some(*thread),
            Self::NoOp | Self::ClearActiveAndNavigate { .. } => None,
        }
    }
}

/// The exact renderer-facing editor-route surface.
///
/// `NoSurface` covers every settled state without an active project-backed
/// thread. It is distinct from `Loading` so a renderer cannot accidentally
/// retain the loading surface after the catalog has settled.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditorRouteGatePresentation<'a> {
    /// Mount the editor for one active thread and workspace.
    Editor {
        /// The active thread identity passed to the editor.
        thread_id: &'a str,
        /// The active thread's primary project identity passed to the editor.
        workspace_id: &'a str,
    },
    /// Show the exact loading status text.
    Loading {
        /// The reader-facing loading message.
        message: &'static str,
    },
    /// Render neither an editor nor a loading surface.
    NoSurface,
}

/// The inputs used by the renderer-facing presentation projection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EditorRouteGatePresentationInput<'a> {
    /// The caller-owned active thread, if reconciliation installed one.
    pub active_thread: Option<ResolvedThread<'a>>,
    /// Whether the authoritative thread catalog has finished loading.
    pub catalog_loaded: bool,
}

impl<'a> EditorRouteGatePresentationInput<'a> {
    /// Creates one presentation input without deriving or reading UI state.
    #[must_use]
    pub const fn new(active_thread: Option<ResolvedThread<'a>>, catalog_loaded: bool) -> Self {
        Self {
            active_thread,
            catalog_loaded,
        }
    }
}

/// Reconciles one editor route using the legacy gate's exact decision order.
///
/// The order is intentionally observable:
///
/// 1. a route scope that no longer owns the target returns [`NoOp`];
/// 2. an unloaded catalog returns [`NoOp`];
/// 3. a missing thread clears active state and navigates to [`ROOT_ROUTE_PATH`];
/// 4. a non-editor target, workspace mismatch, or exact-URL mismatch clears
///    active state and navigates to that target's exact path; and
/// 5. only an editor target with owned workspace and an exact URL activates.
///
/// A resolved thread without an adapter target is an incomplete adapter
/// snapshot. It fails closed as [`NoOp`] because this leaf has no safe path to
/// navigate to. No branch performs navigation; the caller owns execution.
#[must_use]
pub fn reconcile_editor_route_gate(input: EditorRouteGateInput<'_>) -> EditorRouteGateDecision<'_> {
    if !input.route_owns_target || !input.catalog_loaded {
        return EditorRouteGateDecision::NoOp;
    }

    let Some(thread) = input.resolved_thread else {
        return clear_and_navigate(ROOT_ROUTE_PATH);
    };

    let Some(target) = input.editor_target else {
        return EditorRouteGateDecision::NoOp;
    };

    if !target.is_editor() || !input.route_workspace_owned || input.current_url != target.path() {
        return clear_and_navigate(target.path());
    }

    EditorRouteGateDecision::Activate { thread }
}

/// Projects the exact editor-route render precedence.
///
/// A project-backed active thread wins even while the catalog's loaded flag is
/// false. Otherwise an unloaded catalog yields exactly [`LOADING_THREAD_TEXT`]
/// and every other state yields [`EditorRouteGatePresentation::NoSurface`].
#[must_use]
pub fn present_editor_route_gate(
    input: EditorRouteGatePresentationInput<'_>,
) -> EditorRouteGatePresentation<'_> {
    if let Some(thread) = input.active_thread
        && let Some(project) = thread.primary_project
    {
        return EditorRouteGatePresentation::Editor {
            thread_id: thread.thread_id,
            workspace_id: project.project_id,
        };
    }

    if !input.catalog_loaded {
        return EditorRouteGatePresentation::Loading {
            message: LOADING_THREAD_TEXT,
        };
    }

    EditorRouteGatePresentation::NoSurface
}

/// Builds one exact path navigation intent for a gate redirect.
fn clear_and_navigate<'a>(path: &str) -> EditorRouteGateDecision<'a> {
    EditorRouteGateDecision::ClearActiveAndNavigate {
        navigation: RouteNavigationIntent::from_path(path, EDITOR_ROUTE_NAVIGATION_OPTIONS),
    }
}
