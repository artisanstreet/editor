//! Dependency-free geometry-settlement state for a dropdown hover surface.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/components/dropdown-highlight.ts`. The host
//! owns the DOM or native measurement, marker observation, clock, frame
//! scheduler, and observer lifetimes. It supplies those observations here;
//! this module only reduces the `highlighted`, `settle`, and `watch` commands
//! to deterministic state transitions and host actions.
//!
//! The first reveal is conservative: two consecutive equal geometry strings
//! are required before an apply action is emitted. Once revealed, every
//! settle samples the current geometry and applies only a changed value. A
//! later highlighted command is intentionally eager and applies the supplied
//! geometry immediately, matching the already-settled row behavior of the
//! source attachment.

#![allow(clippy::module_name_repetitions)]

/// Length of one geometry-settlement watch window in milliseconds.
pub const SETTLE_WINDOW_MS: f64 = 250.0;

/// A command delivered by the host-side highlight worker.
///
/// The enum carries no observation data so the command vocabulary remains the
/// same as the source queue. Supply the associated
/// [`DropdownHighlightObservation`] to [`DropdownHighlightSettleState::dispatch`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "a dropdown highlight command should be dispatched"]
pub enum DropdownHighlightCommand {
    /// A `data-highlighted` marker mutation was observed.
    Highlighted,
    /// A scheduled frame completed and should sample settlement state.
    Settle,
    /// A resize observation or initial startup renewed the watch window.
    Watch,
}

/// Host observations needed to execute one dropdown-highlight command.
///
/// `now_ms` is supplied by the host and is never read by this module. A
/// missing `geometry` means that no geometry sample was supplied for this
/// transition; it is useful for a marker-removal settle, where the source
/// does not read geometry at all. Geometry strings are kept byte-for-byte.
#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use = "a host observation should be dispatched"]
pub struct DropdownHighlightObservation<'a> {
    /// Host-supplied millisecond timestamp for this transition.
    pub now_ms: f64,
    /// Whether the highlighted marker is present on the observed row.
    pub marker_present: bool,
    /// The host's current offset/size geometry string, when sampled.
    pub geometry: Option<&'a str>,
}

impl<'a> DropdownHighlightObservation<'a> {
    /// Creates one observation with an optional geometry sample.
    pub const fn new(now_ms: f64, marker_present: bool, geometry: Option<&'a str>) -> Self {
        Self {
            now_ms,
            marker_present,
            geometry,
        }
    }

    /// Creates an observation without a geometry sample.
    pub const fn without_geometry(now_ms: f64, marker_present: bool) -> Self {
        Self::new(now_ms, marker_present, None)
    }

    /// Creates an observation with one geometry sample.
    pub const fn with_geometry(now_ms: f64, marker_present: bool, geometry: &'a str) -> Self {
        Self::new(now_ms, marker_present, Some(geometry))
    }
}

/// An effect the host must perform after a command transition.
///
/// These are intents only. In particular, `ScheduleFrame` does not call a
/// frame API and `CancelFrame` does not carry or own a platform frame handle;
/// the host retains that handle and executes the one current request.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use = "a dropdown highlight action should be executed by its host"]
pub enum DropdownHighlightAction {
    /// Move the hover surface onto the supplied geometry.
    ApplyGeometry(String),
    /// Request one frame and deliver `DropdownHighlightCommand::Settle` when
    /// that frame completes.
    ScheduleFrame,
    /// Cancel the host's current frame request.
    CancelFrame,
    /// Disconnect the host's marker mutation observer.
    DisconnectHighlightObserver,
    /// Disconnect the host's size observer.
    DisconnectSizeObserver,
}

/// Pure state for one dropdown-highlight settlement attachment.
///
/// State starts unrevealed with no pending frame and a zero deadline. The host
/// should dispatch [`DropdownHighlightCommand::Watch`] once during startup,
/// just as the source attachment does. A returned [`DropdownHighlightAction`]
/// must be executed by the host before it reports the resulting observation.
#[derive(Debug)]
#[must_use = "a dropdown highlight state should be retained while its host is mounted"]
pub struct DropdownHighlightSettleState {
    applied_geometry: Option<String>,
    sampled_geometry: Option<String>,
    deadline_ms: f64,
    frame_pending: bool,
    cleaned_up: bool,
}

impl DropdownHighlightSettleState {
    /// Creates an unrevealed settlement state.
    pub const fn new() -> Self {
        Self {
            applied_geometry: None,
            sampled_geometry: None,
            deadline_ms: 0.0,
            frame_pending: false,
            cleaned_up: false,
        }
    }

    /// Reduces one command and its host observation to host actions.
    ///
    /// `Watch` uses only `observation.now_ms`; `Highlighted` and `Settle` use
    /// marker presence and geometry according to their source transition.
    /// Commands after [`Self::cleanup`] are ignored because the source worker
    /// has already been released.
    #[must_use]
    pub fn dispatch(
        &mut self,
        command: DropdownHighlightCommand,
        observation: DropdownHighlightObservation<'_>,
    ) -> Vec<DropdownHighlightAction> {
        if self.cleaned_up {
            return Vec::new();
        }

        match command {
            DropdownHighlightCommand::Highlighted => self.handle_highlighted(observation),
            DropdownHighlightCommand::Settle => self.handle_settle(observation),
            DropdownHighlightCommand::Watch => self.handle_watch(observation.now_ms),
        }
    }

    /// Handles a `highlighted` command.
    ///
    /// A missing marker is a complete no-op. After the first reveal, a
    /// present marker applies the supplied geometry immediately before
    /// renewing the watch. Before reveal, geometry is deliberately ignored;
    /// only settle samples participate in the two-consecutive-samples rule.
    #[must_use]
    pub fn highlighted(
        &mut self,
        observation: DropdownHighlightObservation<'_>,
    ) -> Vec<DropdownHighlightAction> {
        self.dispatch(DropdownHighlightCommand::Highlighted, observation)
    }

    /// Handles a `settle` command.
    ///
    /// The completed frame is cleared before sampling. A sample is read only
    /// while the marker is present. The next frame is requested only when the
    /// supplied time is strictly before the current deadline, so equality at
    /// the deadline is settled and stops the watch.
    #[must_use]
    pub fn settle(
        &mut self,
        observation: DropdownHighlightObservation<'_>,
    ) -> Vec<DropdownHighlightAction> {
        self.dispatch(DropdownHighlightCommand::Settle, observation)
    }

    /// Handles a `watch` command using a host-supplied timestamp.
    ///
    /// Every watch replaces the deadline with `now_ms + 250`, but it emits a
    /// schedule action only if no logical frame is pending.
    #[must_use]
    pub fn watch(&mut self, now_ms: f64) -> Vec<DropdownHighlightAction> {
        self.dispatch(
            DropdownHighlightCommand::Watch,
            DropdownHighlightObservation::without_geometry(now_ms, false),
        )
    }

    /// Performs cleanup and returns the host actions in source order.
    ///
    /// The cancellation action is emitted even when no frame is pending,
    /// matching the source's unconditional `cancelAnimationFrame` call (the
    /// host treats an empty current handle as a no-op). Repeated cleanup is
    /// idempotent and emits no second disconnect or cancellation.
    #[must_use]
    pub fn cleanup(&mut self) -> Vec<DropdownHighlightAction> {
        if self.cleaned_up {
            return Vec::new();
        }

        self.cleaned_up = true;
        self.frame_pending = false;
        vec![
            DropdownHighlightAction::CancelFrame,
            DropdownHighlightAction::DisconnectHighlightObserver,
            DropdownHighlightAction::DisconnectSizeObserver,
        ]
    }

    /// Returns the geometry most recently applied to the hover surface.
    #[must_use]
    pub fn applied_geometry(&self) -> Option<&str> {
        self.applied_geometry.as_deref()
    }

    /// Returns the most recent geometry sample, including an unrevealed one.
    #[must_use]
    pub fn sampled_geometry(&self) -> Option<&str> {
        self.sampled_geometry.as_deref()
    }

    /// Returns the current exclusive end of the watch window.
    #[must_use]
    pub const fn deadline_ms(&self) -> f64 {
        self.deadline_ms
    }

    /// Returns whether one host frame request is logically pending.
    #[must_use]
    pub const fn frame_pending(&self) -> bool {
        self.frame_pending
    }

    /// Returns whether a geometry has been revealed at least once.
    #[must_use]
    pub const fn is_revealed(&self) -> bool {
        self.applied_geometry.is_some()
    }

    /// Returns whether cleanup has retired this state machine.
    #[must_use]
    pub const fn is_cleaned_up(&self) -> bool {
        self.cleaned_up
    }

    fn handle_highlighted(
        &mut self,
        observation: DropdownHighlightObservation<'_>,
    ) -> Vec<DropdownHighlightAction> {
        if !observation.marker_present {
            return Vec::new();
        }

        let mut actions = Vec::new();
        if self.applied_geometry.is_some()
            && let Some(geometry) = observation.geometry
        {
            self.apply_geometry(geometry, &mut actions);
        }
        actions.extend(self.handle_watch(observation.now_ms));
        actions
    }

    fn handle_settle(
        &mut self,
        observation: DropdownHighlightObservation<'_>,
    ) -> Vec<DropdownHighlightAction> {
        // The completed callback no longer owns a pending frame. A fresh
        // request below replaces this slot if the deadline remains open.
        self.frame_pending = false;
        let mut actions = Vec::new();

        if observation.marker_present
            && let Some(geometry) = observation.geometry
        {
            let should_apply = match self.applied_geometry.as_deref() {
                None => self.sampled_geometry.as_deref() == Some(geometry),
                Some(applied) => applied != geometry,
            };

            if should_apply {
                self.apply_geometry(geometry, &mut actions);
            }
            self.sampled_geometry = Some(geometry.to_owned());
        }

        if observation.now_ms < self.deadline_ms {
            self.frame_pending = true;
            actions.push(DropdownHighlightAction::ScheduleFrame);
        }

        actions
    }

    fn handle_watch(&mut self, now_ms: f64) -> Vec<DropdownHighlightAction> {
        self.deadline_ms = now_ms + SETTLE_WINDOW_MS;
        if self.frame_pending {
            return Vec::new();
        }

        self.frame_pending = true;
        vec![DropdownHighlightAction::ScheduleFrame]
    }

    fn apply_geometry(&mut self, geometry: &str, actions: &mut Vec<DropdownHighlightAction>) {
        self.applied_geometry = Some(geometry.to_owned());
        actions.push(DropdownHighlightAction::ApplyGeometry(geometry.to_owned()));
    }
}

impl Default for DropdownHighlightSettleState {
    fn default() -> Self {
        Self::new()
    }
}
