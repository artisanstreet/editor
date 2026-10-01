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
//!
//! The desktop shell does not use the TypeScript viewport clamp for its
//! columns: the chat keeps [`desktop_chat_min_pixels`] and the two ruled
//! columns (left sidebar and thread inspector) share what remains, each up to
//! the caller's full column width. [`desktop_sidebar_pixels`] and
//! [`desktop_inspector_pixels`] take that width from the caller, which keeps
//! this module free of shell dependencies (the integration suite includes it
//! by path) while the shell's sidebar constant stays the single source of the
//! width.

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
    (viewport_width * INSPECTOR_VIEWPORT_RATIO)
        .clamp(INSPECTOR_MIN_WIDTH_PIXELS, INSPECTOR_MAX_WIDTH_PIXELS)
}

/// Clearance kept on each side of the centred desktop reading column: the
/// navigator rail's 8 px inset plus its 40 px width, so the rail never
/// overlaps prose.
pub const DESKTOP_RAIL_CLEARANCE_PIXELS: f64 = 48.0;

/// The narrowest either ruled desktop column (left sidebar or thread
/// inspector) shrinks to before the chat gives up any width.
pub const DESKTOP_COLUMN_MIN_PIXELS: f64 = 240.0;

/// The chat's minimum desktop width: the reading column plus the rail
/// clearance on both sides. The chat never shrinks below it; the ruled
/// columns shrink instead.
#[must_use]
pub const fn desktop_chat_min_pixels(prose_width: ProseWidth) -> f64 {
    prose_width.pixels() + 2.0 * DESKTOP_RAIL_CLEARANCE_PIXELS
}

/// The narrowest the chat itself gets: below the comfortable
/// [`desktop_chat_min_pixels`] both ruled columns are gone and the reading
/// column narrows inside the window, down to this floor.
pub const DESKTOP_CHAT_FLOOR_PIXELS: f64 = 480.0;

/// The narrowest desktop window: the chat alone at its floor. Navigation
/// columns are hidden there; the conversation stays usable.
#[must_use]
pub const fn desktop_min_window_pixels() -> f64 {
    DESKTOP_CHAT_FLOOR_PIXELS
}

/// The window width that seats the chat and both ruled columns at their
/// full `column_max` width.
#[must_use]
pub const fn desktop_full_window_pixels(prose_width: ProseWidth, column_max: f64) -> f64 {
    desktop_chat_min_pixels(prose_width) + 2.0 * column_max
}

/// Resolves the left sidebar width for a desktop window, or `None` while
/// it is hidden.
///
/// The chat keeps its comfortable minimum first; the sidebar takes what
/// remains up to `column_max`, shrinks no further than
/// [`DESKTOP_COLUMN_MIN_PIXELS`], and hides below that so the window can
/// narrow to the chat alone. The inspector only appears once the sidebar is
/// at full width, so the sidebar never jumps as the window grows. The
/// comparison is inclusive. Invalid geometry resolves to `column_max`.
#[must_use]
pub fn desktop_sidebar_pixels(
    window_width: f64,
    prose_width: ProseWidth,
    column_max: f64,
) -> Option<f64> {
    if !is_valid_viewport_width(window_width) || !is_valid_viewport_width(column_max) {
        return Some(column_max);
    }
    let room = window_width - desktop_chat_min_pixels(prose_width);
    (room >= DESKTOP_COLUMN_MIN_PIXELS.min(column_max)).then(|| room.min(column_max))
}

/// Resolves the thread inspector width for the desktop content width (the
/// window minus the left sidebar), or `None` while it does not fit.
///
/// The inspector takes what the minimum chat leaves, up to `column_max`,
/// and hides below [`DESKTOP_COLUMN_MIN_PIXELS`] rather than squeezing the
/// chat. The comparison is inclusive. Invalid geometry never fits.
#[must_use]
pub fn desktop_inspector_pixels(
    content_width: f64,
    prose_width: ProseWidth,
    column_max: f64,
) -> Option<f64> {
    if !is_valid_viewport_width(content_width) || !is_valid_viewport_width(column_max) {
        return None;
    }
    let room = content_width - desktop_chat_min_pixels(prose_width);
    (room >= DESKTOP_COLUMN_MIN_PIXELS).then(|| room.min(column_max))
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
