use super::*;
use aqua_text::{SharedFonts, SharedRuns};

#[test]
fn the_progress_bar_exposes_its_numeric_progress() {
    let bar = bar(&ArtisanTheme::resolved(), 142.0, HEIGHT);
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| UiBox::new().child(bar).build()).unwrap();
    app.set_viewport(200.0, 50.0, 1.0);
    app.build_frame().unwrap();
    let progress = app
        .tree()
        .emit_semantics()
        .into_iter()
        .find(|node| node.role == Role::ProgressIndicator)
        .unwrap();
    assert_eq!(
        progress.numeric_range,
        Some(NumericRangeSemantics {
            min: 0.0,
            max: 100.0,
            value: 100.0,
            step: 1.0,
        })
    );
    assert_eq!(progress.value_text.as_deref(), Some("100%"));
}
