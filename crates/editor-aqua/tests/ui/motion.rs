use aqua::{UiApp, prelude::DisplayItem};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::Box as UiBox;
use editor_aqua::ui::motion::WorkingPulse;

#[test]
fn working_pulse_keeps_mount_clock_and_reduced_motion_removes_scope() {
    let pulse = WorkingPulse::default();
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(50.0)
            .height(50.0)
            .child(pulse.wrap(UiBox::new().width(6.0).height(6.0).build(), false))
            .build()
    })
    .unwrap();
    app.set_viewport(50.0, 50.0, 1.0);
    app.build_frame().unwrap();
    let scene = |app: &mut UiApp| {
        app.tree_mut()
            .emit_scene()
            .into_iter()
            .map(|(_, item)| item)
            .collect::<Vec<_>>()
    };
    let first = scene(&mut app);
    let start = first
        .iter()
        .find_map(|item| match item {
            DisplayItem::PushGpuiQuadPulse(pulse) => Some(pulse.start_ms),
            _ => None,
        })
        .unwrap();
    let end = first
        .iter()
        .position(|item| matches!(item, DisplayItem::PopGpuiQuadPulse))
        .unwrap();
    let begin = first
        .iter()
        .position(|item| matches!(item, DisplayItem::PushGpuiQuadPulse(_)))
        .unwrap();
    assert!(begin < end);
    let second = scene(&mut app);
    assert!(second.iter().any(
        |item| matches!(item, DisplayItem::PushGpuiQuadPulse(pulse) if pulse.start_ms == start)
    ));
    let mut reduced = UiApp::new(SharedFonts::new(), SharedRuns::new());
    reduced
        .mount(|_| {
            UiBox::new()
                .width(50.0)
                .height(50.0)
                .child(pulse.wrap(UiBox::new().width(6.0).height(6.0).build(), true))
                .build()
        })
        .unwrap();
    reduced.set_viewport(50.0, 50.0, 1.0);
    reduced.build_frame().unwrap();
    assert!(!scene(&mut reduced).iter().any(|item| matches!(
        item,
        DisplayItem::PushGpuiQuadPulse(_) | DisplayItem::PopGpuiQuadPulse
    )));
}
