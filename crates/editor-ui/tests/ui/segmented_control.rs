use super::*;
use aqua_core::{KeyEvent, Modifiers};
use aqua_text::{SharedFonts, SharedRuns};

#[test]
fn arrow_keys_move_selection_and_the_focused_option_together() {
    let selected = Rc::new(Cell::new(0_usize));
    let build = |value: usize| {
        let sink = Rc::clone(&selected);
        SegmentedControl::new(
            "modes",
            ArtisanTheme::resolved(),
            "Comparison mode",
            value,
            Segment::new(0, "Slider"),
        )
        .options([Segment::new(1, "Side by side"), Segment::new(2, "Toggle")])
        .on_change(move |value| sink.set(value))
        .build()
    };
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| build(0)).unwrap();
    app.set_viewport(400.0, 100.0, 1.0);
    app.build_frame().unwrap();
    assert!(app.focus_key("modes"));
    let right = KeyEvent {
        key: InputKey::Named(NamedKey::Right),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    for (value, label) in [(1, "Side by side"), (2, "Toggle")] {
        app.handle_input(&InputEvent::KeyDown(right));
        app.handle_input(&InputEvent::KeyUp(right));
        assert_eq!(selected.get(), value);
        app.refresh(vec![build(selected.get())]).unwrap();
        app.build_frame().unwrap();
        let semantics = app.tree().emit_semantics();
        let option = semantics
            .iter()
            .find(|node| node.role == Role::RadioButton && node.label == label)
            .unwrap();
        assert_eq!(option.checked, Some(true));
        let group = semantics
            .iter()
            .find(|node| node.role == Role::RadioGroup)
            .unwrap();
        assert_eq!(app.tree().focused(), Some(group.node));
        assert_eq!(group.active_descendant, Some(option.node));
    }
}
