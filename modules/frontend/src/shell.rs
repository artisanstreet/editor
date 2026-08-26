//! Persistent native shell/layout frame with its startup placeholder.
//!
//! Phase 8 leaf: extracts the shell recipe from the accepted source and composes
//! only shared `artisan-ui` recipes ([`artisan_ui::card`],
//! [`artisan_ui::badge`], [`artisan_ui::separator`]) and tokens from
//! [`artisan_ui::theme`]; there is no private palette or second card design.

use artisan_ui::badge::{BadgeStyle, outline_badge};
use artisan_ui::card::{CardStyle, compact_card};
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::theme::ArtisanTheme;
use gpui::{
    Div, Hsla, Pixels, div, prelude::InteractiveElement as _, prelude::ParentElement as _,
    prelude::Styled as _, px,
};

/// Fixed placeholder-card width: a caller-side sizing refinement over the
/// shared compact-card recipe, keeping the silhouette stable in the flexible
/// main surface.
const PLACEHOLDER_CARD_WIDTH: f32 = 320.0;

/// Startup-placeholder badge label, naming the port phase honestly.
pub const PLACEHOLDER_BADGE_LABEL: &str = "phase 8";
/// Startup-placeholder title.
pub const PLACEHOLDER_TITLE: &str = "Artisan native shell";
/// Startup-placeholder caption explaining the empty surfaces truthfully.
pub const PLACEHOLDER_CAPTION: &str =
    "Startup placeholder \u{00B7} workflow surfaces arrive in later phases.";

/// Debug selectors pinning the shell root, rail, main surface, and
/// placeholder regions for the rendered behavior test.
pub const SHELL_ROOT_SELECTOR: &str = "native-shell-root";
pub const SHELL_RAIL_SELECTOR: &str = "native-shell-rail";
pub const SHELL_MAIN_SELECTOR: &str = "native-shell-main";
pub const SHELL_PLACEHOLDER_SELECTOR: &str = "native-shell-startup-placeholder";

/// Shell-frame geometry and paint values resolved once per mode from shared
/// theme tokens; public so behavior tests can pin exact native semantics
/// without reaching into GPUI internals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellFrameStyle {
    /// Sidebar-rail width: legacy Tailwind `w-14`
    /// (`sectioned-panel.svelte:164`), fourteen steps of the shared 4 px
    /// spacing unit — 56 px.
    pub rail_width: Pixels,
    /// Main-surface padding on the top, right, and bottom edges: legacy
    /// `p-2 pl-0` (`sectioned-panel.svelte:306`), two spacing steps — 8 px.
    /// The leading edge stays flush so rail and main surface read as one
    /// background plane cut by the card.
    pub surface_padding: Pixels,
    /// Root window paint: `--background` resolved for the theme mode.
    pub window_background: Hsla,
}

impl ShellFrameStyle {
    /// Resolves the shell-frame recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            rail_width: theme.spacing.steps(14.0),
            surface_padding: theme.spacing.steps(2.0),
            window_background: theme.colors.background.to_paint(),
        }
    }
}

/// Returns the persistent sidebar rail as a plain GPUI [`Div`].
///
/// A layout-only frame at the audited width: full height, fixed width,
/// non-shrinking. Its contents are interactive product surfaces owned by
/// later leaves; nothing renders inside the frame yet.
#[must_use]
pub fn shell_rail(frame: ShellFrameStyle) -> Div {
    div().h_full().w(frame.rail_width).flex_shrink_0()
}

/// Returns the noninteractive startup placeholder as a plain GPUI [`Div`].
///
/// Composes only shared `artisan-ui` recipes resolved from the one passed
/// theme. Static copy only: no engine state, timers, loading animation, or
/// navigation.
#[must_use]
pub fn startup_placeholder(theme: ArtisanTheme) -> Div {
    let card_style = CardStyle::resolve(theme);
    let badge_style = BadgeStyle::resolve(theme);

    compact_card(card_style)
        .w(px(PLACEHOLDER_CARD_WIDTH))
        .child(
            div()
                .flex()
                .flex_row()
                .child(outline_badge(badge_style, PLACEHOLDER_BADGE_LABEL)),
        )
        .child(PLACEHOLDER_TITLE)
        .child(separator(
            theme.colors.border.to_paint(),
            SeparatorAxis::Horizontal,
        ))
        .child(
            div()
                .text_size(theme.typography.label_text)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(PLACEHOLDER_CAPTION),
        )
}

/// Returns the rendered shell frame: rail plus flexible main surface hosting
/// the startup placeholder. Resolves all paint, spacing, typography, card,
/// badge, and separator values from the one shared theme argument.
#[must_use]
pub fn native_shell(theme: ArtisanTheme) -> Div {
    let frame = ShellFrameStyle::resolve(theme);

    div()
        .size_full()
        .flex()
        .flex_row()
        .bg(frame.window_background)
        .debug_selector(|| SHELL_ROOT_SELECTOR.to_string())
        .child(shell_rail(frame).debug_selector(|| SHELL_RAIL_SELECTOR.to_string()))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .pt(frame.surface_padding)
                .pr(frame.surface_padding)
                .pb(frame.surface_padding)
                .flex()
                .items_center()
                .justify_center()
                .debug_selector(|| SHELL_MAIN_SELECTOR.to_string())
                .child(
                    startup_placeholder(theme)
                        .debug_selector(|| SHELL_PLACEHOLDER_SELECTOR.to_string()),
                ),
        )
}
