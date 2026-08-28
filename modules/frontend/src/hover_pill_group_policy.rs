//! Dependency-free state policy for a shared hover pill group.
//!
//! This is the native counterpart of
//! `modules/frontend/src/routes/components/hover-pill-group.svelte`. The host
//! supplies a target identity and already-observed containment, hover, and
//! focus facts; this module owns only the active target, animation decision,
//! and remeasurement version. It does not access a DOM, subscribe to an
//! observer, receive browser events, or own cleanup.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// The host-observed facts for the currently active target during
/// reconciliation.
///
/// `None` passed to [`HoverPillGroupPolicy::reconcile`] represents a target
/// that is missing. A present target is retained only when it is contained by
/// the group and either hovered or focused within. The booleans are facts
/// supplied by a host adapter; this policy does not attempt to derive them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HoverPillTargetObservation {
    /// Whether the target is still contained by the hover group's surface.
    pub contained: bool,
    /// Whether the target currently matches the host's hover state.
    pub hovered: bool,
    /// Whether the target currently contains the focused element.
    pub focus_within: bool,
}

impl HoverPillTargetObservation {
    /// Creates one host observation without normalizing any supplied fact.
    #[must_use]
    pub const fn new(contained: bool, hovered: bool, focus_within: bool) -> Self {
        Self {
            contained,
            hovered,
            focus_within,
        }
    }
}

/// Stateful, platform-independent policy for a shared hover pill group.
///
/// The default identity type is [`String`], but callers may choose any
/// identity that is meaningful at their host boundary. The identity is opaque
/// to the policy: only whether one exists matters when deciding animation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverPillGroupPolicy<T = String> {
    active_target: Option<T>,
    animated: bool,
    geometry_version: u64,
}

impl<T> HoverPillGroupPolicy<T> {
    /// Creates an empty group with animation disabled and version zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            active_target: None,
            animated: false,
            geometry_version: 0,
        }
    }

    /// Returns the active target identity, if one is retained.
    #[must_use]
    pub const fn active_target(&self) -> Option<&T> {
        self.active_target.as_ref()
    }

    /// Returns the active target identity under the shared `PillHover` name.
    #[must_use]
    pub const fn target(&self) -> Option<&T> {
        self.active_target()
    }

    /// Returns whether the next geometry placement should animate.
    #[must_use]
    pub const fn animated(&self) -> bool {
        self.animated
    }

    /// Returns the monotonic token used to request a geometry remeasurement.
    #[must_use]
    pub const fn geometry_version(&self) -> u64 {
        self.geometry_version
    }

    /// Returns the version under the shared `PillHover` name.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.geometry_version()
    }

    /// Applies one host-reported target.
    ///
    /// An absent target is invalid. A present target with `in_surface ==
    /// false` is out of the group's surface. Either case clears the active
    /// target and animation without changing the geometry version. A valid,
    /// in-surface target sets animation exactly when a target was already
    /// active and always advances the geometry version once.
    pub fn apply_target(&mut self, target: Option<T>, in_surface: bool) {
        if !in_surface || target.is_none() {
            self.clear_hover();
            return;
        }

        self.animated = self.active_target.is_some();
        self.active_target = target;
        self.bump_geometry_version();
    }

    /// Clears the active target and animation without creating geometry work.
    ///
    /// The geometry version intentionally remains unchanged. A later valid
    /// target still receives a fresh increment from the version it had before
    /// this clear.
    pub fn clear_hover(&mut self) {
        self.active_target = None;
        self.animated = false;
    }

    /// Handles pointer departure from the group.
    pub fn pointer_departure(&mut self) {
        self.clear_hover();
    }

    /// Handles focus departure from the group.
    ///
    /// A focus move whose related target remains within the group retains the
    /// current hover state. Missing, invalid, or outside related targets are
    /// represented by `related_target_in_group == false` and clear it.
    pub fn focus_departure(&mut self, related_target_in_group: bool) {
        if !related_target_in_group {
            self.clear_hover();
        }
    }

    /// Reconciles the active target after any host layout or tree change.
    ///
    /// The caller should invoke this same pure method after a model mutation,
    /// resize, or captured scroll. No source-specific event or observer is
    /// represented here. `None` means the active target is missing. A missing,
    /// out-of-surface, or neither-hovered-nor-focused target clears state. A
    /// retained target advances the geometry version exactly once so the host
    /// can remeasure it.
    pub fn reconcile(&mut self, observation: Option<HoverPillTargetObservation>) {
        if self.active_target.is_none() {
            return;
        }

        let Some(observation) = observation else {
            self.clear_hover();
            return;
        };

        if !observation.contained || (!observation.hovered && !observation.focus_within) {
            self.clear_hover();
            return;
        }

        self.bump_geometry_version();
    }

    fn bump_geometry_version(&mut self) {
        // Saturation preserves the version's monotonic invariant even if a
        // host keeps a single policy alive for more than u64::MAX updates.
        self.geometry_version = self.geometry_version.saturating_add(1);
    }
}

impl<T> Default for HoverPillGroupPolicy<T> {
    fn default() -> Self {
        Self::new()
    }
}
