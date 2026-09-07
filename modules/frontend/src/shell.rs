//! Persistent native shell/layout frame with its startup placeholder.
//!
//! Phase 8 leaf: extracts the shell recipe from the accepted source and composes
//! only shared `artisan-ui` recipes ([`artisan_ui::card`],
//! [`artisan_ui::badge`], [`artisan_ui::separator`]) and tokens from
//! [`artisan_ui::theme`]; there is no private palette or second card design.

use artisan_assets::AssetId;
use artisan_ui::asset_seam::asset_glyph;
use artisan_ui::badge::{BadgeStyle, outline_badge};
use artisan_ui::card::{CardStyle, compact_card};
use artisan_ui::icon::{IconSize, IconStyle, IconTint, icon};
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::{
    AnyElement, Div, FontWeight, Hsla, Pixels, div, prelude::InteractiveElement as _,
    prelude::ParentElement as _, prelude::Styled as _, px,
};

use crate::shell_layout::ProseWidth;
use crate::shell_rail_model::RailListModel;
use crate::thread_navigation_core::format_recent_thread_time;
use crate::workspace_header_presentation::{WorkspaceHeaderPresentation, WorkspaceHeaderSegment};

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
        .child(
            div()
                .text_size(theme.typography.dialog_title_text)
                .font_weight(FontWeight::MEDIUM)
                .child(PLACEHOLDER_TITLE),
        )
        .child(separator(
            theme.colors.border.to_paint(),
            SeparatorAxis::Horizontal,
        ))
        .child(
            div()
                .text_size(theme.typography.control_text)
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

// ---------------------------------------------------------------------------
// Legacy application frame (`routes/+layout.svelte` + `sectioned-panel.svelte`).
//
// The composition below reproduces the legacy window-relative frame on the
// Windows (non-mac) branch: title-bar strip, icon rail column, workspace
// header line, and the rounded primary/inspector cards hosting the route
// content slot. Data and behavior stay in the policy leaves
// (`shell_layout`, `shell_rail_model`, `workspace_header_presentation`,
// `thread_hover_rail_policy`); this section only paints them.
// ---------------------------------------------------------------------------

/// Title-bar strip height: legacy `h-10` (`+layout.svelte:583`), ten steps of
/// the shared 4 px spacing unit — 40 px. Also the in-card header row height
/// (`sectioned-panel.svelte:320`, `flex h-10`).
pub const LEGACY_TITLE_BAR_HEIGHT_PX: f32 = 40.0;

/// Rail cluster insets: legacy `top-2` / `bottom-2`
/// (`sectioned-panel.svelte:165,300`), two spacing steps — 8 px.
pub const LEGACY_RAIL_CLUSTER_INSET_PX: f32 = 8.0;

/// Main-surface padding: legacy `p-2 pl-0` (`sectioned-panel.svelte:306`) —
/// 8 px on top/right/bottom, flush on the leading edge.
pub const LEGACY_MAIN_PADDING_PX: f32 = 8.0;

/// Primary/inspector surface gap: legacy `gap-2`
/// (`sectioned-panel.svelte:310`) — 8 px.
pub const LEGACY_SURFACE_GAP_PX: f32 = 8.0;

/// Primary card inner padding: legacy `p-1`
/// (`sectioned-panel.svelte:313,341`) — 4 px.
pub const LEGACY_PANEL_PADDING_PX: f32 = 4.0;

/// In-card header side inset: legacy `px-6` (`sectioned-panel.svelte:321`) —
/// 24 px.
pub const LEGACY_HEADER_SIDE_INSET_PX: f32 = 24.0;

/// Title-bar identity trailing inset: legacy `pr-6` (`+layout.svelte:604`) —
/// 24 px, keeping the line clear of the window controls' end.
pub const LEGACY_TITLE_BAR_TRAILING_INSET_PX: f32 = 24.0;

/// Title-bar content right pad: legacy `pr-1` (`+layout.svelte:587`) — 4 px.
pub const LEGACY_TITLE_BAR_CONTENT_PAD_PX: f32 = 4.0;

/// Closed-inspector title-bar spacer: legacy `w-2` (`+layout.svelte:616`) —
/// 8 px.
pub const LEGACY_TITLE_BAR_SPACER_PX: f32 = 8.0;

/// Open-inspector title-bar spacer addition: the `+1rem` in
/// `w-[calc(clamp(16rem,25vw,350px)+1rem)]` (`+layout.svelte:612`) — 16 px on
/// top of the inspector column width.
pub const LEGACY_INSPECTOR_TITLE_GAP_PX: f32 = 16.0;

/// Rail pill width: legacy `w-10` (`sectioned-panel.svelte:182,256`) — 40 px.
pub const LEGACY_RAIL_PILL_WIDTH_PX: f32 = 40.0;

/// Rail pill vertical padding: legacy `py-1` — 4 px.
pub const LEGACY_RAIL_PILL_PADDING_PX: f32 = 4.0;

/// Rail control edge: legacy `size-8` (`sectioned-panel.svelte:187,237`) —
/// 32 px.
pub const LEGACY_RAIL_BUTTON_PX: f32 = 32.0;

/// Brand-mark edge: legacy `size-7` on the rail logo swap
/// (`sectioned-panel.svelte:205-218`) — 28 px inside the 32 px action.
pub const LEGACY_RAIL_BRAND_MARK_PX: f32 = 28.0;

/// Rail divider height: the literal `h-[2px]` hairline pair
/// (`sectioned-panel.svelte:230`) — 2 px total.
pub const LEGACY_RAIL_DIVIDER_PX: f32 = 2.0;

/// Identity avatar edge: legacy `size-10` (`sidebar-identity.svelte:240`) —
/// 40 px.
pub const LEGACY_IDENTITY_AVATAR_PX: f32 = 40.0;

/// Working-row state-dot edge: legacy `size-1.5`
/// (`thread-hover-rail.svelte:615`) — 6 px.
pub const LEGACY_RAIL_STATE_DOT_PX: f32 = 6.0;

/// Rail-list card padding: legacy `p-1` (`thread-hover-rail.svelte:553,679`) —
/// 4 px.
pub const LEGACY_RAIL_LIST_PADDING_PX: f32 = 4.0;

/// Fallback label painted when the identity has no name or hostname.
pub const RAIL_IDENTITY_FALLBACK_LABEL: &str = "?";

/// Debug selector for the legacy frame root (`flex h-dvh min-h-0 flex-col`).
pub const LEGACY_SHELL_FRAME_SELECTOR: &str = "legacy-shell-frame";
/// Debug selector for the title-bar strip.
pub const LEGACY_SHELL_TITLE_BAR_SELECTOR: &str = "legacy-shell-title-bar";
/// Debug selector for the title-bar identity line.
pub const LEGACY_SHELL_TITLE_IDENTITY_SELECTOR: &str = "legacy-shell-title-identity";
/// Debug selector for the icon rail column.
pub const LEGACY_SHELL_RAIL_COLUMN_SELECTOR: &str = "legacy-shell-rail-column";
/// Debug selector for the primary content surface.
pub const LEGACY_SHELL_CONTENT_SELECTOR: &str = "legacy-shell-content";
/// Debug selector for the in-card workspace header row.
pub const LEGACY_SHELL_HEADER_SELECTOR: &str = "legacy-shell-header";
/// Debug selector for the inspector surface.
pub const LEGACY_SHELL_INSPECTOR_SELECTOR: &str = "legacy-shell-inspector";
/// Debug selector for the rail brand action.
pub const LEGACY_SHELL_RAIL_BRAND_SELECTOR: &str = "legacy-shell-rail-brand";
/// Debug selector for the rail marketplace button.
pub const LEGACY_SHELL_RAIL_MARKETPLACE_SELECTOR: &str = "legacy-shell-rail-marketplace";
/// Debug selector for the rail account avatar.
pub const LEGACY_SHELL_RAIL_AVATAR_SELECTOR: &str = "legacy-shell-rail-avatar";
/// Debug selector for the pinned working-threads card.
pub const LEGACY_SHELL_RAIL_PINNED_SELECTOR: &str = "legacy-shell-rail-pinned";
/// Debug selector for the settled history list.
pub const LEGACY_SHELL_RAIL_HISTORY_SELECTOR: &str = "legacy-shell-rail-history";

/// Legacy frame geometry and paint resolved once per mode from shared theme
/// tokens.
///
/// The rail width, surface padding, and window paint reuse the exact
/// [`ShellFrameStyle`] values so the legacy frame and the current
/// `native_application` mount measure identically.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegacyShellStyle {
    /// Title-bar strip and in-card header height: `h-10` — 40 px.
    pub title_bar_height: Pixels,
    /// Icon rail width: legacy `w-14` — 56 px.
    pub rail_width: Pixels,
    /// Main-surface top/right/bottom padding: legacy `p-2 pl-0` — 8 px.
    pub surface_padding: Pixels,
    /// Primary/inspector gap: legacy `gap-2` — 8 px.
    pub surface_gap: Pixels,
    /// Primary/inspector corner: legacy `rounded-3xl` (24 px) mapped onto the
    /// nearest shared corner token (`--radius-3xl`, 22 px).
    pub panel_radius: Pixels,
    /// Primary/inspector fill: the flat stand-in for the legacy vertical card
    /// gradient (`--surface-125→--surface-75` light,
    /// `--surface-900→--surface-925` dark; `sectioned-panel.svelte:313,341`).
    /// Pinned GPUI paints no CSS gradient, so the lower ramp step stands in
    /// for the blend.
    pub panel_background: Hsla,
    /// Rail pill fill: legacy `bg-surface-125 dark:bg-surface-900`
    /// (`sectioned-panel.svelte:182`).
    pub pill_background: Hsla,
    /// Root window paint: `--background` resolved for the theme mode.
    pub window_background: Hsla,
}

impl LegacyShellStyle {
    /// Resolves the legacy frame recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        let frame = ShellFrameStyle::resolve(theme);
        let (panel_step, pill_step) = match theme.mode {
            ThemeMode::Light => (SurfaceStep::S75, SurfaceStep::S125),
            ThemeMode::Dark => (SurfaceStep::S925, SurfaceStep::S900),
        };
        Self {
            title_bar_height: px(LEGACY_TITLE_BAR_HEIGHT_PX),
            rail_width: frame.rail_width,
            surface_padding: frame.surface_padding,
            surface_gap: px(LEGACY_SURFACE_GAP_PX),
            panel_radius: RadiusTokens::value(RadiusStep::X3l),
            panel_background: theme.surfaces.value(panel_step).to_paint(),
            pill_background: theme.surfaces.value(pill_step).to_paint(),
            window_background: frame.window_background,
        }
    }
}

/// The account identity painted at the rail's bottom edge.
///
/// Mirrors the trigger half of `sidebar-identity.svelte`: the avatar reads as
/// the control and the name/hostname live in the menu it opens, so the rail
/// only needs the monogram source. Controllers, usage state, and the menu
/// itself stay outside this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RailIdentity<'a> {
    /// `display_name ?? username ?? hostname`; `None` renders the fallback.
    pub display_name: Option<&'a str>,
    /// The machine the avatar is derived from; second monogram source.
    pub hostname: Option<&'a str>,
}

impl<'a> RailIdentity<'a> {
    /// Builds one rail identity from already-resolved adapter values.
    #[must_use]
    pub const fn new(display_name: Option<&'a str>, hostname: Option<&'a str>) -> Self {
        Self {
            display_name,
            hostname,
        }
    }
}

/// Returns the one-character monogram painted only when the rail has no
/// identity to seed the gradient tile with.
///
/// The legacy avatar never shows initials (it is the dithered gradient tile
/// below); this stays as the signed-out fallback reading, matching
/// [`RAIL_IDENTITY_FALLBACK_LABEL`].
#[must_use]
pub fn rail_identity_monogram(identity: RailIdentity<'_>) -> String {
    let source = identity
        .display_name
        .filter(|name| !name.is_empty())
        .or_else(|| identity.hostname.filter(|host| !host.is_empty()));
    match source.and_then(|name| name.chars().next()) {
        Some(first) => first.to_uppercase().collect(),
        None => String::from(RAIL_IDENTITY_FALLBACK_LABEL),
    }
}

/// Cells across the gradient tile: 16 lands on 2 px cells at the 32 px avatar
/// (`gradient-avatar.ts:54`).
const GRADIENT_AVATAR_CELLS: u8 = 16;

/// vsx's cap short of full density so the lit end keeps its grain instead of
/// flattening into a solid block (`gradient-avatar.ts:61`).
const GRADIENT_AVATAR_MAX_DENSITY: f64 = 0.9;

/// vsx's threshold matrix (`gradient-avatar.ts:46-51`); each cell is normalized
/// to mid-step at the call site, exactly like the legacy module.
const GRADIENT_AVATAR_BAYER_4: [[u8; 4]; 4] =
    [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Tailwind's chromatic families only, as opaque `oklch` source colors
/// transcribed verbatim from `gradient-avatar.ts:23-41` (the neutrals read as
/// a disabled avatar, so legacy excludes them). Each pair ramps the hue's 600
/// into its 400 toward the top-right corner.
const GRADIENT_AVATAR_PALETTE: [(Oklch, Oklch); 17] = [
    (
        Oklch::new(0.577, 0.245, 27.325),
        Oklch::new(0.704, 0.191, 22.216),
    ), // red
    (
        Oklch::new(0.646, 0.222, 41.116),
        Oklch::new(0.75, 0.183, 55.934),
    ), // orange
    (
        Oklch::new(0.666, 0.179, 58.318),
        Oklch::new(0.828, 0.189, 84.429),
    ), // amber
    (
        Oklch::new(0.681, 0.162, 75.834),
        Oklch::new(0.852, 0.199, 91.936),
    ), // yellow
    (
        Oklch::new(0.648, 0.2, 131.684),
        Oklch::new(0.841, 0.238, 128.85),
    ), // lime
    (
        Oklch::new(0.627, 0.194, 149.214),
        Oklch::new(0.792, 0.209, 151.711),
    ), // green
    (
        Oklch::new(0.596, 0.145, 163.225),
        Oklch::new(0.765, 0.177, 163.223),
    ), // emerald
    (
        Oklch::new(0.6, 0.118, 184.704),
        Oklch::new(0.777, 0.152, 181.912),
    ), // teal
    (
        Oklch::new(0.609, 0.126, 221.723),
        Oklch::new(0.789, 0.154, 211.53),
    ), // cyan
    (
        Oklch::new(0.588, 0.158, 241.966),
        Oklch::new(0.746, 0.16, 232.661),
    ), // sky
    (
        Oklch::new(0.546, 0.245, 262.881),
        Oklch::new(0.707, 0.165, 254.624),
    ), // blue
    (
        Oklch::new(0.511, 0.262, 276.966),
        Oklch::new(0.673, 0.182, 276.935),
    ), // indigo
    (
        Oklch::new(0.541, 0.281, 293.009),
        Oklch::new(0.702, 0.183, 293.541),
    ), // violet
    (
        Oklch::new(0.558, 0.288, 302.321),
        Oklch::new(0.714, 0.203, 305.504),
    ), // purple
    (
        Oklch::new(0.591, 0.293, 322.896),
        Oklch::new(0.74, 0.238, 322.16),
    ), // fuchsia
    (
        Oklch::new(0.592, 0.249, 0.584),
        Oklch::new(0.718, 0.202, 349.761),
    ), // pink
    (
        Oklch::new(0.586, 0.253, 17.585),
        Oklch::new(0.712, 0.194, 13.428),
    ), // rose
];

/// FNV-1a over UTF-16 code units, matching the legacy `SeedHash`
/// (`gradient-avatar.ts:70-77`): the avatar only needs a stable, well-spread
/// bucket per seed, and iterating code units keeps a host's color identical
/// between runtimes.
fn gradient_avatar_seed_hash(seed: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    let mut units = [0_u16; 2];
    for ch in seed.chars() {
        for unit in ch.encode_utf16(&mut units) {
            hash ^= u32::from(*unit);
            hash = hash.wrapping_mul(0x0100_0193);
        }
    }
    hash
}

/// The palette bucket a seed resolves to (`GradientAvatarColorFor`).
fn gradient_avatar_bucket(seed: &str) -> usize {
    let span = GRADIENT_AVATAR_PALETTE.len() as u64;
    let index = u64::from(gradient_avatar_seed_hash(seed)) % span;
    usize::try_from(index).unwrap_or(0)
}

/// The identity seed for the gradient tile: the machine, not the person — one
/// host keeps one avatar whoever is signed in (`sidebar-identity.svelte:54`).
/// `None` means the rail has no identity at all (legacy renders its generic
/// fallback then, not a tile).
fn gradient_avatar_seed(identity: RailIdentity<'_>) -> Option<&str> {
    identity
        .hostname
        .filter(|host| !host.is_empty())
        .or_else(|| identity.display_name.filter(|name| !name.is_empty()))
}

/// The tile's base and lit paints for one seed, through the shared
/// `oklch`→paint pipeline.
fn gradient_avatar_paints(seed: &str) -> (Hsla, Hsla) {
    let (base, lit) = GRADIENT_AVATAR_PALETTE[gradient_avatar_bucket(seed)];
    (base.to_paint(), lit.to_paint())
}

/// Whether one tile cell paints lit: progress along the bottom-left →
/// top-right diagonal against the Bayer threshold (`gradient-avatar.ts:91-116`).
fn gradient_avatar_cell_lit(x: u8, y: u8) -> bool {
    if x >= GRADIENT_AVATAR_CELLS {
        return false;
    }
    let horizontal_progress = (f64::from(x) + 0.5) / f64::from(GRADIENT_AVATAR_CELLS);
    let vertical_progress = 1.0 - (f64::from(y) + 0.5) / f64::from(GRADIENT_AVATAR_CELLS);
    // Inputs are multiples of 1/32, so the halved sum is exact in binary and
    // matches the legacy `(a + b) / 2` bit-for-bit without overflow risk.
    let density = f64::midpoint(horizontal_progress, vertical_progress)
        .clamp(0.0, 1.0)
        .min(GRADIENT_AVATAR_MAX_DENSITY);
    // `x` is below `GRADIENT_AVATAR_CELLS` (16), so the masked indices stay in
    // the 4-wide matrix without a bounds check.
    let threshold =
        (f64::from(GRADIENT_AVATAR_BAYER_4[usize::from(y & 3)][usize::from(x & 3)]) + 0.5) / 16.0;
    density > threshold
}

/// Returns the account avatar: the seeded dithered gradient tile from
/// `gradient-avatar.ts`, clipped round at the legacy 40 px edge.
///
/// The tile fills its box and stays square; the caller clips it round (legacy
/// `Avatar` root). With no identity at all the rail keeps the monogram
/// fallback instead of inventing a host color.
fn gradient_avatar(theme: &ArtisanTheme, identity: RailIdentity<'_>) -> Div {
    let Some(seed) = gradient_avatar_seed(identity) else {
        return div()
            .size(px(LEGACY_IDENTITY_AVATAR_PX))
            .rounded_full()
            .bg(theme.colors.muted.to_paint())
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.colors.foreground.to_paint())
            .text_size(theme.typography.control_text)
            .font_weight(FontWeight::MEDIUM)
            .debug_selector(|| LEGACY_SHELL_RAIL_AVATAR_SELECTOR.to_string())
            .child(rail_identity_monogram(identity));
    };
    let (base, lit) = gradient_avatar_paints(seed);
    let cell_edge = px(LEGACY_IDENTITY_AVATAR_PX / f32::from(GRADIENT_AVATAR_CELLS));
    let mut tile = div()
        .size(px(LEGACY_IDENTITY_AVATAR_PX))
        .rounded_full()
        .overflow_hidden()
        .flex()
        .flex_col()
        .bg(base)
        .debug_selector(|| LEGACY_SHELL_RAIL_AVATAR_SELECTOR.to_string());
    for y in 0..GRADIENT_AVATAR_CELLS {
        let mut row = div().flex().flex_row().w_full().h(cell_edge);
        for x in 0..GRADIENT_AVATAR_CELLS {
            row = row.child(
                div()
                    .flex_1()
                    .h_full()
                    .bg(if gradient_avatar_cell_lit(x, y) {
                        lit
                    } else {
                        base
                    }),
            );
        }
        tile = tile.child(row);
    }
    tile
}

/// Returns the `data-prose-width` token name carried by the frame.
///
/// GPUI renders no DOM, so there is no `data-*` attribute to set; the
/// [`ProseWidth`] value rides in [`LegacyShellProps`] instead and this helper
/// exposes the exact persisted name (`tight` / `balanced` / `loose`) for
/// selectors and diagnostics.
#[must_use]
pub fn legacy_shell_prose_name(prose_width: ProseWidth) -> &'static str {
    prose_width.as_str()
}

/// Returns the debug selector suffix identifying the frame's prose column.
///
/// The slot measures its margin from this column via `shell_layout`
/// (`prose_column_pixels`); the selector lets tests pin which column the
/// mounted frame was built with.
#[must_use]
pub fn legacy_shell_prose_selector(prose_width: ProseWidth) -> String {
    format!("legacy-shell-prose-{}", prose_width.as_str())
}

/// Returns the title-bar trailing spacer width for an inspector state.
///
/// Mirrors `+layout.svelte:610-617`: an open inspector replays its column plus
/// the 1 rem shell gap; a closed one keeps the `w-2` spacer. Callers resolve
/// the column with `shell_layout::inspector_column_pixels(viewport_width)`.
#[must_use]
pub fn title_bar_trailing_spacer_px(inspector_width_px: Option<f32>) -> f32 {
    match inspector_width_px {
        Some(width) => width + LEGACY_INSPECTOR_TITLE_GAP_PX,
        None => LEGACY_TITLE_BAR_SPACER_PX,
    }
}

/// Inputs to the legacy frame render.
///
/// `title_header` paints the title-strip identity line (the desktop shell in
/// `+layout.svelte:594-607`); `card_header` paints the in-card header row
/// (the web shell in `sectioned-panel.svelte:316-325`). Legacy renders
/// exactly one of them: the layout passes `header=undefined` to the card on
/// desktop precisely so the workspace is not named twice. `content` is the
/// route surface; `secondary` is the inspector/editor-files card.
/// `inspector_width_px` drives both the title-bar replay spacer and the
/// inspector card width and should come from
/// `shell_layout::inspector_column_pixels`.
pub struct LegacyShellProps<'a> {
    /// The shared theme every paint value resolves from.
    pub theme: ArtisanTheme,
    /// The `data-prose-width` column state the slot measures from.
    pub prose_width: ProseWidth,
    /// The account identity for the rail avatar.
    pub identity: RailIdentity<'a>,
    /// The title-strip identity line; `None` renders the strip empty (gate
    /// overlay up, or a route with no workspace).
    pub title_header: Option<AnyElement>,
    /// The in-card header row for the web shell; `None` on desktop.
    pub card_header: Option<AnyElement>,
    /// The open inspector width in pixels; `None` closes the column.
    pub inspector_width_px: Option<f32>,
    /// The inspector/editor-files card; only painted when
    /// `inspector_width_px` is `Some`.
    pub secondary: Option<AnyElement>,
}

/// Returns the desktop title-bar strip (Windows branch).
///
/// Layout mirrors `+layout.svelte:582-618` on the non-mac path: `h-10` strip,
/// `w-14` rail spacer, the identity line (`w-full min-w-0 items-center pr-6`),
/// then the inspector replay spacer. The macOS right-aligned variant, the
/// drag region (`-webkit-app-region`), and the `fade-edge-end` mask are
/// web-shell concerns with no GPUI equivalent and are intentionally absent.
#[must_use]
pub fn shell_title_bar(
    theme: ArtisanTheme,
    header: Option<AnyElement>,
    inspector_width_px: Option<f32>,
) -> Div {
    let style = LegacyShellStyle::resolve(theme);
    let mut line = div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .items_center()
        .pr(px(LEGACY_TITLE_BAR_TRAILING_INSET_PX))
        .debug_selector(|| LEGACY_SHELL_TITLE_IDENTITY_SELECTOR.to_string());
    if let Some(header) = header {
        line = line.child(header);
    }

    div()
        .h(style.title_bar_height)
        .flex_shrink_0()
        .flex()
        .bg(style.window_background)
        .debug_selector(|| LEGACY_SHELL_TITLE_BAR_SELECTOR.to_string())
        .child(div().w(style.rail_width).flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .pr(px(LEGACY_TITLE_BAR_CONTENT_PAD_PX))
                .child(line),
        )
        .child(
            div()
                .w(px(title_bar_trailing_spacer_px(inspector_width_px)))
                .flex_shrink_0(),
        )
}

/// Returns the icon rail column with its pill cluster and identity avatar.
///
/// Mirrors `sectioned-panel.svelte:164-303`: the `w-14` column, the top pill
/// (`w-10 rounded-full`) holding the brand action and the marketplace button
/// split by the 2 px hairline pair, and the account avatar pinned to the
/// bottom. The brand paints the vendored `artisan.logo-gradient` artwork bound
/// to this site by the asset manifest (`shell.logo-gradient`); the rest-state
/// jaw-shaded monogram PNG has no catalog entry, so the gradient face stands
/// in and the PNG is reported as missing rather than vendored here. The
/// marketplace paints `tabler.shopping-bag` in the legacy muted tone. The
/// avatar is the data-driven dithered gradient tile seeded by the hostname
/// (manifest `identity.gradient-avatar` carries no vendored bytes by design).
/// The dropdown hover pill, command menu, surface cycle, and account menu are
/// behavior owned by later packets.
#[must_use]
pub fn shell_rail_column(theme: ArtisanTheme, identity: RailIdentity<'_>) -> Div {
    let style = LegacyShellStyle::resolve(theme);
    let card = CardStyle::resolve(theme);

    let brand = div()
        .size(px(LEGACY_RAIL_BUTTON_PX))
        .rounded_full()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .debug_selector(|| LEGACY_SHELL_RAIL_BRAND_SELECTOR.to_string())
        .child(asset_glyph(AssetId::ARTISAN_LOGO_GRADIENT).size(px(LEGACY_RAIL_BRAND_MARK_PX)));
    let marketplace = div()
        .size(px(LEGACY_RAIL_BUTTON_PX))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .debug_selector(|| LEGACY_SHELL_RAIL_MARKETPLACE_SELECTOR.to_string())
        .child(icon(IconStyle::resolve(
            theme,
            AssetId::TABLER_SHOPPING_BAG,
            IconSize::Default,
            IconTint::Muted,
        )));
    let pill = div()
        .w(px(LEGACY_RAIL_PILL_WIDTH_PX))
        .rounded_full()
        .bg(style.pill_background)
        .py(px(LEGACY_RAIL_PILL_PADDING_PX))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(LEGACY_RAIL_PILL_PADDING_PX))
        .shadow(vec![card.ring()])
        .child(brand)
        .child(
            div()
                .h(px(LEGACY_RAIL_DIVIDER_PX))
                .w_full()
                .bg(theme.colors.border.to_paint()),
        )
        .child(marketplace);

    let avatar = gradient_avatar(&theme, identity);

    div()
        .relative()
        .h_full()
        .w(style.rail_width)
        .flex_shrink_0()
        .debug_selector(|| LEGACY_SHELL_RAIL_COLUMN_SELECTOR.to_string())
        .child(
            div()
                .absolute()
                .top(px(LEGACY_RAIL_CLUSTER_INSET_PX))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .flex_col()
                .items_center()
                .child(pill),
        )
        .child(
            div()
                .absolute()
                .bottom(px(LEGACY_RAIL_CLUSTER_INSET_PX))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .justify_center()
                .child(avatar),
        )
}

/// Returns the painted workspace header line for one presentation.
///
/// Segment order and connectors follow `workspace-header.svelte:51-100`
/// exactly: remote link (or folder) + `on` + branch, an `in` + checkout
/// segment only for a diverging checkout name, then `/` + the ellipsized
/// thread title. Icons (folder, git-branch, host mark) have no GPUI asset
/// seam in this packet, so each icon-bearing segment keeps its exact text in
/// the legacy tone: links in `--banner-info`, context in
/// `--muted-foreground`, the thread title in `--foreground`.
#[must_use]
pub fn workspace_header_row<M>(
    presentation: Option<WorkspaceHeaderPresentation<'_, M>>,
    theme: ArtisanTheme,
) -> Div {
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .min_w(px(0.0))
        .text_size(theme.typography.control_text)
        .text_color(theme.colors.muted_foreground.to_paint())
        .debug_selector(|| LEGACY_SHELL_HEADER_SELECTOR.to_string());

    let Some(presentation) = presentation else {
        return row;
    };
    for segment in presentation.into_segments() {
        match segment {
            WorkspaceHeaderSegment::Folder { label }
            | WorkspaceHeaderSegment::Checkout { label } => {
                row = row.child(div().flex_shrink_0().child(label.to_string()));
            }
            WorkspaceHeaderSegment::RemoteLink { link } => {
                row = row.child(
                    div()
                        .flex_shrink_0()
                        .text_color(theme.colors.banner_info.to_paint())
                        .child(link.qualified_label.to_string()),
                );
            }
            WorkspaceHeaderSegment::Text(text) => {
                row = row.child(div().flex_shrink_0().child(text.as_str()));
            }
            WorkspaceHeaderSegment::Branch { branch } => {
                row = row.child(div().flex_shrink_0().child(branch.label().to_string()));
            }
            WorkspaceHeaderSegment::ThreadTitle { title } => {
                row = row.child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_color(theme.colors.foreground.to_paint())
                        .child(title.to_string()),
                );
            }
        }
    }
    row
}

/// Returns one settled history row: title plus relative time.
///
/// Mirrors `thread-hover-rail.svelte:471-487`: `rounded-lg px-2 py-2 text-sm
/// font-medium`, active rows in `--foreground`, the rest in
/// `--muted-foreground` with the `hover:text-foreground-extra` treatment, and
/// the `text-xs` timestamp. The engine mark icon has no asset seam here, so
/// rows keep the exact text reading.
#[must_use]
pub fn rail_history_row(
    theme: ArtisanTheme,
    title: &str,
    time_label: &str,
    is_active: bool,
) -> Div {
    let foreground = theme.colors.foreground.to_paint();
    let extra = theme.colors.foreground_extra.to_paint();
    let row = div()
        .relative()
        .flex()
        .items_center()
        .gap(px(8.0))
        .min_w(px(0.0))
        .rounded(RadiusTokens::value(RadiusStep::Lg))
        .px(px(8.0))
        .py(px(8.0))
        .text_size(theme.typography.control_text)
        .font_weight(FontWeight::MEDIUM)
        .text_color(if is_active {
            foreground
        } else {
            theme.colors.muted_foreground.to_paint()
        });
    let row = if is_active {
        row
    } else {
        row.hover(move |style| style.text_color(extra))
    };
    row.child(
        div()
            .flex_1()
            .min_w(px(0.0))
            .truncate()
            .child(title.to_string()),
    )
    .child(
        div()
            .flex_shrink_0()
            .text_size(theme.typography.label_text)
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(time_label.to_string()),
    )
}

/// Returns one pinned working row: title, project, and state dot.
///
/// Mirrors `thread-hover-rail.svelte:590-630`: the two-line row (title in
/// `--foreground`, project in `text-xs --muted-foreground`) with the trailing
/// state dot. Dot tones follow the row logic exactly: awaiting answers read
/// `--question-from` purple, unread failures `--destructive`, other unread
/// outcomes `--unread`; settled rows keep the untoned dot.
#[must_use]
pub fn rail_working_row(
    theme: ArtisanTheme,
    title: &str,
    project_name: Option<&str>,
    awaiting_answer: bool,
    failed: bool,
    needs_attention: bool,
) -> Div {
    let dot = if awaiting_answer {
        Some(theme.colors.question_from.to_paint())
    } else if needs_attention {
        Some(if failed {
            theme.colors.destructive.to_paint()
        } else {
            theme.colors.unread.to_paint()
        })
    } else {
        None
    };

    let mut row = div()
        .relative()
        .flex()
        .items_center()
        .gap(px(8.0))
        .min_w(px(0.0))
        .rounded(RadiusTokens::value(RadiusStep::Lg))
        .px(px(8.0))
        .py(px(8.0))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_size(theme.typography.control_text)
                        .text_color(theme.colors.foreground.to_paint())
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(theme.typography.label_text)
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(project_name.unwrap_or("No project").to_string()),
                ),
        );
    if let Some(tone) = dot {
        row = row.child(
            div()
                .size(px(LEGACY_RAIL_STATE_DOT_PX))
                .flex_shrink_0()
                .rounded_full()
                .bg(tone),
        );
    }
    row
}

/// Returns the rail thread list: pinned card plus labelled time groups.
///
/// Static visual counterpart of the proximity-revealed
/// `thread-hover-rail.svelte:532-704` list. The working card keeps the
/// `rounded-xl` glass edge with `p-1` rows; each settled group keeps its exact
/// label (`Yesterday`, `Last 3 days`, `Last 7 days`, `Past month`; the today
/// group renders unlabeled) above `rounded-lg` history rows. The hover card,
/// context menu, scroll fade, travelling pill, and proximity behavior stay in
/// `thread_hover_rail_policy` and later interaction packets; the orchestrator
/// mounts this element in the transcript margin the policy measures.
#[must_use]
pub fn rail_thread_list(
    theme: ArtisanTheme,
    model: &RailListModel<'_>,
    active_thread_id: Option<&str>,
    now_ms: i64,
) -> Div {
    use crate::thread_navigation_core::{is_failed_status, thread_has_active_work};

    let card_radius = RadiusTokens::value(RadiusStep::Xl);
    let card = CardStyle::resolve(theme);

    let mut list = div().flex().flex_col().min_w(px(0.0)).gap(px(12.0));

    if !model.pinned.is_empty() {
        let mut pinned = div()
            .w_full()
            .rounded(card_radius)
            .bg(theme.colors.card.to_paint())
            .p(px(LEGACY_RAIL_LIST_PADDING_PX))
            .shadow(vec![card.ring()])
            .flex()
            .flex_col()
            .debug_selector(|| LEGACY_SHELL_RAIL_PINNED_SELECTOR.to_string());
        for thread in &model.pinned {
            pinned = pinned.child(rail_working_row(
                theme,
                thread.title,
                thread.project_name,
                thread.awaiting_answer,
                is_failed_status(thread.live_status),
                thread.needs_attention(),
            ));
        }
        list = list.child(pinned);
    }

    let mut history = div()
        .flex()
        .flex_col()
        .min_w(px(0.0))
        .gap(px(12.0))
        .debug_selector(|| LEGACY_SHELL_RAIL_HISTORY_SELECTOR.to_string());
    for (group_index, group) in model.groups.iter().enumerate() {
        let mut section = div().flex().flex_col().min_w(px(0.0));
        if let Some(label) = group.label() {
            let mut heading = div()
                .px(px(8.0))
                .pb(px(4.0))
                .text_size(theme.typography.label_text)
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(label);
            if group_index == 0 {
                heading = heading.pt(px(8.0));
            }
            section = section.child(heading);
        }
        for thread in &group.threads {
            section = section.child(rail_history_row(
                theme,
                thread.title,
                &format_recent_thread_time(thread.activity_ms, now_ms),
                Some(thread.thread_id) == active_thread_id
                    && !thread_has_active_work(thread.live_status),
            ));
        }
        history = history.child(section);
    }
    list.child(history)
}

/// Returns the full legacy application frame with the route content slot.
///
/// Structure mirrors `+layout.svelte:565-652` (Windows branch) around
/// `sectioned-panel.svelte:154-348`: the `flex-col h-dvh` root carrying the
/// prose-width state, the title-bar strip, then the `flex-row` content row
/// with the icon rail, the `p-2 pl-0` main surface, the `rounded-3xl` primary
/// card (optional in-card header plus `content` slot), and the inspector card
/// when `props.inspector_width_px` is set. This is the entry point the
/// orchestrator mounts to replace the dev frame; `native_shell` above stays
/// as the placeholder-era frame for the running `native_application` mount.
#[must_use]
pub fn legacy_shell_frame(props: LegacyShellProps<'_>, content: AnyElement) -> Div {
    let theme = props.theme;
    let style = LegacyShellStyle::resolve(theme);
    let card = CardStyle::resolve(theme);

    let mut primary = div()
        .relative()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .rounded(style.panel_radius)
        .bg(style.panel_background)
        .p(px(LEGACY_PANEL_PADDING_PX))
        .shadow(vec![card.ring()]);
    if let Some(header) = props.card_header {
        primary = primary.child(
            div()
                .h(style.title_bar_height)
                .flex_shrink_0()
                .flex()
                .items_center()
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .px(px(LEGACY_HEADER_SIDE_INSET_PX))
                        .child(header),
                ),
        );
    }
    primary = primary.child(
        div()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .debug_selector(|| LEGACY_SHELL_CONTENT_SELECTOR.to_string())
            .child(content),
    );

    let mut surfaces = div()
        .flex()
        .flex_row()
        .h_full()
        .min_h(px(0.0))
        .gap(style.surface_gap)
        .child(primary);
    if let (Some(width), Some(secondary)) = (props.inspector_width_px, props.secondary) {
        surfaces = surfaces.child(
            div()
                .w(px(width))
                .flex_shrink_0()
                .min_h(px(0.0))
                .rounded(style.panel_radius)
                .bg(style.panel_background)
                .p(px(LEGACY_PANEL_PADDING_PX))
                .shadow(vec![card.ring()])
                .debug_selector(|| LEGACY_SHELL_INSPECTOR_SELECTOR.to_string())
                .child(secondary),
        );
    }

    let root_selector = format!(
        "{} {}",
        LEGACY_SHELL_FRAME_SELECTOR,
        legacy_shell_prose_selector(props.prose_width)
    );
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_h(px(0.0))
        .bg(style.window_background)
        .debug_selector(|| root_selector.clone())
        .child(shell_title_bar(
            theme,
            props.title_header,
            props.inspector_width_px,
        ))
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .flex()
                .flex_row()
                .child(shell_rail_column(theme, props.identity))
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .max_h_full()
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .pt(px(LEGACY_MAIN_PADDING_PX))
                        .pr(px(LEGACY_MAIN_PADDING_PX))
                        .pb(px(LEGACY_MAIN_PADDING_PX))
                        .child(surfaces),
                ),
        )
}

#[cfg(test)]
mod legacy_shell_tests {
    use super::*;
    use gpui::{Context, IntoElement as _, Render, TestAppContext, Window};

    #[test]
    fn monogram_prefers_display_name_then_hostname() {
        let named = RailIdentity::new(Some("sander"), Some("host"));
        assert_eq!(rail_identity_monogram(named), "S");
        let host_only = RailIdentity::new(None, Some("forge"));
        assert_eq!(rail_identity_monogram(host_only), "F");
        assert_eq!(
            rail_identity_monogram(RailIdentity::new(None, None)),
            RAIL_IDENTITY_FALLBACK_LABEL
        );
        assert_eq!(
            rail_identity_monogram(RailIdentity::new(Some(""), Some(""))),
            RAIL_IDENTITY_FALLBACK_LABEL
        );
    }

    #[test]
    fn prose_selector_names_each_column() {
        assert_eq!(
            legacy_shell_prose_selector(ProseWidth::Tight),
            "legacy-shell-prose-tight"
        );
        assert_eq!(
            legacy_shell_prose_selector(ProseWidth::Balanced),
            "legacy-shell-prose-balanced"
        );
        assert_eq!(
            legacy_shell_prose_selector(ProseWidth::Loose),
            "legacy-shell-prose-loose"
        );
    }

    #[test]
    fn gradient_seed_hash_matches_fnv1a_vectors() {
        assert_eq!(gradient_avatar_seed_hash(""), 0x811c_9dc5);
        assert_eq!(gradient_avatar_seed_hash("foobar"), 0xbf9c_f968);
    }

    #[test]
    fn gradient_bucket_stays_inside_the_palette() {
        for seed in ["", "forge", "sander", "host-01", "☃"] {
            assert!(gradient_avatar_bucket(seed) < GRADIENT_AVATAR_PALETTE.len());
        }
        assert_eq!(
            gradient_avatar_bucket("forge"),
            gradient_avatar_bucket("forge")
        );
    }

    #[test]
    fn gradient_palette_matches_the_theme_tokens() {
        // Independent pin: several transcribed pairs back semantic theme
        // tokens transcribed by an earlier packet from the same legacy
        // sources, so both transcriptions must agree exactly.
        let light = ArtisanTheme::for_mode(ThemeMode::Light);
        let dark = ArtisanTheme::for_mode(ThemeMode::Dark);
        assert_eq!(GRADIENT_AVATAR_PALETTE[0].0, light.colors.destructive);
        assert_eq!(GRADIENT_AVATAR_PALETTE[0].1, dark.colors.destructive);
        assert_eq!(GRADIENT_AVATAR_PALETTE[13].0, light.colors.question_from);
        assert_eq!(GRADIENT_AVATAR_PALETTE[13].1, light.colors.question_to);
        assert_eq!(GRADIENT_AVATAR_PALETTE[3].0, light.colors.banner_warning);
    }

    #[test]
    fn gradient_dither_runs_dark_to_light_on_the_diagonal() {
        // Bottom-left stays the 600 base; top-right lights up; the middle is
        // past the Bayer threshold, matching the documented ramp direction.
        assert!(!gradient_avatar_cell_lit(0, 15));
        assert!(gradient_avatar_cell_lit(15, 0));
        assert!(gradient_avatar_cell_lit(8, 8));
        assert!(!gradient_avatar_cell_lit(16, 0));
    }

    #[test]
    fn gradient_seed_prefers_the_machine_over_the_person() {
        assert_eq!(
            gradient_avatar_seed(RailIdentity::new(Some("sander"), Some("forge"))),
            Some("forge")
        );
        assert_eq!(
            gradient_avatar_seed(RailIdentity::new(Some("sander"), None)),
            Some("sander")
        );
        assert_eq!(
            gradient_avatar_seed(RailIdentity::new(Some(""), Some(""))),
            None
        );
        assert_eq!(gradient_avatar_seed(RailIdentity::new(None, None)), None);
    }

    #[test]
    fn trailing_spacer_replays_inspector_column_plus_gap() {
        assert_eq!(
            title_bar_trailing_spacer_px(None),
            LEGACY_TITLE_BAR_SPACER_PX
        );
        assert_eq!(
            title_bar_trailing_spacer_px(Some(256.0)),
            256.0 + LEGACY_INSPECTOR_TITLE_GAP_PX
        );
    }

    /// Minimal host mounting the rail column so the paint tree can be
    /// inspected: the column is a bare recipe, not a view.
    struct RailProbe {
        theme: ArtisanTheme,
        identity: RailIdentity<'static>,
    }

    impl Render for RailProbe {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> impl gpui::IntoElement {
            shell_rail_column(self.theme, self.identity)
        }
    }

    /// The rail lays out its brand action, marketplace button, and account
    /// avatar for both the signed-out rail and a seeded identity: every
    /// placeholder this packet replaces must be present in the paint tree at
    /// its legacy geometry (32 px actions, 40 px avatar pinned 8 px above
    /// the rail's bottom edge).
    #[gpui::test]
    fn rail_lays_out_brand_marketplace_and_avatar(cx: &mut TestAppContext) {
        for identity in [
            RailIdentity::new(None, None),
            RailIdentity::new(Some("sander"), Some("forge")),
        ] {
            let (_view, cx) = cx.add_window_view(|_, _| RailProbe {
                theme: ArtisanTheme::for_mode(ThemeMode::Dark),
                identity,
            });
            cx.run_until_parked();
            let column = cx
                .debug_bounds(LEGACY_SHELL_RAIL_COLUMN_SELECTOR)
                .expect("rail column lays out");
            let brand = cx
                .debug_bounds(LEGACY_SHELL_RAIL_BRAND_SELECTOR)
                .expect("brand action lays out");
            let marketplace = cx
                .debug_bounds(LEGACY_SHELL_RAIL_MARKETPLACE_SELECTOR)
                .expect("marketplace button lays out");
            let avatar = cx
                .debug_bounds(LEGACY_SHELL_RAIL_AVATAR_SELECTOR)
                .expect("account avatar lays out");
            for (edge, actual) in [
                (LEGACY_RAIL_BUTTON_PX, f32::from(brand.size.width)),
                (LEGACY_RAIL_BUTTON_PX, f32::from(brand.size.height)),
                (LEGACY_RAIL_BUTTON_PX, f32::from(marketplace.size.width)),
                (LEGACY_RAIL_BUTTON_PX, f32::from(marketplace.size.height)),
                (LEGACY_IDENTITY_AVATAR_PX, f32::from(avatar.size.width)),
                (LEGACY_IDENTITY_AVATAR_PX, f32::from(avatar.size.height)),
            ] {
                assert!(
                    (actual - edge).abs() < 0.5,
                    "expected {edge}px, laid out {actual}px"
                );
            }
            let column_bottom = f32::from(column.origin.y) + f32::from(column.size.height);
            let avatar_bottom = f32::from(avatar.origin.y) + f32::from(avatar.size.height);
            assert!(
                (avatar_bottom - (column_bottom - LEGACY_RAIL_CLUSTER_INSET_PX)).abs() < 1.0,
                "avatar bottom {avatar_bottom}px should sit {LEGACY_RAIL_CLUSTER_INSET_PX}px above the column bottom {column_bottom}px"
            );
        }
    }
}
