//! Behavioral coverage for the native vertical transcript scroll viewport.

use artisan_ui::button::FocusVisibility;
use artisan_ui::scroll_area::{
    ROOT_SELECTOR, ScrollArea, ScrollAreaAxis, ScrollAreaStyle, VIEWPORT_SELECTOR,
};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle,
    Styled, TestAppContext, Window, div, point, px, size,
};

const CONTENT_SELECTOR: &str = "scroll-area-content";
const CUSTOM_ROOT_SELECTOR: &str = "scroll-area-custom-root";
const CUSTOM_VIEWPORT_SELECTOR: &str = "scroll-area-custom-root-viewport";
const FIRST_ROOT_SELECTOR: &str = "scroll-area-first-root";
const SECOND_ROOT_SELECTOR: &str = "scroll-area-second-root";
const FIRST_CONTENT_SELECTOR: &str = "scroll-area-first-content";
const SECOND_CONTENT_SELECTOR: &str = "scroll-area-second-content";
const ORDER_ONE_SELECTOR: &str = "scroll-area-order-one";
const ORDER_TWO_SELECTOR: &str = "scroll-area-order-two";
const ORDER_THREE_SELECTOR: &str = "scroll-area-order-three";

struct SingleAreaProbe {
    handle: ScrollHandle,
    focus: FocusHandle,
    theme: ArtisanTheme,
    root_selector: Option<&'static str>,
    width: f32,
    height: f32,
    content_height: f32,
}

impl SingleAreaProbe {
    fn new(
        cx: &mut Context<Self>,
        handle: ScrollHandle,
        theme: &ArtisanTheme,
        root_selector: Option<&'static str>,
        width: f32,
        height: f32,
        content_height: f32,
    ) -> Self {
        Self {
            handle,
            focus: cx.focus_handle(),
            theme: *theme,
            root_selector,
            width,
            height,
            content_height,
        }
    }
}

impl Render for SingleAreaProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut area = ScrollArea::new(self.handle.clone(), self.theme)
            .focus_handle(self.focus.clone())
            .w(px(self.width))
            .h(px(self.height));

        if let Some(selector) = self.root_selector {
            area = area.debug_selector(selector);
        }

        area.child(
            div()
                .w_full()
                .h(px(self.content_height))
                .debug_selector(|| CONTENT_SELECTOR.to_string()),
        )
    }
}

struct PairAreaProbe {
    first: ScrollHandle,
    second: ScrollHandle,
    theme: ArtisanTheme,
}

impl Render for PairAreaProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(240.0))
            .h(px(220.0))
            .child(
                ScrollArea::new(self.first.clone(), self.theme)
                    .debug_selector(FIRST_ROOT_SELECTOR)
                    .w(px(240.0))
                    .h(px(100.0))
                    .child(
                        div()
                            .w_full()
                            .h(px(300.0))
                            .debug_selector(|| FIRST_CONTENT_SELECTOR.to_string()),
                    ),
            )
            .child(
                ScrollArea::new(self.second.clone(), self.theme)
                    .debug_selector(SECOND_ROOT_SELECTOR)
                    .w(px(240.0))
                    .h(px(100.0))
                    .child(
                        div()
                            .w_full()
                            .h(px(180.0))
                            .debug_selector(|| SECOND_CONTENT_SELECTOR.to_string()),
                    ),
            )
    }
}

struct OrderedAreaProbe {
    handle: ScrollHandle,
    theme: ArtisanTheme,
}

impl Render for OrderedAreaProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ScrollArea::new(self.handle.clone(), self.theme)
            .w(px(200.0))
            .h(px(100.0))
            .child(
                div()
                    .w_full()
                    .h(px(20.0))
                    .debug_selector(|| ORDER_ONE_SELECTOR.to_string()),
            )
            .child(
                div()
                    .w_full()
                    .h(px(20.0))
                    .debug_selector(|| ORDER_TWO_SELECTOR.to_string()),
            )
            .child(
                div()
                    .w_full()
                    .h(px(20.0))
                    .debug_selector(|| ORDER_THREE_SELECTOR.to_string()),
            )
    }
}

#[test]
fn defaults_pin_vertical_axis_theme_ring_and_stable_selectors() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let handle = ScrollHandle::new();
        let area = ScrollArea::new(handle.clone(), theme);
        let style = area.visual_style();

        assert_eq!(area.axis(), ScrollAreaAxis::Vertical);
        assert_eq!(area.root_debug_selector(), ROOT_SELECTOR);
        assert_eq!(area.viewport_debug_selector(), VIEWPORT_SELECTOR);
        assert_eq!(area.scroll_handle().offset(), point(px(0.0), px(0.0)));
        assert_eq!(style.focus_ring_width, px(3.0));
        assert_eq!(style.focus_ring_width, theme.interaction.focus_ring_width);
        assert_eq!(
            style.focus_ring_color,
            theme.interaction.focus_ring_color.to_paint()
        );
        assert_eq!(
            style,
            ScrollAreaStyle::resolve(theme),
            "the public recipe and constructor must resolve identically"
        );
    }
}

#[gpui::test]
fn oversized_content_is_clipped_and_shared_handle_moves_without_scrollbar_space(
    cx: &mut TestAppContext,
) {
    let handle = ScrollHandle::new();
    let probe_handle = handle.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        SingleAreaProbe::new(
            cx,
            probe_handle,
            &ArtisanTheme::for_mode(ThemeMode::Dark),
            None,
            240.0,
            120.0,
            480.0,
        )
    });

    let root = cx
        .debug_bounds(ROOT_SELECTOR)
        .expect("scroll-area root must paint inspectable bounds");
    let viewport = cx
        .debug_bounds(VIEWPORT_SELECTOR)
        .expect("scroll-area viewport must paint inspectable bounds");
    let content = cx
        .debug_bounds(CONTENT_SELECTOR)
        .expect("oversized content must retain its full layout bounds");

    assert_eq!(root.size, size(px(240.0), px(120.0)));
    assert_eq!(viewport.size, root.size);
    assert_eq!(content.size, size(px(240.0), px(480.0)));
    assert_eq!(handle.bounds(), viewport);
    assert_eq!(handle.max_offset(), point(px(0.0), px(360.0)));
    assert_eq!(handle.offset(), point(px(0.0), px(0.0)));

    // A zero scrollbar width keeps the viewport at full width and no track or
    // thumb element is fabricated by this leaf.
    assert!(cx.debug_bounds("scroll-area-scrollbar-track").is_none());
    assert!(cx.debug_bounds("scroll-area-scrollbar-thumb").is_none());

    handle.set_offset(point(px(0.0), px(-48.0)));
    cx.update(|_, app| view.update(app, |_, view_cx| view_cx.notify()));
    cx.run_until_parked();
    assert_eq!(handle.offset(), point(px(0.0), px(-48.0)));

    handle.scroll_to_bottom();
    cx.update(|_, app| view.update(app, |_, view_cx| view_cx.notify()));
    cx.run_until_parked();
    assert_eq!(handle.offset(), point(px(0.0), px(-360.0)));
}

#[gpui::test]
fn caller_style_refinement_wins_for_both_root_and_viewport(cx: &mut TestAppContext) {
    let handle = ScrollHandle::new();
    let probe_handle = handle.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        SingleAreaProbe::new(
            cx,
            probe_handle,
            &ArtisanTheme::for_mode(ThemeMode::Light),
            Some(CUSTOM_ROOT_SELECTOR),
            220.0,
            96.0,
            192.0,
        )
    });

    let root = cx
        .debug_bounds(CUSTOM_ROOT_SELECTOR)
        .expect("custom root selector must remain stable");
    let viewport = cx
        .debug_bounds(CUSTOM_VIEWPORT_SELECTOR)
        .expect("custom viewport selector must derive from the root selector");

    assert_eq!(root.size, size(px(220.0), px(96.0)));
    assert_eq!(viewport.size, root.size);

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        let area = ScrollArea::new(handle.clone(), ArtisanTheme::for_mode(ThemeMode::Light))
            .focus_handle(focus.clone())
            .focus_visibility(FocusVisibility::Visible);
        assert!(!area.focus_ring_visible(window));

        window.focus(&focus, app);
        assert!(area.focus_ring_visible(window));
        assert!(
            !area
                .focus_visibility(FocusVisibility::Hidden)
                .focus_ring_visible(window)
        );
    });
}

#[gpui::test]
fn independent_handles_keep_offsets_and_bounds_separate(cx: &mut TestAppContext) {
    let first = ScrollHandle::new();
    let second = ScrollHandle::new();
    let first_probe_handle = first.clone();
    let second_probe_handle = second.clone();
    let (view, cx) = cx.add_window_view(move |_, _| PairAreaProbe {
        first: first_probe_handle,
        second: second_probe_handle,
        theme: ArtisanTheme::for_mode(ThemeMode::Dark),
    });

    assert_eq!(first.max_offset(), point(px(0.0), px(200.0)));
    assert_eq!(second.max_offset(), point(px(0.0), px(80.0)));
    assert_eq!(first.bounds().size, size(px(240.0), px(100.0)));
    assert_eq!(second.bounds().size, size(px(240.0), px(100.0)));

    first.set_offset(point(px(0.0), px(-32.0)));
    cx.update(|_, app| view.update(app, |_, view_cx| view_cx.notify()));
    cx.run_until_parked();

    assert_eq!(first.offset(), point(px(0.0), px(-32.0)));
    assert_eq!(second.offset(), point(px(0.0), px(0.0)));
}

#[gpui::test]
fn child_order_is_preserved_in_the_single_render_pass(cx: &mut TestAppContext) {
    let handle = ScrollHandle::new();
    let probe_handle = handle.clone();
    let (_, cx) = cx.add_window_view(move |_, _| OrderedAreaProbe {
        handle: probe_handle,
        theme: ArtisanTheme::for_mode(ThemeMode::Dark),
    });

    let one = cx
        .debug_bounds(ORDER_ONE_SELECTOR)
        .expect("first child must paint");
    let two = cx
        .debug_bounds(ORDER_TWO_SELECTOR)
        .expect("second child must paint");
    let three = cx
        .debug_bounds(ORDER_THREE_SELECTOR)
        .expect("third child must paint");

    assert_eq!(two.origin.y - one.origin.y, px(20.0));
    assert_eq!(three.origin.y - two.origin.y, px(20.0));
}
