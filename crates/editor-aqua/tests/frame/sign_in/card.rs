use aqua::UiApp;
use aqua_ui::element::Box as UiBox;
use editor_aqua::frame::sign_in::SignIn;
use editor_aqua::frame::sign_in::card::{self, Props};
use editor_aqua::frame::sign_in::model::{Feedback, Page, View};
use editor_aqua::shell::host::ShellResources;
use editor_aqua::ui::theme::ArtisanTheme;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn waiting(reduced: bool) -> (SignIn, Instant) {
    let start = Instant::now();
    let mut sign_in = SignIn::default();
    sign_in.set_reduced(reduced);
    sign_in.present(
        Page::Email,
        true,
        (&Feedback::None, &Feedback::None),
        start,
        reduced,
    );
    let settled = start + Duration::from_secs(5);
    sign_in.tick(settled);
    (sign_in, settled)
}

fn pulses(sign_in: &SignIn, now: Instant) -> bool {
    let mut app: UiApp = ShellResources::new(1.0).expect("sign-in resources").ui;
    let element = card::card(
        &ArtisanTheme::resolved(),
        Props {
            view: &View::Sending,
            sign_in,
            email: UiBox::new().build(),
            code: UiBox::new().build(),
            busy: true,
            width: 480.0,
            now,
            emit: Rc::new(|_| {}),
        },
    );
    app.mount(|_| element).expect("mount sign-in");
    app.set_viewport(800.0, 800.0, 1.0);
    app.build_frame().expect("lay out sign-in");
    app.tree_mut()
        .emit_scene()
        .iter()
        .any(|(_, item)| matches!(item, aqua_core::DisplayItem::PushGpuiQuadPulse(_)))
}

#[test]
fn waiting_for_a_link_breathes_on_the_gpu_without_rebuilding_the_card() {
    let (mut sign_in, settled) = waiting(false);
    assert!(!sign_in.animating());
    assert!(!sign_in.tick(settled + Duration::from_millis(400)));
    assert!(pulses(&sign_in, settled));
}

#[test]
fn waiting_for_a_link_holds_still_under_reduced_motion() {
    let (sign_in, settled) = waiting(true);
    assert!(!sign_in.animating());
    assert!(!pulses(&sign_in, settled));
}
