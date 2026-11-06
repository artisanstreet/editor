use super::*;

#[test]
fn preview_selection_includes_full_clipped_and_offscreen_lines() {
    use aqua_text::document::SelectionDocument;
    let long = "x".repeat(16 * 1024 + 12);
    let input = format!("first\n{long}\nlast\n");
    let content = PreviewText::new(input.clone(), 1);
    let prepared = prepare_text(&content, 1);
    assert_eq!(prepared.ranges.len(), 3);
    assert_eq!(prepared.text.as_ref(), input.trim_end_matches('\n'));
    let mut selection = SelectionDocument::new();
    selection
        .register(SelectionSource {
            id: SelectionSourceId(3),
            order: vec![0],
            text: Arc::clone(&prepared.text),
            separator: Arc::from(""),
            copies: Vec::new(),
        })
        .unwrap();
    assert!(selection.select_all());
    assert_eq!(
        selection.selected_text().as_deref(),
        Some(input.trim_end_matches('\n'))
    );
    assert_eq!(&prepared.text[prepared.ranges[1].clone()], long);
}

#[test]
fn selected_window_is_bounded_and_keeps_target() {
    let content = PreviewText::new("x\n".repeat(100_000), 80_000);
    let prepared = prepare_text(&content, 80_000);
    assert_eq!(prepared.start_line, 77_500);
    assert_eq!(prepared.ranges.len(), 5_000);
    assert_eq!(&prepared.text[prepared.ranges[2_500].clone()], "x");
}

#[test]
fn mounted_copy_actions_keep_full_file_and_generation() {
    use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind, Size};
    use aqua_text::{SharedFonts, SharedRuns};
    use std::cell::RefCell;
    let mut preview = ResourcePreview::new("src/file.rs".into(), Some(2), None);
    preview.content = Some(Ok(PreviewText::new("first\nsecond\n".into(), 1)));
    let mut state = ViewState::new(7, SelectionDocumentId(23), SelectionSourceId(24));
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    assert!(state.prepare(&preview, &mut app).unwrap());
    assert!(!state.prepare(&preview, &mut app).unwrap());
    assert_eq!(state.pending_jump(), Some(1));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let dialog = state
        .dialog(
            &preview,
            ArtisanTheme::resolved(),
            900.0,
            Rc::new(move |action| sink.borrow_mut().push(action)),
        )
        .unwrap();
    app.mount(|_| dialog.build_panel(Size::new(1000.0, 900.0)))
        .unwrap();
    app.set_viewport(1000.0, 900.0, 1.0);
    app.build_frame().unwrap();
    for key in ["resource-copy-path", "resource-copy-text"] {
        let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
        let button = app
            .tree()
            .traverse()
            .into_iter()
            .find(|id| {
                app.tree()
                    .get(*id)
                    .is_some_and(|node| node.key == Some(key))
            })
            .unwrap();
        let bounds = app.tree().rect(button).unwrap();
        let pointer = PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
            buttons: 1,
        };
        app.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
        app.handle_input(&InputEvent::PointerUp(pointer, PointerButton::Primary));
    }
    assert_eq!(
        &*events.borrow(),
        &[
            Action::CopyPath {
                generation: 7,
                text: "src/file.rs".into()
            },
            Action::CopyFile {
                generation: 7,
                text: Arc::from("first\nsecond\n")
            }
        ]
    );
    state.jumped(0);
    assert_eq!(state.pending_jump(), Some(1));
    state.jumped(1);
    assert_eq!(state.pending_jump(), None);
    state.release(&mut app);
    assert!(
        state
            .dialog(&preview, ArtisanTheme::resolved(), 900.0, Rc::new(|_| {}))
            .is_err()
    );
}

#[test]
fn refreshing_body_retains_dialog_and_control_focus() {
    use crate::ui::dialog::DialogHost;
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::Box as UiBox;

    let mut preview = ResourcePreview::new("src/file.rs".into(), None, None);
    preview.content = Some(Ok(PreviewText::new("line\n".into(), 0)));
    let mut state = ViewState::new(4, SelectionDocumentId(9), SelectionSourceId(10));
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(900.0)
            .height(700.0)
            .build()
            .focusable(true)
    })
    .unwrap();
    app.set_viewport(900.0, 700.0, 1.0);
    app.build_frame().unwrap();
    let owner = app.tree().root().unwrap();
    assert!(app.tree_mut().set_focused(Some(owner)));
    state.prepare(&preview, &mut app).unwrap();
    let emit: Rc<dyn Fn(Action)> = Rc::new(|_| {});
    let theme = ArtisanTheme::resolved();
    let dialog = state
        .dialog(&preview, theme, 700.0, Rc::clone(&emit))
        .unwrap();
    let host = DialogHost::open(&mut app, owner, dialog).unwrap();
    assert!(app.focus_key("resource-copy-text"));
    let focused = app.tree().focused();
    let content = state.content(&preview, theme, 700.0, emit).unwrap();
    host.refresh_content(&mut app, content).unwrap();
    assert!(host.is_open(&app));
    assert_eq!(app.tree().focused(), focused);
    host.dispose(&mut app);
    state.release(&mut app);
    assert_eq!(app.tree().focused(), Some(owner));
}

#[test]
fn target_line_jump_waits_for_layout_and_aligns_to_start() {
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::{
        element::Key,
        tree::{NodeKind, hash_key},
    };

    let mut preview = ResourcePreview::new("src/file.rs".into(), Some(51), None);
    preview.content = Some(Ok(PreviewText::new("line\n".repeat(100), 50)));
    let mut state = ViewState::new(8, SelectionDocumentId(31), SelectionSourceId(32));
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    state.prepare(&preview, &mut app).unwrap();
    assert_eq!(state.pending_jump(), Some(50));
    assert!(!state.sync_jump(&mut app));
    let content = state
        .content(&preview, ArtisanTheme::resolved(), 400.0, Rc::new(|_| {}))
        .unwrap();
    app.mount(|_| content).unwrap();
    app.set_viewport(600.0, 400.0, 1.0);
    app.build_frame().unwrap();
    assert!(state.sync_jump(&mut app));
    assert_eq!(state.pending_jump(), None);
    let key = hash_key(Key::from("resource-lines"));
    let list = app
        .tree()
        .traverse()
        .into_iter()
        .find(|id| {
            app.tree()
                .get(*id)
                .is_some_and(|node| node.key == Some(key))
        })
        .unwrap();
    assert!(
        matches!(&app.tree().get(list).unwrap().kind, NodeKind::Scroll(scroll) if (scroll.offset.y - 1200.0).abs() < 0.01)
    );
    state.release(&mut app);
}
