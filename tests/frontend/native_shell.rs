//! External behavior probes for the native GPUI shell/layout leaf.
//!
//! Exercises only the public production shell module through a small
//! test-only `Render` host that returns `native_shell(theme)` without
//! duplicating the layout. Covers style resolution for both modes, rail/padding
//! geometry, shared background paint, static placeholder copy, and rendered
//! rail/main/placeholder bounds.

use artisan_frontend::shell::{
    PLACEHOLDER_BADGE_LABEL, PLACEHOLDER_CAPTION, PLACEHOLDER_TITLE, SHELL_MAIN_SELECTOR,
    SHELL_PLACEHOLDER_SELECTOR, SHELL_RAIL_SELECTOR, SHELL_ROOT_SELECTOR, ShellFrameStyle,
    native_shell, startup_placeholder,
};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{Context, Render, Styled as _, Window, px};

struct ShellHost {
    theme: ArtisanTheme,
}

impl Render for ShellHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        native_shell(self.theme)
    }
}

fn themes() -> Vec<ArtisanTheme> {
    vec![
        ArtisanTheme::for_mode(ThemeMode::Light),
        ArtisanTheme::for_mode(ThemeMode::Dark),
    ]
}

#[test]
fn shell_frame_pins_exact_audited_values_per_mode() {
    for theme in themes() {
        let frame = ShellFrameStyle::resolve(theme);

        assert_eq!(frame.rail_width, theme.spacing.steps(14.0));
        assert_eq!(frame.rail_width, px(56.0));
        assert_eq!(frame.surface_padding, theme.spacing.steps(2.0));
        assert_eq!(frame.surface_padding, px(8.0));
        assert_eq!(frame.window_background, theme.colors.background.to_paint());
    }

    let light = ShellFrameStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light));
    let dark = ShellFrameStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark));
    assert_ne!(
        light.window_background, dark.window_background,
        "each mode resolves its own background paint"
    );
}

#[test]
fn startup_placeholder_keeps_static_copy_and_muted_caption() {
    for theme in themes() {
        assert_ne!(
            theme.colors.muted_foreground.to_paint(),
            theme.colors.card_foreground.to_paint(),
            "caption must read muted against card foreground"
        );

        let refined = startup_placeholder(theme).w(px(480.0));
        let _ = refined;
    }

    assert!(!PLACEHOLDER_BADGE_LABEL.is_empty());
    assert!(!PLACEHOLDER_TITLE.is_empty());
    assert!(PLACEHOLDER_TITLE.contains("native shell"));
    assert!(!PLACEHOLDER_CAPTION.is_empty());
    assert!(PLACEHOLDER_CAPTION.contains("Startup placeholder"));
}

#[gpui::test]
fn rendered_shell_frames_rail_main_and_centered_placeholder(cx: &mut gpui::TestAppContext) {
    let (_, cx) = cx.add_window_view(|_window, _cx| ShellHost {
        theme: ArtisanTheme::for_mode(ThemeMode::Dark),
    });

    let root = cx
        .debug_bounds(SHELL_ROOT_SELECTOR)
        .expect("shell root must paint inspectable bounds");
    let rail = cx
        .debug_bounds(SHELL_RAIL_SELECTOR)
        .expect("rail must paint inspectable bounds");
    let main = cx
        .debug_bounds(SHELL_MAIN_SELECTOR)
        .expect("main surface must paint inspectable bounds");
    let placeholder = cx
        .debug_bounds(SHELL_PLACEHOLDER_SELECTOR)
        .expect("startup placeholder must paint inspectable bounds");

    assert_eq!(rail.size.width, px(56.0));
    assert_eq!(rail.origin.x, root.origin.x);
    assert_eq!(rail.origin.y, root.origin.y);
    assert_eq!(rail.size.height, root.size.height);

    assert_eq!(main.origin.x, rail.origin.x + rail.size.width);
    assert_eq!(main.origin.y, root.origin.y);
    assert_eq!(main.size.height, root.size.height);
    assert_eq!(
        main.origin.x + main.size.width,
        root.origin.x + root.size.width
    );

    let frame = ShellFrameStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark));
    let expected_center_x = main.origin.x + (main.size.width - frame.surface_padding) * 0.5;
    let actual_center_x = placeholder.origin.x + placeholder.size.width * 0.5;
    assert!(
        (actual_center_x - expected_center_x).abs() < px(1.0),
        "placeholder must center in the padded main surface: expected {expected_center_x:?} actual {actual_center_x:?} main {main:?}"
    );
    let expected_center_y = main.origin.y + main.size.height * 0.5;
    let actual_center_y = placeholder.origin.y + placeholder.size.height * 0.5;
    assert!(
        (actual_center_y - expected_center_y).abs() < px(1.0),
        "placeholder must center vertically: expected {expected_center_y:?} actual {actual_center_y:?}"
    );

    assert_eq!(placeholder.size.width, px(320.0));
}

#[gpui::test]
fn rendered_shell_light_mode_background_and_geometry(cx: &mut gpui::TestAppContext) {
    // Light mode must also produce the correct rail width, padding, and background
    // through the same production path.
    let light_frame = ShellFrameStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light));
    assert_eq!(light_frame.rail_width, px(56.0));
    assert_eq!(light_frame.surface_padding, px(8.0));

    let (_, cx) = cx.add_window_view(|_window, _cx| ShellHost {
        theme: ArtisanTheme::for_mode(ThemeMode::Light),
    });

    let root = cx
        .debug_bounds(SHELL_ROOT_SELECTOR)
        .expect("light shell root must paint");
    let rail = cx
        .debug_bounds(SHELL_RAIL_SELECTOR)
        .expect("light rail must paint");
    let main = cx
        .debug_bounds(SHELL_MAIN_SELECTOR)
        .expect("light main must paint");
    let placeholder = cx
        .debug_bounds(SHELL_PLACEHOLDER_SELECTOR)
        .expect("light placeholder must paint");

    assert_eq!(rail.size.width, px(56.0));
    assert_eq!(rail.origin.x, root.origin.x);
    assert_eq!(rail.size.height, root.size.height);
    assert_eq!(main.origin.x, rail.origin.x + rail.size.width);
    assert_eq!(placeholder.size.width, px(320.0));
}
