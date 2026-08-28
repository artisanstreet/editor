//! Pure state and presentation policy for the thread-route loading gate.
//!
//! This is the dependency-free counterpart of
//! `routes/components/thread-route-gate.svelte`. The caller supplies the
//! already-decoded thread-open snapshot and visual-settlement measurement;
//! this module only retains them and describes typed state transitions. It
//! does not know about protocol schemas, Svelte, transport, DOM observation,
//! rendering, asynchronous execution, or controller services.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// The accessible name used by both the cold-load indicator and the route
/// settlement cover.
pub const LOADING_THREAD_ARIA_LABEL: &str = "Loading thread";

/// The role used by both loading indicators in the legacy component.
pub const LOADING_THREAD_ROLE: &str = "status";

/// The result of the route gate's render-precedence projection.
///
/// The order is intentional: an opened route wins over loading and failure,
/// then loading wins over failure, and a failure wins over the empty
/// fallback. This mirrors the component's ordered Svelte branches even when
/// callers supply an overlapping combination of state flags.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadRouteGateRender {
    /// A thread-open snapshot exists, so the real route is mounted.
    OpenedRoute,
    /// No snapshot exists and the gate is currently loading one.
    LoadingIndicator,
    /// No snapshot exists, loading is complete, and a retryable view is
    /// available for the retained failure message.
    FailureRetry,
    /// No snapshot, load, or failure is present; the host renders nothing.
    EmptyFallback,
}

impl ThreadRouteGateRender {
    /// Returns whether the real thread route is the selected branch.
    #[must_use]
    pub const fn is_opened_route(self) -> bool {
        matches!(self, Self::OpenedRoute)
    }

    /// Returns whether the cold-load indicator is the selected branch.
    #[must_use]
    pub const fn is_loading_indicator(self) -> bool {
        matches!(self, Self::LoadingIndicator)
    }

    /// Returns whether the failure/retry branch is selected.
    #[must_use]
    pub const fn is_failure_retry(self) -> bool {
        matches!(self, Self::FailureRetry)
    }

    /// Returns whether the empty fallback is the selected branch.
    #[must_use]
    pub const fn is_empty_fallback(self) -> bool {
        matches!(self, Self::EmptyFallback)
    }
}

/// A typed reason why a load may be started by the route gate.
///
/// `InitialOpen` is the admission used by the component when no cached
/// snapshot was found. `Retry` describes the failure branch's retry action
/// when no snapshot exists. `NotAdmitted` is returned when an opened route
/// wins render precedence, or for an explicit repeated begin that has no
/// initial-open or retry condition; the begin still applies the component's
/// reset semantics because a caller that has already admitted a load owns
/// that transition.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadRouteGateLoadAdmission {
    /// Start the initial cold open because no cached snapshot exists.
    InitialOpen,
    /// Start a retry because a prior open retained a failure message.
    Retry,
    /// No initial-open or retry condition is currently visible.
    NotAdmitted,
}

impl ThreadRouteGateLoadAdmission {
    /// Returns whether this value describes an admitted load.
    #[must_use]
    pub const fn is_admitted(self) -> bool {
        !matches!(self, Self::NotAdmitted)
    }

    /// Returns whether this is the initial cold-open admission.
    #[must_use]
    pub const fn is_initial_open(self) -> bool {
        matches!(self, Self::InitialOpen)
    }

    /// Returns whether this is a failure retry admission.
    #[must_use]
    pub const fn is_retry(self) -> bool {
        matches!(self, Self::Retry)
    }
}

/// A typed input accepted by [`ThreadRouteGate::apply`].
///
/// Every payload is already owned by the caller. In particular, an open
/// failure carries only its reader-facing message, so the policy preserves
/// the exact `error.message` projection without importing a transport error.
#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadRouteGateAction<Snapshot, Measurement> {
    /// Begin an admitted thread-open load.
    BeginLoad,
    /// Publish a successful open and stop the loading state.
    OpenSuccess {
        /// The exact snapshot returned by the open operation.
        snapshot: Snapshot,
    },
    /// Retain the exact reader-facing error message and stop loading.
    OpenFailure {
        /// The exact message projected from the open error.
        message: String,
    },
    /// Repeat the same load reset used by the failure branch's Retry button.
    Retry,
    /// Reveal the route with the first visual-settlement measurement.
    Reveal {
        /// The measurement produced by the host's visual-settlement owner.
        measurement: Measurement,
    },
}

impl<Snapshot, Measurement> ThreadRouteGateAction<Snapshot, Measurement> {
    /// Creates a load-begin action.
    #[must_use = "a route-gate action must be applied or returned"]
    pub const fn begin_load() -> Self {
        Self::BeginLoad
    }

    /// Creates an action carrying a successful open snapshot.
    #[must_use = "a route-gate action must be applied or returned"]
    pub const fn open_success(snapshot: Snapshot) -> Self {
        Self::OpenSuccess { snapshot }
    }

    /// Creates an action carrying an exact open-failure message.
    #[must_use = "a route-gate action must be applied or returned"]
    pub fn open_failure(message: impl Into<String>) -> Self {
        Self::OpenFailure {
            message: message.into(),
        }
    }

    /// Creates the failure-branch retry action.
    #[must_use = "a route-gate action must be applied or returned"]
    pub const fn retry() -> Self {
        Self::Retry
    }

    /// Creates an action carrying the first settlement measurement candidate.
    #[must_use = "a route-gate action must be applied or returned"]
    pub const fn reveal(measurement: Measurement) -> Self {
        Self::Reveal { measurement }
    }
}

/// A state transition applied by the route gate.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadRouteGateTransition {
    /// A begin action applied the load reset and reports its admission reason.
    LoadBegan {
        /// The admission visible immediately before the reset.
        admission: ThreadRouteGateLoadAdmission,
    },
    /// A successful open stored its snapshot and cleared loading.
    OpenSucceeded,
    /// An open failure stored its exact message and cleared loading.
    OpenFailed,
    /// A retry applied the same reset as a load begin.
    RetryBegan {
        /// The admission visible immediately before the retry reset.
        admission: ThreadRouteGateLoadAdmission,
    },
    /// A first reveal stored the measurement and settled the route.
    Revealed,
    /// A reveal was ignored because the route was already settled.
    RevealIgnored,
}

impl ThreadRouteGateTransition {
    /// Returns whether the transition was the repeated-reveal no-op.
    #[must_use]
    pub const fn is_no_op(self) -> bool {
        matches!(self, Self::RevealIgnored)
    }

    /// Returns whether the transition applied the first reveal.
    #[must_use]
    pub const fn is_revealed(self) -> bool {
        matches!(self, Self::Revealed)
    }
}

/// The pure state owner for one thread-route gate.
///
/// `Snapshot` is the caller's already-decoded thread-open snapshot and
/// `Measurement` is the caller's already-produced visual-settlement value.
/// The constructor exactly mirrors the component's initialization: loading
/// is true only for an absent snapshot, while draft handoff starts settled.
/// All subsequent changes go through typed transitions so the snapshot is
/// never fabricated by a load reset.
#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadRouteGate<Snapshot, Measurement> {
    thread_open: Option<Snapshot>,
    draft_handoff: bool,
    loading: bool,
    failure: Option<String>,
    visual_settlement: Option<Measurement>,
    visually_settled: bool,
}

/// Alias emphasizing that [`ThreadRouteGate`] is a state machine.
pub type ThreadRouteGateState<Snapshot, Measurement> = ThreadRouteGate<Snapshot, Measurement>;

/// Alias using the policy name used by the native frontend module family.
pub type ThreadRouteGatePolicy<Snapshot, Measurement> = ThreadRouteGate<Snapshot, Measurement>;

impl<Snapshot, Measurement> Default for ThreadRouteGate<Snapshot, Measurement> {
    fn default() -> Self {
        Self {
            thread_open: None,
            draft_handoff: false,
            loading: true,
            failure: None,
            visual_settlement: None,
            visually_settled: false,
        }
    }
}

impl<Snapshot, Measurement> ThreadRouteGate<Snapshot, Measurement> {
    /// Creates a gate from the current cached/open snapshot and draft-handoff
    /// observation.
    ///
    /// A present snapshot makes initial loading false. A draft handoff makes
    /// the route visually settled even when there is no snapshot yet; this is
    /// the same first-submission exception used by the source component.
    #[must_use = "a route-gate policy must be retained or transitioned"]
    pub fn new(thread_open: Option<Snapshot>, draft_handoff: bool) -> Self {
        Self {
            loading: thread_open.is_none(),
            visually_settled: draft_handoff,
            thread_open,
            draft_handoff,
            failure: None,
            visual_settlement: None,
        }
    }

    /// Returns the cached or successfully opened snapshot, if any.
    #[must_use]
    pub fn thread_open(&self) -> Option<&Snapshot> {
        self.thread_open.as_ref()
    }

    /// Returns the snapshot through a concise alias for host adapters.
    #[must_use]
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.thread_open()
    }

    /// Returns whether this gate was initialized for a draft handoff.
    #[must_use]
    pub const fn draft_handoff(&self) -> bool {
        self.draft_handoff
    }

    /// Returns whether an open operation is currently loading.
    #[must_use]
    pub const fn is_loading(&self) -> bool {
        self.loading
    }

    /// Returns the exact retained open-failure message, if any.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Returns whether an exact open-failure message is retained.
    #[must_use]
    pub const fn has_failure(&self) -> bool {
        self.failure.is_some()
    }

    /// Returns the first visual-settlement measurement, if one was stored.
    #[must_use]
    pub fn visual_settlement(&self) -> Option<&Measurement> {
        self.visual_settlement.as_ref()
    }

    /// Returns whether the route has reached visual settlement.
    #[must_use]
    pub const fn is_visually_settled(&self) -> bool {
        self.visually_settled
    }

    /// Returns the exact load admission currently visible to the host.
    #[must_use = "a load admission must be acted on or inspected"]
    pub const fn load_admission(&self) -> ThreadRouteGateLoadAdmission {
        if self.thread_open.is_some() {
            ThreadRouteGateLoadAdmission::NotAdmitted
        } else if self.failure.is_some() {
            ThreadRouteGateLoadAdmission::Retry
        } else {
            ThreadRouteGateLoadAdmission::InitialOpen
        }
    }

    /// Alias for [`Self::load_admission`] for hosts that call the step
    /// admission explicitly.
    #[must_use = "a load admission must be acted on or inspected"]
    pub const fn admit_load(&self) -> ThreadRouteGateLoadAdmission {
        self.load_admission()
    }

    /// Applies the component's load-begin reset.
    ///
    /// Loading becomes true, any failure and prior measurement are cleared,
    /// and settlement returns to the initial draft-handoff value. The
    /// existing snapshot is intentionally retained unchanged.
    #[must_use = "a route-gate transition must be handled"]
    pub fn begin_load(&mut self) -> ThreadRouteGateTransition {
        let admission = self.load_admission();
        self.reset_for_load();
        ThreadRouteGateTransition::LoadBegan { admission }
    }

    /// Stores an open success and clears loading.
    ///
    /// This transition does not independently alter failure or visual
    /// settlement; the source's load begin owns those resets. That preserves
    /// exact behavior even when a host applies transitions out of order.
    #[must_use = "a route-gate transition must be handled"]
    pub fn open_success(&mut self, snapshot: Snapshot) -> ThreadRouteGateTransition {
        self.thread_open = Some(snapshot);
        self.loading = false;
        ThreadRouteGateTransition::OpenSucceeded
    }

    /// Stores the exact open-failure message and clears loading.
    ///
    /// The snapshot and settlement are left as they were; a normal load path
    /// has already reset them as required before the failure arrives.
    #[must_use = "a route-gate transition must be handled"]
    pub fn open_failure(&mut self, message: impl Into<String>) -> ThreadRouteGateTransition {
        self.failure = Some(message.into());
        self.loading = false;
        ThreadRouteGateTransition::OpenFailed
    }

    /// Applies the same reset as [`Self::begin_load`] for the Retry control.
    #[must_use = "a route-gate transition must be handled"]
    pub fn retry(&mut self) -> ThreadRouteGateTransition {
        let admission = self.load_admission();
        self.reset_for_load();
        ThreadRouteGateTransition::RetryBegan { admission }
    }

    /// Stores the first settlement measurement and reveals the route.
    ///
    /// Once settled, later measurements are ignored and the first value is
    /// preserved byte-for-byte through the caller's measurement type.
    #[must_use = "a route-gate transition must be handled"]
    pub fn reveal(&mut self, measurement: Measurement) -> ThreadRouteGateTransition {
        if self.visually_settled {
            return ThreadRouteGateTransition::RevealIgnored;
        }

        self.visual_settlement = Some(measurement);
        self.visually_settled = true;
        ThreadRouteGateTransition::Revealed
    }

    /// Applies one typed action and returns the resulting typed transition.
    #[must_use = "a route-gate transition must be handled"]
    pub fn apply(
        &mut self,
        action: ThreadRouteGateAction<Snapshot, Measurement>,
    ) -> ThreadRouteGateTransition {
        match action {
            ThreadRouteGateAction::BeginLoad => self.begin_load(),
            ThreadRouteGateAction::OpenSuccess { snapshot } => self.open_success(snapshot),
            ThreadRouteGateAction::OpenFailure { message } => self.open_failure(message),
            ThreadRouteGateAction::Retry => self.retry(),
            ThreadRouteGateAction::Reveal { measurement } => self.reveal(measurement),
        }
    }

    /// Projects the exact ordered render branch.
    #[must_use = "a render branch must be inspected or rendered"]
    pub const fn render(&self) -> ThreadRouteGateRender {
        thread_route_gate_render(
            self.thread_open.is_some(),
            self.loading,
            self.failure.is_some(),
        )
    }

    /// Projects render, cover, and accessibility facts without performing any
    /// rendering or DOM work.
    #[must_use = "a route-gate presentation must be inspected or rendered"]
    pub const fn presentation(&self) -> ThreadRouteGatePresentation {
        let render = self.render();
        let route_mounted = render.is_opened_route();
        let cover_visible = route_mounted && !self.visually_settled;
        let loading_indicator_visible = cover_visible || render.is_loading_indicator();

        ThreadRouteGatePresentation {
            render,
            route_mounted,
            loading_indicator_visible,
            cover_visible,
            route_opacity_hidden: cover_visible,
            route_pointer_events_none: cover_visible,
            route_aria_hidden: cover_visible,
            route_inert: cover_visible,
            aria_busy: if route_mounted {
                Some(!self.visually_settled)
            } else {
                None
            },
            visual_settlement_callback_attached: route_mounted && !self.draft_handoff,
            loading_indicator_role: if loading_indicator_visible {
                Some(LOADING_THREAD_ROLE)
            } else {
                None
            },
            loading_indicator_aria_label: if loading_indicator_visible {
                Some(LOADING_THREAD_ARIA_LABEL)
            } else {
                None
            },
            visual_settlement_present: route_mounted && self.visual_settlement.is_some(),
        }
    }

    fn reset_for_load(&mut self) {
        self.loading = true;
        self.failure = None;
        self.visual_settlement = None;
        self.visually_settled = self.draft_handoff;
    }
}

/// Projects the legacy branch order from already-observed presence flags.
///
/// The helper keeps the precedence independently testable, including the
/// empty fallback combination that is a legal final branch even though the
/// normal constructor starts a cold, uncached gate in loading state.
#[must_use = "a render branch must be inspected or rendered"]
pub const fn thread_route_gate_render(
    has_thread_open: bool,
    loading: bool,
    has_failure: bool,
) -> ThreadRouteGateRender {
    if has_thread_open {
        ThreadRouteGateRender::OpenedRoute
    } else if loading {
        ThreadRouteGateRender::LoadingIndicator
    } else if has_failure {
        ThreadRouteGateRender::FailureRetry
    } else {
        ThreadRouteGateRender::EmptyFallback
    }
}

/// Alias for the render branch's descriptive state name.
pub type ThreadRouteGateRenderState = ThreadRouteGateRender;

/// Pure render and accessibility facts projected by a route gate.
///
/// `None` for [`Self::aria_busy`] means that the opened-route root does not
/// exist in the selected Svelte branch. The other route flags are false when
/// that route is not mounted. `loading_indicator_visible` includes both the
/// cold-load indicator and the opened-route settlement cover.
#[must_use]
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadRouteGatePresentation {
    /// The ordered branch selected by the gate.
    pub render: ThreadRouteGateRender,
    /// Whether the real thread route is mounted.
    pub route_mounted: bool,
    /// Whether either loading indicator is visible.
    pub loading_indicator_visible: bool,
    /// Whether the absolute settlement cover is visible over the route.
    pub cover_visible: bool,
    /// Whether the route is visually hidden with the cover.
    pub route_opacity_hidden: bool,
    /// Whether pointer events are disabled on the covered route.
    pub route_pointer_events_none: bool,
    /// The route's `aria-hidden` value while covered.
    pub route_aria_hidden: bool,
    /// The route's `inert` value while covered.
    pub route_inert: bool,
    /// The root's `aria-busy` value, absent outside the opened-route branch.
    pub aria_busy: Option<bool>,
    /// Whether the visual-settlement callback is passed to the route.
    pub visual_settlement_callback_attached: bool,
    /// The loading indicator's role, when an indicator exists.
    pub loading_indicator_role: Option<&'static str>,
    /// The loading indicator's accessible label, when an indicator exists.
    pub loading_indicator_aria_label: Option<&'static str>,
    /// Whether the opened route has visual-settlement data attributes with a
    /// retained value.
    pub visual_settlement_present: bool,
}

/// The event spelling commonly used by controller adapters.
pub type ThreadRouteGateEvent<Snapshot, Measurement> = ThreadRouteGateAction<Snapshot, Measurement>;
