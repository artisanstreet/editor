//! Pure placement policy for the active-thread light.
//!
//! This is the dependency-free state boundary of
//! `routes/components/active-thread-light.svelte`. The host owns DOM lookup,
//! browser yielding, layout-key reactivity, resize events, and rendering. It
//! supplies a complete geometry observation here, or `None` when the surface,
//! active row, or measurement is unavailable.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// A complete geometry observation for one active-thread row.
///
/// The values are deliberately unvalidated. Browser geometry can be negative
/// or fractional, and the policy preserves every supplied value, including a
/// negative row height or scroll offset. A host that cannot produce all four
/// values should pass `None` to [`ActiveThreadLightPolicy::measure`] instead.
#[must_use = "supply the measurement to ActiveThreadLightPolicy::measure"]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveThreadLightMeasurement {
    /// The active row's viewport-relative top coordinate.
    pub row_top: f64,
    /// The active row's measured height.
    pub row_height: f64,
    /// The surface's viewport-relative top coordinate.
    pub surface_top: f64,
    /// The surface's current scroll offset.
    pub scroll_top: f64,
}

impl ActiveThreadLightMeasurement {
    /// Creates a complete row-and-surface geometry observation.
    pub const fn new(row_top: f64, row_height: f64, surface_top: f64, scroll_top: f64) -> Self {
        Self {
            row_top,
            row_height,
            surface_top,
            scroll_top,
        }
    }
}

/// Stateful, platform-independent policy for the active-thread light.
///
/// A valid measurement places the light in surface scroll coordinates and
/// makes it visible. Animation is requested only when the light was already
/// visible and the optional active-thread identity differs from the last
/// successfully measured identity. Missing observations hide the light but
/// intentionally preserve all other state, matching the browser component's
/// early return.
#[must_use = "apply measurements or inspect the active-thread light state"]
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveThreadLightPolicy {
    lit_thread_id: Option<String>,
    visible: bool,
    animated: bool,
    top: f64,
    height: f64,
    resize_revision: u64,
}

impl ActiveThreadLightPolicy {
    /// Creates a hidden light with zero geometry and revision zero.
    pub const fn new() -> Self {
        Self {
            lit_thread_id: None,
            visible: false,
            animated: false,
            top: 0.0,
            height: 0.0,
            resize_revision: 0,
        }
    }

    /// Returns the last optional thread identity accepted by a valid
    /// measurement.
    #[must_use]
    pub fn lit_thread_id(&self) -> Option<&str> {
        self.lit_thread_id.as_deref()
    }

    /// Returns whether the light is currently visible.
    #[must_use]
    pub const fn visible(&self) -> bool {
        self.visible
    }

    /// Returns whether the current placement should animate.
    #[must_use]
    pub const fn animated(&self) -> bool {
        self.animated
    }

    /// Returns the current surface-relative top position in pixels.
    #[must_use]
    pub const fn top(&self) -> f64 {
        self.top
    }

    /// Returns the current measured row height in pixels.
    #[must_use]
    pub const fn height(&self) -> f64 {
        self.height
    }

    /// Returns the monotonic token the host can use to trigger a
    /// remeasurement after a resize.
    #[must_use]
    pub const fn resize_revision(&self) -> u64 {
        self.resize_revision
    }

    /// Applies one host-supplied geometry observation.
    ///
    /// `None` represents a missing surface, row, or measurement. In that
    /// case only visibility is set to `false`; the last identity, animation
    /// flag, geometry, and resize revision remain untouched. For a valid
    /// observation, the top position is exactly
    /// `row_top - surface_top + scroll_top`, the height is copied unchanged,
    /// and the optional identity is retained unchanged, including empty and
    /// Unicode identifiers.
    pub fn measure(
        &mut self,
        active_thread_id: Option<String>,
        measurement: Option<ActiveThreadLightMeasurement>,
    ) {
        let Some(measurement) = measurement else {
            self.visible = false;
            return;
        };

        let thread_changed = self.lit_thread_id.as_ref() != active_thread_id.as_ref();
        self.animated = self.visible && thread_changed;
        self.lit_thread_id = active_thread_id;
        self.top = measurement.row_top - measurement.surface_top + measurement.scroll_top;
        self.height = measurement.row_height;
        self.visible = true;
    }

    /// Hides the light after a missing surface, row, or measurement.
    ///
    /// This deliberately changes no state other than visibility. In
    /// particular, it does not clear the last lit identity or reset the
    /// animation flag.
    pub const fn hide(&mut self) {
        self.visible = false;
    }

    /// Advances the resize revision without changing placement state.
    ///
    /// The host owns the actual resize event and should use the resulting
    /// revision as one of its measurement dependencies. Saturation keeps the
    /// revision monotonic for an abnormally long-lived policy.
    pub fn resize(&mut self) {
        self.resize_revision = self.resize_revision.saturating_add(1);
    }
}

impl Default for ActiveThreadLightPolicy {
    fn default() -> Self {
        Self::new()
    }
}
