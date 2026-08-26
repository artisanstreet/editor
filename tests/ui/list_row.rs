//! Behavioral coverage for the native GPUI list-row presentation primitive.
//!
//! Recipe assertions compare [`ListRowStyle::resolve`] output against
//! independently transcribed legacy facts — literal pixels and literal OKLCH
//! source colors from `project-selector.svelte` (:186–214 composing
//! `dropdown-menu-item.svelte:23`) and `thread-hover-rail.svelte`
//! (:473–487 recent rows, :592–629 working rows) — never against the
//! primitive's own derivation, so coverage cannot pass circularly.
//!
//! Interaction absence is covered structurally: the API-shape test constructs
//! every public item from theme data and plain caller elements alone, which
//! type-checks only because the surface accepts no handlers, focus handles,
//! events, assets, tooltips, or domain values.

use artisan_ui::list_row::{
    ListRowContent, ListRowGeometry, ListRowSlots, ListRowStyle, ListRowTone, list_row,
};
use artisan_ui::theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    TestAppContext, Window, div, px,
};

const ROW_SELECTOR: &str = "list-row-under-test";
const LEADING_SELECTOR: &str = "list-row-leading-slot";
const TRAILING_SELECTOR: &str = "list-row-trailing-slot";

/// Legacy facts transcribed verbatim from the audited project-picker rows
/// (`gap-2.5 rounded-xl px-2 py-1.5` over the item defaults).
const LEGACY_MENU_CHILD_GAP_PX: f32 = 10.0;
const LEGACY_MENU_HORIZONTAL_PADDING_PX: f32 = 8.0;
const LEGACY_MENU_VERTICAL_PADDING_PX: f32 = 6.0;
const LEGACY_MENU_RADIUS_PX: f32 = 14.0;

/// Legacy facts transcribed verbatim from the audited thread-rail rows
/// (`gap-2 rounded-lg px-2 py-2`).
const LEGACY_RAIL_CHILD_GAP_PX: f32 = 8.0;
const LEGACY_RAIL_HORIZONTAL_PADDING_PX: f32 = 8.0;
const LEGACY_RAIL_VERTICAL_PADDING_PX: f32 = 8.0;
const LEGACY_RAIL_RADIUS_PX: f32 = 10.0;

/// Shared row type: Tailwind `text-sm`, 14 px on a 20 px named leading.
const LEGACY_TITLE_PX: f32 = 14.0;
const LEGACY_TITLE_LEADING_PX: f32 = 20.0;

/// Trailing time / sans supporting line: Tailwind `text-xs`, 12 px / 16 px.
const LEGACY_XS_PX: f32 = 12.0;
const LEGACY_XS_LEADING_PX: f32 = 16.0;

/// Picker path line: the arbitrary `text-[0.6875rem]` length, 11 px, on the
/// inherited 20 px `text-sm` leading.
const LEGACY_MONO_SUPPORTING_PX: f32 = 11.0;

/// Expected painted row heights: padding twice plus the explicit line boxes.
const MENU_TWO_LINE_HEIGHT_PX: f32 = 6.0 * 2.0 + 20.0 + 20.0;
const RAIL_ONE_LINE_HEIGHT_PX: f32 = 8.0 * 2.0 + 20.0;
const RAIL_TWO_LINE_HEIGHT_PX: f32 = 8.0 * 2.0 + 20.0 + 16.0;

#[test]
fn menu_geometry_pins_exact_audited_picker_values() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let style = ListRowStyle::resolve(
            theme,
            ListRowGeometry::Menu,
            ListRowTone::Foreground,
            FontWeight::NORMAL,
        );

        assert_eq!(style.child_gap, px(LEGACY_MENU_CHILD_GAP_PX));
        assert_eq!(style.child_gap, theme.spacing.steps(2.5));
        assert_eq!(
            style.horizontal_padding,
            px(LEGACY_MENU_HORIZONTAL_PADDING_PX)
        );
        assert_eq!(style.horizontal_padding, theme.spacing.steps(2.0));
        assert_eq!(style.vertical_padding, px(LEGACY_MENU_VERTICAL_PADDING_PX));
        assert_eq!(style.vertical_padding, theme.spacing.steps(1.5));
        assert_eq!(
            style.corner_radius,
            px(LEGACY_MENU_RADIUS_PX),
            "picker rows keep the composed `rounded-xl` ramp step"
        );
        assert_eq!(style.corner_radius, RadiusTokens::value(RadiusStep::Xl));
        assert_eq!(style.title_size, px(LEGACY_TITLE_PX));
        assert_eq!(style.title_size, theme.typography.control_text);
        assert_eq!(
            style.title_line_height,
            px(LEGACY_TITLE_LEADING_PX),
            "`text-sm` carries the 20 px named leading"
        );
        assert_eq!(
            style.supporting_size,
            px(LEGACY_MONO_SUPPORTING_PX),
            "the picker path line is the audited 11 px arbitrary length"
        );
        assert_eq!(
            style.supporting_line_height,
            px(LEGACY_TITLE_LEADING_PX),
            "the arbitrary-size utility sets no leading, so the 20 px \
             `text-sm` leading is inherited"
        );
        assert_eq!(style.supporting_family, "JetBrains Mono");
        assert_eq!(style.supporting_family, theme.typography.mono.family);
    }
}

#[test]
fn rail_geometry_pins_exact_audited_thread_values() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let style = ListRowStyle::resolve(
            theme,
            ListRowGeometry::Rail,
            ListRowTone::Foreground,
            FontWeight::MEDIUM,
        );

        assert_eq!(style.child_gap, px(LEGACY_RAIL_CHILD_GAP_PX));
        assert_eq!(style.child_gap, theme.spacing.steps(2.0));
        assert_eq!(
            style.horizontal_padding,
            px(LEGACY_RAIL_HORIZONTAL_PADDING_PX)
        );
        assert_eq!(style.horizontal_padding, theme.spacing.steps(2.0));
        assert_eq!(style.vertical_padding, px(LEGACY_RAIL_VERTICAL_PADDING_PX));
        assert_eq!(style.vertical_padding, theme.spacing.steps(2.0));
        assert_eq!(
            style.corner_radius,
            px(LEGACY_RAIL_RADIUS_PX),
            "rail rows keep the `rounded-lg` ramp step"
        );
        assert_eq!(style.corner_radius, RadiusTokens::value(RadiusStep::Lg));
        assert_eq!(style.title_size, px(LEGACY_TITLE_PX));
        assert_eq!(style.title_size, theme.typography.control_text);
        assert_eq!(style.supporting_size, px(LEGACY_XS_PX));
        assert_eq!(style.supporting_size, theme.typography.label_text);
        assert_eq!(style.supporting_line_height, px(LEGACY_XS_LEADING_PX));
        assert_eq!(style.supporting_family, "Artisan Neo");
        assert_eq!(style.supporting_family, theme.typography.sans.family);
        // The recent rows' trailing time shares the `text-xs` role.
        assert_eq!(style.caption_size, px(LEGACY_XS_PX));
        assert_eq!(style.caption_line_height, px(LEGACY_XS_LEADING_PX));
    }
}

#[test]
fn semantic_paints_resolve_per_mode_and_tone_from_exact_legacy_sources() {
    let light = ListRowStyle::resolve(
        ArtisanTheme::for_mode(ThemeMode::Light),
        ListRowGeometry::Menu,
        ListRowTone::Foreground,
        FontWeight::NORMAL,
    );
    let dark = ListRowStyle::resolve(
        ArtisanTheme::for_mode(ThemeMode::Dark),
        ListRowGeometry::Menu,
        ListRowTone::Foreground,
        FontWeight::NORMAL,
    );
    let muted_light = ListRowStyle::resolve(
        ArtisanTheme::for_mode(ThemeMode::Light),
        ListRowGeometry::Rail,
        ListRowTone::Muted,
        FontWeight::MEDIUM,
    );
    let muted_dark = ListRowStyle::resolve(
        ArtisanTheme::for_mode(ThemeMode::Dark),
        ListRowGeometry::Rail,
        ListRowTone::Muted,
        FontWeight::MEDIUM,
    );

    // Foreground titles: each mode's shared `--foreground`, transcribed from
    // `theme.css` (`oklch(0.2269 0.0045 285.823)` light,
    // `oklch(0.9006 0.0005 285.823)` dark).
    assert_eq!(
        light.title_color,
        Oklch::new(0.2269, 0.0045, 285.823).to_paint()
    );
    assert_eq!(
        dark.title_color,
        Oklch::new(0.9006, 0.0005, 285.823).to_paint()
    );
    assert_ne!(light.title_color, dark.title_color);

    // Muted titles (the inactive recent rows' presentation state): each
    // mode's `--muted-foreground` — surface 500 light, surface 400 dark.
    let light_muted_source = SurfaceStep::S500.oklch();
    let dark_muted_source = SurfaceStep::S400.oklch();
    assert_eq!(
        light_muted_source,
        Oklch::new(0.552, 0.016, 285.938),
        "light --muted-foreground is surface 500"
    );
    assert_eq!(
        dark_muted_source,
        Oklch::new(0.705, 0.015, 286.067),
        "dark --muted-foreground is surface 400"
    );
    assert_eq!(muted_light.title_color, light_muted_source.to_paint());
    assert_eq!(muted_dark.title_color, dark_muted_source.to_paint());
    assert_ne!(muted_light.title_color, muted_dark.title_color);
    assert_ne!(muted_light.title_color, light.title_color);

    // Supporting lines and captions always carry the muted foreground,
    // independent of the title tone. Foreground-tone rows contrast that
    // muted flanking text against the foreground title; a muted-tone row
    // (the inactive recent rows) intentionally paints all three roles in
    // the same muted semantic color.
    for style in [&light, &dark] {
        assert_eq!(
            style.supporting_color, style.caption_color,
            "supporting and caption text share one muted role"
        );
        assert_ne!(
            style.supporting_color, style.title_color,
            "muted flanking text must differ from the foreground title"
        );
    }
    for style in [&muted_light, &muted_dark] {
        assert_eq!(
            style.supporting_color, style.caption_color,
            "supporting and caption text share one muted role"
        );
        assert_eq!(
            style.title_color, style.supporting_color,
            "a muted-tone row shares the muted semantic color across title, \
             supporting, and caption text"
        );
    }
    assert_eq!(
        light.supporting_color,
        SurfaceStep::S500.oklch().to_paint(),
        "menu supporting line paints light --muted-foreground"
    );
    assert_eq!(
        dark.caption_color,
        SurfaceStep::S400.oklch().to_paint(),
        "rail caption paints dark --muted-foreground"
    );
}

#[test]
fn title_weight_is_caller_supplied_presentation_data() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let regular = ListRowStyle::resolve(
        theme,
        ListRowGeometry::Rail,
        ListRowTone::Foreground,
        FontWeight::NORMAL,
    );
    let medium = ListRowStyle::resolve(
        theme,
        ListRowGeometry::Rail,
        ListRowTone::Foreground,
        FontWeight::MEDIUM,
    );

    assert_ne!(FontWeight::NORMAL, FontWeight::MEDIUM);
    assert_eq!(regular.title_weight, FontWeight::NORMAL);
    assert_eq!(medium.title_weight, FontWeight::MEDIUM);
}

/// Exercises the complete public API from theme data and plain caller
/// elements alone. This compiles only because the primitive owns no domain
/// types, assets/icons, input handlers, focus handles, tooltips, or events;
/// the returned value is a plain `Div` that later `Styled` refinements win
/// on.
#[test]
fn public_api_composes_without_interaction_or_domain_ownership() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Light);

    for geometry in [ListRowGeometry::Menu, ListRowGeometry::Rail] {
        for tone in [ListRowTone::Foreground, ListRowTone::Muted] {
            for weight in [FontWeight::NORMAL, FontWeight::MEDIUM] {
                let style = ListRowStyle::resolve(theme, geometry, tone, weight);

                // Both explicit content shapes, with and without slots.
                let plain_one_line = list_row(
                    style,
                    ListRowContent::one_line("Recent thread"),
                    ListRowSlots::new(),
                );
                let _ = plain_one_line;

                let fully_slotted = list_row(
                    style,
                    ListRowContent::two_line("Project", "supporting detail"),
                    ListRowSlots::new()
                        .leading(div().size(px(16.0)))
                        .trailing_caption("2h")
                        .trailing(div().size(px(16.0))),
                );
                let _ = fully_slotted;
            }
        }
    }

    // The returned element is an ordinary refinable `Div`; later values win.
    let refined = list_row(
        ListRowStyle::resolve(
            theme,
            ListRowGeometry::Menu,
            ListRowTone::Foreground,
            FontWeight::NORMAL,
        ),
        ListRowContent::one_line("Capped"),
        ListRowSlots::new(),
    )
    .max_w(px(240.0))
    .py(px(1.0));
    let _ = refined;
}

/// A menu row with both caller slots inside a top-aligned host, so its full
/// width comes from the recipe's own `w_full` and slot placement is
/// observable.
struct MenuTwoLineProbe {
    style: ListRowStyle,
}

impl Render for MenuTwoLineProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(320.0))
            .h(px(200.0))
            .flex()
            .flex_col()
            .items_start()
            .child(
                list_row(
                    self.style,
                    ListRowContent::two_line("Artisan Editor", r"D:\src\artisan-editor"),
                    ListRowSlots::new()
                        .leading(
                            div()
                                .size(px(16.0))
                                .debug_selector(|| LEADING_SELECTOR.to_string()),
                        )
                        .trailing(
                            div()
                                .size(px(16.0))
                                .debug_selector(|| TRAILING_SELECTOR.to_string()),
                        ),
                )
                .debug_selector(|| ROW_SELECTOR.to_string()),
            )
    }
}

#[gpui::test]
fn menu_two_line_row_fills_host_and_places_slots_at_padded_edges(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| MenuTwoLineProbe {
        style: ListRowStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Dark),
            ListRowGeometry::Menu,
            ListRowTone::Foreground,
            FontWeight::NORMAL,
        ),
    });

    let row = cx
        .debug_bounds(ROW_SELECTOR)
        .expect("row must paint inspectable bounds");
    let leading = cx
        .debug_bounds(LEADING_SELECTOR)
        .expect("leading slot must paint inspectable bounds");
    let trailing = cx
        .debug_bounds(TRAILING_SELECTOR)
        .expect("trailing slot must paint inspectable bounds");

    // The recipe fills the host width even though the host refuses to
    // stretch its children.
    assert_eq!(row.size.width, px(320.0));
    // Two explicit line boxes on the audited leading, plus `py-1.5`.
    assert_eq!(row.size.height, px(MENU_TWO_LINE_HEIGHT_PX));

    // The leading slot sits flush behind the horizontal padding and is
    // vertically centered by the row.
    assert_eq!(leading.size.width, px(16.0));
    assert_eq!(
        leading.origin.x - row.origin.x,
        px(LEGACY_MENU_HORIZONTAL_PADDING_PX)
    );
    assert_eq!(
        leading.origin.y - row.origin.y,
        (row.size.height - leading.size.height) / 2.0
    );

    // The trailing slot sits flush before the horizontal padding on the
    // other edge, proving the audited child order (leading, center,
    // trailing).
    assert_eq!(
        (row.origin.x + row.size.width) - (trailing.origin.x + trailing.size.width),
        px(LEGACY_MENU_HORIZONTAL_PADDING_PX)
    );
}

/// A recent-thread-shaped rail row squeezed into a 112 px host whose fixed
/// content (slots, caption, gaps, padding) leaves the long title far less
/// room than it naturally wants, forcing center truncation. The muted time
/// caption is primitive-styled text and carries no test hook, so its role is
/// pinned by the recipe unit tests while this probe proves the surrounding
/// layout contract.
struct NarrowRailOneLineProbe {
    style: ListRowStyle,
}

impl Render for NarrowRailOneLineProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(112.0))
            .h(px(120.0))
            .flex()
            .flex_col()
            .items_start()
            .child(
                list_row(
                    self.style,
                    ListRowContent::one_line("A very long thread title that cannot possibly fit"),
                    ListRowSlots::new()
                        .leading(
                            div()
                                .size(px(16.0))
                                .debug_selector(|| LEADING_SELECTOR.to_string()),
                        )
                        .trailing_caption("2h")
                        .trailing(
                            div()
                                .size(px(12.0))
                                .debug_selector(|| TRAILING_SELECTOR.to_string()),
                        ),
                )
                .debug_selector(|| ROW_SELECTOR.to_string()),
            )
    }
}

#[gpui::test]
fn rail_one_line_row_truncates_center_and_keeps_slots_intact_under_pressure(
    cx: &mut TestAppContext,
) {
    let (_, cx) = cx.add_window_view(|_, _| NarrowRailOneLineProbe {
        style: ListRowStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Light),
            ListRowGeometry::Rail,
            ListRowTone::Muted,
            FontWeight::MEDIUM,
        ),
    });

    let row = cx
        .debug_bounds(ROW_SELECTOR)
        .expect("row must paint inspectable bounds");
    let leading = cx
        .debug_bounds(LEADING_SELECTOR)
        .expect("leading slot must paint inspectable bounds");
    let trailing = cx
        .debug_bounds(TRAILING_SELECTOR)
        .expect("trailing slot must paint inspectable bounds");

    // The row fills the narrow host exactly; `min-width: 0` on the center
    // content lets it absorb the pressure instead of growing the row.
    assert_eq!(row.size.width, px(112.0));
    // Truncation keeps the title on one explicit line box: the row stays at
    // `py-2` padding plus one 20 px leading instead of wrapping (the caption
    // line box is shorter still).
    assert_eq!(row.size.height, px(RAIL_ONE_LINE_HEIGHT_PX));

    // The leading engine-mark-sized slot refused to shrink.
    assert_eq!(leading.size.width, px(16.0));
    assert_eq!(
        leading.origin.x - row.origin.x,
        px(LEGACY_RAIL_HORIZONTAL_PADDING_PX)
    );

    // The trailing state-dot-sized slot anchors the far padded edge after
    // the truncated center content and the muted caption.
    assert_eq!(trailing.size.width, px(12.0));
    assert_eq!(
        (row.origin.x + row.size.width) - (trailing.origin.x + trailing.size.width),
        px(LEGACY_RAIL_HORIZONTAL_PADDING_PX)
    );
}

/// A working-thread-shaped rail row: two explicit lines with no slots.
struct RailTwoLineProbe {
    style: ListRowStyle,
}

impl Render for RailTwoLineProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(320.0))
            .h(px(160.0))
            .flex()
            .flex_col()
            .items_start()
            .child(
                list_row(
                    self.style,
                    ListRowContent::two_line("Refactor theme tokens", "Artisan Editor"),
                    ListRowSlots::new(),
                )
                .debug_selector(|| ROW_SELECTOR.to_string()),
            )
    }
}

#[gpui::test]
fn rail_two_line_row_keeps_explicit_two_line_geometry(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| RailTwoLineProbe {
        style: ListRowStyle::resolve(
            ArtisanTheme::for_mode(ThemeMode::Dark),
            ListRowGeometry::Rail,
            ListRowTone::Foreground,
            FontWeight::NORMAL,
        ),
    });

    let row = cx
        .debug_bounds(ROW_SELECTOR)
        .expect("row must paint inspectable bounds");

    // Full-width row at `px-2 py-2` with one 20 px title line box above one
    // 16 px `text-xs` supporting line box.
    assert_eq!(row.size.width, px(320.0));
    assert_eq!(row.size.height, px(RAIL_TWO_LINE_HEIGHT_PX));
}
