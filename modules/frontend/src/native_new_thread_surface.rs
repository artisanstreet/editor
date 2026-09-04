//! New-thread surface: sentence heading, recents panel, usage grid, composer bar.
//!
//! Native counterpart of `routes/components/new-thread-route.svelte` minus its
//! Effect controllers and streams, which the application owns. The same
//! surface serves `/` and `/t/<workspace>` (`routes/+page.svelte` renders
//! `NewThreadRoute` with no workspace; `routes/t/[workspace]/+page.svelte`
//! renders it with the URL segment as the project answer); the route question
//! is answered by the caller, never here.
//!
//! Layout contract, in legacy order (`new-thread-route.svelte:338-423`):
//!
//! - The surface owns the composer's prose column and width bound
//!   (`prose-column` at `max-w-(--prose-width)`), centred in the frame, so
//!   the panel and the composer bar read as one unit.
//! - The sentence sits above the panel (`routes/+page.svelte` names the
//!   project through it on `/`).
//! - The panel is a `2fr / 1fr` split with no drawn container: the recents
//!   list scrolls in the left pane, the year of token spend
//!   (`VerticalCalendarActivityGrid`) fills the right pane, and the panes are
//!   told apart by their contents and spacing alone
//!   (`new-thread-route.svelte:354-358`).
//! - The composer bar closes the column: editor area with placeholder above
//!   the controls row carrying the model-selector trigger and the send
//!   control (`thread-composer.svelte`, `composer/controls.svelte`).
//!
//! Data fetching, draft control, policy state, and navigation stay out. The
//! usage grid renders caller-supplied day totals through
//! [`render_activity_grid`]; with no days it draws the full zero-token year,
//! which is exactly what the legacy surface leaves standing when Forge cannot
//! answer (`new-thread-route.svelte:308-317`). The composer bar is a static
//! visual: the interactive composer is mounted separately by the application,
//! so this bar owns no draft, focus, submission gate, or policy behavior.

use artisan_assets::AssetId;
use artisan_ui::{
    icon::{IconSize, IconStyle, IconTint, icon},
    list_row::{
        ListRowContent, ListRowGeometry, ListRowSlots, ListRowStyle, ListRowTone, list_row,
    },
    theme::{ArtisanTheme, RadiusStep, RadiusTokens},
};
use gpui::{
    Div, FontWeight, Hsla, Pixels, SharedString, StatefulInteractiveElement as _, div,
    prelude::{InteractiveElement as _, ParentElement as _, Styled as _},
    px, relative,
};

use crate::onboarding_screen::brand_asset_for;

/// Debug selector for the surface root.
pub const NEW_THREAD_SURFACE_SELECTOR: &str = "native-new-thread-surface";
/// Debug selector for the sentence heading.
pub const NEW_THREAD_SENTENCE_SELECTOR: &str = "native-new-thread-sentence";
/// Debug selector for the recents list container.
pub const NEW_THREAD_RECENTS_SELECTOR: &str = "native-new-thread-recents";
/// Debug selector for the recents/usage panel.
pub const NEW_THREAD_PANEL_SELECTOR: &str = "native-new-thread-panel";
/// Debug selector for the token-usage pane (`aria-label="Token usage"` in
/// `new-thread-route.svelte:417`).
pub const NEW_THREAD_USAGE_SELECTOR: &str = "native-new-thread-usage";
/// Debug selector for the static composer bar.
pub const NEW_THREAD_COMPOSER_BAR_SELECTOR: &str = "native-new-thread-composer-bar";
/// Debug selector for the static send control.
pub const NEW_THREAD_COMPOSER_SEND_SELECTOR: &str = "native-new-thread-composer-send";
/// Empty-recents copy, byte-identical to the legacy surface
/// (`new-thread-route.svelte:378`).
pub const NEW_THREAD_EMPTY_RECENTS: &str = "No threads yet.";
/// Composer editor placeholder. The interactive composer paints
/// `NATIVE_COMPOSER_PLACEHOLDER` (`native_composer.rs`); the static bar shows
/// the same phrase so the two never disagree.
pub const NEW_THREAD_COMPOSER_PLACEHOLDER: &str = "Do anything";
/// Model-selector trigger label. The legacy trigger names its action
/// (`model-selector/view.svelte:484`, `aria-label="Select model"`); with no
/// session policy resolved here, the bar states the action rather than
/// inventing a model name.
pub const NEW_THREAD_MODEL_TRIGGER_LABEL: &str = "Select model";

/// Prose-column width bound: `--prose-width: 48rem` at the 16 px root
/// (`theme.css:157`).
pub const NEW_THREAD_PROSE_WIDTH_PX: f32 = 768.0;
/// Panel width: `w-4/5` on the legacy grid (`new-thread-route.svelte:373`).
pub const NEW_THREAD_PANEL_WIDTH_FRACTION: f32 = 0.8;
/// Panel height: `aspect-3/2` at the full prose width, `0.8 * 768 * 2/3`
/// (`new-thread-route.svelte:373`). A static tree cannot keep a ratio, so the
/// height is pinned to the ratio's value at the width bound and documented
/// here: on narrower viewports the panel keeps this height while its width
/// shrinks, where the legacy grid would keep the ratio instead.
pub const NEW_THREAD_PANEL_HEIGHT_PX: f32 = 410.0;
/// Recents pane share of the `grid-cols-[minmax(0,2fr)_minmax(0,1fr)]` split
/// (`new-thread-route.svelte:373`).
pub const NEW_THREAD_LIST_SHARE: f32 = 2.0 / 3.0;
/// Usage pane share of the same split.
pub const NEW_THREAD_USAGE_SHARE: f32 = 1.0 / 3.0;
/// Engine-mark edge: `size-4` on the row mark (`new-thread-route.svelte:395`)
/// and on the model-trigger mark (`model-selector/view.svelte:491`).
pub const NEW_THREAD_ENGINE_MARK_EDGE_PX: f32 = 16.0;
/// Composer editor minimum height: `min-h-16` (`thread-composer.svelte:567`).
pub const NEW_THREAD_COMPOSER_EDITOR_MIN_H_PX: f32 = 64.0;
/// Composer card minimum height: `min-h-32` (`thread-composer.svelte:551`).
pub const NEW_THREAD_COMPOSER_CARD_MIN_H_PX: f32 = 128.0;

/// Days of token spend behind the usage grid: `usage_day_count`
/// (`new-thread-route.svelte:276`).
pub const ACTIVITY_DAY_COUNT: usize = 365;
/// Largest day cell: `MaximumCellSize`
/// (`vertical-calendar-activity-grid.svelte:30`).
pub const ACTIVITY_CELL_MAX_PX: f32 = 12.0;
/// Smallest day cell: `MinimumCellSize` (`vertical-calendar-activity-grid.svelte:31`).
pub const ACTIVITY_CELL_MIN_PX: f32 = 3.0;
/// Gap between day cells: `CellGap` (`vertical-calendar-activity-grid.svelte:32`).
pub const ACTIVITY_CELL_GAP_PX: f32 = 2.0;

/// One day of token spend feeding the usage grid, oldest first like the
/// legacy `activities` prop. Only the total paints a cell; per-engine slices
/// own the hover tooltip, which has no static counterpart here.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CalendarActivityDay {
    /// Total input plus output tokens for the day.
    pub tokens: u64,
}

impl CalendarActivityDay {
    /// Builds one day from its token total.
    #[must_use]
    pub const fn new(tokens: u64) -> Self {
        Self { tokens }
    }
}

/// Day-cell corner radius: `Math.min(2, cell_size / 3)` from the canvas
/// `roundRect` call (`vertical-calendar-activity-grid.svelte:130`).
#[must_use]
pub fn activity_cell_radius(cell_px: f32) -> Pixels {
    px((cell_px / 3.0).min(2.0))
}

/// Spend bucket for one day: `0` for a zero-token day, otherwise
/// `Math.max(1, Math.ceil(Math.sqrt(tokens / max) * 4))`, giving `1..=4`
/// (`vertical-calendar-activity-grid.svelte:123`). A zero maximum means no
/// day spent anything, so every day reads as empty.
///
/// The comparison runs in integer arithmetic: bucket `k` (`1..=3`) holds
/// exactly when `(k-1) < 4*sqrt(tokens/max) <= k`, i.e. `(k-1)^2*max <
/// 16*tokens <= k^2*max`, so no floating-point rounding can shift a boundary.
#[must_use]
pub fn activity_intensity(tokens: u64, max_tokens: u64) -> u8 {
    if tokens == 0 || max_tokens == 0 {
        return 0;
    }
    let scaled = u128::from(tokens) * 16;
    let max = u128::from(max_tokens);
    if scaled <= max {
        1
    } else if scaled <= max * 4 {
        2
    } else if scaled <= max * 9 {
        3
    } else {
        4
    }
}

/// Paint for one spend bucket from the shared chart ramp.
///
/// The canvas reads `--chart-5` down to `--chart-1` into `activity_colors[0]`
/// through `[4]` and fills cell `(intensity)` with that slot
/// (`vertical-calendar-activity-grid.svelte:85-88,127-128`), so bucket `b`
/// paints `charts[4 - b]`: empty days the darkest ramp stop, the heaviest day
/// the lightest.
#[must_use]
pub fn activity_bucket_paint(theme: ArtisanTheme, intensity: u8) -> Hsla {
    let slot = 4_usize.saturating_sub(usize::from(intensity.min(4)));
    theme.colors.charts[slot].to_paint()
}

/// Cell edge the canvas chooses for a host height: the largest size that fits
/// four rows, clamped to the minimum/maximum
/// (`vertical-calendar-activity-grid.svelte:95`).
#[must_use]
pub fn activity_cell_size_for_height(height_px: f32) -> f32 {
    ((height_px / 4.0).floor() - ACTIVITY_CELL_GAP_PX)
        .clamp(ACTIVITY_CELL_MIN_PX, ACTIVITY_CELL_MAX_PX)
}

/// Column count the canvas fits into a host width:
/// `Math.max(1, Math.floor((width + CellGap) / step))`
/// (`vertical-calendar-activity-grid.svelte:97`).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#[must_use]
pub fn activity_columns_for_width(width_px: f32, cell_px: f32) -> usize {
    // The quotient is floored and clamped to at least one before the cast;
    // a non-finite input collapses to one through `max`, and a huge quotient
    // saturates rather than wrapping.
    ((width_px + ACTIVITY_CELL_GAP_PX) / (cell_px + ACTIVITY_CELL_GAP_PX))
        .floor()
        .max(1.0) as usize
}

/// Row count the canvas fits into a host height:
/// `Math.max(1, Math.floor((height + CellGap) / step))`
/// (`vertical-calendar-activity-grid.svelte:98`).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#[must_use]
pub fn activity_rows_for_height(height_px: f32, cell_px: f32) -> usize {
    // Same flooring and clamping contract as
    // [`activity_columns_for_width`].
    ((height_px + ACTIVITY_CELL_GAP_PX) / (cell_px + ACTIVITY_CELL_GAP_PX))
        .floor()
        .max(1.0) as usize
}

/// Renders the token-spend grid: newest day first at the top-left, filling
/// row-major exactly like the canvas offsets
/// (`offset = row * column_count + column` from the end date,
/// `vertical-calendar-activity-grid.svelte:94,115-125`).
///
/// The canvas measures its host and redraws on resize; a static element tree
/// cannot, so cells wrap at the pane width instead, and the grid box fills
/// its pane and clips: like the canvas filling exactly its host, only whole
/// visible rows paint. At the maximum cell size the wrap order matches the
/// canvas fill order. Days beyond [`ACTIVITY_DAY_COUNT`] are dropped (oldest
/// first); missing days paint as zero-token cells, which is also the
/// fresh-install shape: the grid always draws its full year
/// (`new-thread-route.svelte:280-284`).
#[must_use]
pub fn render_activity_grid(theme: ArtisanTheme, days: &[CalendarActivityDay]) -> Div {
    let kept: Vec<CalendarActivityDay> = days
        .iter()
        .rev()
        .take(ACTIVITY_DAY_COUNT)
        .copied()
        .collect();
    let max_tokens = kept.iter().map(|day| day.tokens).max().unwrap_or(0);
    let mut grid = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .w_full()
        .h_full()
        .overflow_hidden()
        .gap(px(ACTIVITY_CELL_GAP_PX));
    for day in &kept {
        let paint = activity_bucket_paint(theme, activity_intensity(day.tokens, max_tokens));
        grid = grid.child(
            div()
                .size(px(ACTIVITY_CELL_MAX_PX))
                .rounded(activity_cell_radius(ACTIVITY_CELL_MAX_PX))
                .bg(paint),
        );
    }
    for _ in kept.len()..ACTIVITY_DAY_COUNT {
        grid = grid.child(
            div()
                .size(px(ACTIVITY_CELL_MAX_PX))
                .rounded(activity_cell_radius(ACTIVITY_CELL_MAX_PX))
                .bg(activity_bucket_paint(theme, 0)),
        );
    }
    grid
}

/// Resolves the provider mark for one engine id, reusing the harness table
/// shared with onboarding.
///
/// The mapping mirrors `EngineMarkFor` (`lib/engine/presentation.ts`): Codex
/// wears the `OpenAI` mark, Claude/Cursor/Grok wear their product marks, and
/// `OpenCode`/`Hermes` wear their brand marks. Unknown ids fall back to the
/// neutral question-mark placeholder, matching `unknown_engine_mark`. The
/// table itself lives in [`brand_asset_for`] so the two surfaces can never
/// diverge; this alias names the recent-row slot.
#[must_use]
pub fn engine_mark_asset(engine_id: &str) -> AssetId {
    brand_asset_for(engine_id)
}

/// One caller-formatted recent-thread row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewThreadRecentRow {
    /// Display title for the row.
    pub title: SharedString,
    /// Relative-time detail (already formatted by the caller with
    /// `format_recent_thread_time`, matching `FormatRecentThreadTime`).
    pub detail: SharedString,
    /// Stable per-row selector suffix.
    pub selector_suffix: String,
    /// Engine-mark asset for the leading mark (the thread's engine identity
    /// resolved through [`engine_mark_asset`]). `None` renders no mark: the
    /// domain `ThreadSummary` projection carries no engine identity, so the
    /// application cannot supply one yet.
    pub engine_mark: Option<AssetId>,
}

impl NewThreadRecentRow {
    /// Builds a row from display parts, with no engine mark.
    #[must_use]
    pub fn new(
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        selector_suffix: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            selector_suffix: selector_suffix.into(),
            engine_mark: None,
        }
    }

    /// Attaches the engine mark, resolving the engine id to its provider
    /// asset exactly like the legacy row's `EngineMarkFor(thread.engine_id)`
    /// (`new-thread-route.svelte:385,395`).
    #[must_use]
    pub fn with_engine_mark(mut self, engine_id: &str) -> Self {
        self.engine_mark = Some(engine_mark_asset(engine_id));
        self
    }
}

/// Builds the stable selector for one recent row under `root`.
#[must_use]
pub fn recent_row_selector(root: &str, suffix: &str) -> String {
    format!("{root}-recent-{suffix}")
}

/// Renders one recent row: engine mark, truncated muted title, muted
/// relative-time caption pinned right (`new-thread-route.svelte:387-402`).
/// Titles stay muted until hovered in the legacy surface; there is no hover
/// pill in a static tree, so rows resolve the muted rail tone directly.
fn render_recent_row(
    theme: &ArtisanTheme,
    row_style: ListRowStyle,
    row: &NewThreadRecentRow,
    root_selector: &str,
) -> Div {
    let selector = recent_row_selector(root_selector, &row.selector_suffix);
    let mut slots = ListRowSlots::new().trailing_caption(row.detail.clone());
    if let Some(asset) = &row.engine_mark {
        // The legacy row always paints the mark at `size-4`
        // (`EngineMarkClass(thread_mark, "size-4 shrink-0")`); only the
        // unknown-engine fallback wears the muted tone
        // (`unknown_engine_mark.muted`), everything else inherits.
        let tint = if *asset == AssetId::TABLER_QUESTION_MARK {
            IconTint::Muted
        } else {
            IconTint::Inherit
        };
        slots = slots.leading(icon(IconStyle::resolve(
            *theme,
            *asset,
            IconSize::Default,
            tint,
        )));
    }
    list_row(
        row_style,
        ListRowContent::one_line(row.title.clone()),
        slots,
    )
    .debug_selector(move || selector.clone())
}

/// Renders the static composer bar closing the column: card, editor area with
/// placeholder, and the controls row with the model-selector trigger and the
/// send control.
///
/// Geometry follows `thread-composer.svelte:550-607`: the card wears
/// `--radius-2xl` with the nested `--radius-gap` arithmetic, the editor area
/// keeps `min-h-16` inside the `min-h-32` card with `px-3 py-2 text-base`, and
/// the send control is the ghost icon control with the `ArrowUp` face and the
/// `Send message` name (`composer/controls.svelte`, `SendButtonStill`).
/// The trigger keeps the legacy `h-8 px-2` shape with the `Select model`
/// action label plus the trailing `Selector` chevron at `size-3.5` in the
/// muted tone (`model-selector/view.svelte:483-511`); the trigger's engine
/// mark stays out because with no session policy resolved here there is no
/// honest model to name it for. The bar owns no draft, focus, or submission
/// behavior.
#[must_use]
pub fn render_new_thread_composer_bar(theme: ArtisanTheme) -> Div {
    let card_radius = RadiusTokens::value(RadiusStep::X2l);
    let nested_radius = RadiusTokens::nested(card_radius, theme.spacing.steps(2.0));
    let send_glyph = IconStyle::resolve(
        theme,
        AssetId::TABLER_ARROW_UP,
        IconSize::Default,
        IconTint::Muted,
    );
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_h(px(NEW_THREAD_COMPOSER_CARD_MIN_H_PX))
        .p(theme.spacing.steps(2.0))
        .rounded(card_radius)
        .border_1()
        .border_color(theme.colors.border.to_paint())
        .bg(theme.colors.card.to_paint())
        .debug_selector(|| NEW_THREAD_COMPOSER_BAR_SELECTOR.to_string())
        .child(
            div()
                .flex_1()
                .min_h(px(NEW_THREAD_COMPOSER_EDITOR_MIN_H_PX))
                .px(theme.spacing.steps(3.0))
                .py(theme.spacing.steps(2.0))
                .text_size(theme.typography.editor_text_base)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(NEW_THREAD_COMPOSER_PLACEHOLDER),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(theme.spacing.steps(2.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(theme.spacing.steps(8.0))
                        .px(theme.spacing.steps(2.0))
                        .gap(theme.spacing.steps(2.0))
                        .rounded(nested_radius)
                        .text_size(theme.typography.control_text)
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(NEW_THREAD_MODEL_TRIGGER_LABEL)
                        .child(icon(IconStyle::resolve(
                            theme,
                            AssetId::TABLER_SELECTOR,
                            IconSize::Compact,
                            IconTint::Muted,
                        ))),
                )
                .child(
                    div()
                        .rounded(nested_radius)
                        .p(theme.spacing.steps(1.0))
                        .debug_selector(|| NEW_THREAD_COMPOSER_SEND_SELECTOR.to_string())
                        .child(icon(send_glyph)),
                ),
        )
}

/// Renders the centered new-thread surface: sentence heading, recents/usage
/// panel, and composer bar (or the empty-recents copy inside the list pane).
/// Resolves all paint, spacing, and typography from the one shared theme
/// argument.
///
/// The column keeps the legacy `gap-3` rhythm; the panel holds the
/// `2fr / 1fr` split with the list scrolling in its pane (`p-1`,
/// `overflow-y-auto`) and the grid filling its pane (`p-2`). No border or
/// background is drawn around or between the panes: like the legacy grid,
/// the two are told apart by their contents and spacing. The composer bar is
/// in-flow in this tree (rather than floating with a reserved `pb-44`
/// footprint) because the application mounts no separate composer on this
/// route.
#[must_use]
pub fn render_new_thread_surface(
    theme: ArtisanTheme,
    sentence: &str,
    rows: &[NewThreadRecentRow],
    root_selector: &str,
) -> Div {
    let row_style = ListRowStyle::resolve(
        theme,
        ListRowGeometry::Rail,
        ListRowTone::Muted,
        FontWeight::MEDIUM,
    );
    let sentence_row = div()
        .text_size(theme.typography.dialog_title_text)
        .text_color(theme.colors.foreground.to_paint())
        .debug_selector(|| NEW_THREAD_SENTENCE_SELECTOR.to_string())
        .child(SharedString::from(sentence.to_owned()));

    let mut recents = div()
        .id("native-new-thread-recents-scroll")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p(px(4.0))
        .debug_selector(|| NEW_THREAD_RECENTS_SELECTOR.to_string());
    if rows.is_empty() {
        recents = recents.child(
            div()
                .px(theme.spacing.steps(2.0))
                .py(theme.spacing.steps(2.0))
                .text_size(theme.typography.control_text)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(NEW_THREAD_EMPTY_RECENTS),
        );
    }
    for row in rows {
        recents = recents.child(render_recent_row(&theme, row_style, row, root_selector));
    }

    let panel = div()
        .flex()
        .flex_row()
        .w(relative(NEW_THREAD_PANEL_WIDTH_FRACTION))
        .h(px(NEW_THREAD_PANEL_HEIGHT_PX))
        .min_h(px(0.0))
        .debug_selector(|| NEW_THREAD_PANEL_SELECTOR.to_string())
        .child(
            div()
                .flex()
                .flex_col()
                .h_full()
                .min_w(px(0.0))
                .flex_basis(relative(NEW_THREAD_LIST_SHARE))
                .flex_grow_1()
                .child(recents),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .h_full()
                .min_w(px(0.0))
                .flex_basis(relative(NEW_THREAD_USAGE_SHARE))
                .flex_grow_1()
                .p(theme.spacing.steps(2.0))
                .debug_selector(|| NEW_THREAD_USAGE_SELECTOR.to_string())
                .child(render_activity_grid(theme, &[])),
        );

    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .size_full()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .debug_selector(|| root_selector.to_owned())
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .w_full()
                .max_w(px(NEW_THREAD_PROSE_WIDTH_PX))
                .gap(theme.spacing.steps(3.0))
                .child(sentence_row)
                .child(panel)
                .child(render_new_thread_composer_bar(theme)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_selectors_nest_under_root() {
        assert_eq!(
            recent_row_selector(NEW_THREAD_SURFACE_SELECTOR, "abc"),
            "native-new-thread-surface-recent-abc".to_owned()
        );
    }

    #[test]
    fn rows_retain_display_parts() {
        let row = NewThreadRecentRow::new("Title", "2h", "row-0");
        assert_eq!(row.title.as_ref(), "Title");
        assert_eq!(row.detail.as_ref(), "2h");
        assert_eq!(row.selector_suffix, "row-0");
        assert_eq!(row.engine_mark, None);
    }

    #[test]
    fn empty_copy_matches_legacy_surface() {
        assert_eq!(NEW_THREAD_EMPTY_RECENTS, "No threads yet.");
    }

    #[test]
    fn engine_mark_builder_resolves_provider_marks() {
        let row = NewThreadRecentRow::new("Title", "2h", "row-0").with_engine_mark("codex");
        assert_eq!(row.engine_mark, Some(AssetId::SVGL_OPENAI));
        let row = NewThreadRecentRow::new("Title", "2h", "row-0").with_engine_mark("mystery");
        assert_eq!(row.engine_mark, Some(AssetId::TABLER_QUESTION_MARK));
    }

    #[test]
    fn engine_mark_assets_cover_the_legacy_engine_table() {
        assert_eq!(engine_mark_asset("codex"), AssetId::SVGL_OPENAI);
        assert_eq!(engine_mark_asset("claude"), AssetId::SVGL_CLAUDE_AI);
        assert_eq!(engine_mark_asset("cursor"), AssetId::SVGL_CURSOR);
        assert_eq!(engine_mark_asset("grok"), AssetId::SVGL_GROK);
        assert_eq!(engine_mark_asset("opencode2"), AssetId::BRANDS_OPENCODE);
        assert_eq!(engine_mark_asset("hermes"), AssetId::BRANDS_HERMES);
        assert_eq!(engine_mark_asset("unknown"), AssetId::TABLER_QUESTION_MARK);
    }

    #[test]
    fn activity_intensity_matches_canvas_buckets() {
        assert_eq!(activity_intensity(0, 100), 0);
        assert_eq!(activity_intensity(100, 0), 0);
        assert_eq!(activity_intensity(1, 10_000), 1);
        assert_eq!(activity_intensity(625, 10_000), 1);
        assert_eq!(activity_intensity(10_000, 10_000), 4);
        assert_eq!(activity_intensity(2_500, 10_000), 2);
    }

    #[test]
    fn activity_cell_radius_matches_round_rect() {
        assert_eq!(activity_cell_radius(12.0), px(2.0));
        assert_eq!(activity_cell_radius(3.0), px(1.0));
    }

    #[test]
    fn activity_grid_dimensions_match_canvas_math() {
        assert_eq!(activity_cell_size_for_height(300.0), 12.0);
        assert_eq!(activity_cell_size_for_height(20.0), 3.0);
        // floor((200 + 2) / (12 + 2)) = 14 columns.
        assert_eq!(activity_columns_for_width(200.0, 12.0), 14);
        // floor((410 + 2) / (12 + 2)) = 29 rows.
        assert_eq!(activity_rows_for_height(410.0, 12.0), 29);
    }

    #[test]
    fn panel_split_keeps_legacy_two_to_one_ratio() {
        assert!((NEW_THREAD_LIST_SHARE + NEW_THREAD_USAGE_SHARE - 1.0).abs() < f32::EPSILON);
        assert!((NEW_THREAD_PANEL_WIDTH_FRACTION - 0.8).abs() < f32::EPSILON);
        assert_eq!(NEW_THREAD_PROSE_WIDTH_PX, 768.0);
        // 0.8 * 768 * 2/3, rounded to the whole pixel.
        assert_eq!(NEW_THREAD_PANEL_HEIGHT_PX, 410.0);
        assert_eq!(ACTIVITY_DAY_COUNT, 365);
    }

    #[test]
    fn composer_copy_matches_reached_surfaces() {
        assert_eq!(NEW_THREAD_COMPOSER_PLACEHOLDER, "Do anything");
        assert_eq!(NEW_THREAD_MODEL_TRIGGER_LABEL, "Select model");
    }
}
