//! Dependency-free viewport breakpoint policy for the native frontend.
//!
//! This is the deterministic value boundary of the legacy
//! `modules/frontend/src/lib/hooks/is-mobile.svelte.ts` helper. The legacy
//! helper emits an inclusive `max-width` query one pixel below its exclusive
//! breakpoint. This module preserves that relationship without modeling
//! Svelte reactivity, browser globals, DOM listeners, layout, or device
//! detection.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

use std::fmt;
use std::num::NonZeroU32;

/// The legacy default mobile breakpoint, in CSS pixels.
pub const DEFAULT_MOBILE_BREAKPOINT_PIXELS: u32 = 768;

/// A strictly positive viewport breakpoint measured in CSS pixels.
///
/// The nonzero representation makes it impossible for a valid breakpoint to
/// underflow while deriving its inclusive mobile maximum. Caller-supplied
/// positive values are retained exactly; this type does not clamp them to a
/// device-specific range.
#[must_use = "use the validated breakpoint to build a viewport policy"]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MobileBreakpoint(NonZeroU32);

impl MobileBreakpoint {
    /// Validates and retains a breakpoint in CSS pixels.
    ///
    /// # Errors
    ///
    /// Returns [`MobileBreakpointError::Zero`] when `pixels` is zero. Every
    /// positive `u32`, including [`u32::MAX`], is accepted unchanged.
    #[must_use = "handle an invalid zero breakpoint or use the validated value"]
    pub const fn new(pixels: u32) -> Result<Self, MobileBreakpointError> {
        match NonZeroU32::new(pixels) {
            Some(pixels) => Ok(Self(pixels)),
            None => Err(MobileBreakpointError::Zero),
        }
    }

    /// Returns the exact breakpoint in CSS pixels.
    #[must_use]
    pub const fn pixels(self) -> u32 {
        self.0.get()
    }

    /// Returns the inclusive largest mobile width for this breakpoint.
    ///
    /// A valid breakpoint is strictly positive, so this is exactly one less
    /// than [`Self::pixels`] without an underflow path.
    #[must_use]
    pub const fn inclusive_mobile_max(self) -> u32 {
        self.pixels() - 1
    }
}

impl Default for MobileBreakpoint {
    fn default() -> Self {
        Self::new(DEFAULT_MOBILE_BREAKPOINT_PIXELS)
            .expect("the positive default breakpoint must validate")
    }
}

/// Why a viewport breakpoint could not be validated.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MobileBreakpointError {
    /// Zero cannot produce the predecessor required by an inclusive query.
    Zero,
}

impl fmt::Display for MobileBreakpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => formatter.write_str("mobile breakpoint must be greater than zero"),
        }
    }
}

impl std::error::Error for MobileBreakpointError {}

/// Pure policy for classifying viewport widths and emitting the legacy query.
///
/// A width is mobile exactly when `width < breakpoint`. The corresponding
/// browser query uses the mathematically equivalent inclusive condition
/// `width <= breakpoint - 1`, rendered as `(max-width: Npx)`.
#[must_use = "use the policy to classify a width or emit its media query"]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MobileBreakpointPolicy {
    breakpoint: MobileBreakpoint,
}

impl MobileBreakpointPolicy {
    /// Validates and creates a policy for a caller-supplied breakpoint.
    ///
    /// Positive values are preserved exactly, with no arbitrary clamping.
    ///
    /// # Errors
    ///
    /// Returns [`MobileBreakpointError::Zero`] when `breakpoint_pixels` is
    /// zero.
    #[must_use = "handle an invalid zero breakpoint or use the policy"]
    pub const fn new(breakpoint_pixels: u32) -> Result<Self, MobileBreakpointError> {
        match MobileBreakpoint::new(breakpoint_pixels) {
            Ok(breakpoint) => Ok(Self { breakpoint }),
            Err(error) => Err(error),
        }
    }

    /// Creates a policy from an already validated breakpoint.
    #[must_use = "use the policy built from the validated breakpoint"]
    pub const fn from_breakpoint(breakpoint: MobileBreakpoint) -> Self {
        Self { breakpoint }
    }

    /// Returns the validated breakpoint used by this policy.
    #[must_use = "use the returned breakpoint value"]
    pub const fn breakpoint(self) -> MobileBreakpoint {
        self.breakpoint
    }

    /// Returns the exact exclusive breakpoint in CSS pixels.
    #[must_use]
    pub const fn breakpoint_pixels(self) -> u32 {
        self.breakpoint.pixels()
    }

    /// Returns the exact inclusive maximum mobile width in CSS pixels.
    #[must_use]
    pub const fn mobile_max_width(self) -> u32 {
        self.breakpoint.inclusive_mobile_max()
    }

    /// Returns the exact browser media-query text for this policy.
    #[must_use]
    pub fn media_query(self) -> String {
        format!("(max-width: {}px)", self.mobile_max_width())
    }

    /// Returns whether `width` belongs to the mobile range.
    ///
    /// This is intentionally the exclusive comparison against the validated
    /// breakpoint. It agrees with the emitted inclusive media query because
    /// every valid breakpoint has the predecessor returned by
    /// [`Self::mobile_max_width`].
    #[must_use]
    pub const fn is_mobile(self, width: u32) -> bool {
        width < self.breakpoint_pixels()
    }
}

impl Default for MobileBreakpointPolicy {
    fn default() -> Self {
        Self::from_breakpoint(MobileBreakpoint::default())
    }
}
