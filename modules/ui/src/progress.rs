//! Noninteractive determinate progress bar for the reached context-usage
//! reading.
//!
//! Ported from the audited legacy wrapper
//! (`modules/frontend/src/lib/components/ui/progress/progress.svelte`;
//! INVENTORY §2 row 19) at the single configuration the product reaches: the
//! bar under the selected first-workflow usage details
//! (`routes/components/context-usage-details.svelte:56`), which passes a
//! percentage clamped to `0..=100` with the fixed maximum `100`. The legacy
//! track recipe `bg-primary/20 relative h-2 w-full overflow-hidden
//! rounded-full` maps onto one full-width, fixed 8 px (`h-2`) pill whose
//! background is this mode's `--primary` at exactly 20% alpha. The legacy
//! indicator (`bg-primary h-full w-full flex-1 transition-all`, shifted left
//! by `translateX(-(100 - pct)%)`) leaves exactly `pct%` of a full-width bar
//! visible from the left edge, so it maps onto a rectangular clip of that
//! same share of the track containing one pill-shaped copy of the whole
//! track (see [`progress_indicator`]). Both faces are resolved from one
//! caller-supplied `--primary` paint, so they cannot drift apart between
//! modes.
//!
//! The reached `value`/`max` call becomes [`ProgressFraction`] at the call
//! boundary: the product already clamps its reading
//! (`Math.min(100, Math.max(0, percent))`) and pins `max = 100`
//! (`context-usage-details.svelte:34,56`), so callers map that fact to
//! [`ProgressFraction::new`] over the same number divided by 100. Bits UI's
//! open-ended `value`/`max` pair therefore has no native counterpart: no
//! reached call site uses another maximum, and inventing invalid-max
//! semantics here would add unaudited behavior. The pinned Bits state class
//! also publishes `role="progressbar"`, `aria-valuemin/max/now`,
//! `data-value`, `data-state`, and an indeterminate mode when `value` is
//! `null` (`<bits>/dist/bits/progress/progress.svelte.js:17–35`);
//! deliberately none of that is ported because no reached call site is
//! indeterminate and pinned GPUI 0.2.2 exposes no platform accessibility
//! tree that could carry those properties truthfully.
//!
//! The element is fully static: no focusable, pointer, or keyboard surface;
//! no product state; no label, variant, or indeterminate rendering. The
//! legacy indicator's `transition-all` is intentionally not ported as a
//! stateful animation subsystem: a GPUI element repaints whenever its view
//! state changes, so an updated [`ProgressFraction`] renders immediately on
//! the next frame with no transition machinery of its own. Later frontend/
//! motion ownership may animate reached-value changes through the shared
//! motion policy once product state and lifecycle work is approved.
//!
//! The legacy root's `relative` positioning has no GPUI counterpart to
//! preserve: GPUI children always lay out inside their parent's bounds and
//! nothing offsets the fill, so the recipe omits positioning entirely.
//!
//! ## Rounded-silhouette compensation (pinned GPUI 0.2.2)
//!
//! - **Child clipping is rectangular.** `overflow_hidden` installs a
//!   rectangular [`gpui::ContentMask`] built purely from the element bounds
//!   (the engine behavior documented for the card and badge primitives), so
//!   a bare full-height rectangle of the fill share would overpaint the four
//!   transparent corner caps of the track pill on every nonzero fill.
//!   [`progress_indicator`] therefore reproduces the legacy
//!   `translateX(-(100 - pct)%)` geometry with a bounded, exact layout
//!   compensation instead of inventing colors or radii: a rectangular clip
//!   as wide as the share holds one pill-shaped copy of the entire track
//!   whose width is the exact reciprocal share. The copy starts flush with
//!   the track, so the visible result is the true rounded silhouette up to
//!   the straight vertical cut the legacy transform produced. Bounds tests
//!   pin the resulting layout; pixel paint is not claimable from them.

use gpui::{Div, Hsla, ParentElement, Pixels, Styled, div, px, relative};

use crate::theme::ArtisanTheme;

/// Legacy `bg-primary/20`: the reached track alpha.
const TRACK_ALPHA: f32 = 0.2;

/// Legacy `h-2`: two 4 px spacing steps.
const TRACK_HEIGHT_PX: f32 = 8.0;

/// Legacy `rounded-full`, pinned by gpui's Tailwind-compatible token as
/// 9999 px (rendering caps at half the box height).
const PILL_RADIUS_PX: f32 = 9999.0;

/// Normalized determinate fill share, saturated into `0.0..=1.0`.
///
/// The narrowest honest stand-in for Bits UI's unbounded `value`/`max` pair
/// at the one reached call site, which always divides a `0..=100` reading by
/// the maximum `100`. Values outside the domain saturate instead of
/// panicking or producing an invalid width: below-range and non-finite
/// inputs (including both infinities) become empty, and finite above-range
/// inputs become full.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProgressFraction(f32);

impl ProgressFraction {
    /// The completely empty bar.
    pub const EMPTY: Self = Self(0.0);

    /// The completely filled bar.
    pub const FULL: Self = Self(1.0);

    /// Saturates any input into the valid `0.0..=1.0` domain.
    ///
    /// Non-finite inputs (including positive infinity) become empty; finite
    /// inputs clamp.
    #[must_use]
    pub fn new(value: f32) -> Self {
        if !value.is_finite() {
            return Self::EMPTY;
        }
        Self(value.clamp(0.0, 1.0))
    }

    /// The stored normalized share.
    #[must_use]
    pub const fn value(self) -> f32 {
        self.0
    }
}

/// Paint and geometry values resolved for one determinate progress bar.
///
/// This record is public so behavioral tests and the future component gallery
/// can compare exact native semantics without reaching into GPUI internals,
/// mirroring [`crate::button::ButtonStyle`], [`crate::badge::BadgeStyle`],
/// and [`crate::card::CardStyle`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgressStyle {
    /// Fixed track height: the legacy `h-2`, 8 px.
    pub track_height: Pixels,
    /// Pill corner token applied to the track and to the compensated fill
    /// copy: gpui's `rounded_full`, which pins as 9999 px and renders
    /// capped at half the box height.
    pub corner_radius: Pixels,
    /// Track background: this mode's `--primary` at exactly 20% alpha (the
    /// legacy `bg-primary/20`).
    pub track_color: Hsla,
    /// Fill background: this mode's opaque `--primary` (the legacy
    /// `bg-primary`).
    pub fill_color: Hsla,
}

impl ProgressStyle {
    /// Resolves the exact reached recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        let primary = theme.colors.primary.to_paint();
        Self {
            track_height: px(TRACK_HEIGHT_PX),
            corner_radius: px(PILL_RADIUS_PX),
            track_color: Hsla {
                alpha: TRACK_ALPHA,
                ..primary
            },
            fill_color: primary,
        }
    }
}

/// Returns the visible determinate fill as a plain GPUI [`Div`].
///
/// The single piece of the reached recipe GPUI cannot expose through the
/// composite [`progress`] element: public so behavioral tests and the future
/// component gallery compose the identical element, exactly like
/// [`crate::card::compact_card_content`].
///
/// Because pinned GPUI 0.2.2 clips children to straight-edged rectangles,
/// the legacy translated-full-bar geometry is reproduced with an exact
/// bounded compensation rather than a bare rectangle: the returned element
/// is an `overflow_hidden` rectangular clip as wide as [`ProgressFraction`]
/// states, and for nonzero shares it contains one pill-shaped copy of the
/// whole track (`h_full`, the recipe corner radius, the opaque `--primary`)
/// whose width is the exact reciprocal share `relative(1.0 / share)` and
/// which refuses to shrink. The copy always starts flush with the track, so
/// the clip reveals precisely the leading share of the true pill silhouette
/// and cuts the remainder at the straight vertical boundary the legacy
/// `translateX(-(100 - pct)%)` produced. An empty share skips the child
/// entirely rather than dividing by zero, and a sub-pixel share whose
/// reciprocal is not finite paints the unclipped sliver directly, which is
/// visually identical at that scale. Further [`gpui::Styled`] refinements
/// chain onto the returned element and later values win.
#[must_use]
pub fn progress_indicator(style: ProgressStyle, fill: ProgressFraction) -> Div {
    let share = fill.value();
    if share <= 0.0 {
        return div().h_full().w(relative(0.0));
    }

    let clip = div().h_full().overflow_hidden();
    let span = 1.0 / share;
    if !span.is_finite() {
        // A sub-pixel share has no expressible reciprocal; the sliver is
        // visually identical either way at this scale.
        return clip.w(relative(share)).bg(style.fill_color);
    }

    clip.w(relative(share)).child(
        div()
            .h_full()
            .w(relative(span))
            .flex_shrink_0()
            .rounded(style.corner_radius)
            .bg(style.fill_color),
    )
}

/// Returns the reached progress recipe as a plain GPUI [`Div`].
///
/// The track consumes the caller-resolved [`ProgressStyle`] verbatim: full
/// width, fixed 8 px height, pill corners, clipped overflow, and this mode's
/// `--primary` at exactly 20% alpha. Its only child is
/// [`progress_indicator`], so the bounded fill always shares one resolved
/// recipe with its track. Further [`gpui::Styled`] refinements chain onto
/// the returned element and later values win.
#[must_use]
pub fn progress(style: ProgressStyle, fill: ProgressFraction) -> Div {
    div()
        .w_full()
        .h(style.track_height)
        .rounded(style.corner_radius)
        .overflow_hidden()
        .bg(style.track_color)
        .child(progress_indicator(style, fill))
}
