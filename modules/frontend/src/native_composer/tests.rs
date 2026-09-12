//! Focused behavior tests for the native composer editor.
//!
//! Extracted verbatim from `native_composer.rs` during the module split.

#![forbid(unsafe_code)]

use std::{cell::Cell, rc::Rc};

use super::native_composer_attachments::ComposerAttachment;
use super::{
    DocumentEnd, DocumentHome, NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR,
    NATIVE_COMPOSER_ATTACHMENT_TRAY_SELECTOR, NATIVE_COMPOSER_EDITOR_SELECTOR,
    NATIVE_COMPOSER_PLACEHOLDER_SELECTOR, NATIVE_COMPOSER_SEND_SELECTOR, NativeComposer,
    NativeComposerEvent, SelectDocumentEnd, SelectDocumentHome, SelectEnd, SelectHome,
    caret_offset_for_paint, localize_painted_point, logical_vertical_target, offset_layout_bounds,
    replace_text_preserving_raw, utf8_offset_to_utf16, utf16_offset_to_utf8, utf16_range_to_utf8,
};
use crate::composer::DraftDisposition;
use crate::image_policy::ImageMediaType;
use crate::native_composer_visuals::composer_placeholder_phrase;
use artisan_domain::{AuthoredText, ImageAttachment, QueueMessagePayload};
use artisan_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use base64::Engine as _;
use gpui::{
    Bounds, Entity, EntityInputHandler as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
    Subscription, Task, TestAppContext, VisualTestContext, point, px, size,
};
use std::sync::Arc;

fn ready_attachment(id: &str, bytes: &[u8]) -> ComposerAttachment {
    let bytes = Arc::new(bytes.to_vec());
    ComposerAttachment {
        id: id.to_owned(),
        name: format!("{id}.png"),
        format: Some(gpui::ImageFormat::Png),
        mime_type: ImageMediaType::Png.as_mime_type().to_owned(),
        bytes: Some(bytes.clone()),
        content_base64: base64::engine::general_purpose::STANDARD.encode(bytes.as_ref()),
        thumbnail: Some(Arc::new(gpui::RenderImage::new(Vec::<image::Frame>::new()))),
        source_digest: "source-digest".to_owned(),
        encoded_digest: "encoded-digest".to_owned(),
        source_size_bytes: bytes.len(),
        size_bytes: bytes.len(),
    }
}

const RECALL_PRIMARY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x72, 0x9c, 0x52, 0x67, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

const RECALL_SECONDARY_PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

fn recalled_image_payload(text: Option<AuthoredText>) -> QueueMessagePayload {
    let secondary = base64::engine::general_purpose::STANDARD
        .decode(RECALL_SECONDARY_PNG_BASE64)
        .expect("secondary PNG fixture");
    QueueMessagePayload::new(
        text,
        vec![
            ImageAttachment::new("image/png", RECALL_PRIMARY_PNG.to_vec(), "first.png")
                .expect("primary image"),
            ImageAttachment::new("image/png", secondary, "second.png").expect("secondary image"),
        ],
    )
    .expect("recall payload")
}

fn set_draft(cx: &mut VisualTestContext, view: &Entity<NativeComposer>, draft: &str) {
    let draft = draft.to_owned();
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_draft(draft);
            composer_cx.notify();
        });
    });
    cx.run_until_parked();
}

#[gpui::test]
fn draft_body_match_is_exact_and_does_not_change_editor_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let draft = "  exact\n😀  ";
    set_draft(cx, &view, draft);

    cx.update(|_, app| {
        view.update(app, |composer, _| {
            composer.selection = 2..7;
            composer.selection_reversed = true;
            composer.marked_range = Some(2..7);
            let selection = composer.selection.clone();
            let selection_reversed = composer.selection_reversed;
            let marked_range = composer.marked_range.clone();
            let layout_present = composer.layout.is_some();
            let painted_bounds_present = composer.painted_bounds.is_some();
            let submitting = composer.is_submitting();
            let body = artisan_domain::MessageBody::parse(draft.to_owned()).expect("draft body");
            let changed = artisan_domain::MessageBody::parse("  exact\n😀  !".to_owned())
                .expect("changed body");

            assert!(composer.draft_matches_body(&body));
            assert!(!composer.draft_matches_body(&changed));
            assert_eq!(composer.selection, selection);
            assert_eq!(composer.selection_reversed, selection_reversed);
            assert_eq!(composer.marked_range, marked_range);
            assert_eq!(composer.layout.is_some(), layout_present);
            assert_eq!(composer.painted_bounds.is_some(), painted_bounds_present);
            assert_eq!(composer.is_submitting(), submitting);
        });
    });
}

#[test]
fn caret_paints_focused_collapsed_only_at_valid_boundaries() {
    // Empty field, focused: visible at the origin.
    assert_eq!(caret_offset_for_paint(true, &(0..0), ""), Some(0));
    // Collapsed selection follows movement through multibyte text.
    let draft = "a😀b";
    assert_eq!(caret_offset_for_paint(true, &(1..1), draft), Some(1));
    assert_eq!(caret_offset_for_paint(true, &(5..5), draft), Some(5));
    assert_eq!(caret_offset_for_paint(true, &(6..6), draft), Some(6));
    // Mid-character offsets never place the caret inside a character.
    assert_eq!(caret_offset_for_paint(true, &(2..2), draft), None);
    // A selection hides the caret in favor of the highlight.
    assert_eq!(caret_offset_for_paint(true, &(0..1), draft), None);
    assert_eq!(caret_offset_for_paint(true, &(1..5), draft), None);
    // Blur hides the caret even for a collapsed selection.
    assert_eq!(caret_offset_for_paint(false, &(0..0), ""), None);
    assert_eq!(caret_offset_for_paint(false, &(1..1), draft), None);
    // Stale offsets past the draft hide rather than misplace.
    assert_eq!(caret_offset_for_paint(true, &(7..7), draft), None);
}

fn bind_actions(cx: &mut VisualTestContext) {
    cx.update(|_, app| NativeComposer::bind_actions(app));
}

#[gpui::test]
fn mounted_caret_paints_focused_geometry_and_hides_on_blur_or_selection(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.simulate_resize(size(px(900.0), px(600.0)));
    focus_editor(cx, &view);
    // Focused empty field: the caret paints at the text origin. Its
    // height is the reference `text-base` leading (24px), matching the
    // editor's explicit line height.
    set_draft(cx, &view, "");
    let empty = cx.update(|window, app| {
        view.read(app)
            .caret_quad(window)
            .expect("focused empty caret")
    });
    assert_eq!(empty.bounds.size, size(px(2.0), px(24.0)));
    let editor = cx
        .debug_bounds(NATIVE_COMPOSER_EDITOR_SELECTOR)
        .expect("editor bounds");
    assert!(editor.contains(&empty.bounds.origin));
    // A collapsed multibyte selection follows typing.
    set_draft(cx, &view, "a😀b");
    let end = cx.update(|window, app| {
        view.read(app)
            .caret_quad(window)
            .expect("focused end caret")
    });
    assert_eq!(end.bounds.size, size(px(2.0), px(24.0)));
    assert!(
        end.bounds.origin.x > empty.bounds.origin.x,
        "caret must advance past typed text"
    );
    assert!(editor.contains(&end.bounds.origin));
    // A selection hides the caret in favor of the highlight.
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.selection = 0..1;
            composer.selection_reversed = false;
            composer_cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(
        cx.update(|window, app| view.read(app).caret_quad(window))
            .is_none()
    );
    // Blur hides the caret even for a collapsed selection.
    cx.update(|window, app| {
        view.update(app, |composer, composer_cx| {
            composer.selection = 6..6;
            composer.selection_reversed = false;
            composer_cx.notify();
        });
        let send = view.read(app).send_focus_handle.clone();
        window.focus(&send, app);
    });
    cx.run_until_parked();
    assert!(
        cx.update(|window, app| view.read(app).caret_quad(window))
            .is_none()
    );
}

fn focus_editor(cx: &mut VisualTestContext, view: &Entity<NativeComposer>) {
    cx.update(|window, app| {
        let focus = view.read(app).focus_handle.clone();
        window.focus(&focus, app);
    });
    cx.run_until_parked();
}

fn observe_send_requests(
    cx: &mut VisualTestContext,
    view: &Entity<NativeComposer>,
) -> (Rc<Cell<usize>>, Subscription) {
    let requests = Rc::new(Cell::new(0));
    let observed_requests = requests.clone();
    let subscription = cx.update(|_, app| {
        app.subscribe(view, move |_, event: &NativeComposerEvent, _| {
            if *event == NativeComposerEvent::SendRequested {
                observed_requests.set(observed_requests.get() + 1);
            }
        })
    });
    cx.run_until_parked();
    (requests, subscription)
}

fn send_button(focus: gpui::FocusHandle) -> Button {
    Button::new(
        NATIVE_COMPOSER_SEND_SELECTOR,
        focus,
        ArtisanTheme::for_mode(ThemeMode::Dark),
        MotionPolicy::Reduced,
        ButtonVariant::Ghost,
        ButtonSize::Small,
        ButtonContent::text("Send"),
    )
    .expect("the native composer send button configuration is valid")
    .focus_visibility(FocusVisibility::Visible)
}

#[gpui::test]
fn plain_enter_requests_send_and_shift_enter_inserts_a_newline(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    set_draft(cx, &view, "draft");
    bind_actions(cx);
    focus_editor(cx, &view);
    let (requests, _subscription) = observe_send_requests(cx, &view);

    cx.simulate_keystrokes("enter");
    cx.update(|_, app| assert_eq!(view.read(app).draft(), "draft"));
    assert_eq!(requests.get(), 1);

    cx.simulate_keystrokes("shift-enter");
    cx.update(|_, app| assert_eq!(view.read(app).draft(), "draft\n"));
    assert_eq!(requests.get(), 1);
}

#[gpui::test]
fn modified_and_unready_enter_requests_are_refused(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    bind_actions(cx);
    focus_editor(cx, &view);
    let (requests, _subscription) = observe_send_requests(cx, &view);

    set_draft(cx, &view, "draft");
    cx.simulate_keystrokes("ctrl-enter alt-enter cmd-enter");
    cx.update(|_, app| assert_eq!(view.read(app).draft(), "draft"));
    assert_eq!(requests.get(), 0);

    for draft in ["", " \t\n"] {
        set_draft(cx, &view, draft);
        cx.simulate_keystrokes("enter");
        assert_eq!(requests.get(), 0);
    }

    set_draft(cx, &view, "disabled");
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_disabled(true, composer_cx);
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    assert_eq!(requests.get(), 0);

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_disabled(false, composer_cx);
            composer.set_draft("in flight");
            assert!(composer.begin_submission().is_ok());
            composer_cx.notify();
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    assert_eq!(requests.get(), 0);
    cx.update(|_, app| assert!(view.read(app).is_submitting()));
}

#[gpui::test]
fn marked_ime_text_refuses_enter_until_composition_is_unmarked(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    bind_actions(cx);
    focus_editor(cx, &view);
    let (requests, _subscription) = observe_send_requests(cx, &view);

    cx.update(|window, app| {
        view.update(app, |composer, composer_cx| {
            composer.replace_and_mark_text_in_range(
                Some(0..0),
                "preedit",
                Some(0..7),
                window,
                composer_cx,
            );
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let composer = view.read(app);
        assert_eq!(composer.draft(), "preedit");
        assert!(composer.marked_range.is_some());
        assert!(!composer.send_ready());
    });

    cx.simulate_keystrokes("enter");
    assert_eq!(requests.get(), 0);

    cx.update(|window, app| {
        view.update(app, |composer, composer_cx| {
            composer.unmark_text(window, composer_cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| assert!(view.read(app).send_ready()));

    cx.simulate_keystrokes("enter");
    assert_eq!(requests.get(), 1);
}

#[gpui::test]
fn empty_ime_mark_is_still_a_composition_and_cannot_submit(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    bind_actions(cx);
    focus_editor(cx, &view);
    set_draft(cx, &view, "draft");
    let (requests, _subscription) = observe_send_requests(cx, &view);

    cx.update(|window, app| {
        view.update(app, |composer, composer_cx| {
            composer.replace_and_mark_text_in_range(
                Some(0..0),
                "",
                Some(0..0),
                window,
                composer_cx,
            );
            assert!(
                composer
                    .marked_range
                    .as_ref()
                    .is_some_and(std::ops::Range::is_empty)
            );
            assert!(!composer.send_ready());
        });
    });
    cx.simulate_keystrokes("enter");
    assert_eq!(requests.get(), 0);
}

#[gpui::test]
fn multiline_selection_actions_keep_an_anchor_when_crossing_zero(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.update(|window, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_draft("one\ntwo");
            composer.move_to(5, composer_cx);
            composer.select_home_action(&SelectHome, window, composer_cx);
            assert_eq!(composer.selection, 4..5);
            assert!(composer.selection_reversed);

            composer.select_end_action(&SelectEnd, window, composer_cx);
            assert_eq!(composer.selection, 5..7);
            assert!(!composer.selection_reversed);

            composer.move_to(2, composer_cx);
            composer.move_right(true, composer_cx);
            composer.move_left(true, composer_cx);
            composer.move_left(true, composer_cx);
            assert_eq!(composer.selection, 1..2);
            assert!(composer.selection_reversed);
        });
    });
}

#[gpui::test]
fn document_selection_actions_cover_ctrl_home_end_targets(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.update(|window, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_draft("one\ntwo");
            composer.move_to(5, composer_cx);
            composer.select_document_home_action(&SelectDocumentHome, window, composer_cx);
            assert_eq!(composer.selection, 0..5);
            assert!(composer.selection_reversed);

            composer.select_document_end_action(&SelectDocumentEnd, window, composer_cx);
            assert_eq!(composer.selection, 5..7);
            assert!(!composer.selection_reversed);

            composer.move_document_home_action(&DocumentHome, window, composer_cx);
            assert_eq!(composer.selection, 0..0);
            composer.move_document_end_action(&DocumentEnd, window, composer_cx);
            assert_eq!(composer.selection, 7..7);
        });
    });
}

#[gpui::test]
fn multiline_key_bindings_drive_navigation_and_selection(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    set_draft(cx, &view, "one\ntwo");
    bind_actions(cx);
    focus_editor(cx, &view);

    cx.simulate_keystrokes("ctrl-home");
    cx.update(|_, app| assert_eq!(view.read(app).selection, 0..0));

    cx.simulate_keystrokes("down");
    cx.update(|_, app| assert_eq!(view.read(app).selection, 4..4));

    cx.simulate_keystrokes("shift-end");
    cx.update(|_, app| {
        let composer = view.read(app);
        assert_eq!(composer.selection, 4..7);
        assert!(!composer.selection_reversed);
    });

    cx.simulate_keystrokes("ctrl-shift-home");
    cx.update(|_, app| {
        let composer = view.read(app);
        assert_eq!(composer.selection, 0..4);
        assert!(composer.selection_reversed);
    });

    cx.simulate_keystrokes("ctrl-end");
    cx.update(|_, app| assert_eq!(view.read(app).selection, 7..7));
}

#[test]
fn logical_vertical_navigation_preserves_column_across_short_lines_and_unicode() {
    let text = "abcd\nx\nab😀d";
    assert_eq!(logical_vertical_target(text, 4, 1, 4), 6);
    assert_eq!(logical_vertical_target(text, 6, 1, 4), 13);
    assert_eq!(logical_vertical_target(text, 13, -1, 4), 6);

    let unicode = "x\n😀";
    // UTF-16 column 1 falls inside the surrogate pair and is clamped to
    // the preceding valid boundary rather than splitting the character.
    assert_eq!(logical_vertical_target(unicode, 1, 1, 1), 2);
}

#[gpui::test]
fn completed_attachment_task_handles_are_pruned_without_touching_live_work(
    cx: &mut TestAppContext,
) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let live_task = cx.spawn(|_| std::future::pending::<()>());
    cx.update(|_, app| {
        view.update(app, |composer, _| {
            composer.attachment_tasks.push(live_task);
            composer.attachment_tasks.push(Task::ready(()));
            composer.attachment_tasks.push(Task::ready(()));
            composer.prune_attachment_tasks();
            assert_eq!(composer.attachment_tasks.len(), 1);
            assert!(!composer.attachment_tasks[0].is_ready());
        });
    });
}

#[gpui::test]
fn thread_drafts_restore_without_cross_thread_undo(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.update(|_, app| {
        view.update(app, |composer, cx| {
            composer.switch_thread("one", false, cx);
            composer.replace_range(0..0, "first draft", None, cx);
            composer.switch_thread("two", false, cx);
            assert_eq!(composer.state.draft(), "");
            assert!(composer.undo.is_empty());
            composer.replace_range(0..0, "second draft", None, cx);
            composer.switch_thread("one", false, cx);
            assert_eq!(composer.state.draft(), "first draft");
            composer.switch_thread("two", false, cx);
            assert_eq!(composer.state.draft(), "second draft");
        });
    });
}

#[gpui::test]
fn attachment_snapshot_preserves_order_and_rejects_a_new_thread_generation(
    cx: &mut TestAppContext,
) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let snapshot = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.switch_thread("one", false, composer_cx);
            composer
                .attachments
                .push(ready_attachment("first", &[1, 2]));
            composer
                .attachments
                .push(ready_attachment("second", &[3, 4]));
            composer_cx.notify();
            let snapshot = composer
                .snapshot_ordered_ready_attachments()
                .expect("ready attachment snapshot");
            assert_eq!(snapshot.attachments[0].position, 0);
            assert_eq!(snapshot.attachments[1].position, 1);
            assert_eq!(
                composer.match_retry_attachment_payload(&snapshot),
                Some(snapshot.clone())
            );
            composer.switch_thread("two", false, composer_cx);
            assert!(composer.match_retry_attachment_payload(&snapshot).is_none());
            snapshot
        })
    });
    assert_eq!(snapshot.attachments.len(), 2);
}

#[gpui::test]
fn recalled_text_restores_into_the_exact_empty_thread(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let payload = QueueMessagePayload::text_only("queued\nmessage").expect("text payload");
    let target = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.switch_thread("recall-text", false, composer_cx);
            composer
                .capture_recall_target()
                .expect("real empty thread is recallable")
        })
    });

    let result = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.restore_recalled_payload(&target, payload.clone(), composer_cx)
        })
    });
    assert_eq!(result, Ok(()));
    cx.update(|_, app| {
        let composer = view.read(app);
        assert_eq!(composer.draft(), "queued\nmessage");
        assert_eq!(composer.attachment_count(), 0);
        assert!(composer.draft_matches_payload(&payload));
    });
}

#[gpui::test]
fn recall_race_returns_the_full_payload_after_typing_without_overwrite(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let payload = QueueMessagePayload::text_only("queued message").expect("text payload");
    let target = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.switch_thread("recall-typing", false, composer_cx);
            composer
                .capture_recall_target()
                .expect("real empty thread is recallable")
        })
    });

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.replace_range(0..0, "user started typing", None, composer_cx);
            assert!(composer.capture_recall_target().is_none());
        });
    });
    let result = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.restore_recalled_payload(&target, payload.clone(), composer_cx)
        })
    });
    assert_eq!(result, Err(payload));
    cx.update(|_, app| {
        let composer = view.read(app);
        assert_eq!(composer.draft(), "user started typing");
        assert_eq!(composer.attachment_count(), 0);
    });
}

#[gpui::test]
fn recall_race_returns_the_full_payload_after_thread_change(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let payload = QueueMessagePayload::text_only("queued message").expect("text payload");
    let target = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.switch_thread("recall-old-thread", false, composer_cx);
            composer
                .capture_recall_target()
                .expect("real empty thread is recallable")
        })
    });
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.switch_thread("recall-new-thread", false, composer_cx);
        });
    });

    let result = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.restore_recalled_payload(&target, payload.clone(), composer_cx)
        })
    });
    assert_eq!(result, Err(payload));
    cx.update(|_, app| {
        let composer = view.read(app);
        assert_eq!(composer.draft(), "");
        assert_eq!(composer.attachment_count(), 0);
    });
}

#[gpui::test]
fn existing_draft_never_offers_a_recall_target(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.switch_thread("recall-existing", false, composer_cx);
            composer.replace_range(0..0, "keep this draft", None, composer_cx);
            assert!(composer.capture_recall_target().is_none());
            assert_eq!(composer.draft(), "keep this draft");
        });
    });
}

#[gpui::test]
fn image_only_recall_preserves_absent_text_order_and_exact_bytes(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let payload = recalled_image_payload(None);
    let expected = payload
        .attachments()
        .iter()
        .map(|attachment| (attachment.name().to_owned(), attachment.bytes().to_vec()))
        .collect::<Vec<_>>();
    let target = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.switch_thread("recall-images", false, composer_cx);
            composer
                .capture_recall_target()
                .expect("real empty thread is recallable")
        })
    });
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer
                .restore_recalled_payload(&target, payload, composer_cx)
                .expect("valid queued image payload restores");
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_attachment_delivery_enabled(true, composer_cx);
            assert_eq!(composer.attachments.len(), expected.len());
            assert!(
                composer
                    .attachments
                    .iter()
                    .all(ComposerAttachment::is_ready)
            );
            let (restored, token) = composer
                .begin_payload_submission()
                .expect("prepared recalled images submit");
            assert!(restored.text().is_none());
            assert_eq!(
                restored
                    .attachments()
                    .iter()
                    .map(|attachment| {
                        (attachment.name().to_owned(), attachment.bytes().to_vec())
                    })
                    .collect::<Vec<_>>(),
                expected
            );
            composer.finish_submission(token, DraftDisposition::Retained, composer_cx);
        });
    });
}

#[gpui::test]
fn image_only_recall_preserves_present_empty_text(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let payload = recalled_image_payload(Some(AuthoredText::empty()));
    let target = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.switch_thread("recall-empty-text", false, composer_cx);
            composer
                .capture_recall_target()
                .expect("real empty thread is recallable")
        })
    });
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer
                .restore_recalled_payload(&target, payload, composer_cx)
                .expect("valid queued image payload restores");
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_attachment_delivery_enabled(true, composer_cx);
            let (restored, token) = composer
                .begin_payload_submission()
                .expect("prepared recalled images submit");
            assert_eq!(restored.text().map(AuthoredText::as_str), Some(""));
            composer.finish_submission(token, DraftDisposition::Retained, composer_cx);
        });
    });
}

#[gpui::test]
fn recalled_attachment_work_is_fenced_after_thread_change(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let payload = recalled_image_payload(None);
    let target = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.switch_thread("recall-pending", false, composer_cx);
            composer
                .capture_recall_target()
                .expect("real empty thread is recallable")
        })
    });
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer
                .restore_recalled_payload(&target, payload.clone(), composer_cx)
                .expect("valid queued image payload restores");
        });
    });
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.switch_thread("recall-after-switch", false, composer_cx);
        });
    });
    cx.run_until_parked();

    let result = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            assert_eq!(composer.draft(), "");
            assert_eq!(composer.attachment_count(), 0);
            composer.restore_recalled_payload(&target, payload, composer_cx)
        })
    });
    assert!(result.is_err());
}

#[gpui::test]
fn image_only_typed_payload_has_no_placeholder_and_cleans_after_acceptance(
    cx: &mut TestAppContext,
) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let token = cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer
                .attachments
                .push(ready_attachment("image", &[1, 2, 3]));
            composer.set_attachment_delivery_enabled(true, composer_cx);
            let (payload, token) = composer
                .begin_payload_submission()
                .expect("typed image-only payload begins");
            assert_eq!(payload.text().expect("authored text").as_str(), "");
            assert_eq!(payload.attachments().len(), 1);
            assert!(composer.draft_matches_payload(&payload));
            token
        })
    });

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.finish_submission(token, DraftDisposition::Accepted, composer_cx);
            assert!(composer.attachments.is_empty());
            assert_eq!(composer.draft(), "");
        });
    });
}

#[gpui::test]
fn attachment_tray_remove_action_keeps_the_text_transport_refusal_visible(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.attachments.push(ComposerAttachment::pending(
                "pending",
                "capture.png",
                Some(gpui::ImageFormat::Png),
                "image/png",
                0,
            ));
            composer_cx.notify();
        });
    });
    cx.run_until_parked();

    assert!(
        cx.debug_bounds(NATIVE_COMPOSER_ATTACHMENT_TRAY_SELECTOR)
            .is_some()
    );
    assert!(
        cx.debug_bounds(NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR)
            .is_some()
    );
    let remove = cx
        .debug_bounds("artisan-native-composer-attachment-remove")
        .expect("pending attachment remove action");
    cx.simulate_click(remove.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        assert_eq!(view.read(app).attachment_count(), 0);
        assert!(!view.read(app).send_ready());
    });
}

#[gpui::test]
fn offline_drafting_and_undo_preserve_text_without_admitting_send(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.update(|window, app| {
        view.update(app, |composer, cx| {
            composer.set_surface(true, "Select model".into(), cx);
            composer.replace_range(0..0, "hello 🌍", None, cx);
            assert_eq!(composer.state.draft(), "hello 🌍");
            assert!(!composer.send_ready());
            assert!(matches!(
                composer.begin_submission(),
                Err(super::SubmissionBlocked::Disabled)
            ));
            composer.undo_action(&super::Undo, window, cx);
            assert_eq!(composer.state.draft(), "");
            composer.redo_action(&super::Redo, window, cx);
            assert_eq!(composer.state.draft(), "hello 🌍");
        });
    });
}

#[gpui::test]
fn send_has_stable_identity_focusability_visible_focus_and_local_tab_order(
    cx: &mut TestAppContext,
) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    set_draft(cx, &view, "draft");

    assert!(
        cx.debug_bounds(NATIVE_COMPOSER_SEND_SELECTOR).is_some(),
        "the native Send button must retain its stable selector"
    );
    cx.update(|_, app| {
        let composer = view.read(app);
        assert_eq!(composer.focus_handle.tab_index, 0);
        assert_eq!(composer.send_focus_handle.tab_index, 2);
        assert!(composer.focus_handle.tab_stop);
        assert!(composer.send_focus_handle.tab_stop);
    });

    let ring_visible = cx.update(|window, app| {
        let focus = view.read(app).send_focus_handle.clone();
        window.focus(&focus, app);
        send_button(focus).focus_ring_visible(window)
    });
    assert!(ring_visible);

    cx.update(|window, app| {
        let composer = view.read(app);
        let editor_focus = composer.focus_handle.clone();
        let send_focus = composer.send_focus_handle.clone();
        let model_focus = composer.model_focus_handle.clone();
        window.focus(&editor_focus, app);
        window.focus_next(app);
        assert!(model_focus.is_focused(window));
        window.focus_next(app);
        assert!(send_focus.is_focused(window));
        window.focus_prev(app);
        assert!(model_focus.is_focused(window));
        window.focus_prev(app);
        assert!(editor_focus.is_focused(window));
    });
}

#[gpui::test]
fn pointer_enter_and_space_activate_through_the_same_send_path(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    set_draft(cx, &view, "draft");
    let (requests, _subscription) = observe_send_requests(cx, &view);
    let send_bounds = cx
        .debug_bounds(NATIVE_COMPOSER_SEND_SELECTOR)
        .expect("the native Send button must paint");

    cx.simulate_click(send_bounds.center(), Modifiers::none());
    cx.update(|window, app| {
        assert!(
            view.read(app).send_focus_handle.is_focused(window),
            "a pointer activation must focus the shared Send button"
        );
    });
    // The fork only synthesizes a keyboard click for a key-up preceded
    // by a matching key-down on the same focus generation, so each
    // activation simulates the full press a real user produces.
    for key in ["enter", "space"] {
        cx.simulate_event(KeyDownEvent {
            keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
        });
    }

    assert_eq!(requests.get(), 3);
}

#[gpui::test]
fn repeated_activation_is_blocked_by_the_composer_single_flight(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    set_draft(cx, &view, "draft");
    let (requests, _subscription) = observe_send_requests(cx, &view);
    let send_bounds = cx
        .debug_bounds(NATIVE_COMPOSER_SEND_SELECTOR)
        .expect("the native Send button must paint");

    cx.simulate_click(send_bounds.center(), Modifiers::none());
    assert_eq!(requests.get(), 1);

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            assert!(composer.begin_submission().is_ok());
            composer_cx.notify();
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| assert!(!view.read(app).send_focus_handle.tab_stop));

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.request_send(composer_cx);
        });
    });
    cx.simulate_click(send_bounds.center(), Modifiers::none());
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("known keyboard activation key"),
    });
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("space").expect("known keyboard activation key"),
    });

    assert_eq!(requests.get(), 1);
    cx.update(|_, app| assert!(view.read(app).is_submitting()));
}

#[test]
fn painted_geometry_translates_global_points_and_layout_bounds() {
    let painted_bounds = Bounds::new(point(px(120.0), px(48.0)), size(px(300.0), px(96.0)));
    assert_eq!(
        localize_painted_point(&painted_bounds, point(px(137.0), px(79.0))),
        Some(point(px(17.0), px(31.0)))
    );
    assert!(localize_painted_point(&painted_bounds, point(px(119.0), px(79.0))).is_none());
    assert!(localize_painted_point(&painted_bounds, point(px(137.0), px(145.0))).is_none());

    let text_bounds = offset_layout_bounds(
        &painted_bounds,
        point(px(8.0), px(14.0)),
        point(px(88.0), px(14.0)),
        px(18.0),
    )
    .expect("valid painted geometry");
    assert_eq!(text_bounds.origin, point(px(128.0), px(62.0)));
    assert_eq!(text_bounds.size, size(px(80.0), px(18.0)));
}

#[test]
fn invalid_painted_geometry_fails_closed() {
    let empty_bounds = Bounds::new(point(px(120.0), px(48.0)), size(px(0.0), px(96.0)));
    assert!(localize_painted_point(&empty_bounds, point(px(120.0), px(48.0))).is_none());
    assert!(
        offset_layout_bounds(
            &empty_bounds,
            point(px(0.0), px(0.0)),
            point(px(4.0), px(0.0)),
            px(18.0),
        )
        .is_none()
    );
}

#[test]
fn raw_editing_preserves_whitespace_newlines_and_unicode() {
    let original = "\u{200b}  café\r\n\t第二行  ";
    let range = utf16_range_to_utf8(original, 3..7).expect("valid range");
    let edited = replace_text_preserving_raw(original, range, "é").expect("edit");
    assert_eq!(edited, "\u{200b}  é\r\n\t第二行  ");
    assert_eq!(edited.as_bytes(), "\u{200b}  é\r\n\t第二行  ".as_bytes());
}

#[test]
fn utf16_and_utf8_offsets_reject_surrogate_splits_and_round_trip() {
    let text = "a😀b";
    assert_eq!(utf16_offset_to_utf8(text, 1), Some(1));
    assert_eq!(utf16_offset_to_utf8(text, 2), None);
    assert_eq!(utf16_offset_to_utf8(text, 3), Some(5));
    assert_eq!(utf8_offset_to_utf16(text, 5), Some(3));
    assert!(utf16_range_to_utf8(text, 2..3).is_none());
    assert!(replace_text_preserving_raw(text, 2..2, "x").is_none());
}

#[gpui::test]
fn empty_composer_paints_exact_placeholder_selector_and_phrase(cx: &mut TestAppContext) {
    assert_eq!(composer_placeholder_phrase(0), "Do anything");
    assert_eq!(
        NATIVE_COMPOSER_PLACEHOLDER_SELECTOR,
        "artisan-native-composer-placeholder"
    );

    let (_view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    assert!(
        cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
            .is_some()
    );
}

#[gpui::test]
fn nonempty_and_whitespace_drafts_hide_placeholder_without_rewriting(cx: &mut TestAppContext) {
    for draft in ["message", " \t\n"] {
        let (view, window_cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

        window_cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft(draft);
                composer_cx.notify();
            });
        });
        window_cx.run_until_parked();

        assert!(
            window_cx
                .debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_none()
        );
        window_cx.update(|_, app| assert_eq!(view.read(app).draft(), draft));
    }
}

#[gpui::test]
fn clearing_a_nonempty_draft_restores_placeholder(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_draft("message");
            composer_cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
            .is_none()
    );

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_draft("");
            composer_cx.notify();
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| assert_eq!(view.read(app).draft(), ""));
    assert!(
        cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
            .is_some()
    );
}

#[gpui::test]
fn accepted_submission_clear_restores_placeholder(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_draft("accepted");
            composer_cx.notify();
        });
    });
    cx.run_until_parked();

    let token = cx.update(|_, app| {
        view.update(app, |composer, _| {
            composer
                .begin_submission()
                .expect("nonempty draft begins a submission")
                .1
        })
    });
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.finish_submission(token, DraftDisposition::Accepted, composer_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        let composer = view.read(app);
        assert_eq!(composer.draft(), "");
        assert!(!composer.is_submitting());
    });
    assert!(
        cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
            .is_some()
    );
}

#[gpui::test]
fn disabled_and_submitting_empty_states_keep_one_unchanged_placeholder(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_disabled(true, composer_cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let composer = view.read(app);
        assert_eq!(composer.draft(), "");
        assert!(!composer.is_submitting());
    });
    assert!(
        cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
            .is_some()
    );

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_disabled(false, composer_cx);
            composer.set_draft("in flight");
            composer_cx.notify();
        });
    });
    cx.run_until_parked();
    let token = cx.update(|_, app| {
        view.update(app, |composer, _| {
            composer
                .begin_submission()
                .expect("nonempty draft begins a submission")
                .1
        })
    });

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.set_draft("");
            composer_cx.notify();
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let composer = view.read(app);
        assert_eq!(composer.draft(), "");
        assert!(composer.is_submitting());
    });
    assert!(
        cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
            .is_some()
    );

    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            composer.finish_submission(token, DraftDisposition::Accepted, composer_cx);
        });
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
            .is_some()
    );
}

#[gpui::test]
fn clicking_painted_placeholder_uses_editor_selection_surface(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let placeholder = cx
        .debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
        .expect("empty composer paints the placeholder");

    cx.update(|window, app| {
        let focus = view.read(app).focus_handle.clone();
        window.focus(&focus, app);
        view.update(app, |composer, _| {
            composer.selection_reversed = true;
        });
    });

    cx.simulate_click(placeholder.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|window, app| {
        let composer = view.read(app);
        assert!(composer.focus_handle.is_focused(window));
        assert_eq!(composer.selection, 0..0);
        assert!(!composer.selection_reversed);
    });
}
