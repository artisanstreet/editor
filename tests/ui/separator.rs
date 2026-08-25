//! Behavioral coverage for the static native GPUI separator primitive.

use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::theme::{ArtisanTheme, Oklch, SurfaceStep, ThemeMode};
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px,
};

const HOST_SELECTOR: &str = "separator-host";
const SEPARATOR_SELECTOR: &str = "separator-under-test";
const LEADING_SELECTOR: &str = "separator-leading-probe";
const TRAILING_SELECTOR: &str = "separator-trailing-probe";

/// The reached status-row composition: fixed siblings around a flexible rule.
struct FlexibleHorizontalProbe;

impl Render for FlexibleHorizontalProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let border = ArtisanTheme::for_mode(ThemeMode::Dark)
            .colors
            .border
            .to_paint();
        div()
            .flex()
            .flex_row()
            .items_center()
            .w(px(320.0))
            .h(px(20.0))
            .debug_selector(|| HOST_SELECTOR.to_string())
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(40.0))
                    .h(px(8.0))
                    .debug_selector(|| LEADING_SELECTOR.to_string()),
            )
            .child(
                separator(border, SeparatorAxis::Horizontal)
                    .flex_1()
                    .min_w_0()
                    .debug_selector(|| SEPARATOR_SELECTOR.to_string()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(56.0))
                    .h(px(8.0))
                    .debug_selector(|| TRAILING_SELECTOR.to_string()),
            )
    }
}

/// The documented legacy vertical recipe: 1 px wide and full-height.
struct VerticalProbe;

impl Render for VerticalProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let border = ArtisanTheme::for_mode(ThemeMode::Light)
            .colors
            .border
            .to_paint();
        div()
            .w(px(20.0))
            .h(px(120.0))
            .debug_selector(|| HOST_SELECTOR.to_string())
            .child(
                separator(border, SeparatorAxis::Vertical)
                    .debug_selector(|| SEPARATOR_SELECTOR.to_string()),
            )
    }
}

/// A caller override proving later width refinements replace `w_full`.
struct WidthOverrideProbe;

impl Render for WidthOverrideProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let border = ArtisanTheme::for_mode(ThemeMode::Dark)
            .colors
            .border
            .to_paint();
        div().size_full().child(
            separator(border, SeparatorAxis::Horizontal)
                .w(px(72.0))
                .debug_selector(|| SEPARATOR_SELECTOR.to_string()),
        )
    }
}

#[test]
fn default_axis_is_horizontal() {
    assert_eq!(SeparatorAxis::default(), SeparatorAxis::Horizontal);
}

#[test]
fn separator_paint_comes_from_the_exact_mode_border_token() {
    let light = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark = ArtisanTheme::for_mode(ThemeMode::Dark);

    assert_eq!(light.colors.border, SurfaceStep::S200.oklch());
    assert_eq!(
        dark.colors.border,
        Oklch::new(1.0, 0.0, 0.0).with_alpha(0.10)
    );
    assert_ne!(
        light.colors.border.to_paint(),
        dark.colors.border.to_paint()
    );

    let horizontal = separator(light.colors.border.to_paint(), SeparatorAxis::default());
    let vertical = separator(dark.colors.border.to_paint(), SeparatorAxis::Vertical);
    let _ = (horizontal, vertical);
}

#[gpui::test]
fn horizontal_separator_is_one_pixel_and_fills_status_row_remainder(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| FlexibleHorizontalProbe);

    let host = cx
        .debug_bounds(HOST_SELECTOR)
        .expect("host must paint inspectable bounds");
    let leading = cx
        .debug_bounds(LEADING_SELECTOR)
        .expect("leading probe must paint inspectable bounds");
    let separator = cx
        .debug_bounds(SEPARATOR_SELECTOR)
        .expect("separator must paint inspectable bounds");
    let trailing = cx
        .debug_bounds(TRAILING_SELECTOR)
        .expect("trailing probe must paint inspectable bounds");

    assert_eq!(separator.size.height, px(1.0));
    assert_eq!(separator.origin.x, leading.origin.x + leading.size.width);
    assert_eq!(trailing.origin.x, separator.origin.x + separator.size.width);
    assert_eq!(separator.size.width, px(224.0));
    assert_eq!(
        separator.origin.y - host.origin.y,
        (host.size.height - separator.size.height) / 2.0
    );
}

#[gpui::test]
fn vertical_separator_is_one_pixel_wide_and_uses_full_host_height(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| VerticalProbe);

    let host = cx
        .debug_bounds(HOST_SELECTOR)
        .expect("host must paint inspectable bounds");
    let separator = cx
        .debug_bounds(SEPARATOR_SELECTOR)
        .expect("separator must paint inspectable bounds");

    assert_eq!(separator.size.width, px(1.0));
    assert_eq!(separator.size.height, host.size.height);
}

#[gpui::test]
fn caller_width_refinement_overrides_horizontal_full_width(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| WidthOverrideProbe);

    let separator = cx
        .debug_bounds(SEPARATOR_SELECTOR)
        .expect("separator must paint inspectable bounds");

    assert_eq!(separator.size.width, px(72.0));
    assert_eq!(separator.size.height, px(1.0));
}
