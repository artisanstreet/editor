//! External behavior probes for the seamless native title-bar leaf.
//!
//! Exercises only the public production shell module through a small
//! test-only `Render` host that returns `shell_title_bar(theme, None, ...)`
//! without duplicating the layout. Covers the `h-10` strip, the drag region
//! sibling layout (the caption cluster must sit beside the drag area, never
//! inside it, or the backend's first-hitbox-wins `WM_NCHITTEST` resolution
//! would shadow the native Min/Max/Close areas with `HTCAPTION`), the fixed
//! 138 px trailing cluster, Windows button order, and trailing-edge flush.

use artisan_frontend::shell::{
    LEGACY_SHELL_TITLE_BAR_SELECTOR, LEGACY_SHELL_TITLE_CLOSE_SELECTOR,
    LEGACY_SHELL_TITLE_CONTROLS_SELECTOR, LEGACY_SHELL_TITLE_DRAG_SELECTOR,
    LEGACY_SHELL_TITLE_MAXIMIZE_SELECTOR, LEGACY_SHELL_TITLE_MINIMIZE_SELECTOR,
    LEGACY_TITLE_BAR_CONTROL_WIDTH_PX, shell_title_bar,
};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{Bounds, Context, Pixels, Render, Window, px};

struct TitleBarHost {
    theme: ArtisanTheme,
    inspector_width_px: Option<f32>,
}

impl Render for TitleBarHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        shell_title_bar(self.theme, None, self.inspector_width_px)
    }
}

fn bounds(cx: &mut gpui::VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} must paint inspectable bounds"))
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "the assert pins the shared constant to its exact reference width; an epsilon would not verify the literal"
)]
fn caption_cluster_width_is_three_windows_buttons() {
    assert_eq!(LEGACY_TITLE_BAR_CONTROL_WIDTH_PX, 46.0);
}

#[gpui::test]
fn rendered_title_bar_keeps_drag_and_controls_as_siblings(cx: &mut gpui::TestAppContext) {
    let (_, cx) = cx.add_window_view(|_window, _cx| TitleBarHost {
        theme: ArtisanTheme::for_mode(ThemeMode::Dark),
        inspector_width_px: None,
    });

    let strip = bounds(cx, LEGACY_SHELL_TITLE_BAR_SELECTOR);
    let drag = bounds(cx, LEGACY_SHELL_TITLE_DRAG_SELECTOR);
    let controls = bounds(cx, LEGACY_SHELL_TITLE_CONTROLS_SELECTOR);
    let minimize = bounds(cx, LEGACY_SHELL_TITLE_MINIMIZE_SELECTOR);
    let maximize = bounds(cx, LEGACY_SHELL_TITLE_MAXIMIZE_SELECTOR);
    let close = bounds(cx, LEGACY_SHELL_TITLE_CLOSE_SELECTOR);

    assert_eq!(strip.size.height, px(40.0));

    // The drag region starts at the strip's leading edge and ends exactly
    // where the caption cluster begins: siblings, never nested.
    assert_eq!(drag.origin.x, strip.origin.x);
    assert_eq!(drag.origin.x + drag.size.width, controls.origin.x);
    assert_eq!(drag.size.height, strip.size.height);

    // Fixed 138 px cluster flush with the strip's trailing edge.
    assert_eq!(
        controls.size.width,
        px(LEGACY_TITLE_BAR_CONTROL_WIDTH_PX * 3.0)
    );
    assert_eq!(controls.size.height, strip.size.height);
    assert_eq!(
        controls.origin.x + controls.size.width,
        strip.origin.x + strip.size.width
    );

    // Windows caption order, each button full strip height.
    for button in [minimize, maximize, close] {
        assert_eq!(button.size.width, px(LEGACY_TITLE_BAR_CONTROL_WIDTH_PX));
        assert_eq!(button.size.height, strip.size.height);
        assert_eq!(button.origin.y, strip.origin.y);
    }
    assert_eq!(minimize.origin.x, controls.origin.x);
    assert_eq!(maximize.origin.x, minimize.origin.x + minimize.size.width);
    assert_eq!(close.origin.x, maximize.origin.x + maximize.size.width);
    assert_eq!(
        close.origin.x + close.size.width,
        strip.origin.x + strip.size.width
    );
}

#[gpui::test]
fn rendered_title_bar_light_mode_keeps_chrome_geometry(cx: &mut gpui::TestAppContext) {
    let (_, mut cx) = cx.add_window_view(|_window, _cx| TitleBarHost {
        theme: ArtisanTheme::for_mode(ThemeMode::Light),
        inspector_width_px: None,
    });

    let strip = bounds(cx, LEGACY_SHELL_TITLE_BAR_SELECTOR);
    let controls = bounds(cx, LEGACY_SHELL_TITLE_CONTROLS_SELECTOR);
    let close = bounds(cx, LEGACY_SHELL_TITLE_CLOSE_SELECTOR);

    assert_eq!(strip.size.height, px(40.0));
    assert_eq!(
        controls.size.width,
        px(LEGACY_TITLE_BAR_CONTROL_WIDTH_PX * 3.0)
    );
    assert_eq!(
        close.origin.x + close.size.width,
        strip.origin.x + strip.size.width
    );
}
