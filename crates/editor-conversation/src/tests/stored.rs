//! Bodies too long to keep inline, read from Forge a window at a time.

use super::*;

/// The topmost of the rows `prefix:<n>` for `n` in `range` that painted,
/// and where.
fn topmost(
    cx: &mut VisualTestContext,
    prefix: &str,
    range: std::ops::Range<usize>,
) -> Option<(usize, f32)> {
    range
        .filter_map(|n| Some((n, f32::from(shown(cx, format!("{prefix}:{n}"))?.top()))))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
}

/// Draws a few frames with the window in front, so whatever shows could
/// mark the thread read.
fn look(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.activate_window());
    for _ in 0..10 {
        draw(cx);
        cx.run_until_parked();
    }
}

/// Selecting all and copying copies every character of long messages,
/// however they were split into lines on screen.
#[gpui::test]
fn copying_a_long_message_keeps_every_character(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let texts = [
        "界🙂".repeat(500),
        "first\n\n  second\t🙂 third ".repeat(200),
    ];
    let ids: Vec<ItemId> = texts
        .iter()
        .map(|text| {
            forge
                .put(
                    &thread,
                    &ItemId::new(),
                    Status::Done,
                    ItemKind::UserMessage {
                        text: text.clone().into(),
                        images: Vec::new(),
                        steer: false,
                    },
                )
                .id
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "both messages", |cx| {
        loaded(&conversation, cx).len() == 2
    });
    settle(cx, &conversation);
    let last = ids.last().unwrap();
    let viewport = cx.read(|cx| conversation.read(cx).list.viewport_bounds());
    let bounds = (0..16)
        .filter_map(|line| shown(cx, format!("user:{last}:{line}")))
        .find(|bounds| viewport.contains(&bounds.center()))
        .expect("the message shows");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.simulate_keystrokes("ctrl-a ctrl-c");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    let copied = copied.expect("the selection copies");
    assert!(
        copied.contains(texts.last().unwrap().as_str()),
        "the message's every character is copied: {copied:?}"
    );
}

/// Long stored messages open at their end, read from Forge a window at a
/// time, and mark the thread read once that end shows.
#[gpui::test]
fn a_long_message_reads_only_once_its_end_paints(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let user = ItemId::new();
    let error = ItemId::new();
    forge.put(
        &thread,
        &user,
        Status::Done,
        ItemKind::UserMessage {
            text: "A long literal user message with UTF8 界🙂.\n"
                .repeat(8000)
                .into(),
            images: Vec::new(),
            steer: false,
        },
    );
    forge.put(
        &thread,
        &error,
        Status::Failed,
        ItemKind::Error {
            message: "A long error explaining the failed operation.\n"
                .repeat(8000)
                .into(),
            kind: artisan_api::ErrorKind::Execution,
            retryable: false,
        },
    );
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    cx.deactivate_window();
    open(&core, &host, &thread, cx);
    wait(cx, "the end of the error", |cx| {
        draw(cx);
        shown(cx, format!("error:{error}:8000")).is_some()
    });
    assert!(
        shown(cx, format!("error:{error}:0")).is_none()
            && shown(cx, format!("user:{user}:0")).is_none(),
        "the starts of both are not laid out"
    );
    assert!(
        !read(&forge, &thread),
        "nothing is read while the window is behind"
    );
    look(cx);
    wait(cx, "the thread read at its end", |cx| {
        draw(cx);
        read(&forge, &thread)
    });
    let _ = &conversation;
}

/// A message too long to lay out on one line in the transcript shows its
/// error and a way to its full text, and is never taken as read.
#[gpui::test]
fn a_preview_of_a_stored_reply_does_not_mark_it_read(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let id = ItemId::new();
    let text = "Long literal source 界🙂 with preserved whitespace. ".repeat(2000);
    let item = forge.put(
        &thread,
        &id,
        Status::Done,
        ItemKind::UserMessage {
            text: text.into(),
            images: Vec::new(),
            steer: false,
        },
    );
    assert!(matches!(
        &item.kind,
        ItemKind::UserMessage {
            text: artisan_api::TextContent::Stored(_),
            ..
        }
    ));
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the stored message", |cx| {
        draw(cx);
        shown(cx, format!("user:{id}:0")).is_some()
    });
    settle(cx, &conversation);
    look(cx);
    assert!(
        !read(&forge, &thread),
        "loading and raw preview are not the message read"
    );
}

/// A long stored reply opens at its end, read from Forge a window at a
/// time, and reads the thread only once its last block shows.
#[gpui::test]
fn a_long_stored_reply_opens_at_its_end_and_reads_once_its_last_block_paints(
    cx: &mut TestAppContext,
) {
    let forge = Forge::start();
    let thread = forge.thread();
    let text = (0..750)
        .map(|index| {
            format!("Paragraph **{index}** retains its authored *formatting* and source order.\n\n")
        })
        .collect::<String>();
    let message = forge.put(&thread, &ItemId::new(), Status::Done, reply(&text));
    let ItemKind::AssistantMessage {
        text: artisan_api::TextContent::Stored(reference),
        ..
    } = &message.kind
    else {
        panic!("fixture must exercise immutable Stored text, not inline preparation");
    };
    assert!(reference.bytes > artisan_api::INLINE_TEXT_BYTES as u64);
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    cx.deactivate_window();
    open(&core, &host, &thread, cx);
    let prefix = format!("prose:{}", message.id);
    wait(cx, "the reply's end", |cx| {
        draw(cx);
        shown(cx, format!("{prefix}:749")).is_some()
    });
    assert!(
        shown(cx, format!("{prefix}:0")).is_none(),
        "its start is not laid out"
    );
    conversation.update(cx, |c, cx| c.jump_to(&message, cx));
    settle(cx, &conversation);
    assert!(shown(cx, format!("{prefix}:749")).is_none());
    look(cx);
    assert!(
        !read(&forge, &thread),
        "an early block does not read the reply"
    );
    conversation.update(cx, |c, cx| c.jump_to_latest(cx));
    wait(cx, "the thread read at the reply's end", |cx| {
        draw(cx);
        read(&forge, &thread)
    });
}

/// One huge line shows its end without laying out all of it, and reads
/// the thread only once that end shows.
#[gpui::test]
fn one_huge_stored_line_shows_its_end_without_laying_out_all_of_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let id = ItemId::new();
    let text = "original native glyphs 界🙂 ".repeat(1500);
    assert!(text.len() > 16 * 1024 && text.len() < 64 * 1024);
    let item = forge.put(
        &thread,
        &id,
        Status::Done,
        ItemKind::UserMessage {
            text: text.into(),
            images: Vec::new(),
            steer: false,
        },
    );
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    cx.deactivate_window();
    open(&core, &host, &thread, cx);
    let prefix = format!("user:{id}:0");
    // Wrapped, the line is hundreds of lines on screen.
    wait(cx, "the line's end", |cx| {
        draw(cx);
        topmost(cx, &prefix, 128..1000).is_some()
    });
    assert!(
        shown(cx, format!("{prefix}:0")).is_none(),
        "its start is not laid out"
    );
    conversation.update(cx, |c, cx| c.jump_to(&item, cx));
    settle(cx, &conversation);
    look(cx);
    assert!(
        !read(&forge, &thread),
        "the line's early lines do not read it"
    );
    conversation.update(cx, |c, cx| c.jump_to_latest(cx));
    wait(cx, "the thread read at the line's end", |cx| {
        draw(cx);
        read(&forge, &thread)
    });
}
