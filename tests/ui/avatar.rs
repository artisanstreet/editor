//! Focused behavioral coverage for the native GPUI Avatar primitive.

use artisan_ui::avatar::{Avatar, AvatarImageState, AvatarStyle, DEFAULT_DEBUG_SELECTOR};
use artisan_ui::theme::{ArtisanTheme, SurfaceStep, ThemeMode};
use gpui::{Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div, px};

const AVATAR_SELECTOR: &str = "avatar-under-test";
const LOADED_SELECTOR: &str = "avatar-loaded";
const PENDING_SELECTOR: &str = "avatar-pending";
const FAILED_SELECTOR: &str = "avatar-failed";
const ABSENT_SELECTOR: &str = "avatar-absent";

#[test]
fn style_resolves_default_geometry_typography_and_muted_theme_paints() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    let light_style = AvatarStyle::resolve(light);
    let dark_style = AvatarStyle::resolve(dark);

    assert_eq!(light_style.diameter, px(32.0));
    assert_eq!(light_style.corner_radius, px(9999.0));
    assert_eq!(light_style.text_size, px(12.0));
    assert_eq!(light_style.line_height, px(16.0));
    assert_eq!(light_style.background, SurfaceStep::S100.oklch().to_paint());
    assert_eq!(dark_style.background, SurfaceStep::S800.oklch().to_paint());
    assert_eq!(light_style.background, light.colors.muted.to_paint());
    assert_eq!(dark_style.background, dark.colors.muted.to_paint());
    assert_eq!(
        light_style.foreground,
        light.colors.muted_foreground.to_paint()
    );
    assert_eq!(
        dark_style.foreground,
        dark.colors.muted_foreground.to_paint()
    );
    assert_ne!(light_style.background, dark_style.background);
    assert_ne!(light_style.foreground, dark_style.foreground);
}

struct DefaultAvatarProbe;

impl Render for DefaultAvatarProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(Avatar::new(ArtisanTheme::for_mode(ThemeMode::Light), "AB"))
    }
}

#[gpui::test]
fn default_avatar_is_32px_round_clipped_and_fallback_is_centered(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| DefaultAvatarProbe);

    let root = cx
        .debug_bounds(DEFAULT_DEBUG_SELECTOR)
        .expect("default avatar selector must be stable");
    let fallback = cx
        .debug_bounds("artisan-avatar-fallback")
        .expect("fallback branch must be present without an image");

    assert_eq!(root.size.width, px(32.0));
    assert_eq!(root.size.height, px(32.0));
    assert_eq!(fallback.size.width, px(32.0));
    assert_eq!(fallback.size.height, px(32.0));
    assert_eq!(fallback.origin, root.origin);
}

struct StateBranchProbe;

impl Render for StateBranchProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        div().size_full().flex().flex_col().children([
            Avatar::new(theme, "L")
                .debug_selector(LOADED_SELECTOR)
                .image(div().size_full(), AvatarImageState::Loaded),
            Avatar::new(theme, "P")
                .debug_selector(PENDING_SELECTOR)
                .image(div().size_full(), AvatarImageState::Pending),
            Avatar::new(theme, "F")
                .debug_selector(FAILED_SELECTOR)
                .image(div().size_full(), AvatarImageState::Failed),
            Avatar::new(theme, "A").debug_selector(ABSENT_SELECTOR),
        ])
    }
}

#[gpui::test]
fn only_loaded_state_with_an_image_selects_the_image_branch(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| StateBranchProbe);

    assert!(cx.debug_bounds("avatar-loaded-image").is_some());
    for selector in [
        "avatar-loaded-fallback",
        "avatar-pending-image",
        "avatar-failed-image",
        "avatar-absent-image",
    ] {
        assert!(
            cx.debug_bounds(selector).is_none(),
            "unexpected image branch selector {selector}"
        );
    }
    for selector in [
        "avatar-pending-fallback",
        "avatar-failed-fallback",
        "avatar-absent-fallback",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "expected fallback branch selector {selector}"
        );
    }
}

struct RefinedAvatarProbe;

impl Render for RefinedAvatarProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            Avatar::new(ArtisanTheme::for_mode(ThemeMode::Light), "A")
                .debug_selector(AVATAR_SELECTOR)
                .w(px(48.0))
                .h(px(40.0))
                .rounded(px(6.0)),
        )
    }
}

#[gpui::test]
fn caller_size_and_rounding_refinements_win_on_the_root(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| RefinedAvatarProbe);
    let root = cx
        .debug_bounds(AVATAR_SELECTOR)
        .expect("refined avatar selector must be present");

    assert_eq!(root.size.width, px(48.0));
    assert_eq!(root.size.height, px(40.0));
}
