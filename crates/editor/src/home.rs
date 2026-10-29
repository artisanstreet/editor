//! Home: the account's Forges, when none is open yet.

use editor_core::Core;
use editor_settings::account::forges::ForgesView;
use editor_ui::theme::ActiveTheme as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Window, div, px,
};

pub struct Home {
    forges: Entity<ForgesView>,
}

impl Home {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        Self {
            forges: cx.new(|cx| ForgesView::new(core, cx)),
        }
    }
}

impl Render for Home {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .debug_selector(|| "artisan-desktop-home".to_owned())
                    .pb(px(32.0))
                    .child(
                        div()
                            .text_size(px(28.0))
                            .text_color(theme.colors.text.to_paint())
                            .child("Choose a Forge"),
                    )
                    .child(
                        div()
                            .mt(px(8.0))
                            .max_w(px(560.0))
                            .w_full()
                            .child(self.forges.clone()),
                    ),
            )
    }
}
