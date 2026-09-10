//! Pure shell sizing policy for the native frontend.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/root/shell-layout.ts`. It resolves the selected
//! prose column and inspector width, then answers whether the inspector can
//! coexist with the transcript's proximity rail. Rendering and viewport
//! measurement remain outside this module.
//!
//! Valid finite, non-negative viewport widths use the same arithmetic and
//! inclusive boundary as the TypeScript policy. A negative or non-finite
//! viewport is malformed geometry: the inspector width falls back to its
//! minimum, while the fit predicate conservatively returns `false`. This
//! keeps malformed measurements from producing `NaN` or infinity in a layout
//! decision.

/// The tight reading-column width in pixels.
pub const TIGHT_PROSE_WIDTH_PIXELS: f64 = 672.0;

/// The balanced reading-column width in pixels.
pub const BALANCED_PROSE_WIDTH_PIXELS: f64 = 768.0;

/// The loose reading-column width in pixels.
pub const LOOSE_PROSE_WIDTH_PIXELS: f64 = 896.0;

/// The minimum transcript margin reserved for the proximity rail in pixels.
pub const THREAD_RAIL_BAND_PIXELS: f64 = 144.0;

/// The gap between the rail band and the prose column in pixels.
pub const THREAD_RAIL_GAP_PIXELS: f64 = 16.0;

/// The gutter after the prose column in pixels.
pub const PROSE_GUTTER_PIXELS: f64 = 32.0;

/// Width spent by shell chrome before the transcript region in pixels.
pub const SHELL_CHROME_PIXELS: f64 = 80.0;

/// The lower bound of the inspector width clamp in pixels.
pub const INSPECTOR_MIN_WIDTH_PIXELS: f64 = 256.0;

/// The upper bound of the inspector width clamp in pixels.
pub const INSPECTOR_MAX_WIDTH_PIXELS: f64 = 350.0;

/// The viewport fraction used for the unclamped inspector width.
pub const INSPECTOR_VIEWPORT_RATIO: f64 = 0.25;

/// The supported transcript reading-column widths.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProseWidth {
    /// A 672px reading column.
    Tight,
    /// A 768px reading column.
    Balanced,
    /// An 896px reading column.
    Loose,
}

impl ProseWidth {
    /// Every supported width in the TypeScript policy's canonical order.
    pub const ALL: [Self; 3] = [Self::Tight, Self::Balanced, Self::Loose];

    /// Returns the canonical persisted/display name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tight => "tight",
            Self::Balanced => "balanced",
            Self::Loose => "loose",
        }
    }

    /// Returns the resolved reading-column width in pixels.
    #[must_use]
    pub const fn pixels(self) -> f64 {
        match self {
            Self::Tight => TIGHT_PROSE_WIDTH_PIXELS,
            Self::Balanced => BALANCED_PROSE_WIDTH_PIXELS,
            Self::Loose => LOOSE_PROSE_WIDTH_PIXELS,
        }
    }
}

/// Resolves a selected prose width to pixels.
#[must_use]
pub const fn prose_column_pixels(prose_width: ProseWidth) -> f64 {
    prose_width.pixels()
}

/// Resolves the inspector column width for a viewport.
///
/// For valid geometry this is exactly `clamp(viewport * 0.25, 256, 350)`.
/// Negative and non-finite viewport widths are treated as a zero-width
/// viewport before applying the clamp, so the result is the finite minimum
/// width rather than an invalid layout dimension.
#[must_use]
pub fn inspector_column_pixels(viewport_width: f64) -> f64 {
    let viewport_width = sanitize_viewport_width(viewport_width);
    clamp_inspector_width(viewport_width * INSPECTOR_VIEWPORT_RATIO)
}

/// Resolves the inspector column width from actual content width.
///
/// Same clamp as [`inspector_column_pixels`], but measured from the width
/// left after the desktop sidebar instead of the total window: the legacy
/// formula's total-window fraction overstates the room beside a 218 px
/// desktop rail and would squeeze the conversation column.
#[must_use]
pub fn desktop_inspector_column_pixels(content_width: f64) -> f64 {
    clamp_inspector_width(sanitize_viewport_width(content_width) * INSPECTOR_VIEWPORT_RATIO)
}

fn clamp_inspector_width(raw_width: f64) -> f64 {
    raw_width.clamp(INSPECTOR_MIN_WIDTH_PIXELS, INSPECTOR_MAX_WIDTH_PIXELS)
}

/// Returns whether the inspector fits in actual desktop content width.
///
/// Same band arithmetic as [`thread_inspector_fits_beside_rail`], but measured
/// from the width left after the desktop sidebar (`content_width =
/// window − sidebar`, both in logical pixels) instead of the total window:
/// the legacy total-window form assumes a 56 px rail, while the desktop rail
/// is 218 px expanded (58 px collapsed), so the legacy form would seat an
/// inspector beside a conversation it then squeezes. At 1280 px total with an
/// expanded rail the content is 1062 px (band −19.5 → hidden); at 1400 px
/// total it is 1182 px (band 70.5 → still hidden, no squeeze); the balanced
/// column returns once content reaches 1280 px (window ≥ 1498 px expanded).
/// Invalid geometry never fits.
#[must_use]
pub fn desktop_thread_inspector_fits(content_width: f64, prose_width: ProseWidth) -> bool {
    if !is_valid_viewport_width(content_width) {
        return false;
    }

    let band_width = content_width
        - desktop_inspector_column_pixels(content_width)
        - prose_column_pixels(prose_width)
        - PROSE_GUTTER_PIXELS
        - THREAD_RAIL_GAP_PIXELS;

    band_width >= THREAD_RAIL_BAND_PIXELS
}

/// Returns whether the inspector fits beside the transcript proximity rail.
///
/// The comparison is inclusive and mirrors the TypeScript calculation:
/// `viewport - chrome - inspector - prose - gutter - gap >= rail band`.
/// Invalid geometry is rejected before arithmetic; a negative or non-finite
/// viewport therefore never makes the inspector appear to fit.
#[must_use]
pub fn thread_inspector_fits_beside_rail(viewport_width: f64, prose_width: ProseWidth) -> bool {
    if !is_valid_viewport_width(viewport_width) {
        return false;
    }

    let transcript_width =
        viewport_width - SHELL_CHROME_PIXELS - inspector_column_pixels(viewport_width);
    let band_width = transcript_width
        - prose_column_pixels(prose_width)
        - PROSE_GUTTER_PIXELS
        - THREAD_RAIL_GAP_PIXELS;

    band_width >= THREAD_RAIL_BAND_PIXELS
}

fn is_valid_viewport_width(viewport_width: f64) -> bool {
    viewport_width.is_finite() && viewport_width >= 0.0
}

fn sanitize_viewport_width(viewport_width: f64) -> f64 {
    if is_valid_viewport_width(viewport_width) {
        viewport_width
    } else {
        0.0
    }
}
