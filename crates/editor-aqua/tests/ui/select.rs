use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use aqua_core::{ImeEvent, InputEvent, Role};
use aqua_text::{SharedFonts, SharedRuns};
use editor_aqua::ui::select::{Select, SelectEntry, SelectState};
use editor_aqua::ui::theme::ArtisanTheme;

#[test]
fn typeahead_matches_composed_and_decomposed_accents() {
    let state = Rc::new(RefCell::new(SelectState::new(Instant::now())));
    let entries = vec![
        SelectEntry::item("dark", "Dark"),
        SelectEntry::item("eclair", "\u{c9}clair"),
        SelectEntry::item("ember", "Ember"),
    ];
    let select_state = Rc::clone(&state);
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(move |_| {
            Select::new(
                "theme",
                ArtisanTheme::resolved(),
                Some("dark"),
                entries.clone(),
                Rc::clone(&select_state),
            )
            .build()
        })
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let trigger = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            runtime
                .tree()
                .get(*node)
                .is_some_and(|record| record.semantics.role == Role::ComboBox)
        })
        .unwrap();
    runtime.tree_mut().set_focused(Some(trigger));
    runtime.handle_input(&InputEvent::TextInput {
        text: "e\u{301}".into(),
    });
    assert_eq!(state.borrow().highlighted(), Some(&"eclair"));
    state.borrow_mut().set_open(false);
    state.borrow_mut().set_highlighted(Some("dark"));
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Commit { text: "em".into() }));
    assert_eq!(state.borrow().highlighted(), Some(&"ember"));
}
