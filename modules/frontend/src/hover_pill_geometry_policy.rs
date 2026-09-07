//! Pure geometry and presentation facts for the shared frontend hover pill.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/routes/components/hover-pill.svelte`. The host
//! boundary supplies already-read client-rectangle origins and offset
//! dimensions; this module does not access a DOM, observe layout, or produce
//! CSS.
//!
//! The target and anchor client rectangles are subtracted in client
//! coordinates. Shared ancestor transforms and scroll offsets therefore
//! cancel, including when the target is in a sibling card. The containing
//! block for the absolutely positioned pill must be borderless: a client
//! rectangle includes that containing block's border, while the absolute
//! positioning origin begins inside it. This invariant belongs to the host
//! layout and is documented here without attempting to inspect or enforce it.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// The client-coordinate origin needed from one measured element.
///
/// The browser rectangle contains more values, but the hover-pill contract
/// consumes only `left` and `top`. Keeping this value to the used fields makes
/// the DOM-to-Rust boundary explicit and prevents this pure policy from
/// growing a browser representation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClientRect {
    /// Horizontal client-coordinate origin.
    pub left: f64,
    /// Vertical client-coordinate origin.
    pub top: f64,
}

impl ClientRect {
    /// Creates a client-coordinate origin while preserving fractional and
    /// negative values exactly.
    #[must_use]
    pub const fn new(left: f64, top: f64) -> Self {
        Self { left, top }
    }
}

/// Inputs needed to derive one hover-pill presentation.
///
/// `target_rect` and `anchor_rect` model the optional DOM measurements. The
/// offset dimensions are kept as `f64` because they arrive as JavaScript
/// numbers; browser `offsetWidth` and `offsetHeight` are normally integral,
/// but this policy does not add validation or alter supplied measurements.
/// `measurement_version` is copied through as a remeasurement token. It is
/// deliberately not part of the geometry arithmetic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverPillGeometryInput {
    /// The target row's client rectangle, if a row is currently hovered.
    pub target_rect: Option<ClientRect>,
    /// The pill's containing-block client rectangle, if its anchor exists.
    pub anchor_rect: Option<ClientRect>,
    /// The target row's measured `offsetWidth`.
    pub target_offset_width: f64,
    /// The target row's measured `offsetHeight`.
    pub target_offset_height: f64,
    /// Whether the presentation should animate the next placement.
    pub animated: bool,
    /// Token incremented by the owner when geometry should be reread.
    pub measurement_version: u64,
}

impl HoverPillGeometryInput {
    /// Creates one complete set of host-supplied hover-pill measurements.
    #[must_use]
    pub const fn new(
        target_rect: Option<ClientRect>,
        anchor_rect: Option<ClientRect>,
        target_offset_width: f64,
        target_offset_height: f64,
        animated: bool,
        measurement_version: u64,
    ) -> Self {
        Self {
            target_rect,
            anchor_rect,
            target_offset_width,
            target_offset_height,
            animated,
            measurement_version,
        }
    }
}

/// The target geometry projected into the anchor's coordinate system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverPillGeometry {
    /// Target top in anchor-local client coordinates.
    pub top: f64,
    /// Target left in anchor-local client coordinates.
    pub left: f64,
    /// Target `offsetWidth`.
    pub width: f64,
    /// Target `offsetHeight`.
    pub height: f64,
}

impl HoverPillGeometry {
    /// Creates projected geometry from the exact values supplied by the host.
    #[must_use]
    pub const fn new(left: f64, top: f64, width: f64, height: f64) -> Self {
        Self {
            top,
            left,
            width,
            height,
        }
    }
}

/// Geometry plus the non-geometric presentation facts exposed by the Svelte
/// component.
///
/// `active` is true exactly when `geometry` exists. `animate` remains the
/// caller's animation flag even while geometry is absent, matching the
/// component's independent `data-active` and `data-animate` attributes.
/// `measurement_version` is returned for the owner that tracks remeasurement;
/// changing it alone never changes `geometry`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverPillPresentation {
    /// Whether the pill has enough measurements to be active.
    pub active: bool,
    /// Whether the next placement should animate.
    pub animate: bool,
    /// Projected target geometry, absent until both measurements exist.
    pub geometry: Option<HoverPillGeometry>,
    /// The input token that caused this presentation to be evaluated.
    pub measurement_version: u64,
}

impl HoverPillPresentation {
    /// Returns whether the pill has active geometry.
    #[must_use]
    pub const fn is_active(self) -> bool {
        self.active
    }

    /// Returns whether the next placement should animate.
    #[must_use]
    pub const fn should_animate(self) -> bool {
        self.animate
    }
}

/// Computes geometry from optional client rectangles and target offsets.
///
/// This is the exact pure arithmetic behind the component's derived value:
/// missing target or anchor yields no geometry; otherwise the coordinates are
/// target minus anchor and dimensions come directly from target offsets. No
/// clamping, rounding, or finite-value filtering is applied, so negative and
/// fractional translations remain deterministic.
#[must_use]
pub const fn compute_hover_pill_geometry(
    target_rect: Option<ClientRect>,
    anchor_rect: Option<ClientRect>,
    target_offset_width: f64,
    target_offset_height: f64,
) -> Option<HoverPillGeometry> {
    let (Some(target), Some(anchor)) = (target_rect, anchor_rect) else {
        return None;
    };

    Some(HoverPillGeometry::new(
        target.left - anchor.left,
        target.top - anchor.top,
        target_offset_width,
        target_offset_height,
    ))
}

/// Resolves the hover-pill geometry and its independent presentation facts.
///
/// The measurement version is intentionally carried through rather than used
/// as an input to arithmetic. An owner can therefore rerun this function for
/// a new version and observe an equivalent geometry when the measurements are
/// unchanged.
#[must_use]
pub const fn hover_pill_presentation(input: HoverPillGeometryInput) -> HoverPillPresentation {
    let geometry = compute_hover_pill_geometry(
        input.target_rect,
        input.anchor_rect,
        input.target_offset_width,
        input.target_offset_height,
    );

    HoverPillPresentation {
        active: geometry.is_some(),
        animate: input.animated,
        geometry,
        measurement_version: input.measurement_version,
    }
}
