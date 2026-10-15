//! Real paired HTTP page replies held at their completion boundary. No
//! wall-clock race is needed to exercise cancellation and generation fences.

use super::*;
use crate::viewport::{Fetch, State};
use futures::channel::oneshot;
use std::sync::atomic::{AtomicBool, Ordering};

fn hold_page(
    conversation: &Entity<Conversation>,
    cx: &mut VisualTestContext,
) -> (Arc<AtomicBool>, oneshot::Sender<bool>) {
    let arrived = Arc::new(AtomicBool::new(false));
    let (send, receive) = oneshot::channel();
    conversation.update(cx, |conversation, _| {
        assert!(conversation.page_completion_gate.is_none());
        conversation.page_completion_gate = Some((arrived.clone(), receive));
    });
    (arrived, send)
}

fn user_message(text: impl Into<String>) -> ItemKind {
    let text: String = text.into();
    ItemKind::UserMessage {
        text: text.into(),
        images: Vec::new(),
        steer: false,
    }
}

fn history(forge: &Forge, thread: &ThreadId, count: usize) -> Vec<Item> {
    (0..count)
        .map(|index| {
            forge.put(
                thread,
                &ItemId::new(),
                Status::Done,
                user_message(format!("line {index}")),
            )
        })
        .collect()
}

fn held_older(
    conversation: &Entity<Conversation>,
    cx: &mut VisualTestContext,
) -> oneshot::Sender<bool> {
    let (arrived, release) = hold_page(conversation, cx);
    conversation.update(cx, |conversation, cx| {
        conversation.viewport(vec![Input::TopReached], cx);
    });
    wait(cx, "the held HTTP page response", |_| {
        arrived.load(Ordering::Acquire)
    });
    release
}

#[gpui::test]
fn delayed_page_is_cancelled_when_the_same_thread_reopens(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let stored = history(&forge, &thread, 250);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "initial tail", |cx| {
        loaded(&conversation, cx).len() == 100
    });
    let release = held_older(&conversation, cx);
    conversation.update(cx, |conversation, cx| {
        conversation.open_thread_on(&host, &thread, cx);
    });
    wait(cx, "replacement tail", |cx| {
        cx.read(|cx| {
            !conversation
                .read(cx)
                .view
                .as_ref()
                .unwrap()
                .window()
                .in_flight
        })
    });
    assert!(
        release.send(false).is_err(),
        "reopening owns and cancels the old page task"
    );
    cx.run_until_parked();
    assert_eq!(loaded(&conversation, cx), stored[150..]);
}

#[gpui::test]
fn stream_reset_cancels_an_older_reply_and_advances_its_generation(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let stored = history(&forge, &thread, 250);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "initial tail", |cx| {
        loaded(&conversation, cx).len() == 100
    });
    let release = held_older(&conversation, cx);
    let generation = cx.read(|cx| conversation.read(cx).open_generation);
    let data = cx.read(|cx| core.read(cx).host(&host).unwrap().clone());
    conversation.update(cx, |conversation, cx| {
        conversation.host_event(&data, &editor_core::HostEvent::Reset, cx)
    });
    wait(cx, "reset tail", |cx| {
        cx.read(|cx| {
            !conversation
                .read(cx)
                .view
                .as_ref()
                .unwrap()
                .window()
                .in_flight
        })
    });
    assert!(release.send(false).is_err());
    assert_ne!(
        generation,
        cx.read(|cx| conversation.read(cx).open_generation)
    );
    assert_eq!(loaded(&conversation, cx), stored[150..]);
}

#[gpui::test]
fn a_completed_reply_cannot_cross_its_generation_fence(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let stored = history(&forge, &thread, 250);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "initial tail", |cx| {
        loaded(&conversation, cx).len() == 100
    });
    let release = held_older(&conversation, cx);
    // Deliberately retain the task: cancellation and the completion fence are
    // independent protections, and this proves the latter actually rejects it.
    conversation.update(cx, |conversation, _| {
        conversation.open_generation += 1;
        conversation.load_error = Some("new scope error".into());
    });
    release.send(false).unwrap();
    cx.run_until_parked();
    assert_eq!(loaded(&conversation, cx), stored[150..]);
    assert_eq!(
        cx.read(|cx| conversation.read(cx).load_error.clone()),
        Some("new scope error".into())
    );
    assert!(
        cx.read(|cx| conversation
            .read(cx)
            .view
            .as_ref()
            .unwrap()
            .window()
            .in_flight),
        "the obsolete completion cannot change the new scope's page lifecycle either"
    );
}

#[gpui::test]
fn delayed_page_is_cancelled_across_hosts_even_for_the_same_thread_id(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let other = Forge::start();
    let thread = forge.thread();
    history(&forge, &thread, 250);
    other
        .runtime
        .block_on(other.store.create_thread(CreateThread {
            id: thread.clone(),
            project_id: other.project.clone(),
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: None,
        }))
        .unwrap();
    let other_item = other.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        user_message("other host"),
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "first host tail", |cx| {
        loaded(&conversation, cx).len() == 100
    });
    let release = held_older(&conversation, cx);
    // Pair using the same device identity, then let Core persist both hosts.
    let (files, second_host) = other.pair(dir.path());
    let config = files
        .hosts()
        .unwrap()
        .hosts
        .into_iter()
        .find(|host| host.id == second_host)
        .unwrap();
    core.update(cx, |core, cx| core.add_host(config, cx).unwrap());
    open(&core, &second_host, &thread, cx);
    wait(cx, "second host tail", |cx| {
        loaded(&conversation, cx) == vec![other_item.clone()]
    });
    assert!(release.send(false).is_err());
    cx.run_until_parked();
    assert_eq!(loaded(&conversation, cx), vec![other_item]);
    assert_eq!(
        cx.read(|cx| conversation.read(cx).open.clone()),
        Some((second_host, thread))
    );
}

#[gpui::test]
fn failed_page_does_not_retry_during_paint_but_explicit_retry_succeeds(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let stored = history(&forge, &thread, 250);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    let (arrived, release) = hold_page(&conversation, cx);
    open(&core, &host, &thread, cx);
    wait(cx, "initial held response", |_| {
        arrived.load(Ordering::Acquire)
    });
    release.send(true).unwrap();
    wait(cx, "reported failure", |cx| {
        cx.read(|cx| conversation.read(cx).load_error.is_some())
    });
    let requests = cx.read(|cx| conversation.read(cx).page_requests);
    for _ in 0..24 {
        draw(cx);
        cx.run_until_parked();
    }
    assert!(loaded(&conversation, cx).is_empty());
    assert_eq!(
        requests,
        cx.read(|cx| conversation.read(cx).page_requests),
        "painting must never create a retry loop"
    );
    conversation.update(cx, |conversation, cx| conversation.fetch(Fetch::Tail, cx));
    wait(cx, "explicit retry", |cx| {
        loaded(&conversation, cx).len() == 100
    });
    assert_eq!(loaded(&conversation, cx), stored[150..]);
    assert!(cx.read(|cx| conversation.read(cx).load_error.is_none()));
    assert_eq!(
        requests + 1,
        cx.read(|cx| conversation.read(cx).page_requests)
    );
}

#[gpui::test]
fn twelve_hundred_live_items_while_reading_preserve_a_bounded_native_viewport(
    cx: &mut TestAppContext,
) {
    let forge = Forge::start();
    let thread = forge.thread();
    let stored = history(&forge, &thread, 100);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "initial tail", |cx| {
        loaded(&conversation, cx).len() == 100
    });
    settle(cx, &conversation);
    let anchor = stored[60].position;
    let anchor_id = stored[60].id.clone();
    conversation.update(cx, |conversation, cx| {
        conversation.list.scroll_to(gpui::ListOffset {
            item_ix: 60,
            offset_in_item: gpui::px(7.0),
        });
        conversation.viewport(vec![Input::ScrolledAway { anchor }], cx);
    });
    draw(cx);
    let before = cx.read(|cx| conversation.read(cx).list.logical_scroll_top());
    let data = cx.read(|cx| core.read(cx).host(&host).unwrap().clone());
    // Use real persisted canonical Items and the production HostEvent path.
    // Deliver deterministically so the test observes every eviction boundary,
    // rather than racing the independently buffered SSE reader.
    for index in 0..1200 {
        let item = forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            user_message(format!("incoming {index}")),
        );
        conversation.update(cx, |conversation, cx| {
            conversation.host_event(&data, &editor_core::HostEvent::Item(Arc::new(item)), cx);
            assert!(conversation.view.as_ref().unwrap().items.len() <= 800);
            assert!(
                conversation
                    .view
                    .as_ref()
                    .unwrap()
                    .item(&anchor_id)
                    .is_some()
            );
            assert_eq!(
                conversation.view.as_ref().unwrap().viewport.state(),
                &State::Reading { anchor }
            );
        });
    }
    draw(cx);
    let after = cx.read(|cx| conversation.read(cx).list.logical_scroll_top());
    assert_eq!(before.item_ix, after.item_ix);
    assert!(
        (f32::from(before.offset_in_item) - f32::from(after.offset_in_item)).abs() < 0.5,
        "live retention must preserve the reader's native pixel position"
    );
    assert!(
        cx.read(|cx| conversation
            .read(cx)
            .view
            .as_ref()
            .unwrap()
            .window()
            .gap
            .is_some()),
        "evicted incoming history remains an explicit loadable lower gap"
    );
}
