//! Static hairline separator for reached conversation and selection surfaces.
//!
//! Ported from the audited legacy wrapper
//! (`modules/frontend/src/lib/components/ui/separator/separator.svelte`;
//! INVENTORY §2 row 22). The wrapper is one noninteractive paint recipe:
//! `bg-border shrink-0`, one physical pixel thick on its cross axis, and full
//! size on its main axis. Horizontal is the reached default. Vertical retains
//! the legacy project's deliberate `h-full` deviation from upstream shadcn's
//! self-stretch behavior so later selected surfaces cannot silently acquire a
//! different geometry.
//!
//! The production compaction-status row chains `flex-1 min-w-0` after this
//! recipe, replacing the horizontal `w-full` basis so equal separators grow
//! around the fixed status label. Select-specific negative margins, alpha,
//! width override, and pointer handling remain caller composition rather than
//! entering this primitive. Pinned GPUI 0.2.2 exposes no platform
//! accessibility tree, so legacy decorative-role intent cannot truthfully be
//! represented here; the element has no input, focus, state, or motion.

use gpui::{Div, Hsla, Styled, div, px};

/// Axis controlling which separator dimension is the one-pixel hairline.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SeparatorAxis {
    /// One-pixel height spanning the available width.
    #[default]
    Horizontal,
    /// One-pixel width spanning the available height.
    Vertical,
}

/// Returns the legacy separator recipe as a plain GPUI [`Div`].
///
/// `border` is resolved by the caller from its [`crate::theme::ArtisanTheme`]
/// so normal separators use the exact mode-specific `--border` paint and
/// composed menu separators can intentionally apply audited alpha overrides.
/// Further [`gpui::Styled`] refinements are applied after the recipe and win.
#[must_use]
pub fn separator(border: Hsla, axis: SeparatorAxis) -> Div {
    let base = div().flex_shrink_0().bg(border);
    match axis {
        SeparatorAxis::Horizontal => base.h(px(1.0)).w_full(),
        SeparatorAxis::Vertical => base.w(px(1.0)).h_full(),
    }
}
