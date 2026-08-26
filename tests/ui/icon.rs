//! External coverage for the first-workflow native GPUI icon primitive in
//! `artisan_ui::icon`.
//!
//! These tests pin the audited contract over the sealed catalog: exact
//! documented asset identities resolving byte-identical tintable sources,
//! spacing-token-derived square edges, mode-resolved tints, the never-tint
//! rule for multicolor artwork, and rendered layout proof through the pinned
//! GPUI test harness. Ambient color-forwarding behavior of inherited tints
//! is covered by the lifecycle suite in `tests/ui/asset_seam.rs`; the bounds
//! assertions here are geometry evidence only.

use artisan_ui::AssetId;
use artisan_ui::asset_seam::{CatalogAssetSource, Presentation};
use artisan_ui::icon::{IconSize, IconStyle, IconTint, icon};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    AssetSource, Context, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    TestAppContext, Window, div, px,
};

/// Every catalog glyph the documented first workflow reaches, with its stable
/// manifest key and the legacy call site that justifies it (`docs/ui/
/// ASSETS.md` §2.1 rows; paths under `modules/frontend/src/routes/components/`
/// unless noted).
const FIRST_WORKFLOW_GLYPHS: [(AssetId, &str, &str); 9] = [
    (
        AssetId::TABLER_CHECK,
        "tabler.check",
        "picker selected-row check, project-selector.svelte:212",
    ),
    (
        AssetId::TABLER_FOLDER_PLUS,
        "tabler.folder-plus",
        "picker New-project glyph, project-selector.svelte:238",
    ),
    (
        AssetId::TABLER_SELECTOR,
        "tabler.selector",
        "picker trigger chevron, project-selector.svelte:151",
    ),
    (
        AssetId::TABLER_ARROW_UP,
        "tabler.arrow-up",
        "composer send, composer/controls.svelte:170",
    ),
    (
        AssetId::TABLER_PLAYER_STOP_FILLED,
        "tabler.player-stop-filled",
        "composer stop, composer/controls.svelte:172",
    ),
    (
        AssetId::TABLER_MESSAGE_PLUS,
        "tabler.message-plus",
        "new-thread action, composer/controls.svelte:140",
    ),
    (
        AssetId::TABLER_PENCIL,
        "tabler.pencil",
        "steering-lip edit, composer/steering-lip.svelte:43",
    ),
    (
        AssetId::TABLER_TRASH,
        "tabler.trash",
        "steering-lip discard, composer/steering-lip.svelte:53",
    ),
    (
        AssetId::TABLER_X,
        "tabler.x",
        "attachment remove, composer/attachment-tray.svelte:48",
    ),
];

const DEFAULT_ICON_SELECTOR: &str = "icon-under-test-default";
const COMPACT_ICON_SELECTOR: &str = "icon-under-test-compact";
const FULL_COLOR_ICON_SELECTOR: &str = "icon-under-test-full-color";
const INHERIT_ICON_SELECTOR: &str = "icon-under-test-inherit";

#[test]
fn first_workflow_glyphs_resolve_exact_documented_catalog_identities() {
    let source = CatalogAssetSource;

    for (id, key, role) in FIRST_WORKFLOW_GLYPHS {
        assert_eq!(
            id.as_str(),
            key,
            "{role} must keep its stable sealed catalog identity"
        );

        let bytes = source
            .load(key)
            .expect("catalog loads never error")
            .unwrap_or_else(|| panic!("{role} must resolve through the sealed catalog"));
        let text = std::str::from_utf8(bytes.as_ref())
            .expect("vendored tabler sources are utf-8")
            .trim_start();

        assert!(
            text.starts_with("<svg"),
            "{role} must serve a standalone SVG document"
        );
        assert!(
            text.contains("currentColor"),
            "{role} must remain tintable through GPUI text color"
        );
    }

    // The resolved recipe seals its identity: rendering can never be pointed
    // at a different asset than the one the style was computed for.
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    for (id, _, role) in FIRST_WORKFLOW_GLYPHS {
        assert_eq!(
            IconStyle::resolve(theme, id, IconSize::Default, IconTint::Muted).asset_id(),
            id,
            "{role} recipe must render its own sealed identity"
        );
    }
}

#[test]
fn edges_derive_from_the_shared_spacing_token_in_both_modes() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);

        let default = IconStyle::resolve(
            theme,
            AssetId::TABLER_CHECK,
            IconSize::Default,
            IconTint::Inherit,
        );
        assert_eq!(
            default.edge,
            theme.spacing.steps(IconSize::Default.spacing_steps()),
            "the default edge must be the shared token product"
        );
        assert_eq!(default.edge, px(16.0), "`size-4` resolves to 16 px");

        let compact = IconStyle::resolve(
            theme,
            AssetId::TABLER_CHECK,
            IconSize::Compact,
            IconTint::Inherit,
        );
        assert_eq!(
            compact.edge,
            theme.spacing.steps(IconSize::Compact.spacing_steps()),
            "the compact edge must be the shared token product"
        );
        assert_eq!(compact.edge, px(14.0), "`size-3.5` resolves to 14 px");
    }
}

#[test]
fn muted_tint_resolves_the_mode_token_and_inherit_stays_ambient() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let light_muted = IconStyle::resolve(
        light,
        AssetId::TABLER_CHECK,
        IconSize::Default,
        IconTint::Muted,
    );
    assert_eq!(
        light_muted.color,
        Some(light.colors.muted_foreground.to_paint()),
        "light muted tint must be the light `--muted-foreground` paint"
    );

    let dark_muted = IconStyle::resolve(
        dark,
        AssetId::TABLER_CHECK,
        IconSize::Default,
        IconTint::Muted,
    );
    assert_eq!(
        dark_muted.color,
        Some(dark.colors.muted_foreground.to_paint()),
        "dark muted tint must be the dark `--muted-foreground` paint"
    );

    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let inherit = IconStyle::resolve(
            ArtisanTheme::for_mode(mode),
            AssetId::TABLER_CHECK,
            IconSize::Default,
            IconTint::Inherit,
        );
        assert_eq!(
            inherit.color, None,
            "inherit must defer paint to the ambient text color"
        );
        assert_eq!(
            inherit.presentation,
            Presentation::Tinted,
            "monochrome tabler artwork routes through the tinted pipeline"
        );
    }
}

#[test]
fn multicolor_artwork_never_receives_a_tint_even_when_requested() {
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    for id in [AssetId::SVGL_GITLAB, AssetId::ARTISAN_APP_ICON] {
        let style = IconStyle::resolve(dark, id, IconSize::Default, IconTint::Muted);
        assert_eq!(
            style.presentation,
            Presentation::FullColor,
            "{} routes through the full-color pipeline",
            id.as_str()
        );
        assert_eq!(
            style.color,
            None,
            "{} must keep authored colors; a tint would corrupt the mark",
            id.as_str()
        );
    }

    let monochrome =
        IconStyle::resolve(dark, AssetId::TABLER_X, IconSize::Compact, IconTint::Muted);
    assert_eq!(monochrome.presentation, Presentation::Tinted);
    assert!(monochrome.color.is_some());
}

/// Renders one glyph of every routed/tinted shape for harness inspection.
///
/// Each glyph sits inside a content-sized wrapper `div` that carries the
/// debug selector, so recorded bounds equal the icon's own laid-out box.
struct IconProbe;

impl Render for IconProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let muted_default = IconStyle::resolve(
            theme,
            AssetId::TABLER_CHECK,
            IconSize::Default,
            IconTint::Muted,
        );
        let muted_compact =
            IconStyle::resolve(theme, AssetId::TABLER_X, IconSize::Compact, IconTint::Muted);
        let brand_request = IconStyle::resolve(
            theme,
            AssetId::SVGL_GITLAB,
            IconSize::Default,
            IconTint::Muted,
        );
        let ambient_inherit = IconStyle::resolve(
            theme,
            AssetId::TABLER_ARROW_UP,
            IconSize::Default,
            IconTint::Inherit,
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_start()
            .child(
                div()
                    .child(icon(muted_default))
                    .debug_selector(|| DEFAULT_ICON_SELECTOR.to_string()),
            )
            .child(
                div()
                    .child(icon(muted_compact))
                    .debug_selector(|| COMPACT_ICON_SELECTOR.to_string()),
            )
            .child(
                div()
                    .child(icon(brand_request))
                    .debug_selector(|| FULL_COLOR_ICON_SELECTOR.to_string()),
            )
            .child(
                div()
                    .child(icon(ambient_inherit))
                    .debug_selector(|| INHERIT_ICON_SELECTOR.to_string()),
            )
    }
}

fn assert_square(bounds: gpui::Bounds<gpui::Pixels>, edge: gpui::Pixels, selector: &str) {
    assert_eq!(
        bounds.size.width, edge,
        "{selector} must lay out exactly {edge:?} wide"
    );
    assert_eq!(
        bounds.size.height, edge,
        "{selector} must lay out exactly {edge:?} tall"
    );
}

#[gpui::test]
fn icons_lay_out_at_their_documented_square_edges(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| IconProbe);

    let default_bounds = cx
        .debug_bounds(DEFAULT_ICON_SELECTOR)
        .expect("tinted default icon must record inspectable bounds");
    assert_square(default_bounds, px(16.0), DEFAULT_ICON_SELECTOR);

    let compact_bounds = cx
        .debug_bounds(COMPACT_ICON_SELECTOR)
        .expect("tinted compact icon must record inspectable bounds");
    assert_square(compact_bounds, px(14.0), COMPACT_ICON_SELECTOR);

    let brand_bounds = cx
        .debug_bounds(FULL_COLOR_ICON_SELECTOR)
        .expect("full-color icon must record inspectable bounds");
    assert_square(brand_bounds, px(16.0), FULL_COLOR_ICON_SELECTOR);
}

#[gpui::test]
fn inherit_tinted_icons_keep_their_layout_box_without_ancestors(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| IconProbe);

    // Geometry evidence ONLY: this asserts the documented 16 px layout box
    // of an inherit-tinted glyph rendered outside any colored ancestor. It
    // does not pin painting; paint-color forwarding is covered by the
    // precedence and lifecycle suites in `tests/ui/asset_seam.rs` (under
    // the repaired semantics, such a glyph resolves the default Window
    // text style when painted).
    let bounds = cx
        .debug_bounds(INHERIT_ICON_SELECTOR)
        .expect("inherit-tinted icon must still lay out");
    assert_square(bounds, px(16.0), INHERIT_ICON_SELECTOR);
}
