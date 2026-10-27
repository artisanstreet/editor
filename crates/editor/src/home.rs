//! Home: the account's Forges. Choosing and connecting happens here;
//! enrollment itself lives in Account settings and the entry gate.

use crate::entry::setup::SetupView;
use editor_core::{Core, Screen, SettingsPage};
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::theme::ActiveTheme as _;
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div, px,
};

pub struct Home {
    core: Entity<Core>,
    setup: Entity<SetupView>,
    _events: Subscription,
}

impl Home {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        let setup = cx.new(|cx| SetupView::new(core.clone(), false, cx));
        let events = cx.subscribe(&core, |_, _, _: &editor_core::CoreEvent, cx| {
            cx.notify();
        });
        Self {
            core,
            setup,
            _events: events,
        }
    }
}

impl Render for Home {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let core = self.core.clone();
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
                            .child(self.setup.clone()),
                    )
                    .child(
                        div()
                            .mt(px(16.0))
                            .child(
                                Button::text(
                                    "home-manage-forges",
                                    theme,
                                    "Manage Forges in Account settings",
                                )
                                .variant(ButtonVariant::Ghost)
                                .debug_selector("home-manage-forges")
                                .on_activate(cx.listener(move |_, _, _, cx| {
                                    core.update(cx, |core, cx| {
                                        core.navigate(
                                            Screen::Settings(SettingsPage::Account),
                                            cx,
                                        );
                                    });
                                }))
                                .into_any_element(),
                            ),
                    ),
            )
            .into_any_element()
    }
}
