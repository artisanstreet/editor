//! Images in messages.

use super::*;

/// A 2×2 WebP, as the composer uploads.
fn image_bytes() -> Vec<u8> {
    const WEBP: &str =
        "524946461e000000574542505650384c110000002f014000000750a82217a5ff8188e87f0000";
    (0..WEBP.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&WEBP[at..at + 2], 16).unwrap())
        .collect()
}

/// Images sent with a message show in its bubble once fetched from Forge,
/// also for a steer, which sits inside the running turn's work.
#[gpui::test]
fn images_sent_with_a_message_show_in_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(&thread, &ItemId::new(), Status::Done, reply("Earlier."));
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 1);
    settle(cx, &conversation);
    // Steered into the running turn: uploaded first, then the message.
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Running,
        shell("cargo build"),
    );
    let bytes = image_bytes();
    let blob = forge.runtime.block_on(forge.store.put_blob(bytes)).unwrap();
    let message = forge.put(
        &thread,
        &ItemId::from("message-with-image"),
        Status::Done,
        ItemKind::UserMessage {
            text: "Look at this".into(),
            images: vec![blob.hash.clone()],
            steer: true,
            from_agent: None,
            sender: None,
        },
    );
    wait(cx, "the steer", |cx| {
        loaded(&conversation, cx)
            .iter()
            .any(|item| item.id == message.id)
    });
    // The steer is inside a collapsed work disclosure. Hidden attachments must
    // not start image downloads; opening the disclosure paints the message.
    conversation.update(cx, |conversation, cx| {
        conversation.toggle_work(&RunId::from("run"), true, cx);
    });
    wait(cx, "the visible image", |cx| {
        draw(cx);
        if cx.debug_bounds("sent-image-message-with-image-0").is_none() {
            return false;
        }
        cx.read(|cx| {
            matches!(
                core.read(cx)
                    .host(&host)
                    .unwrap()
                    .read(cx)
                    .image(&blob.hash),
                Some(crate::row::Blob::Ready(_))
            )
        })
    });
}

/// Native ListState may measure a whole UserMessage while only a few of its
/// attachments intersect the transcript clip. Only those tiles request blobs.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: attachments scroll into view one by one and load once"
)]
fn attachments_load_only_once_they_show(cx: &mut TestAppContext) {
    // Pinned GPUI test selectors require static strings. This fixture has one
    // fixed canonical Item identity and exactly 32 attachments.
    const SELECTORS: [&str; 32] = [
        "sent-image-clipped-attachments-0",
        "sent-image-clipped-attachments-1",
        "sent-image-clipped-attachments-2",
        "sent-image-clipped-attachments-3",
        "sent-image-clipped-attachments-4",
        "sent-image-clipped-attachments-5",
        "sent-image-clipped-attachments-6",
        "sent-image-clipped-attachments-7",
        "sent-image-clipped-attachments-8",
        "sent-image-clipped-attachments-9",
        "sent-image-clipped-attachments-10",
        "sent-image-clipped-attachments-11",
        "sent-image-clipped-attachments-12",
        "sent-image-clipped-attachments-13",
        "sent-image-clipped-attachments-14",
        "sent-image-clipped-attachments-15",
        "sent-image-clipped-attachments-16",
        "sent-image-clipped-attachments-17",
        "sent-image-clipped-attachments-18",
        "sent-image-clipped-attachments-19",
        "sent-image-clipped-attachments-20",
        "sent-image-clipped-attachments-21",
        "sent-image-clipped-attachments-22",
        "sent-image-clipped-attachments-23",
        "sent-image-clipped-attachments-24",
        "sent-image-clipped-attachments-25",
        "sent-image-clipped-attachments-26",
        "sent-image-clipped-attachments-27",
        "sent-image-clipped-attachments-28",
        "sent-image-clipped-attachments-29",
        "sent-image-clipped-attachments-30",
        "sent-image-clipped-attachments-31",
    ];
    let forge = Forge::start();
    let thread = forge.thread();
    let mut hashes = Vec::new();
    for index in 0..32u32 {
        let mut bytes = image_bytes();
        if !bytes.len().is_multiple_of(2) {
            bytes.push(0);
        }
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&index.to_le_bytes());
        let length = u32::try_from(bytes.len() - 8).unwrap();
        bytes[4..8].copy_from_slice(&length.to_le_bytes());
        hashes.push(
            forge
                .runtime
                .block_on(forge.store.put_blob(bytes))
                .unwrap()
                .hash,
        );
    }
    let message = forge.put(
        &thread,
        &ItemId::from("clipped-attachments"),
        Status::Done,
        ItemKind::UserMessage {
            text: "".into(),
            images: hashes.clone(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host_id, conversation, cx) = editor(&forge, dir.path(), cx);
    // Keep the initial published row under a zero-area native clip. Otherwise
    // legitimate initial tail paint may start requests before measurement.
    cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(0.0)));
    let host = cx.read(|cx| core.read(cx).host(&host_id).unwrap().clone());
    open(&core, &host_id, &thread, cx);
    wait(cx, "attachment message data", |cx| {
        loaded(&conversation, cx)
            .iter()
            .any(|item| item.id == message.id)
    });
    let previously_painted: std::collections::HashSet<_> = cx.read(|cx| {
        hashes
            .iter()
            .enumerate()
            .filter_map(|(index, hash)| host.read(cx).image(hash).is_some().then_some(index))
            .collect()
    });
    assert!(
        previously_painted.len() < hashes.len(),
        "offscreen siblings must exist before measurement"
    );
    // An explicit public jump exits Following before native geometry places
    // the first attachment row above the clip, guaranteeing partial paint.
    cx.update(|_, cx| {
        conversation.update(cx, |c, cx| {
            c.jump_to(&message, cx);
            // This fixture checks stationary native clipping; navigation motion
            // has separate tests and must not move its measured tile boundary.
            c.arrival = None;
            let index = c.list.logical_scroll_top().item_ix;
            c.list.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: gpui::px(100.0),
            });
            cx.notify();
        });
    });
    // Install the reading anchor while the native clip is still empty. A
    // resize is itself a draw and must not first paint the Following tail.
    cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(400.0)));
    wait(cx, "visible attachment previews", |cx| {
        draw(cx);
        let clip = cx.read(|cx| conversation.read(cx).list.viewport_bounds());
        let visible: Vec<_> = (0..hashes.len())
            .filter(|index| {
                cx.debug_bounds(SELECTORS[*index]).is_some_and(|bounds| {
                    bounds.right() > clip.left()
                        && bounds.left() < clip.right()
                        && bounds.bottom() > clip.top()
                        && bounds.top() < clip.bottom()
                })
            })
            .collect();
        !visible.is_empty()
            && cx.read(|cx| {
                visible.iter().all(|index| {
                    matches!(
                        host.read(cx).image(&hashes[*index]),
                        Some(crate::row::Blob::Ready(_))
                    )
                })
            })
    });
    let clip = cx.read(|cx| conversation.read(cx).list.viewport_bounds());
    let mut visible = std::collections::HashSet::new();
    let mut partial = false;
    for (index, selector) in SELECTORS.iter().enumerate().take(hashes.len()) {
        if let Some(bounds) = cx.debug_bounds(selector)
            && bounds.right() > clip.left()
            && bounds.left() < clip.right()
            && bounds.bottom() > clip.top()
            && bounds.top() < clip.bottom()
        {
            visible.insert(index);
            partial |= bounds.top() < clip.top() || bounds.bottom() > clip.bottom();
        }
    }
    assert!(
        !visible.is_empty() && visible.len() < hashes.len(),
        "fixture must contain painted and clipped-out sibling tiles"
    );
    assert!(
        partial,
        "fixture must include a tile cut by the native clip"
    );
    cx.read(|cx| {
        for (index, hash) in hashes.iter().enumerate() {
            assert_eq!(
                host.read(cx).image(hash).is_some(),
                visible.contains(&index) || previously_painted.contains(&index),
                "only a currently or previously individually painted tile {index} may own a request/result"
            );
        }
    });
    // Newly visible tail tiles still load automatically through the existing
    // public navigation action; no permanent preview-admission pause is added.
    conversation.update(cx, super::super::Conversation::jump_to_latest);
    wait(cx, "newly painted tail attachment", |cx| {
        draw(cx);
        cx.read(|cx| {
            matches!(
                host.read(cx).image(hashes.last().unwrap()),
                Some(crate::row::Blob::Ready(_))
            )
        })
    });
}
