use std::rc::Rc;

use editor_aqua::ui::text::{
    CommandOutcome, EditOutcome, Editor, ListKey, Mode, PasteOutcome, TokenRange,
};

#[test]
fn typing_groups_words_and_restores_selection() {
    let mut editor = Editor::new("", Mode::Document);
    for value in ["o", "n", "e", " ", "t", "w", "o"] {
        assert_eq!(
            editor.replace_input(None, value, None),
            EditOutcome::Changed
        );
    }
    assert!(editor.undo());
    assert_eq!(editor.text(), "one");
    assert!(editor.undo());
    assert_eq!(editor.text(), "");
    assert!(editor.redo());
    assert_eq!(editor.text(), "one");
}

#[test]
fn composition_uses_utf16_and_blocks_undo_until_unmarked() {
    let mut editor = Editor::new("a😀b", Mode::Prompt);
    assert_eq!(
        editor.replace_input(Some(1..3), "候補", Some(0..2)),
        EditOutcome::Changed
    );
    assert_eq!(editor.text(), "a候補b");
    assert_eq!(editor.marked_utf16(), Some(1..3));
    assert_eq!(editor.selected_utf16(), Some(1..3));
    assert_eq!(editor.enter(false), CommandOutcome::Ignored);
    assert!(!editor.undo());
    editor.unmark_text();
    assert!(editor.undo());
    assert_eq!(editor.text(), "a😀b");
    assert_eq!(
        editor.replace_input(Some(2..2), "x", None),
        EditOutcome::Unchanged
    );
}

#[test]
fn tokens_are_filtered_and_deleted_as_a_unit() {
    let mut editor = Editor::new("hi @person!", Mode::Prompt);
    editor.set_tokenizer(Some(Rc::new(|_| {
        vec![TokenRange { range: 3..10 }, TokenRange { range: 4..7 }]
    })));
    assert_eq!(editor.tokens().len(), 1);
    assert!(editor.set_selection(10..10, false));
    editor.move_horizontal(false, false, false);
    assert_eq!(editor.caret(), Some(3));
    assert_eq!(editor.delete_toward(true, false), EditOutcome::Changed);
    assert_eq!(editor.text(), "hi !");
    assert!(editor.undo());
    assert_eq!(editor.text(), "hi @person!");
}

#[test]
fn prompt_prioritizes_file_paste_and_list_keys() {
    let mut editor = Editor::new("", Mode::Prompt);
    let item = aqua_ui::input::ClipboardPaste {
        text: Some("text".into()),
        image: None,
        paths: vec!["a.txt".into()].into(),
    };
    assert!(matches!(editor.paste(&item), PasteOutcome::Files { .. }));
    assert_eq!(editor.text(), "");
    editor.set_list_open(true);
    assert_eq!(editor.enter(false), CommandOutcome::List(ListKey::Accept));
    assert_eq!(editor.tab(), CommandOutcome::List(ListKey::Accept));
    editor.set_list_open(false);
    assert_eq!(editor.tab(), CommandOutcome::Propagate);
}

#[test]
fn limit_rejects_whole_edit_and_truncates_programmatic_text_at_utf8_boundary() {
    let mut editor = Editor::new(&"a".repeat(65535), Mode::SingleLine);
    assert_eq!(
        editor.replace_input(None, "é", None),
        EditOutcome::LimitReached
    );
    assert_eq!(editor.text().len(), 65535);
    assert!(editor.limit_notice());
    editor.set_text(&format!("{}é", "x".repeat(65535)));
    assert_eq!(editor.text().len(), 65535);
    assert!(editor.limit_notice());
}

#[test]
fn non_prompt_file_paste_matches_gpui_text_fallback() {
    let mut editor = Editor::new("", Mode::Document);
    let item = aqua_ui::input::ClipboardPaste {
        text: None,
        image: None,
        paths: vec!["one".into(), "two".into()].into(),
    };
    assert_eq!(
        editor.paste(&item),
        PasteOutcome::Edit(EditOutcome::Changed)
    );
    assert_eq!(editor.text(), "onetwo");
}

#[test]
fn word_and_paragraph_drags_extend_by_the_original_unit() {
    let mut editor = Editor::new("one two three\nlast", Mode::Document);
    editor.pointer_down(5, false, 2);
    editor.pointer_drag(1);
    let expected_end = if aqua_text::navigation::WordBehavior::platform()
        == aqua_text::navigation::WordBehavior::SkipSpaces
    {
        8
    } else {
        7
    };
    assert_eq!(editor.selection(), 0..expected_end);
    assert!(editor.selection_reversed());
    editor.pointer_up();
    editor.pointer_down(1, false, 3);
    editor.pointer_drag(15);
    assert_eq!(editor.selection(), 0..18);
    assert!(!editor.selection_reversed());
}

#[test]
fn text_service_keeps_utf16_ranges_and_selection_direction() {
    use aqua_text::input_service::{InputChange, TextInputService};

    let mut editor = Editor::new("x😀y", Mode::Prompt);
    assert!(editor.set_selection(1..5, true));
    let service: &mut dyn TextInputService = &mut editor;
    assert_eq!(service.selected_utf16_range(false), Some((1..3, true)));
    assert_eq!(service.text_for_utf16_range(1..3), Some("😀"));
    assert_eq!(service.text_for_utf16_range(1..2), None);
    assert_eq!(
        service.replace_utf16_range(None, "候", Some(0..1)),
        InputChange::Changed
    );
    assert_eq!(service.marked_utf16_range(), Some(1..2));
    assert_eq!(service.text(), "x候y");
}

#[test]
fn logical_vertical_navigation_preserves_utf16_goal_across_short_lines() {
    let mut editor = Editor::new("a😀b\nx\na😀b", Mode::Document);
    assert!(editor.set_selection(5..5, false));
    editor.move_vertical(false, false, None);
    assert_eq!(editor.caret(), Some(8));
    editor.move_vertical(false, false, None);
    assert_eq!(editor.caret(), Some(14));
}

#[test]
fn mounted_input_and_composition_edit_editor_once() {
    use aqua_core::{ImeEvent, InputEvent};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::ui::text::mount::{EditorController, EditorEvent};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::cell::RefCell;

    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = events.clone();
    let controller = EditorController::new(Editor::new("", Mode::Prompt), move |event| {
        recorded.borrow_mut().push(event)
    });
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| controller.element(Mode::Prompt, &ArtisanTheme::resolved(), ""))
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| runtime.tree().editor_snapshot(*node).is_some())
        .unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime.handle_input(&InputEvent::TextInput { text: "a".into() });
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Update {
        marked: "候".into(),
        cursor: Some((3, 3)),
    }));
    assert!(runtime.tree().editor_snapshot(node).unwrap().composing);
    assert_eq!(controller.state().borrow().marked_range(), Some(1..4));
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Commit { text: "候".into() }));
    assert_eq!(controller.state().borrow().text(), "a候");
    assert_eq!(runtime.tree().editor_snapshot(node).unwrap().text, "a候");
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|event| **event == EditorEvent::Changed)
            .count(),
        2
    );
    assert!(controller.state().borrow_mut().undo());
    assert_eq!(controller.state().borrow().text(), "a");
}

#[test]
fn mounted_preedit_without_cursor_follows_editor_source_and_submits() {
    use aqua_core::{ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::ui::text::mount::{EditorController, EditorEvent};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::cell::RefCell;

    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = events.clone();
    let controller = EditorController::new(Editor::new("", Mode::Prompt), move |event| {
        recorded.borrow_mut().push(event)
    });
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| controller.element(Mode::Prompt, &ArtisanTheme::resolved(), ""))
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| runtime.tree().editor_snapshot(*node).is_some())
        .unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Update {
        marked: "候".into(),
        cursor: None,
    }));
    assert_eq!(controller.state().borrow().marked_range(), None);
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    assert!(events.borrow().contains(&EditorEvent::Submitted));
}

#[test]
fn mounted_vertical_keys_update_authoritative_selection() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::ui::text::mount::EditorController;
    use editor_aqua::ui::theme::ArtisanTheme;

    let controller = EditorController::new(Editor::new("one\nx\nlong", Mode::Document), |_| {});
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| controller.element(Mode::Document, &ArtisanTheme::resolved(), ""))
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| runtime.tree().editor_snapshot(*node).is_some())
        .unwrap();
    runtime.tree_mut().set_focused(Some(node));
    let up = InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Up),
        modifiers: Modifiers::NONE,
        repeat: false,
    });
    runtime.handle_input(&up);
    assert_eq!(controller.state().borrow().caret(), Some(5));
    runtime.build_frame().unwrap();
    runtime.handle_input(&up);
    assert_eq!(controller.state().borrow().caret(), Some(3));
    assert_eq!(
        runtime
            .tree()
            .editor_snapshot(node)
            .unwrap()
            .selection
            .caret,
        3
    );
}

#[test]
fn mounted_pointer_placement_changes_authoritative_selection() {
    use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::ui::text::mount::EditorController;
    use editor_aqua::ui::theme::ArtisanTheme;

    let controller = EditorController::new(Editor::new("hello", Mode::Document), |_| {});
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| controller.element(Mode::Document, &ArtisanTheme::resolved(), ""))
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| runtime.tree().editor_snapshot(*node).is_some())
        .unwrap();
    let bounds = runtime.tree().rect(node).unwrap();
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(bounds.x + 1.0, bounds.y + 8.0),
        buttons: 1,
    };
    runtime.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
    runtime.handle_input(&InputEvent::PointerUp(pointer, PointerButton::Primary));
    assert_eq!(controller.state().borrow().caret(), Some(0));
    assert_eq!(
        runtime
            .tree()
            .editor_snapshot(node)
            .unwrap()
            .selection
            .caret,
        0
    );
}

#[test]
fn mounted_cut_changes_authoritative_state_once_and_is_undoable() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::ui::text::mount::EditorController;
    use editor_aqua::ui::theme::ArtisanTheme;

    let controller = EditorController::new(Editor::new("hello", Mode::Document), |_| {});
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| controller.element(Mode::Document, &ArtisanTheme::resolved(), ""))
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| runtime.tree().editor_snapshot(*node).is_some())
        .unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('a'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    assert_eq!(runtime.selected_text().as_deref(), Some("hello"));
    assert_eq!(runtime.cut_external_selection(), Some(true));
    assert_eq!(controller.state().borrow().text(), "");
    assert_eq!(runtime.tree().editor_snapshot(node).unwrap().text, "");
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('z'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    assert_eq!(controller.state().borrow().text(), "hello");
    assert_eq!(runtime.tree().editor_snapshot(node).unwrap().text, "hello");
}

#[test]
fn shared_controller_reports_selection_and_controls_disabled_state() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::ui::text::mount::{EditorController, EditorEvent};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::cell::RefCell;

    let shared = Rc::new(RefCell::new(Editor::new("abc", Mode::Prompt)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = events.clone();
    let controller = EditorController::from_shared(shared.clone(), move |event| {
        recorded.borrow_mut().push(event)
    });
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| controller.element(Mode::Prompt, &ArtisanTheme::resolved(), ""))
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| runtime.tree().editor_snapshot(*node).is_some())
        .unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Left),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    assert_eq!(shared.borrow().caret(), Some(2));
    assert!(events.borrow().contains(&EditorEvent::SelectionChanged {
        range: 2..2,
        reversed: false,
        caret: Some(2)
    }));
    assert!(controller.set_disabled(runtime.tree_mut(), node, true));
    assert!(shared.borrow().is_disabled());
    assert!(runtime.tree().is_disabled(node));
    runtime.handle_input(&InputEvent::TextInput { text: "x".into() });
    assert_eq!(shared.borrow().text(), "abc");
    assert!(controller.set_disabled(runtime.tree_mut(), node, false));
    assert!(!runtime.tree().is_disabled(node));
}

#[test]
fn styled_tokenizer_caches_each_revision_and_retains_atomic_ranges() {
    use editor_aqua::ui::text::StyledToken;
    use std::cell::Cell;

    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let mut editor = Editor::new("@person", Mode::Prompt);
    editor.set_styled_tokenizer(Some(Rc::new(move |text| {
        observed.set(observed.get() + 1);
        if text.starts_with('@') {
            vec![StyledToken {
                range: 0..text.len(),
                color: None,
                font_weight: Some(aqua_text::FontWeight::MEDIUM),
                glyph: None,
            }]
        } else {
            Vec::new()
        }
    })));
    assert_eq!(editor.tokens().len(), 1);
    assert_eq!(editor.tokens().len(), 1);
    assert_eq!(calls.get(), 1);
    assert_eq!(editor.delete_toward(false, false), EditOutcome::Changed);
    assert_eq!(editor.text(), "");
    assert!(editor.tokens().is_empty());
    assert_eq!(calls.get(), 2);
    assert!(editor.undo());
    assert_eq!(editor.tokens().len(), 1);
    assert_eq!(calls.get(), 3);
}

#[test]
fn token_runs_keep_text_and_preserve_sigil_cell_and_glyph() {
    use editor_aqua::ui::text::{StyledToken, tokens::compile_tokens};
    use editor_aqua::ui::theme::ArtisanTheme;

    let theme = ArtisanTheme::resolved();
    let token = StyledToken {
        range: 3..10,
        color: Some(theme.colors.mention_skill),
        font_weight: Some(aqua_text::FontWeight::MEDIUM),
        glyph: Some(editor_assets::AssetId::TABLER_FILE_SPARK),
    };
    let compiled = compile_tokens(
        "hi @person!",
        &aqua_text::TextStyle::default(),
        theme.colors.text,
        &[token],
        |_| 24.0,
    )
    .unwrap();
    assert_eq!(
        compiled
            .spans
            .iter()
            .map(|span| span.text.as_str())
            .collect::<String>(),
        "hi @person!"
    );
    assert_eq!(compiled.spans[1].inline_cell_width(), Some(24.0));
    assert_eq!(
        compiled.spans[1].source_color_hsla,
        Some([0.0, 0.0, 0.0, 0.0])
    );
    assert_eq!(
        compiled.spans[2].style.weight,
        aqua_text::FontWeight::MEDIUM
    );
    let source = theme.colors.mention_skill.to_reference_text_color();
    assert_eq!(
        compiled.spans[2].source_color_hsla,
        Some([
            source.hue_degrees,
            source.saturation,
            source.lightness,
            source.alpha
        ])
    );
    assert_eq!(compiled.glyphs[0].range, 3..4);
}

#[test]
fn keyed_editor_leaf_retains_focus_and_selection_across_notice_rebuild() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::ui::text::mount::EditorController;
    use editor_aqua::ui::theme::ArtisanTheme;

    let controller = EditorController::new(Editor::new("abc", Mode::Prompt), |_| {});
    let theme = ArtisanTheme::resolved();
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            controller.element_with_notice_keyed(
                "composer-prompt",
                Mode::Prompt,
                &theme,
                "",
                false,
                false,
            )
        })
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let scroll = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            matches!(
                runtime.tree().get(*node).map(|record| &record.kind),
                Some(aqua_ui::tree::NodeKind::Scroll(_))
            )
        })
        .unwrap();
    let leaf = runtime.tree().get(scroll).unwrap().children[0];
    runtime.tree_mut().set_focused(Some(leaf));
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Left),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    runtime
        .refresh(vec![controller.element_with_notice_keyed(
            "composer-prompt",
            Mode::Prompt,
            &theme,
            "",
            false,
            false,
        )])
        .unwrap();
    let scroll_after = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            matches!(
                runtime.tree().get(*node).map(|record| &record.kind),
                Some(aqua_ui::tree::NodeKind::Scroll(_))
            )
        })
        .unwrap();
    assert_eq!(runtime.tree().get(scroll_after).unwrap().children[0], leaf);
    assert_eq!(runtime.tree().focused(), Some(leaf));
    assert_eq!(controller.state().borrow().caret(), Some(2));
}

#[test]
fn mounted_mentions_paint_as_glyphs_and_follow_atomic_undo() {
    use aqua_core::{DisplayItem, InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use aqua_text::{FontWeight, SharedFonts, SharedRuns};
    use editor_aqua::ui::text::{StyledToken, mount::EditorController};
    use editor_aqua::ui::theme::ArtisanTheme;

    let theme = ArtisanTheme::resolved();
    let controller = EditorController::new(Editor::new("@name", Mode::Prompt), |_| {});
    controller.set_tokenizer(Some(Rc::new(move |text| {
        if text == "@name" {
            vec![StyledToken {
                range: 0..5,
                color: Some(theme.colors.mention_connector),
                font_weight: Some(FontWeight::MEDIUM),
                glyph: Some(editor_assets::AssetId::TABLER_FILE_SPARK),
            }]
        } else {
            Vec::new()
        }
    })));
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| controller.element(Mode::Prompt, &theme, ""))
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| runtime.tree().editor_snapshot(*node).is_some())
        .unwrap();
    runtime.tree_mut().set_focused(Some(node));
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .filter(|(_, item)| matches!(item, DisplayItem::GpuiSvg { .. }))
            .count(),
        1
    );
    let glyph = runtime
        .scene()
        .ordered()
        .find_map(|(_, item)| match item {
            DisplayItem::GpuiSvg { bounds, .. } => Some(*bounds),
            _ => None,
        })
        .unwrap();
    let expected_tint = theme
        .colors
        .mention_connector
        .at(editor_aqua::ui::theme::Opacity::MentionGlyph)
        .to_reference_paint();
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::GpuiSvg { tint: Some(tint), .. } if *tint == expected_tint)));
    let slot = runtime.text_position_rect(node, 0).unwrap();
    assert!((glyph.x - slot.x).abs() < 0.01);
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Backspace),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    runtime.build_frame().unwrap();
    assert_eq!(controller.state().borrow().text(), "");
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .filter(|(_, item)| matches!(item, DisplayItem::GpuiSvg { .. }))
            .count(),
        0
    );
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('z'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    runtime.build_frame().unwrap();
    assert_eq!(controller.state().borrow().text(), "@name");
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .filter(|(_, item)| matches!(item, DisplayItem::GpuiSvg { .. }))
            .count(),
        1
    );
}

#[test]
fn keyed_prompt_caps_height_and_scrolls_to_keyboard_caret() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::ui::text::mount::EditorController;
    use editor_aqua::ui::theme::ArtisanTheme;

    let controller = EditorController::new(Editor::new(&"line\n".repeat(30), Mode::Prompt), |_| {});
    let theme = ArtisanTheme::resolved();
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            controller.element_with_notice_keyed("prompt", Mode::Prompt, &theme, "", false, false)
        })
        .unwrap();
    runtime.set_viewport(400.0, 400.0, 1.0);
    runtime.build_frame().unwrap();
    let scroll = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            matches!(
                runtime.tree().get(*node).map(|record| &record.kind),
                Some(aqua_ui::tree::NodeKind::Scroll(_))
            )
        })
        .unwrap();
    let leaf = runtime.tree().get(scroll).unwrap().children[0];
    assert!(runtime.tree().rect(scroll).unwrap().h <= 160.0 + 0.01);
    assert!(
        runtime.tree().content_size(scroll).unwrap().h > runtime.tree().rect(scroll).unwrap().h
    );
    runtime.tree_mut().set_focused(Some(leaf));
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::End),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    runtime.build_frame().unwrap();
    assert!(runtime.tree().scroll_offset(scroll).unwrap().y > 0.0);
}

#[test]
fn suggestion_hint_preserves_prompt_focus_and_tab_accepts_full_text() {
    use aqua_core::{
        InputEvent, Key, KeyEvent, Modifiers, NamedKey, Point, PointerButton, PointerEvent,
        PointerKind,
    };
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::tree::NodeKind;
    use editor_aqua::ui::text::mount::EditorController;
    use editor_aqua::ui::theme::ArtisanTheme;

    let controller = EditorController::new(Editor::new("", Mode::Prompt), |_| {});
    let theme = ArtisanTheme::resolved();
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    let build = || {
        controller.element_with_notice_keyed("prompt", Mode::Prompt, &theme, "Write", false, false)
    };
    runtime.mount(|_| build()).unwrap();
    runtime.set_viewport(180.0, 160.0, 1.0);
    runtime.build_frame().unwrap();
    let leaf = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| runtime.tree().editor_snapshot(*node).is_some())
        .unwrap();
    runtime.tree_mut().set_focused(Some(leaf));
    let suggestion = "This suggestion is long enough to need ellipsis";
    controller.set_suggestion(Some(suggestion.into()));
    runtime.refresh(vec![build()]).unwrap();
    runtime.build_frame().unwrap();
    assert_eq!(runtime.tree().focused(), Some(leaf));
    assert!(runtime.tree().traverse().into_iter().any(|node| matches!(&runtime.tree().get(node).unwrap().kind, NodeKind::Text(text) if text.text() == suggestion)));
    let bounds = runtime.tree().rect(leaf).unwrap();
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(bounds.x + 15.0, bounds.y + 16.0),
        buttons: 1,
    };
    runtime.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
    runtime.handle_input(&InputEvent::PointerUp(pointer, PointerButton::Primary));
    assert_eq!(runtime.tree().focused(), Some(leaf));
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    assert_eq!(controller.state().borrow().text(), suggestion);
    runtime.refresh(vec![build()]).unwrap();
    runtime.build_frame().unwrap();
    assert_eq!(runtime.tree().focused(), Some(leaf));
    assert_eq!(runtime.tree().traverse().into_iter().filter(|node| matches!(&runtime.tree().get(*node).unwrap().kind, NodeKind::Text(text) if text.text() == suggestion)).count(), 1);
}

#[test]
fn native_text_service_queries_live_selection_and_invalidates_after_edit() {
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::ui::text::mount::EditorController;
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::cell::RefCell;

    let model = Rc::new(RefCell::new(Editor::new("abc", Mode::Document)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let output = events.clone();
    let controller =
        EditorController::from_shared(model.clone(), move |event| output.borrow_mut().push(event));
    let theme = ArtisanTheme::resolved();
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| controller.element(Mode::Document, &theme, ""))
        .unwrap();
    runtime.set_viewport(400.0, 400.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(node));
    let service = runtime.native_text_service().unwrap();
    let at_end = service.selected_bounds().unwrap();
    model.borrow_mut().set_selection(0..0, false);
    let at_start = service.selected_bounds().unwrap();
    assert!(at_start.x < at_end.x);
    service.replace_utf16("候", Some(1..1));
    assert_eq!(model.borrow().text(), "候abc");
    assert_eq!(model.borrow().marked_utf16(), Some(0..1));
    assert!(service.selected_bounds().is_none());
    assert!(
        events
            .borrow()
            .iter()
            .any(|event| matches!(event, editor_aqua::ui::text::mount::EditorEvent::Changed))
    );
}

#[test]
fn inline_source_paints_follow_shaped_slot_and_preserve_layer_order() {
    use aqua_core::{DisplayItem, GpuiHsla};
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::Text;
    use aqua_ui::tree::{TextInlinePaint, TextInlinePaintContent};

    let color = GpuiHsla {
        h: 0.2,
        s: 0.5,
        l: 0.4,
        a: 1.0,
    };
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Text::new("a\u{2003}b")
                .inline_paints(vec![TextInlinePaint {
                    bytes: 1..4,
                    content: TextInlinePaintContent::Swatch {
                        fill: color,
                        edge: color,
                        centered: true,
                    },
                }])
                .build()
        })
        .unwrap();
    runtime.set_viewport(400.0, 100.0, 1.0);
    runtime.build_frame().unwrap();
    let items: Vec<_> = runtime.scene().ordered().map(|(_, item)| item).collect();
    let text = items
        .iter()
        .position(|item| matches!(item, DisplayItem::Text { .. }))
        .unwrap();
    let fill = items
        .iter()
        .position(|item| matches!(item, DisplayItem::GpuiBackground { .. }))
        .unwrap();
    let border = items
        .iter()
        .position(|item| matches!(item, DisplayItem::GpuiBorder { .. }))
        .unwrap();
    assert!(text < fill && fill < border);
    match (items[fill], items[border]) {
        (
            DisplayItem::GpuiBackground { bounds: a, .. },
            DisplayItem::GpuiBorder {
                bounds: b, widths, ..
            },
        ) => {
            assert_eq!(a, b);
            assert!(a.w > 0.0 && a.h > 0.0);
            assert_eq!(widths.left, 1.0);
        }
        _ => unreachable!(),
    }
}

#[test]
fn secondary_text_click_reports_shaped_byte_without_selecting() {
    use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::Text;
    use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
    use std::cell::RefCell;

    let hits = Rc::new(RefCell::new(Vec::new()));
    let recorded = hits.clone();
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Text::new("hello world").selectable(true).build().on_input(
                InputKind::TextContext,
                EventPhase::Bubble,
                move |event| {
                    if let RoutedInput::TextContext { byte, .. } = &event.input {
                        recorded.borrow_mut().push(*byte);
                    }
                    InputResponse::handled()
                },
            )
        })
        .unwrap();
    runtime.set_viewport(400.0, 100.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime.tree().root().unwrap();
    let caret = runtime.text_position_rect(node, 6).unwrap();
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(caret.x + 1.0, caret.y + caret.h / 2.0),
        buttons: 2,
    };
    assert!(runtime.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Secondary)));
    assert_eq!(&*hits.borrow(), &[6]);
    assert!(runtime.selected_text().is_none());
}

#[test]
fn prompt_switches_raw_placeholder_paint_to_raw_input_paint() {
    use aqua_core::{DisplayItem, InputEvent};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::ui::text::mount::EditorController;
    use editor_aqua::ui::theme::ArtisanTheme;

    let theme = ArtisanTheme::resolved();
    let controller = EditorController::new(Editor::new("", Mode::Prompt), |_| {});
    let mut runtime = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| controller.element(Mode::Prompt, &theme, "Write here"))
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::PushGpuiSourceTextColor(color) if *color == theme.colors.text_muted.to_reference_text_color())));
    let leaf = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| runtime.tree().editor_snapshot(*node).is_some())
        .unwrap();
    runtime.tree_mut().set_focused(Some(leaf));
    runtime.handle_input(&InputEvent::TextInput { text: "a".into() });
    runtime.build_frame().unwrap();
    assert!(runtime.scene().ordered().any(|(_, item)| matches!(item, DisplayItem::PushGpuiSourceTextColor(color) if *color == theme.colors.text.to_reference_text_color())));
}
