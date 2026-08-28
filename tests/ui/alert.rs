//! Behavioral coverage for the native GPUI inline alert primitive.

use artisan_ui::alert::{
    ALERT_ACTION_SELECTOR, ALERT_CONTENT_SELECTOR, ALERT_DESCRIPTION_SELECTOR, ALERT_ICON_SELECTOR,
    ALERT_ROLE, ALERT_ROOT_SELECTOR, ALERT_TITLE_SELECTOR, Alert, AlertStyle, AlertVariant,
    alert_description, alert_root, alert_title,
};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::{Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div, px};

const CUSTOM_PREFIX: &str = "custom-alert";

/// Legacy facts transcribed verbatim from `alert.svelte`
/// (`rounded-lg border px-4 py-3 gap-0.5 has-[>svg]:gap-x-2.5 size-4`).
const LEGACY_CORNER_RADIUS_PX: f32 = 10.0;
const LEGACY_HORIZONTAL_PADDING_PX: f32 = 16.0;
const LEGACY_VERTICAL_PADDING_PX: f32 = 12.0;
const LEGACY_CONTENT_GAP_PX: f32 = 2.0;
const LEGACY_ICON_GAP_PX: f32 = 10.0;
const LEGACY_ICON_SIZE_PX: f32 = 16.0;
const LEGACY_TEXT_SIZE_PX: f32 = 14.0;
const LEGACY_BORDER_WIDTH_PX: f32 = 1.0;
const LEGACY_ACTION_RESERVED_PX: f32 = 72.0;
const LEGACY_ACTION_RIGHT_PX: f32 = 12.0;
const LEGACY_ACTION_TOP_PX: f32 = 10.0;

/// Probe rendering a title + description alert without icon or action.
struct TitleDescriptionProbe {
    style: AlertStyle,
}

impl Render for TitleDescriptionProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let alert = Alert::new(self.style)
            .title("Heads up")
            .description("You can add components and dependencies.")
            .debug_selector(ALERT_ROOT_SELECTOR);
        div().w(px(360.0)).child(alert)
    }
}

/// Probe rendering an alert with a leading icon.
struct IconProbe {
    style: AlertStyle,
}

impl Render for IconProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Use a sealed catalog asset; the exact glyph identity is not
        // semantically load-bearing for the bounds tests below.
        let alert = Alert::new(self.style)
            .icon(artisan_ui::AssetId::TABLER_WORLD)
            .title("Note")
            .description("Inline notice with an icon.")
            .debug_selector(ALERT_ROOT_SELECTOR);
        div().w(px(400.0)).child(alert)
    }
}

/// Probe rendering an alert with a trailing action.
struct ActionProbe {
    style: AlertStyle,
}

impl Render for ActionProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let alert = Alert::new(self.style)
            .title("Update available")
            .description("Restart to apply changes.")
            .action(div().w(px(48.0)).h(px(20.0)).child("Action"))
            .debug_selector(ALERT_ROOT_SELECTOR);
        div().w(px(360.0)).child(alert)
    }
}

/// Probe rendering freeform content below the description.
struct ContentProbe {
    style: AlertStyle,
}

impl Render for ContentProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let alert = Alert::new(self.style)
            .title("Title")
            .description("Description")
            .content(div().h(px(24.0)).w_full().child("freeform"))
            .debug_selector(ALERT_ROOT_SELECTOR);
        div().w(px(360.0)).child(alert)
    }
}

/// Probe using a custom selector prefix to exercise the stable suffix
/// convention (`-title`, `-description`, etc.).
struct CustomSelectorProbe {
    style: AlertStyle,
}

impl Render for CustomSelectorProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let alert = Alert::new(self.style)
            .title("Custom")
            .description("Prefixed selectors")
            .action(div().w(px(20.0)).h(px(10.0)))
            .debug_selector(CUSTOM_PREFIX);
        div().w(px(360.0)).child(alert)
    }
}

#[test]
fn alert_style_pins_exact_audited_geometry() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        for variant in [AlertVariant::Default, AlertVariant::Destructive] {
            let theme = ArtisanTheme::for_mode(mode);
            let style = AlertStyle::resolve(theme, variant);

            assert_eq!(style.variant, variant);
            assert_eq!(style.corner_radius, px(LEGACY_CORNER_RADIUS_PX));
            assert_eq!(style.corner_radius, RadiusTokens::value(RadiusStep::Lg));
            assert_eq!(style.horizontal_padding, px(LEGACY_HORIZONTAL_PADDING_PX));
            assert_eq!(style.horizontal_padding, theme.spacing.steps(4.0));
            assert_eq!(style.vertical_padding, px(LEGACY_VERTICAL_PADDING_PX));
            assert_eq!(style.vertical_padding, theme.spacing.steps(3.0));
            assert_eq!(style.content_gap, px(LEGACY_CONTENT_GAP_PX));
            assert_eq!(style.content_gap, theme.spacing.steps(0.5));
            assert_eq!(style.icon_gap, px(LEGACY_ICON_GAP_PX));
            assert_eq!(style.icon_gap, theme.spacing.steps(2.5));
            assert_eq!(style.icon_size, px(LEGACY_ICON_SIZE_PX));
            assert_eq!(style.icon_size, theme.spacing.steps(4.0));
            assert_eq!(style.text_size, px(LEGACY_TEXT_SIZE_PX));
            assert_eq!(style.text_size, theme.typography.control_text);
            assert_eq!(style.action_reserved_padding, px(LEGACY_ACTION_RESERVED_PX));
            assert_eq!(style.action_reserved_padding, theme.spacing.steps(18.0));
            assert_eq!(style.action_right, px(LEGACY_ACTION_RIGHT_PX));
            assert_eq!(style.action_top, px(LEGACY_ACTION_TOP_PX));
            assert_eq!(style.background, theme.colors.card.to_paint());
            assert_eq!(style.border_color, theme.colors.border.to_paint());
        }
    }
}

#[test]
fn alert_variant_palette_resolves_from_exact_legacy_sources() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let default = AlertStyle::resolve(theme, AlertVariant::Default);
        let destructive = AlertStyle::resolve(theme, AlertVariant::Destructive);

        // Default: card fills with card-foreground text and muted description.
        assert_eq!(default.foreground, theme.colors.card_foreground.to_paint());
        assert_eq!(
            default.description_foreground,
            theme.colors.muted_foreground.to_paint()
        );

        // Destructive: same bg-card but destructive text, description at 90%.
        assert_eq!(destructive.foreground, theme.colors.destructive.to_paint());
        assert_eq!(
            destructive.description_foreground,
            theme.colors.destructive.with_alpha(0.90).to_paint()
        );
        assert_eq!(
            destructive.description_foreground.a.to_bits(),
            0.90_f32.to_bits(),
            "destructive description carries exactly 90% alpha"
        );

        // Both variants share the same background and border.
        assert_eq!(default.background, theme.colors.card.to_paint());
        assert_eq!(destructive.background, theme.colors.card.to_paint());
        assert_eq!(default.border_color, theme.colors.border.to_paint());
        assert_eq!(destructive.border_color, theme.colors.border.to_paint());

        // The two variants must differ on primary text.
        assert_ne!(default.foreground, destructive.foreground);
        assert_ne!(
            default.description_foreground,
            destructive.description_foreground
        );
    }
}

#[test]
fn variant_string_matches_legacy_slot_key() {
    assert_eq!(AlertVariant::Default.as_str(), "default");
    assert_eq!(AlertVariant::Destructive.as_str(), "destructive");
}

#[test]
fn default_variant_is_default() {
    assert_eq!(AlertVariant::default(), AlertVariant::Default);
}

#[test]
fn semantics_retain_role_variant_and_composition_flags() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let default_style = AlertStyle::resolve(theme, AlertVariant::Default);
    let empty = Alert::new(default_style);
    let semantics = empty.semantics();
    assert_eq!(semantics.role, ALERT_ROLE);
    assert_eq!(semantics.role, "alert");
    assert_eq!(semantics.variant, AlertVariant::Default);
    assert_eq!(semantics.title, None);
    assert_eq!(semantics.description, None);
    assert!(!semantics.has_icon);
    assert!(!semantics.has_action);
    assert!(!semantics.has_content);
    assert_eq!(empty.variant(), AlertVariant::Default);
    assert!(!empty.has_title());
    assert!(!empty.has_description());

    let filled = Alert::new(AlertStyle::resolve(theme, AlertVariant::Destructive))
        .title("Title")
        .description("Description")
        .icon(artisan_ui::AssetId::TABLER_WORLD)
        .content(div().child("body"))
        .action(div().child("undo"));
    let filled_semantics = filled.semantics();
    assert_eq!(filled_semantics.role, ALERT_ROLE);
    assert_eq!(filled_semantics.variant, AlertVariant::Destructive);
    assert_eq!(
        filled_semantics.title.as_ref().map(|value| value.as_ref()),
        Some("Title")
    );
    assert_eq!(
        filled_semantics
            .description
            .as_ref()
            .map(|value| value.as_ref()),
        Some("Description")
    );
    assert!(filled_semantics.has_icon);
    assert!(filled_semantics.has_action);
    assert!(filled_semantics.has_content);
    assert!(filled.has_title());
    assert!(filled.has_description());
    assert!(filled.has_icon());
    assert!(filled.has_action());
    assert!(filled.has_content());
}

#[test]
fn compile_only_helpers_chain_styled_refinements() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let style = AlertStyle::resolve(theme, AlertVariant::Default);

    // The builder returns a RenderOnce element that participates as a child;
    // compile-only evidence that the methods chain.
    let alert = Alert::new(style)
        .title("T")
        .description("D")
        .icon(artisan_ui::AssetId::TABLER_WORLD)
        .content(div().child("extra"))
        .action(div().child("act"))
        .debug_selector("chain-test");
    let _ = alert;

    // The plain Div helpers also chain refinements; later values win.
    let root = alert_root(style, true).max_w(px(200.0));
    let title = alert_title(style, "T").opacity(0.5);
    let description = alert_description(style, "D").opacity(0.7);
    let from_theme = Alert::from_theme(theme, AlertVariant::Destructive)
        .title("t")
        .visual_style();
    assert_eq!(from_theme.variant, AlertVariant::Destructive);
    let _ = (root, title, description);
}

#[gpui::test]
fn rendered_alert_exposes_stable_title_and_description_selectors(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| TitleDescriptionProbe {
        style: AlertStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Dark),
            AlertVariant::Default,
        ),
    });

    assert!(cx.debug_bounds(ALERT_ROOT_SELECTOR).is_some());
    assert!(cx.debug_bounds(ALERT_TITLE_SELECTOR).is_some());
    assert!(cx.debug_bounds(ALERT_DESCRIPTION_SELECTOR).is_some());
    // No icon or action were mounted, so their selectors must be absent.
    assert!(cx.debug_bounds(ALERT_ICON_SELECTOR).is_none());
    assert!(cx.debug_bounds(ALERT_ACTION_SELECTOR).is_none());
    assert!(cx.debug_bounds(ALERT_CONTENT_SELECTOR).is_none());
}

#[gpui::test]
fn rendered_icon_alert_preserves_horizontal_gap_geometry(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| IconProbe {
        style: AlertStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Light),
            AlertVariant::Default,
        ),
    });

    let root = cx
        .debug_bounds(ALERT_ROOT_SELECTOR)
        .expect("root must be present");
    let icon = cx
        .debug_bounds(ALERT_ICON_SELECTOR)
        .expect("icon must be present");
    let title = cx
        .debug_bounds(ALERT_TITLE_SELECTOR)
        .expect("title must be present");

    // The icon sits inset by the 1 px border + horizontal padding (16 px)
    // and the 1 px border + top padding (12 px + 2 px baseline nudge).
    assert_eq!(
        icon.origin.x - root.origin.x,
        px(LEGACY_BORDER_WIDTH_PX + LEGACY_HORIZONTAL_PADDING_PX)
    );
    assert_eq!(
        icon.origin.y - root.origin.y,
        px(LEGACY_BORDER_WIDTH_PX + LEGACY_VERTICAL_PADDING_PX + 2.0)
    );
    assert_eq!(icon.size.width, px(LEGACY_ICON_SIZE_PX));
    assert_eq!(icon.size.height, px(LEGACY_ICON_SIZE_PX));

    // Title starts after the icon plus the 10 px icon gap.
    assert_eq!(
        title.origin.x - (icon.origin.x + icon.size.width),
        px(LEGACY_ICON_GAP_PX)
    );
}

#[gpui::test]
fn rendered_action_alert_mounts_absolute_action(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| ActionProbe {
        style: AlertStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Light),
            AlertVariant::Destructive,
        ),
    });

    let root = cx
        .debug_bounds(ALERT_ROOT_SELECTOR)
        .expect("root must be present");
    let action = cx
        .debug_bounds(ALERT_ACTION_SELECTOR)
        .expect("action must be present");

    // Absolute `top-2.5 right-3` within the bordered root (1 px border +
    // 12 px padding offsets).
    assert_eq!(
        action.origin.y - root.origin.y,
        px(LEGACY_BORDER_WIDTH_PX + LEGACY_ACTION_TOP_PX)
    );
    assert_eq!(
        (root.origin.x + root.size.width) - (action.origin.x + action.size.width),
        px(LEGACY_BORDER_WIDTH_PX + LEGACY_ACTION_RIGHT_PX)
    );
    // The root reserves 72 px on the right so content cannot overlap the
    // absolute action; the root's width already includes that reservation.
    assert!(root.size.width >= px(100.0));
}

#[gpui::test]
fn rendered_content_slot_mounts_below_description(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| ContentProbe {
        style: AlertStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Dark),
            AlertVariant::Default,
        ),
    });

    let title = cx
        .debug_bounds(ALERT_TITLE_SELECTOR)
        .expect("title must be present");
    let description = cx
        .debug_bounds(ALERT_DESCRIPTION_SELECTOR)
        .expect("description must be present");
    let content = cx
        .debug_bounds(ALERT_CONTENT_SELECTOR)
        .expect("freeform content must be present");

    // Vertical stack uses the 2 px gap between each slot.
    // Title above description.
    assert!(description.origin.y > title.origin.y + title.size.height);
    // Content below description.
    assert!(content.origin.y > description.origin.y + description.size.height);
}

#[gpui::test]
fn custom_debug_prefix_derives_stable_suffix_selectors(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| CustomSelectorProbe {
        style: AlertStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Light),
            AlertVariant::Default,
        ),
    });

    assert!(cx.debug_bounds(CUSTOM_PREFIX).is_some());
    assert!(cx.debug_bounds("custom-alert-title").is_some());
    assert!(cx.debug_bounds("custom-alert-description").is_some());
    assert!(cx.debug_bounds("custom-alert-action").is_some());
    // The default stable selectors must not appear when a custom prefix is used.
    assert!(cx.debug_bounds(ALERT_ROOT_SELECTOR).is_none());
}
