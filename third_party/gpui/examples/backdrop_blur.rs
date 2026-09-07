use gpui::{
    App, Application, Bounds, Context, Window, WindowBounds, WindowOptions, div, hsla, prelude::*,
    px, rgb, size,
};

struct BackdropBlur {}

impl Render for BackdropBlur {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Three saturated bands form the backdrop; the translucent card in
        // front blurs whatever is behind it.
        div()
            .size_full()
            .flex()
            .flex_row()
            .children(vec![
                div().flex_1().bg(rgb(0xef4444)),
                div().flex_1().bg(rgb(0x22c55e)),
                div().flex_1().bg(rgb(0x3b82f6)),
            ])
            .child(
                div()
                    .absolute()
                    .left(px(80.))
                    .top(px(60.))
                    .w(px(320.))
                    .h(px(140.))
                    .rounded(px(16.))
                    .bg(hsla(0.0, 0.0, 1.0, 0.35))
                    .backdrop_blur(px(16.)),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.0), px(600.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| BackdropBlur {}),
        )
        .unwrap();

        cx.activate(true);
    });
}
