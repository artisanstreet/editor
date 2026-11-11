use aqua::UiApp;
use aqua_core::DisplayItem;
use aqua_text::{SharedFonts, SharedRuns, TextStyle};
use editor_aqua::ui::{
    shimmer_text::{ShimmerText, ShimmerTiming},
    theme::ArtisanTheme,
};
use std::time::Duration;

fn sweeps(app: &mut UiApp) -> Vec<(u64, Duration, Duration)> {
    app.tree_mut()
        .emit_scene()
        .into_iter()
        .filter_map(|(_, item)| match item {
            DisplayItem::PushGpuiTextShimmer(shimmer) => {
                Some((shimmer.key, shimmer.duration, shimmer.delay))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn status_text_shimmers_from_its_first_frame_and_only_while_active() {
    let theme = ArtisanTheme::resolved();
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = UiApp::new(fonts, SharedRuns::new());
    let render = |text: &str, active: bool, reduced_motion: bool| {
        ShimmerText::new("status", text, theme)
            .timing(ShimmerTiming::Status)
            .active(active)
            .reduced_motion(reduced_motion)
            .build(TextStyle::default())
    };
    app.mount(|_| render("Thinking", true, false)).unwrap();
    app.set_viewport(200.0, 30.0, 1.0);
    app.build_frame().unwrap();
    let first = sweeps(&mut app);
    assert_eq!(
        first.iter().map(|sweep| (sweep.1, sweep.2)).collect::<Vec<_>>(),
        vec![(Duration::from_secs(3), Duration::from_millis(1_500))],
        "a status sweeps for three seconds after a pause"
    );

    app.refresh(vec![render("Working", true, false)]).unwrap();
    app.build_frame().unwrap();
    assert_eq!(
        sweeps(&mut app),
        first,
        "new status text keeps the same sweep clock"
    );

    app.refresh(vec![render("Working", true, true)]).unwrap();
    app.build_frame().unwrap();
    assert!(sweeps(&mut app).is_empty(), "reduced motion stops the sweep");

    app.refresh(vec![render("Worked", false, false)]).unwrap();
    app.build_frame().unwrap();
    assert!(sweeps(&mut app).is_empty(), "a settled status is still");
}
