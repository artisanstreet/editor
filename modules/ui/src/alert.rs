//! Reusable inline native GPUI alert primitive.
//!
//! Ported from the audited legacy wrapper
//! (`modules/frontend/src/lib/components/ui/alert/*.svelte`; INVENTORY §2 row 1)
//! at the single configuration the product actually reaches: the inline,
//! non-modal status alert rendered as a bordered card with optional leading
//! icon, title, description, and trailing action affordance. The legacy
//! `alert.svelte` taxonomy is `variant: default | destructive`, the grid
//! `gap-0.5 rounded-lg border px-4 py-3 text-sm` container with
//! `has-[>svg]:grid-cols-[auto_1fr] has-[>svg]:gap-x-2.5` icon layout,
//! `has-data-[slot=alert-action]:pr-18` action reservation, and the
//! `role="alert"` + `data-slot` semantics. The title and description slots
//! carry `font-medium` and `text-muted-foreground` treatments respectively,
//! and the destructive variant tints content via `text-destructive` (with the
//! description at `destructive/90`) while retaining the `bg-card` fill.
//! Alert-dialog is a distinct modal surface and is not represented here.
//!
//! This primitive preserves that one recipe exactly: `rounded-lg` (10 px),
//! `border` (1 px `--border`), `px-4` (16 px) / `py-3` (12 px), `gap-0.5`
//! (2 px) vertical stack, `gap-x-2.5` (10 px) icon column gap, `size-4`
//! (16 px) icon edge, `text-sm` (14 px) typography, `bg-card` fill in both
//! variants, mode-resolved `card-foreground` / `destructive` text, the
//! `muted-foreground` / `destructive/90` description treatment, and the
//! absolute `top-2.5 right-3` action placement with the `pr-18` (72 px)
//! reservation. Composition is explicit: the caller supplies an optional
//! catalog icon ([`AssetId`]), an optional title, an optional description,
//! optional freeform content, and an optional trailing action element. No
//! web/DOM shims are introduced, no focus or dismissal policy is owned here
//! (the element has no input), and no animation is encoded: a GPUI alert
//! repaints at the final state whenever its inputs change.
//!
//! Stable selectors and semantic metadata mirror the dialog pattern so
//! behavioral tests and future platform accessibility wiring can assert
//! exact semantics without reaching into GPUI internals. Callers add further
//! [`gpui::Styled`] refinements by wrapping the returned element; later values
//! win over the recipe defaults when they chain onto the `Div` helpers.
//!
//! ## Known rendering limitations (pinned GPUI 0.2.2)
//!
//! - **Rounded clipping is not honored for children.** As documented for the
//!   card and badge primitives, `overflow_hidden` installs a rectangular
//!   [`gpui::ContentMask`] built purely from bounds, so the 10 px corners
//!   round the painted background and border but do not clip overflowing
//!   descendants to the corner shape. Reached alerts keep text inset.
//! - **There is no grid layout.** The legacy `grid` container participates as
//!   an ordinary flex item through Taffy. The two-column icon layout is
//!   reproduced with a flex row (`gap-2.5`) rather than `grid-cols-[auto_1fr]`,
//!   and the vertical title/description/content stack uses a flex column with
//!   `gap-0.5`. The geometries coincide for the reached content shapes.
//! - **Platform accessibility is metadata only.** Pinned GPUI exposes no
//!   platform accessibility tree, so `role="alert"` and the retained title/
//!   description are first-class metadata for tests and future adapters; the
//!   current renderer does not claim a screen reader consumes them.

use artisan_assets::AssetId;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, Div, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, Styled, Window, div, px,
};

use crate::asset_seam::asset_glyph;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};

/// Semantic role retained for every alert.
pub const ALERT_ROLE: &str = "alert";

/// Stable debug selector for the alert root.
pub const ALERT_ROOT_SELECTOR: &str = "artisan-alert";

/// Stable debug selector for the alert title.
pub const ALERT_TITLE_SELECTOR: &str = "artisan-alert-title";

/// Stable debug selector for the alert description.
pub const ALERT_DESCRIPTION_SELECTOR: &str = "artisan-alert-description";

/// Stable debug selector for the alert icon.
pub const ALERT_ICON_SELECTOR: &str = "artisan-alert-icon";

/// Stable debug selector for the alert action.
pub const ALERT_ACTION_SELECTOR: &str = "artisan-alert-action";

/// Stable debug selector for the alert freeform content slot.
pub const ALERT_CONTENT_SELECTOR: &str = "artisan-alert-content";

/// Visual variants reached by the audited `alert.svelte` wrapper.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum AlertVariant {
    /// `variant="default"`: `bg-card text-card-foreground`.
    #[default]
    Default,
    /// `variant="destructive"`: `bg-card text-destructive` with
    /// `text-destructive/90` on the description.
    Destructive,
}

impl AlertVariant {
    /// Returns the stable slot value matching the legacy `alertVariants` key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Destructive => "destructive",
        }
    }
}

/// Paint and geometry values resolved for one alert configuration.
///
/// This record is public so behavioral tests and the future component gallery
/// can compare exact native semantics without reaching into GPUI internals,
/// mirroring [`crate::badge::BadgeStyle`] and [`crate::card::CardStyle`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlertStyle {
    /// Selected semantic variant.
    pub variant: AlertVariant,
    /// Corner radius: the legacy `rounded-lg`, 10 px.
    pub corner_radius: Pixels,
    /// Horizontal padding: the legacy `px-4`, 16 px.
    pub horizontal_padding: Pixels,
    /// Vertical padding: the legacy `py-3`, 12 px.
    pub vertical_padding: Pixels,
    /// Vertical gap between title/description/content: `gap-0.5`, 2 px.
    pub content_gap: Pixels,
    /// Horizontal gap between a leading icon and the content column:
    /// `gap-x-2.5`, 10 px.
    pub icon_gap: Pixels,
    /// Leading icon edge: `size-4`, 16 px.
    pub icon_size: Pixels,
    /// One-pixel `--border` hairline.
    pub border_color: Hsla,
    /// Container background: `bg-card` (both variants).
    pub background: Hsla,
    /// Primary text/icon color: `text-card-foreground` or
    /// `text-destructive`.
    pub foreground: Hsla,
    /// Description text color: `text-muted-foreground` or
    /// `text-destructive/90`.
    pub description_foreground: Hsla,
    /// Shared control typography: `text-sm`, 14 px.
    pub text_size: Pixels,
    /// Reserved right padding when a trailing action is mounted:
    /// legacy `pr-18`, 72 px.
    pub action_reserved_padding: Pixels,
    /// Action's absolute `right-3` inset, 12 px.
    pub action_right: Pixels,
    /// Action's absolute `top-2.5` inset, 10 px.
    pub action_top: Pixels,
}

impl AlertStyle {
    /// Resolves the exact reached recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, variant: AlertVariant) -> Self {
        let (foreground, description_foreground) = match variant {
            AlertVariant::Default => (
                theme.colors.card_foreground.to_paint(),
                theme.colors.muted_foreground.to_paint(),
            ),
            AlertVariant::Destructive => (
                theme.colors.destructive.to_paint(),
                theme.colors.destructive.with_alpha(0.90).to_paint(),
            ),
        };

        Self {
            variant,
            corner_radius: RadiusTokens::value(RadiusStep::Lg),
            horizontal_padding: theme.spacing.steps(4.0),
            vertical_padding: theme.spacing.steps(3.0),
            content_gap: theme.spacing.steps(0.5),
            icon_gap: theme.spacing.steps(2.5),
            icon_size: theme.spacing.steps(4.0),
            border_color: theme.colors.border.to_paint(),
            background: theme.colors.card.to_paint(),
            foreground,
            description_foreground,
            text_size: theme.typography.control_text,
            action_reserved_padding: theme.spacing.steps(18.0),
            action_right: theme.spacing.steps(3.0),
            action_top: theme.spacing.steps(2.5),
        }
    }
}

/// Retained semantic metadata for an alert.
///
/// Pinned GPUI has no platform accessibility tree, so these values are
/// metadata for tests and future platform adapters.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AlertSemantics {
    /// The retained `role="alert"` value.
    pub role: &'static str,
    /// The resolved variant.
    pub variant: AlertVariant,
    /// The retained title string, if any.
    pub title: Option<SharedString>,
    /// The retained description string, if any.
    pub description: Option<SharedString>,
    /// Whether a leading icon is present.
    pub has_icon: bool,
    /// Whether a trailing action is present.
    pub has_action: bool,
    /// Whether freeform content is present.
    pub has_content: bool,
}

/// A reusable inline GPUI alert with explicit title/description/content
/// composition and optional icon/action affordances.
#[derive(IntoElement)]
pub struct Alert {
    style: AlertStyle,
    title: Option<SharedString>,
    description: Option<SharedString>,
    icon: Option<AssetId>,
    content: Option<AnyElement>,
    action: Option<AnyElement>,
    debug_selector: Option<SharedString>,
}

impl Alert {
    /// Creates an inline alert from a caller-resolved recipe.
    #[must_use]
    pub fn new(style: AlertStyle) -> Self {
        Self {
            style,
            title: None,
            description: None,
            icon: None,
            content: None,
            action: None,
            debug_selector: None,
        }
    }

    /// Convenience that resolves the theme and variant before construction.
    #[must_use]
    pub fn from_theme(theme: ArtisanTheme, variant: AlertVariant) -> Self {
        Self::new(AlertStyle::resolve(theme, variant))
    }

    /// Adds a visible title. The title is rendered with `font-medium` and the
    /// alert's primary foreground.
    #[must_use]
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Adds a visible description below the title. The description uses the
    /// variant-specific muted/destructive-90 treatment.
    #[must_use]
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Adds an optional leading catalog icon (16 px, tinted by the foreground).
    #[must_use]
    pub const fn icon(mut self, icon: AssetId) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Adds freeform content below the title/description stack.
    ///
    /// Content is an owned element rather than plain text so callers can
    /// compose richer inline affordances while keeping the audited title and
    /// description typography intact.
    #[must_use]
    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }

    /// Adds a trailing action affordance rendered at the legacy
    /// `absolute top-2.5 right-3` placement.
    ///
    /// When present the root reserves `pr-18` (72 px) so action content does
    /// not overlap the primary text. The action element itself is caller-owned
    /// and unstyled: place a button or link inside.
    #[must_use]
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    /// Adds a stable selector prefix for the root and inspectable parts.
    ///
    /// The root receives the supplied selector; title, description, content,
    /// icon, and action receive `-title`, `-description`, `-content`,
    /// `-icon`, and `-action` suffixes. With no override the stable
    /// `artisan-alert*` constants are used.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the resolved visual recipe retained by this alert.
    #[must_use]
    pub fn visual_style(&self) -> AlertStyle {
        self.style
    }

    /// Returns the retained semantic variant.
    #[must_use]
    pub const fn variant(&self) -> AlertVariant {
        self.style.variant
    }

    /// Whether a leading icon will be rendered.
    #[must_use]
    pub const fn has_icon(&self) -> bool {
        self.icon.is_some()
    }

    /// Whether a trailing action will be rendered.
    #[must_use]
    pub const fn has_action(&self) -> bool {
        self.action.is_some()
    }

    /// Whether freeform content will be rendered.
    #[must_use]
    pub const fn has_content(&self) -> bool {
        self.content.is_some()
    }

    /// Whether a title will be rendered.
    #[must_use]
    pub fn has_title(&self) -> bool {
        self.title.is_some()
    }

    /// Whether a description will be rendered.
    #[must_use]
    pub fn has_description(&self) -> bool {
        self.description.is_some()
    }

    /// Returns the retained semantic metadata for this alert.
    #[must_use]
    pub fn semantics(&self) -> AlertSemantics {
        AlertSemantics {
            role: ALERT_ROLE,
            variant: self.style.variant,
            title: self.title.clone(),
            description: self.description.clone(),
            has_icon: self.has_icon(),
            has_action: self.has_action(),
            has_content: self.has_content(),
        }
    }
}

struct AlertSelectors {
    root: String,
    title: String,
    description: String,
    content: String,
    icon: String,
    action: String,
}

fn resolve_alert_selectors(prefix: Option<&String>) -> AlertSelectors {
    AlertSelectors {
        root: prefix
            .cloned()
            .unwrap_or_else(|| ALERT_ROOT_SELECTOR.to_string()),
        title: prefix.map_or_else(
            || ALERT_TITLE_SELECTOR.to_string(),
            |selector| format!("{selector}-title"),
        ),
        description: prefix.map_or_else(
            || ALERT_DESCRIPTION_SELECTOR.to_string(),
            |selector| format!("{selector}-description"),
        ),
        content: prefix.map_or_else(
            || ALERT_CONTENT_SELECTOR.to_string(),
            |selector| format!("{selector}-content"),
        ),
        icon: prefix.map_or_else(
            || ALERT_ICON_SELECTOR.to_string(),
            |selector| format!("{selector}-icon"),
        ),
        action: prefix.map_or_else(
            || ALERT_ACTION_SELECTOR.to_string(),
            |selector| format!("{selector}-action"),
        ),
    }
}

fn alert_root_container(style: AlertStyle, has_action: bool, root_selector: String) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .w_full()
        .gap(style.content_gap)
        .px(style.horizontal_padding)
        .py(style.vertical_padding)
        .rounded(style.corner_radius)
        .border_1()
        .border_color(style.border_color)
        .bg(style.background)
        .text_color(style.foreground)
        .text_size(style.text_size)
        .overflow_hidden()
        .when(has_action, |element| {
            element.pr(style.action_reserved_padding)
        })
        .debug_selector(move || root_selector)
}

fn alert_content_column(
    style: AlertStyle,
    selectors: &AlertSelectors,
    title: Option<SharedString>,
    description: Option<SharedString>,
    content: Option<AnyElement>,
    has_icon: bool,
) -> Div {
    let mut column = div()
        .flex()
        .flex_col()
        .gap(style.content_gap)
        .min_w_0()
        .flex_1();

    if let Some(title) = title {
        let selector = selectors.title.clone();
        let title_element = div()
            .min_w_0()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(style.foreground)
            .debug_selector(move || selector)
            .child(title);
        column = column.child(title_element);
    } else if has_icon {
        // Keep layout stable when an icon is present without a title.
    }

    if let Some(description) = description {
        let selector = selectors.description.clone();
        let description_element = div()
            .min_w_0()
            .text_size(style.text_size)
            .text_color(style.description_foreground)
            .debug_selector(move || selector)
            .child(description);
        column = column.child(description_element);
    }

    if let Some(content) = content {
        let selector = selectors.content.clone();
        let freeform = div()
            .min_w_0()
            .debug_selector(move || selector)
            .child(content);
        column = column.child(freeform);
    }

    column
}

impl RenderOnce for Alert {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let has_action = self.action.is_some();
        let has_icon = self.icon.is_some();
        let has_title = self.title.is_some();
        let has_description = self.description.is_some();
        let has_content = self.content.is_some();
        let style = self.style;

        let prefix = self.debug_selector.as_ref().map(ToString::to_string);
        let selectors = resolve_alert_selectors(prefix.as_ref());

        let mut root = alert_root_container(style, has_action, selectors.root.clone());

        let mut row: Div = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(style.icon_gap)
            .w_full();

        if let Some(icon) = self.icon {
            let icon_selector = selectors.icon.clone();
            let mut icon_element = div()
                .flex_shrink_0()
                .debug_selector(move || icon_selector)
                .child(
                    asset_glyph(icon)
                        .size(style.icon_size)
                        .text_color(style.foreground),
                );
            icon_element = icon_element.mt(px(2.0));
            row = row.child(icon_element);
        }

        let content_column = alert_content_column(
            style,
            &selectors,
            self.title,
            self.description,
            self.content,
            has_icon,
        );
        row = row.child(content_column);
        root = root.child(row);

        if let Some(action) = self.action {
            let action_selector = selectors.action.clone();
            let action_wrapper = div()
                .absolute()
                .top(style.action_top)
                .right(style.action_right)
                .debug_selector(move || action_selector)
                .child(action);
            root = root.child(action_wrapper);
        }

        let _ = (
            selectors.title,
            selectors.description,
            selectors.content,
            selectors.icon,
            selectors.action,
            has_title,
            has_description,
            has_content,
        );

        root
    }
}

/// Returns an inline alert root as a plain GPUI [`Div`] for callers that
/// compose the audited geometry without the typed [`Alert`] builder.
///
/// The returned element consumes the caller-resolved recipe (border, radius,
/// paddings, gap, `bg-card`/foreground, `text-sm`, and the `pr-18` reservation
/// when `has_action` is true). Further [`gpui::Styled`] refinements chain onto
/// the returned [`Div`] and later values win, mirroring the `Div` helpers
/// exposed by the card and badge primitives.
#[must_use]
pub fn alert_root(style: AlertStyle, has_action: bool) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .w_full()
        .gap(style.content_gap)
        .px(style.horizontal_padding)
        .py(style.vertical_padding)
        .rounded(style.corner_radius)
        .border_1()
        .border_color(style.border_color)
        .bg(style.background)
        .text_color(style.foreground)
        .text_size(style.text_size)
        .overflow_hidden()
        .when(has_action, |element| {
            element.pr(style.action_reserved_padding)
        })
}

/// Returns the alert title as a plain GPUI [`Div`] owning its visible text.
///
/// The element uses `font-medium` over the variant foreground. Chaining
/// further [`gpui::Styled`] refinements overrides the recipe defaults.
#[must_use]
pub fn alert_title(style: AlertStyle, title: impl Into<SharedString>) -> Div {
    div()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(style.foreground)
        .child(title.into())
}

/// Returns the alert description as a plain GPUI [`Div`] owning its text.
///
/// The element carries the variant-specific `muted-foreground` or
/// `destructive/90` treatment. Further refinements chain on top.
#[must_use]
pub fn alert_description(style: AlertStyle, description: impl Into<SharedString>) -> Div {
    div()
        .text_size(style.text_size)
        .text_color(style.description_foreground)
        .child(description.into())
}
