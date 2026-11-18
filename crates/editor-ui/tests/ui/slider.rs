use super::*;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::tree::{ActionEvent, ActionKind};

#[test]
fn assistive_tech_reads_and_sets_the_slider_range() {
    let state = Rc::new(RefCell::new(SliderState::new(
        SliderRange::new(0.0, 10.0, Some(1.0)),
        4.0,
    )));
    let changes = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&changes);
    let slider = Slider::new(
        "threshold",
        ArtisanTheme::resolved(),
        "Threshold",
        "4",
        state.clone(),
    )
    .on_change(move |value| sink.borrow_mut().push(value))
    .build();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| slider).unwrap();
    app.set_viewport(400.0, 100.0, 1.0);
    app.build_frame().unwrap();
    let node = app
        .tree()
        .emit_semantics()
        .into_iter()
        .find(|node| node.role == Role::Slider && node.label == "Threshold")
        .unwrap();
    assert_eq!(
        node.numeric_range,
        Some(NumericRangeSemantics {
            min: 0.0,
            max: 10.0,
            value: 4.0,
            step: 1.0,
        })
    );
    for request in [
        AccessibilityRequest::SetValue(7.2),
        AccessibilityRequest::Increment,
        AccessibilityRequest::Decrement,
        AccessibilityRequest::Decrement,
    ] {
        app.tree().fire_action(ActionEvent {
            source: None,
            node: node.node,
            kind: ActionKind::AccessibilityRequest,
            text: None,
            focused: None,
            request: Some(request),
        });
    }
    assert_eq!(changes.borrow().as_slice(), &[7.0, 8.0, 7.0, 6.0]);
    assert_eq!(state.borrow().value(), 6.0);
}
