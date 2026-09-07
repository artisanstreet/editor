//! Behavioral coverage for the compact native GPUI card primitive.

use artisan_ui::card::{CardStyle, compact_card, compact_card_content};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::{
    BoxShadow, Context, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    TestAppContext, Window, div, point, px,
};

const CARD_SELECTOR: &str = "compact-card-under-test";
const TOP_PROBE_SELECTOR: &str = "card-top-probe";
const BOTTOM_PROBE_SELECTOR: &str = "card-bottom-probe";
const CONTENT_BAND_SELECTOR: &str = "card-content-band";
const CONTENT_PROBE_SELECTOR: &str = "card-content-probe";
const OVERRIDE_PROBE_SELECTOR: &str = "card-override-probe";
const OVERFLOW_PROBE_SELECTOR: &str = "card-overflow-probe";

/// A card with two fixed-height probe children for stacking geometry.
struct StackedCardProbe {
    style: CardStyle,
}

impl Render for StackedCardProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        compact_card(self.style)
            .w(px(320.0))
            .debug_selector(|| CARD_SELECTOR.to_string())
            .child(
                div()
                    .h(px(24.0))
                    .w_full()
                    .debug_selector(|| TOP_PROBE_SELECTOR.to_string()),
            )
            .child(
                div()
                    .h(px(40.0))
                    .w_full()
                    .debug_selector(|| BOTTOM_PROBE_SELECTOR.to_string()),
            )
    }
}

/// A card holding one content band with one probe child.
struct ContentCardProbe {
    style: CardStyle,
}

impl Render for ContentCardProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        compact_card(self.style)
            .w(px(320.0))
            .debug_selector(|| CARD_SELECTOR.to_string())
            .child(
                compact_card_content(self.style)
                    .debug_selector(|| CONTENT_BAND_SELECTOR.to_string())
                    .child(
                        div()
                            .h(px(20.0))
                            .w_full()
                            .debug_selector(|| CONTENT_PROBE_SELECTOR.to_string()),
                    ),
            )
    }
}

/// A card whose caller overrides the default vertical padding to 12 px.
struct OverrideCardProbe {
    style: CardStyle,
}

impl Render for OverrideCardProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        compact_card(self.style)
            .py(px(12.0))
            .w(px(320.0))
            .debug_selector(|| CARD_SELECTOR.to_string())
            .child(
                div()
                    .h(px(24.0))
                    .w_full()
                    .debug_selector(|| OVERRIDE_PROBE_SELECTOR.to_string()),
            )
    }
}

/// A card containing one absolutely positioned child that extends past the
/// root's right and bottom edges.
struct OverflowCardProbe {
    style: CardStyle,
}

impl Render for OverflowCardProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        compact_card(self.style)
            .w(px(200.0))
            .debug_selector(|| CARD_SELECTOR.to_string())
            .child(
                div()
                    .absolute()
                    .left(px(150.0))
                    .top(px(0.0))
                    .w(px(200.0))
                    .h(px(50.0))
                    .debug_selector(|| OVERFLOW_PROBE_SELECTOR.to_string()),
            )
    }
}

#[test]
fn compact_style_pins_exact_audited_values() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let style = CardStyle::resolve(theme);

        assert_eq!(style.corner_radius, px(18.0));
        assert_eq!(style.corner_radius, RadiusTokens::value(RadiusStep::X2l));
        assert_eq!(style.vertical_padding, theme.density.card_padding_compact);
        assert_eq!(style.vertical_padding, px(16.0));
        assert_eq!(style.vertical_gap, px(16.0));
        assert_eq!(
            style.content_horizontal_padding,
            theme.density.card_padding_compact
        );
        assert_eq!(style.content_horizontal_padding, px(16.0));
        assert_eq!(style.text_size, theme.typography.control_text);
        assert_eq!(style.text_size, px(14.0));
        assert_eq!(style.ring_spread, px(1.0));

        let expected_ring = BoxShadow {
            color: theme.colors.foreground.with_alpha(0.10).to_paint(),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(1.0),
        };
        assert_eq!(style.ring(), expected_ring);
        assert_eq!(
            style.ring_color.alpha.to_bits(),
            0.10_f32.to_bits(),
            "the legacy ring is foreground at exactly 10% alpha"
        );
    }
}

#[test]
fn card_surfaces_resolve_per_mode_from_theme_tokens() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);
    let light_style = CardStyle::resolve(light);
    let dark_style = CardStyle::resolve(dark);

    assert_eq!(light_style.background, light.colors.card.to_paint());
    assert_eq!(dark_style.background, dark.colors.card.to_paint());
    assert_ne!(
        light_style.background, dark_style.background,
        "light --card (surface 0) and dark --card (surface 900) must differ"
    );

    assert_eq!(
        light_style.foreground,
        light.colors.card_foreground.to_paint()
    );
    assert_eq!(
        dark_style.foreground,
        dark.colors.card_foreground.to_paint()
    );
    assert_ne!(light_style.foreground, dark_style.foreground);

    assert_eq!(
        light_style.ring_color,
        light.colors.foreground.with_alpha(0.10).to_paint()
    );
    assert_eq!(
        dark_style.ring_color,
        dark.colors.foreground.with_alpha(0.10).to_paint()
    );
    assert_ne!(
        light_style.ring_color, dark_style.ring_color,
        "each mode's ring follows its own shared foreground"
    );
}

#[test]
fn compile_only_constructors_share_one_resolved_recipe() {
    // Resolve once and hand the identical recipe to root and content; the
    // chaining below is compile-only API-shape evidence, with no window.
    let style = CardStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark));
    let overridden = compact_card(style).py(px(12.0)).gap(px(8.0));
    let content = compact_card_content(style).px(px(12.0));
    let _ = (overridden, content);
}

#[gpui::test]
fn rendered_card_stacks_children_with_compact_gap_and_vertical_padding(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| StackedCardProbe {
        style: CardStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
    });

    let card = cx
        .debug_bounds(CARD_SELECTOR)
        .expect("card must paint inspectable bounds");
    let top = cx
        .debug_bounds(TOP_PROBE_SELECTOR)
        .expect("top probe must paint inspectable bounds");
    let bottom = cx
        .debug_bounds(BOTTOM_PROBE_SELECTOR)
        .expect("bottom probe must paint inspectable bounds");

    assert_eq!(card.size.width, px(320.0));

    // Compact vertical padding insets the first child by 16 px on top.
    assert_eq!(top.origin.y - card.origin.y, px(16.0));
    // The flex-column gap separates consecutive children by 16 px.
    assert_eq!(bottom.origin.y - (top.origin.y + top.size.height), px(16.0));
    // The bottom padding closes the stack symmetrically at 16 px.
    assert_eq!(
        (card.origin.y + card.size.height) - (bottom.origin.y + bottom.size.height),
        px(16.0)
    );
    // The card root carries no horizontal padding of its own.
    assert_eq!(top.origin.x - card.origin.x, px(0.0));
}

#[gpui::test]
fn rendered_card_content_bands_children_by_16px_horizontal_padding(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| ContentCardProbe {
        style: CardStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light)),
    });

    let card = cx
        .debug_bounds(CARD_SELECTOR)
        .expect("card must paint inspectable bounds");
    let band = cx
        .debug_bounds(CONTENT_BAND_SELECTOR)
        .expect("content band must paint inspectable bounds");
    let probe = cx
        .debug_bounds(CONTENT_PROBE_SELECTOR)
        .expect("content probe must paint inspectable bounds");

    // The band itself sits at the card's vertical padding.
    assert_eq!(band.origin.y - card.origin.y, px(16.0));
    // Its child is inset by the band's horizontal padding on both sides.
    assert_eq!(probe.origin.x - card.origin.x, px(16.0));
    assert_eq!(
        (card.origin.x + card.size.width) - (probe.origin.x + probe.size.width),
        px(16.0)
    );
}

#[gpui::test]
fn caller_py_12_override_replaces_the_default_vertical_padding(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| OverrideCardProbe {
        style: CardStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
    });

    let card = cx
        .debug_bounds(CARD_SELECTOR)
        .expect("card must paint inspectable bounds");
    let probe = cx
        .debug_bounds(OVERRIDE_PROBE_SELECTOR)
        .expect("override probe must paint inspectable bounds");

    // The later Styled refinement wins over the 16 px recipe default.
    assert_eq!(probe.origin.y - card.origin.y, px(12.0));
}

#[gpui::test]
fn overflowing_children_keep_layout_bounds_under_overflow_hidden(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| OverflowCardProbe {
        style: CardStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light)),
    });

    let card = cx
        .debug_bounds(CARD_SELECTOR)
        .expect("card must paint inspectable bounds");
    let overflow = cx
        .debug_bounds(OVERFLOW_PROBE_SELECTOR)
        .expect("overflow probe must paint inspectable bounds");

    // The absolute child leaves the flow: the auto-height card collapses to
    // its two 16 px paddings.
    assert_eq!(card.size.height, px(32.0));
    // `overflow_hidden` clips painting only; layout bounds stay unclipped and
    // the probe reports its full size past both card edges. Painting clips to
    // the rectangular bounds mask — corner radii are not honored (documented
    // module limitation).
    assert!(
        overflow.origin.x + overflow.size.width > card.origin.x + card.size.width + px(100.0),
        "overflowing layout must extend past the card's right edge"
    );
    assert!(
        overflow.origin.y + overflow.size.height > card.origin.y + card.size.height,
        "overflowing layout must extend past the card's bottom edge"
    );
}
