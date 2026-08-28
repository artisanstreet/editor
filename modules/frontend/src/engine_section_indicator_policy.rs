//! Dependency-free geometry and state policy for the engine-section indicator.
//!
//! This is the deterministic counterpart of the indicator state in
//! `routes/components/model-selector/engine-section.svelte`. The host owns
//! the browser task, DOM lookup, tab and surface rectangles, resize event,
//! rendering, and animation CSS. This module only consumes an optional
//! measurement and retains the resulting indicator state.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// One successful browser geometry measurement for an engine tab.
///
/// The host should construct this value only after it has both a surface and
/// a tab rectangle. A missing surface, tab, or browser measurement is
/// represented by `None` at [`EngineSectionIndicatorPolicy::measure`] and is
/// therefore a strict no-op.
#[must_use = "pass the measurement to the indicator policy"]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineSectionIndicatorMeasurement {
    /// The surface's viewport-relative left coordinate.
    pub surface_left: f64,
    /// The tab's viewport-relative left coordinate.
    pub tab_left: f64,
    /// The tab's measured width.
    pub tab_width: f64,
}

impl EngineSectionIndicatorMeasurement {
    /// Creates a measurement without normalizing or clamping its geometry.
    ///
    /// Negative, zero, and fractional values are retained exactly so the
    /// native adapter can preserve the browser's layout result.
    #[must_use = "use the constructed geometry measurement"]
    pub const fn new(surface_left: f64, tab_left: f64, tab_width: f64) -> Self {
        Self {
            surface_left,
            tab_left,
            tab_width,
        }
    }
}

/// Deterministic state owner for one engine-section indicator.
///
/// A policy starts with no lit engine and a hidden, non-animated indicator.
/// The first valid measurement makes it visible without animation. Later
/// measurements animate only when the indicator was already visible and the
/// measured engine differs from the last engine successfully measured.
/// Geometry remains untouched by missing measurements and by resize events;
/// resize only advances [`Self::resize_revision`].
#[must_use = "use the retained indicator state"]
#[derive(Clone, Debug, PartialEq)]
pub struct EngineSectionIndicatorPolicy {
    lit_engine: Option<String>,
    indicator_visible: bool,
    indicator_animated: bool,
    indicator_left: f64,
    indicator_width: f64,
    resize_revision: u64,
}

impl EngineSectionIndicatorPolicy {
    /// Creates an empty indicator state with zero geometry and revision.
    #[must_use = "use the new indicator policy"]
    pub const fn new() -> Self {
        Self {
            lit_engine: None,
            indicator_visible: false,
            indicator_animated: false,
            indicator_left: 0.0,
            indicator_width: 0.0,
            resize_revision: 0,
        }
    }

    /// Returns the engine associated with the last successful measurement.
    #[must_use]
    pub fn lit_engine(&self) -> Option<&str> {
        self.lit_engine.as_deref()
    }

    /// Returns whether a successful measurement has made the indicator
    /// visible.
    #[must_use]
    pub const fn indicator_visible(&self) -> bool {
        self.indicator_visible
    }

    /// Returns whether the most recent successful measurement should animate.
    #[must_use]
    pub const fn indicator_animated(&self) -> bool {
        self.indicator_animated
    }

    /// Returns the indicator's surface-relative left offset.
    #[must_use]
    pub const fn indicator_left(&self) -> f64 {
        self.indicator_left
    }

    /// Returns the measured width of the lit tab.
    #[must_use]
    pub const fn indicator_width(&self) -> f64 {
        self.indicator_width
    }

    /// Returns the monotonic token used by a host to request remeasurement.
    #[must_use]
    pub const fn resize_revision(&self) -> u64 {
        self.resize_revision
    }

    /// Applies one optional successful geometry measurement.
    ///
    /// `None` represents any missing host prerequisite: the surface, the tab,
    /// or the browser measurement itself. It leaves every state field
    /// unchanged and returns `false`. A present measurement sets the left
    /// offset to `tab_left - surface_left`, copies the tab width, makes the
    /// indicator visible, and records `engine`. Animation is enabled exactly
    /// when the indicator was already visible and the engine differs from the
    /// previous lit engine. Geometry is not validated or clamped.
    pub fn measure(
        &mut self,
        engine: impl Into<String>,
        measurement: Option<EngineSectionIndicatorMeasurement>,
    ) -> bool {
        let Some(measurement) = measurement else {
            return false;
        };

        let engine = engine.into();
        self.indicator_animated =
            self.indicator_visible && self.lit_engine.as_deref() != Some(engine.as_str());
        self.indicator_left = measurement.tab_left - measurement.surface_left;
        self.indicator_width = measurement.tab_width;
        self.indicator_visible = true;
        self.lit_engine = Some(engine);
        true
    }

    /// Advances the resize revision without changing any other state.
    ///
    /// Saturation keeps the token stable rather than wrapping if one policy
    /// instance receives more than `u64::MAX` resize events.
    pub fn resize(&mut self) {
        self.resize_revision = self.resize_revision.saturating_add(1);
    }
}

impl Default for EngineSectionIndicatorPolicy {
    fn default() -> Self {
        Self::new()
    }
}
