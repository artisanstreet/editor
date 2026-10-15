use super::press_key;
use crate::{
    button::{Button, ButtonVariant, LabelOverflow},
    theme::ArtisanTheme,
};
use editor_assets::AssetId;
use gpui::{
    Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext, Window, div, px,
};

struct ReferenceFixture {
    clicks: usize,
}
impl Render for ReferenceFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(128.0)).flex().child(
            Button::icon_text(
                "bounded-reference",
                ArtisanTheme::default(),
                AssetId::TABLER_FILE_TEXT,
                "a/very/long/host/path/that/must/keep/the/final/file.rs:120",
            )
            .variant(ButtonVariant::Ghost)
            .label_overflow(LabelOverflow::Start)
            .debug_selector("bounded-reference")
            .on_activate(cx.listener(|this, _, _, cx| {
                this.clicks += 1;
                cx.notify();
            })),
        )
    }
}

#[gpui::test]
fn narrow_reference_keeps_geometry_and_keyboard_activation(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| ReferenceFixture { clicks: 0 });
    let before = cx
        .debug_bounds("bounded-reference")
        .expect("reference painted");
    assert!(
        before.size.width <= px(128.0),
        "long label must shrink inside its parent"
    );
    assert!(
        before.size.width > px(40.0),
        "icon and usable label remain visible"
    );
    cx.simulate_click(before.center(), gpui::Modifiers::none());
    assert!(cx.update(|window, cx| window.focused(cx)).is_some());
    press_key(cx, "enter");
    press_key(cx, "space");
    view.read_with(cx, |view, _| assert_eq!(view.clicks, 3));
    assert_eq!(
        before,
        cx.debug_bounds("bounded-reference").unwrap(),
        "focus ring must preserve geometry"
    );
}
