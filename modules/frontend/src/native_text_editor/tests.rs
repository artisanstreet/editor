//! Behaviour tests for the native text editor.

#![forbid(unsafe_code)]

use std::{cell::Cell, rc::Rc};

use gpui::{
    ClipboardItem, Entity, EntityInputHandler as _, Subscription, TestAppContext,
    VisualTestContext, px, size,
};

use super::text::{logical_column, logical_vertical_target, truncate_to_bytes, word_range_at};
use super::{
    NATIVE_TEXT_EDITOR_PLACEHOLDER_SELECTOR, NATIVE_TEXT_EDITOR_SELECTOR, NativeTextEditor,
    NativeTextEditorEvent,
};

fn mount<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
) -> (Entity<NativeTextEditor>, &'a mut VisualTestContext) {
    let text = text.to_owned();
    let (view, cx) =
        cx.add_window_view(move |_, cx| NativeTextEditor::new(&text, "Write a prompt", cx));
    cx.simulate_resize(size(px(800.0), px(600.0)));
    cx.update(|window, app| {
        NativeTextEditor::bind_actions(app);
        let focus = view.read(app).focus_handle.clone();
        window.focus(&focus, app);
    });
    cx.run_until_parked();
    (view, cx)
}

fn count_changes(
    cx: &mut VisualTestContext,
    view: &Entity<NativeTextEditor>,
) -> (Rc<Cell<usize>>, Subscription) {
    let changes = Rc::new(Cell::new(0));
    let observed = changes.clone();
    let subscription = cx.update(|_, app| {
        app.subscribe(view, move |_, event: &NativeTextEditorEvent, _| {
            if *event == NativeTextEditorEvent::Changed {
                observed.set(observed.get() + 1);
            }
        })
    });
    (changes, subscription)
}

fn text(cx: &mut VisualTestContext, view: &Entity<NativeTextEditor>) -> String {
    cx.update(|_, app| view.read(app).text().to_owned())
}

#[gpui::test]
fn typing_reads_back_and_emits_changed(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx, "");
    assert!(cx.debug_bounds(NATIVE_TEXT_EDITOR_SELECTOR).is_some());
    assert!(
        cx.debug_bounds(NATIVE_TEXT_EDITOR_PLACEHOLDER_SELECTOR)
            .is_some()
    );
    let (changes, _subscription) = count_changes(cx, &view);

    cx.simulate_input("hello");
    assert_eq!(text(cx, &view), "hello");
    assert_eq!(changes.get(), 5);
    assert!(
        cx.debug_bounds(NATIVE_TEXT_EDITOR_PLACEHOLDER_SELECTOR)
            .is_none()
    );
}

#[gpui::test]
fn enter_inserts_newline_and_tab_inserts_two_spaces(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx, "one");
    cx.simulate_keystrokes("enter tab");
    cx.simulate_input("two");
    assert_eq!(text(cx, &view), "one\n  two");
}

#[gpui::test]
fn backspace_joins_lines_across_a_newline(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx, "one\ntwo");
    cx.simulate_keystrokes("home backspace");
    assert_eq!(text(cx, &view), "onetwo");
    cx.update(|_, app| assert_eq!(view.read(app).selection, 3..3));
    cx.simulate_keystrokes("delete");
    assert_eq!(text(cx, &view), "onewo");
}

#[gpui::test]
fn select_all_then_cut_empties_and_fills_the_clipboard(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx, "line one\nline two");
    cx.simulate_keystrokes("ctrl-a ctrl-x");
    assert_eq!(text(cx, &view), "");
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("line one\nline two".to_owned())
    );
}

#[gpui::test]
fn paste_inserts_clipboard_text_at_the_caret(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx, "ab");
    cx.write_to_clipboard(ClipboardItem::new_string("X\r\nY".to_owned()));
    cx.simulate_keystrokes("left ctrl-v");
    assert_eq!(text(cx, &view), "aX\nYb");
    cx.update(|_, app| assert_eq!(view.read(app).selection, 4..4));
}

#[gpui::test]
fn undo_restores_and_redo_reapplies(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx, "");
    let (changes, _subscription) = count_changes(cx, &view);
    cx.simulate_input("hello");
    cx.simulate_keystrokes("space");
    cx.simulate_input("world");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("x");
    assert_eq!(text(cx, &view), "hello world\nx");

    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(cx, &view), "hello world\n");
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(cx, &view), "hello world");
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(cx, &view), "hello");
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(cx, &view), "");

    cx.simulate_keystrokes("ctrl-shift-z");
    assert_eq!(text(cx, &view), "hello");
    cx.simulate_keystrokes("ctrl-y ctrl-y ctrl-y");
    assert_eq!(text(cx, &view), "hello world\nx");
    assert_eq!(changes.get(), 13 + 8);

    // A new edit after undo drops the redo branch.
    cx.simulate_keystrokes("ctrl-z");
    cx.simulate_input("y");
    cx.simulate_keystrokes("ctrl-y");
    assert_eq!(text(cx, &view), "hello world\ny");
}

#[gpui::test]
fn input_beyond_max_bytes_is_truncated_or_refused(cx: &mut TestAppContext) {
    let almost_full = "a".repeat(NativeTextEditor::MAX_BYTES - 3);
    let (view, cx) = mount(cx, &almost_full);
    let (changes, _subscription) = count_changes(cx, &view);

    // "é" is two bytes: one fits, the second would leave one byte, which
    // cannot hold it, so the paste is cut at the character boundary.
    cx.write_to_clipboard(ClipboardItem::new_string("ééé".to_owned()));
    cx.simulate_keystrokes("ctrl-v");
    let after_paste = text(cx, &view);
    assert_eq!(after_paste.len(), NativeTextEditor::MAX_BYTES - 1);
    assert!(after_paste.ends_with('é'));
    assert_eq!(changes.get(), 1);

    cx.simulate_input("é");
    assert_eq!(text(cx, &view), after_paste);
    cx.simulate_input("z");
    assert_eq!(text(cx, &view).len(), NativeTextEditor::MAX_BYTES);
    cx.simulate_input("z");
    assert_eq!(text(cx, &view).len(), NativeTextEditor::MAX_BYTES);
    assert_eq!(changes.get(), 2);

    // Construction and set_text clamp too.
    let oversized = "b".repeat(NativeTextEditor::MAX_BYTES + 10);
    cx.update(|_, app| {
        view.update(app, |editor, editor_cx| {
            editor.set_text(&oversized, editor_cx);
        });
    });
    assert_eq!(text(cx, &view).len(), NativeTextEditor::MAX_BYTES);
}

#[gpui::test]
fn set_text_clears_undo_and_does_not_emit_changed(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx, "");
    cx.simulate_input("typed");
    let (changes, _subscription) = count_changes(cx, &view);
    cx.update(|_, app| {
        view.update(app, |editor, editor_cx| {
            editor.set_text("loaded\ntext", editor_cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(changes.get(), 0);
    cx.update(|_, app| assert_eq!(view.read(app).selection, 11..11));

    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(cx, &view), "loaded\ntext");
    assert_eq!(changes.get(), 0);
}

#[gpui::test]
fn disabled_editor_ignores_input(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx, "keep");
    let (changes, _subscription) = count_changes(cx, &view);
    cx.update(|_, app| {
        view.update(app, |editor, editor_cx| {
            editor.set_disabled(true, editor_cx);
        });
    });
    cx.run_until_parked();
    cx.simulate_input("x");
    cx.simulate_keystrokes("backspace enter ctrl-a ctrl-x");
    cx.update(|window, app| {
        view.update(app, |editor, editor_cx| {
            editor.replace_text_in_range(None, "y", window, editor_cx);
        });
        assert!(view.read(app).caret_quad(window).is_none());
    });
    assert_eq!(text(cx, &view), "keep");
    assert_eq!(changes.get(), 0);

    cx.update(|_, app| {
        view.update(app, |editor, editor_cx| {
            editor.set_disabled(false, editor_cx);
        });
    });
    cx.run_until_parked();
    cx.update(|window, app| {
        let focus = view.read(app).focus_handle.clone();
        window.focus(&focus, app);
    });
    cx.run_until_parked();
    cx.simulate_input("!");
    assert_eq!(text(cx, &view), "keep!");
}

#[gpui::test]
fn ime_composition_marks_then_commits_as_one_edit(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx, "a");
    cx.update(|window, app| {
        view.update(app, |editor, editor_cx| {
            editor.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, editor_cx);
            assert_eq!(editor.marked_range, Some(1..3));
            editor.replace_and_mark_text_in_range(None, "nih", Some(3..3), window, editor_cx);
            assert_eq!(editor.marked_range, Some(1..4));
            editor.replace_text_in_range(None, "你", window, editor_cx);
            assert_eq!(editor.marked_range, None);
        });
    });
    assert_eq!(text(cx, &view), "a你");
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(cx, &view), "a");
}

#[gpui::test]
fn up_and_down_move_between_lines_keeping_the_goal(cx: &mut TestAppContext) {
    let (view, cx) = mount(cx, "abcdef\nab\nabcdef");
    // Caret at the end of line one, column 4.
    cx.simulate_keystrokes("ctrl-home right right right right");
    cx.simulate_keystrokes("down");
    cx.update(|_, app| assert_eq!(view.read(app).selection, 9..9));
    cx.simulate_keystrokes("down");
    cx.update(|_, app| assert_eq!(view.read(app).selection, 14..14));
    cx.simulate_keystrokes("up up");
    cx.update(|_, app| assert_eq!(view.read(app).selection, 4..4));
    cx.simulate_keystrokes("shift-down");
    cx.update(|_, app| {
        let editor = view.read(app);
        assert_eq!(editor.selection, 4..9);
        assert!(!editor.selection_reversed);
    });
}

#[test]
fn logical_vertical_target_keeps_the_goal_column() {
    let text = "abcdef\nab\nabcdef";
    let goal = logical_column(text, 4);
    assert_eq!(goal, 4);
    let middle = logical_vertical_target(text, 4, false, goal);
    assert_eq!(middle, 9);
    assert_eq!(logical_vertical_target(text, middle, false, goal), 14);
    assert_eq!(logical_vertical_target(text, middle, true, goal), 4);
    assert_eq!(logical_vertical_target(text, 2, true, 2), 2);
    assert_eq!(logical_vertical_target(text, 14, false, 4), 14);
    // Multibyte columns count UTF-16 units and never split a character.
    assert_eq!(logical_vertical_target("a😀b\nabcd", 7, true, 2), 1);
    assert_eq!(logical_vertical_target("a😀b\nabcd", 7, true, 3), 5);
}

#[test]
fn helpers_respect_character_boundaries() {
    assert_eq!(truncate_to_bytes("aé", 2), "a");
    assert_eq!(truncate_to_bytes("aé", 3), "aé");
    assert_eq!(word_range_at("hello world", 2), 0..5);
    assert_eq!(word_range_at("hello world", 5), 5..6);
    assert_eq!(word_range_at("hello world", 11), 6..11);
    assert_eq!(word_range_at("a\n\nb", 2), 2..2);
}

#[gpui::test]
fn down_follows_soft_wrapped_visual_lines(cx: &mut TestAppContext) {
    let long_line = "word ".repeat(80);
    let (view, cx) = mount(cx, &long_line);
    cx.simulate_resize(size(px(240.0), px(600.0)));
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-home down");
    cx.update(|_, app| {
        let editor = view.read(app);
        assert!(editor.painted_bounds.is_some());
        let offset = editor.selection.end;
        assert!(editor.selection.is_empty());
        assert!(
            offset > 0 && offset < long_line.len(),
            "down must land on the second visual row of the one logical line, got {offset}"
        );
    });
    cx.simulate_keystrokes("up");
    cx.update(|_, app| assert_eq!(view.read(app).selection, 0..0));
}
