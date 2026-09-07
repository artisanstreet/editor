//! Noninteractive list-row presentation primitive for the reached selection
//! surfaces.
//!
//! Ported from the two audited row families recorded in INVENTORY §6.2–§6.3
//! and expressed as one shared skeleton with two typed geometries:
//!
//! - **Menu rows** are the composed dropdown-menu item defaults overridden at
//!   the picker call site: the shared item recipe supplies the 10 px child gap
//!   (`gap-2.5`), the 14 px `--radius-xl` corners, and 14 px `text-sm` type
//!   while the call site overrides padding to 8 px horizontal / 6 px vertical
//!   (`px-2 py-1.5`) and strips every highlight paint
//!   (`project-selector.svelte:186,227` composing
//!   `dropdown-menu-item.svelte:23`). The optional second line is the compact
//!   path: the mono face at the audited 11 px arbitrary size in muted
//!   foreground (`project-selector.svelte:201`).
//! - **Rail rows** are the sidebar thread rows: 8 px child gap, 8 px
//!   horizontal and vertical padding, 10 px `--radius-lg` corners, and 14 px
//!   `text-sm` type (`thread-hover-rail.svelte:473` recent rows and :592
//!   working rows share this geometry). Recent rows add the `font-medium`
//!   title and a 12 px muted trailing time (`text-xs text-muted-foreground`);
//!   working rows add a 12 px muted project subtitle (:609).
//!
//! All rows stay width-constrained and truncation-safe exactly as audited:
//! leading and trailing content refuses to shrink, the center content takes
//! `min-width: 0` with `flex: 1` and truncates, and the one-line/two-line
//! shapes are chosen explicitly through [`ListRowContent`] rather than
//! inferred from any domain value. Leading marks, chosen checks, engine
//! marks, and state dots are caller-owned elements supplied through
//! [`ListRowSlots`]; the primitive owns no icons or assets.
//!
//! Deliberately absent because the audit assigns them elsewhere: every
//! interaction and moving behavior around these rows — pointer/hover-pill
//! machinery (`dropdown-hover-surface.svelte`), the sliding active-thread
//! light (`active-thread-light.svelte`), navigation and activation, context
//! menus, selection, command-menu ranking, tooltips, focus, and animation —
//! belongs to callers or later interaction primitives. This module contains
//! no event listeners, no [`gpui::FocusHandle`], no disabled policy, and no
//! product types; visual state enters only as typed presentation data
//! ([`ListRowTone`]).
//!
//! Callers resolve [`ListRowStyle::resolve`] once for their mode, geometry,
//! tone, and title weight, then pass the recipe with explicit content and
//! slots to [`list_row`]; further [`gpui::Styled`] refinements chain onto the
//! returned [`Div`] and later values win, keeping audited caller-side
//! adjustments reachable without new APIs.
//!
//! ## Known rendering limitations (pinned GPUI 0.2.2)
//!
//! - **No rounded child clipping.** As documented for the card primitive,
//!   corner radii round painted backgrounds but children clip to straight
//!   edges. Row-level clipping is therefore deliberately *not* installed:
//!   each text line truncates inside the padded content area, so reached
//!   content never reaches the rounded corners.
//! - **Ellipsis truncation** relies on GPUI's `Styled::truncate`
//!   (`overflow_hidden` + `whitespace_nowrap` + `TextOverflow::Truncate`);
//!   the pinned renderer supports it, and external tests pin the resulting
//!   single-line heights under width pressure.

use gpui::{
    AnyElement, Div, FontWeight, Hsla, IntoElement, ParentElement, Pixels, SharedString, Styled,
    div, px,
};

use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};

/// The audited 11 px supporting-mono size: the arbitrary length utility
/// `text-[0.6875rem]` (`project-selector.svelte:201`, 0.6875 rem at the
/// 16 px root). The shared theme intentionally carries only the named
/// Tailwind text roles ([`crate::theme::TypographyTokens`]), so this one
/// narrowly evidenced value is named here instead of widening the theme.
const SUPPORTING_MONO_PX: f32 = 11.0;

/// The audited row geometries sharing one presentation skeleton.
///
/// Both keep 8 px horizontal padding and 14 px control type; they differ in
/// the audited child gap, vertical padding, corner ramp step, supporting-line
/// face, and supporting-line leading.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ListRowGeometry {
    /// Anchored-menu rows (audited project picker): 10 px child gap
    /// (`gap-2.5`), 8 px horizontal / 6 px vertical padding (`px-2 py-1.5`),
    /// 14 px `--radius-xl` corners (`rounded-xl`), and the mono-faced
    /// supporting line.
    Menu,
    /// Sidebar-rail rows (audited thread lists): 8 px child gap (`gap-2`),
    /// 8 px horizontal / 8 px vertical padding (`px-2 py-2`), 10 px
    /// `--radius-lg` corners (`rounded-lg`), and the sans-faced supporting
    /// line.
    Rail,
}

/// Typed presentation tone for the row's title text.
///
/// The audit paints recent-thread titles with the shared foreground while a
/// row is current and with the muted foreground otherwise
/// (`thread-hover-rail.svelte:473`); picker and working-row titles always
/// carry the foreground. The primitive cannot know either state, so callers
/// supply the resolved tone as data.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ListRowTone {
    /// Title text paints the mode's `--foreground`.
    #[default]
    Foreground,
    /// Title text paints the mode's `--muted-foreground`.
    Muted,
}

/// Paint and geometry values resolved for one list row.
///
/// This record is public so behavioral tests and the future component gallery
/// can compare exact native semantics without reaching into GPUI internals,
/// mirroring [`crate::badge::BadgeStyle`] and [`crate::card::CardStyle`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListRowStyle {
    /// Corner radius from the legacy ramp step selected by the geometry.
    pub corner_radius: Pixels,
    /// Horizontal padding: the shared `px-2`, 8 px.
    pub horizontal_padding: Pixels,
    /// Vertical padding: `py-1.5` (6 px) for menu rows, `py-2` (8 px) for
    /// rail rows.
    pub vertical_padding: Pixels,
    /// Flex-row gap between row children: `gap-2.5` (10 px) for menu rows,
    /// `gap-2` (8 px) for rail rows.
    pub child_gap: Pixels,
    /// Row title size: the shared control type `text-sm`, 14 px.
    pub title_size: Pixels,
    /// Row title leading: the named `text-sm` pair's 1.25 rem, 20 px.
    pub title_line_height: Pixels,
    /// Title weight, supplied by the caller because the audit reaches both
    /// regular titles and the recent rows' `font-medium`.
    pub title_weight: FontWeight,
    /// Title color resolved for the theme mode and [`ListRowTone`].
    pub title_color: Hsla,
    /// Trailing caption size (the recent rows' muted time): `text-xs`, 12 px.
    pub caption_size: Pixels,
    /// Trailing caption leading: the named `text-xs` pair, 16 px.
    pub caption_line_height: Pixels,
    /// Trailing caption color: the mode's `--muted-foreground`.
    pub caption_color: Hsla,
    /// Supporting-line size: the audited 11 px mono length for menu rows,
    /// `text-xs` (12 px) for rail rows.
    pub supporting_size: Pixels,
    /// Supporting-line leading: the inherited 20 px `text-sm` leading on menu
    /// rows (an arbitrary-size utility sets no leading of its own), the named
    /// 16 px `text-xs` leading on rail rows.
    pub supporting_line_height: Pixels,
    /// Supporting-line color: the mode's `--muted-foreground`.
    pub supporting_color: Hsla,
    /// Supporting-line family: `--font-mono` for menu rows, `--font-sans` for
    /// rail rows.
    pub supporting_family: &'static str,
}

impl ListRowStyle {
    /// Resolves the exact audited recipe from shared theme tokens.
    ///
    /// `title_weight` is explicit presentation data: the audited recent rail
    /// rows pass [`FontWeight::MEDIUM`] (`font-medium`,
    /// `thread-hover-rail.svelte:473`); menu and working rows pass
    /// [`FontWeight::NORMAL`].
    #[must_use]
    pub fn resolve(
        theme: ArtisanTheme,
        geometry: ListRowGeometry,
        tone: ListRowTone,
        title_weight: FontWeight,
    ) -> Self {
        let title_color = match tone {
            ListRowTone::Foreground => theme.colors.foreground.to_paint(),
            ListRowTone::Muted => theme.colors.muted_foreground.to_paint(),
        };
        let muted_foreground = theme.colors.muted_foreground.to_paint();

        let (
            vertical_padding,
            child_gap,
            corner_step,
            supporting_size,
            supporting_line_height,
            supporting_family,
        ) = match geometry {
            ListRowGeometry::Menu => (
                theme.spacing.steps(1.5),
                theme.spacing.steps(2.5),
                RadiusStep::Xl,
                px(SUPPORTING_MONO_PX),
                theme.spacing.steps(5.0),
                theme.typography.mono.family,
            ),
            ListRowGeometry::Rail => (
                theme.spacing.steps(2.0),
                theme.spacing.steps(2.0),
                RadiusStep::Lg,
                theme.typography.label_text,
                theme.spacing.steps(4.0),
                theme.typography.sans.family,
            ),
        };

        Self {
            corner_radius: RadiusTokens::value(corner_step),
            horizontal_padding: theme.spacing.steps(2.0),
            vertical_padding,
            child_gap,
            title_size: theme.typography.control_text,
            title_line_height: theme.spacing.steps(5.0),
            title_weight,
            title_color,
            caption_size: theme.typography.label_text,
            caption_line_height: theme.spacing.steps(4.0),
            caption_color: muted_foreground,
            supporting_size,
            supporting_line_height,
            supporting_color: muted_foreground,
            supporting_family,
        }
    }
}

/// Explicit content model for a row's center text.
///
/// The caller chooses the shape; the primitive never infers one from content
/// or domain values. Both shapes truncate and own `min-width: 0` so flanking
/// content survives any width pressure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ListRowContent {
    /// One truncated title line, optionally followed by a muted trailing
    /// caption (the recent rows' formatted time) supplied through
    /// [`ListRowSlots::trailing_caption`].
    OneLine(SharedString),
    /// A truncated title above one truncated supporting line (the picker's
    /// mono-faced path, the working rows' project subtitle).
    TwoLine {
        /// The primary truncated line.
        title: SharedString,
        /// The secondary truncated line.
        supporting: SharedString,
    },
}

impl ListRowContent {
    /// Builds the one-line shape from a title.
    #[must_use]
    pub fn one_line(title: impl Into<SharedString>) -> Self {
        Self::OneLine(title.into())
    }

    /// Builds the two-line shape from a title and supporting line.
    #[must_use]
    pub fn two_line(title: impl Into<SharedString>, supporting: impl Into<SharedString>) -> Self {
        Self::TwoLine {
            title: title.into(),
            supporting: supporting.into(),
        }
    }
}

/// Caller-owned flanking content for one row.
///
/// Leading and trailing slots carry generic caller-built elements (identity
/// marks, chosen checks, engine marks, state dots); the primitive wraps them
/// only in a non-shrinking box and never interprets them. The trailing
/// caption is the one audited piece of flanking *text*: the recent rows'
/// muted time, styled by the recipe but formatted entirely by the caller.
///
/// # Examples
///
/// ```
/// use artisan_ui::list_row::ListRowSlots;
///
/// let slots = ListRowSlots::new().trailing_caption("2h");
/// ```
#[derive(Default)]
pub struct ListRowSlots {
    /// Optional element placed first, refusing to shrink.
    pub leading: Option<AnyElement>,
    /// Optional muted caption placed after the center content.
    pub trailing_caption: Option<SharedString>,
    /// Optional element placed last, refusing to shrink.
    pub trailing: Option<AnyElement>,
}

impl ListRowSlots {
    /// Creates empty slots.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Supplies the leading caller-owned element.
    #[must_use]
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }

    /// Supplies the muted trailing caption text.
    #[must_use]
    pub fn trailing_caption(mut self, caption: impl Into<SharedString>) -> Self {
        self.trailing_caption = Some(caption.into());
        self
    }

    /// Supplies the trailing caller-owned element.
    #[must_use]
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }
}

/// Returns the list row as a plain GPUI [`Div`] with its center content and
/// slots composed in audited order: leading element, center text, trailing
/// caption, trailing element.
///
/// The row fills the available width, lays its children out in a centered
/// flex row with the resolved gap, padding, and corner radius, and keeps
/// flanking slots non-shrinking while the center content absorbs pressure
/// through `min-width: 0` and truncation. Every visible line carries its
/// resolved typography and theme color explicitly. The element registers no
/// state, focus, or event handlers; callers add further [`gpui::Styled`]
/// refinements (later values override the recipe defaults) and own any
/// interaction they require.
#[must_use]
pub fn list_row(style: ListRowStyle, content: ListRowContent, slots: ListRowSlots) -> Div {
    let ListRowSlots {
        leading,
        trailing_caption,
        trailing,
    } = slots;

    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .gap(style.child_gap)
        .px(style.horizontal_padding)
        .py(style.vertical_padding)
        .rounded(style.corner_radius);

    if let Some(leading) = leading {
        row = row.child(div().flex_shrink_0().child(leading));
    }

    row = match content {
        ListRowContent::OneLine(title) => {
            // The legacy one-line row is a single span that both grows and
            // truncates (`min-w-0 flex-1 truncate`).
            row.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(style.title_size)
                    .line_height(style.title_line_height)
                    .font_weight(style.title_weight)
                    .text_color(style.title_color)
                    .child(title),
            )
        }
        ListRowContent::TwoLine { title, supporting } => {
            // The legacy two-line column is `flex min-w-0 flex-1 flex-col`
            // with each line carrying `min-w-0 truncate`.
            row.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex_col()
                    .child(title_line(&style, title))
                    .child(supporting_line(&style, supporting)),
            )
        }
    };

    if let Some(caption) = trailing_caption {
        row = row.child(
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .text_size(style.caption_size)
                .line_height(style.caption_line_height)
                .text_color(style.caption_color)
                .child(caption),
        );
    }

    if let Some(trailing) = trailing {
        row = row.child(div().flex_shrink_0().child(trailing));
    }

    row
}

/// Builds the truncated title line shared by both content shapes.
fn title_line(style: &ListRowStyle, title: SharedString) -> Div {
    div()
        .min_w_0()
        .truncate()
        .text_size(style.title_size)
        .line_height(style.title_line_height)
        .font_weight(style.title_weight)
        .text_color(style.title_color)
        .child(title)
}

/// Builds the truncated supporting line with its geometry-selected face.
fn supporting_line(style: &ListRowStyle, supporting: SharedString) -> Div {
    div()
        .min_w_0()
        .truncate()
        .font_family(style.supporting_family)
        .text_size(style.supporting_size)
        .line_height(style.supporting_line_height)
        .text_color(style.supporting_color)
        .child(supporting)
}
