//! Composer drafts live on the Forge: the view reads them, saves each change
//! latest-wins, uploads images before the draft names them, and a new host
//! view restores exactly what its Forge stored.

use super::*;
use crate::native_transport_service::{ComposerDraftCommand, ComposerDraftEvent};
use artisan_domain::{
    AuthoredText, ComposerAttachmentDigest, ComposerAttachmentRef, ComposerDraft,
    ComposerDraftRevision, ComposerDraftScope, ImageAttachment, ImageMimeType, QueueMessagePayload,
    SaveComposerDraft, UnixMillis,
};
use sha2::{Digest as _, Sha256};

const THREAD: &str = "forge-draft-thread";

const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x72, 0x9c, 0x52, 0x67, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn scope() -> ComposerDraftScope {
    ComposerDraftScope::Thread(ThreadId::parse(THREAD).unwrap())
}

type View = gpui::Entity<NativeApplication>;

fn open_view(cx: &mut TestAppContext) -> View {
    let (view, _) = cx.add_window_view(test_application);
    let (sink, _) = command_sink([]);
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.composer.update(cx, |composer, cx| {
                composer.set_disabled(false, cx);
                composer.switch_thread(THREAD, false, cx);
            });
        });
    });
    view
}

/// Takes the draft commands the view admitted since the last call.
fn take_sent(cx: &mut TestAppContext, view: &View) -> Vec<ComposerDraftCommand> {
    cx.update(|app| {
        view.update(app, |application, _| {
            application
                .composer_drafts
                .sent
                .borrow_mut()
                .drain(..)
                .collect()
        })
    })
}

fn deliver(cx: &mut TestAppContext, view: &View, event: ComposerDraftEvent) {
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.handle_service_event(NativeTransportEvent::ComposerDraft(event), cx);
        });
    });
}

fn type_text(cx: &mut TestAppContext, view: &View, text: &str) {
    cx.update(|app| {
        view.update(app, |application, cx| {
            application
                .composer
                .update(cx, |composer, cx| composer.type_at_end(text, cx));
        });
    });
}

/// The saves among `commands`, with their local send sequence.
fn saves(commands: &[ComposerDraftCommand]) -> Vec<(u64, &SaveComposerDraft)> {
    commands
        .iter()
        .filter_map(|command| match command {
            ComposerDraftCommand::Save { sequence, command } => Some((*sequence, command.as_ref())),
            _ => None,
        })
        .collect()
}

fn texts(commands: &[ComposerDraftCommand]) -> Vec<(u64, ComposerDraftScope, String)> {
    saves(commands)
        .into_iter()
        .map(|(sequence, save)| {
            (
                sequence,
                save.scope().clone(),
                save.text().as_str().to_owned(),
            )
        })
        .collect()
}

fn draft_text(cx: &mut TestAppContext, view: &View) -> String {
    cx.update(|app| view.read(app).composer.read(app).draft().to_owned())
}

fn saved(sequence: u64, revision: u64) -> ComposerDraftEvent {
    ComposerDraftEvent::Saved {
        scope: scope(),
        sequence,
        revision: ComposerDraftRevision::new(revision).unwrap(),
    }
}

fn read_nothing(cx: &mut TestAppContext, view: &View) {
    deliver(
        cx,
        view,
        ComposerDraftEvent::Read {
            scope: scope(),
            result: Ok(None),
        },
    );
    let _ = take_sent(cx, view);
}

fn stored_png(name: &str) -> ComposerAttachmentRef {
    ComposerAttachmentRef::new(
        ComposerAttachmentDigest::new(Sha256::digest(ONE_PIXEL_PNG).into()),
        ImageMimeType::Png,
        name,
        u32::try_from(ONE_PIXEL_PNG.len()).unwrap(),
    )
    .unwrap()
}

/// Recalls a text-and-image payload into the composer and returns the
/// upload it starts: `(attachment id, image)`.
fn recall_image(cx: &mut TestAppContext, view: &View) -> (String, Vec<ComposerDraftCommand>) {
    let image = ImageAttachment::new("image/png", ONE_PIXEL_PNG.to_vec(), "shot.png").unwrap();
    let payload =
        QueueMessagePayload::new(Some(AuthoredText::parse("recalled").unwrap()), vec![image])
            .unwrap();
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.composer.update(cx, |composer, cx| {
                let target = composer.capture_recall_target().expect("empty composer");
                composer
                    .restore_recalled_payload(&target, payload, cx)
                    .unwrap();
            });
        });
    });
    cx.run_until_parked();
    let sent = take_sent(cx, view);
    let id = sent
        .iter()
        .find_map(|command| match command {
            ComposerDraftCommand::Upload {
                attachment_id,
                command,
                ..
            } => {
                assert_eq!(command.image.bytes(), ONE_PIXEL_PNG);
                Some(attachment_id.clone())
            }
            _ => None,
        })
        .expect("the ready image uploads");
    (id, sent)
}

#[gpui::test]
fn typed_drafts_save_latest_wins_and_a_new_host_view_restores_them(cx: &mut TestAppContext) {
    let view = open_view(cx);
    assert_eq!(
        take_sent(cx, &view),
        vec![ComposerDraftCommand::Read(scope())],
        "opening a thread reads its Forge draft"
    );
    read_nothing(cx, &view);
    type_text(cx, &view, "h");
    assert_eq!(
        texts(&take_sent(cx, &view)),
        vec![(1, scope(), "h".to_owned())]
    );
    // Keystrokes while save 1 is in flight coalesce into one body.
    for text in ["e", "l", "l", "o"] {
        type_text(cx, &view, text);
    }
    assert!(
        take_sent(cx, &view).is_empty(),
        "one save in flight per thread"
    );
    deliver(cx, &view, saved(1, 1));
    assert_eq!(
        texts(&take_sent(cx, &view)),
        vec![(2, scope(), "hello".to_owned())],
        "only the newest body follows the ack"
    );
    // A stale or repeated acknowledgement settles nothing.
    deliver(cx, &view, saved(1, 1));
    assert!(take_sent(cx, &view).is_empty());
    deliver(cx, &view, saved(2, 2));
    assert!(take_sent(cx, &view).is_empty());

    // A host switch rebuilds the view. It shows the Forge draft, whatever
    // revision it is at, and its next change is simply stored.
    let reopened = open_view(cx);
    assert_eq!(draft_text(cx, &reopened), "");
    assert_eq!(
        take_sent(cx, &reopened),
        vec![ComposerDraftCommand::Read(scope())]
    );
    let stored = ComposerDraft::new(
        ComposerDraftRevision::new(100).unwrap(),
        AuthoredText::parse("hello").unwrap(),
        Vec::new(),
        UnixMillis::from_millis(5),
    )
    .unwrap();
    deliver(
        cx,
        &reopened,
        ComposerDraftEvent::Read {
            scope: scope(),
            result: Ok(Some(stored)),
        },
    );
    assert_eq!(draft_text(cx, &reopened), "hello");
    assert!(
        take_sent(cx, &reopened).is_empty(),
        "showing the Forge draft saves nothing"
    );
    type_text(cx, &reopened, "!");
    assert_eq!(
        texts(&take_sent(cx, &reopened)),
        vec![(1, scope(), "hello!".to_owned())]
    );
}

#[gpui::test]
fn a_keystroke_in_the_frame_of_a_switch_saves_to_its_own_thread(cx: &mut TestAppContext) {
    let view = open_view(cx);
    read_nothing(cx, &view);
    let other = ComposerDraftScope::Thread(ThreadId::parse("other-thread").unwrap());
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.composer.update(cx, |composer, cx| {
                composer.type_at_end("typed then switched", cx);
                composer.switch_thread("other-thread", false, cx);
            });
        });
    });
    let sent = take_sent(cx, &view);
    assert_eq!(
        texts(&sent),
        vec![(1, scope(), "typed then switched".to_owned())],
        "the change reaches the thread it was typed in, and nothing else"
    );
    assert!(sent.contains(&ComposerDraftCommand::Read(other)));
}

#[gpui::test]
fn an_upload_that_completes_later_is_saved_into_the_draft_at_once(cx: &mut TestAppContext) {
    let view = open_view(cx);
    read_nothing(cx, &view);
    let (attachment_id, sent) = recall_image(cx, &view);
    // The recalled text is saved at once; the image joins once stored.
    let text_save = saves(&sent);
    assert_eq!(text_save.len(), 1);
    assert_eq!(text_save[0].1.text().as_str(), "recalled");
    assert!(text_save[0].1.attachments().is_empty());
    // Acknowledge every save until the chain is idle.
    let mut next = Some(text_save[0].0);
    while let Some(sequence) = next {
        deliver(cx, &view, saved(sequence, sequence));
        next = saves(&take_sent(cx, &view))
            .first()
            .map(|(sequence, _)| *sequence);
    }

    // The upload finishing (say while a switch drains) is saved within the
    // same event, under the upload's hold.
    let reference = stored_png("shot.png");
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.receive_draft_event(
                ComposerDraftEvent::Uploaded {
                    scope: scope(),
                    attachment_id: attachment_id.clone(),
                    result: Ok(reference.clone()),
                },
                cx,
            );
            let sent = application.composer_drafts.sent.borrow();
            let with_image = saves(&sent);
            assert_eq!(with_image.len(), 1);
            assert_eq!(
                with_image[0].1.attachments(),
                std::slice::from_ref(&reference)
            );
            assert_eq!(with_image[0].1.text().as_str(), "recalled");
        });
    });
    let sent = take_sent(cx, &view);
    assert!(
        !sent
            .iter()
            .any(|command| matches!(command, ComposerDraftCommand::Upload { .. })),
        "an uploaded image is never uploaded again"
    );
}

#[gpui::test]
fn quitting_with_an_upload_in_flight_saves_the_draft_that_references_it(cx: &mut TestAppContext) {
    let view = open_view(cx);
    read_nothing(cx, &view);
    let _ = recall_image(cx, &view);
    cx.update(|app| {
        view.update(app, NativeApplication::prepare_shutdown);
    });
    let sent = take_sent(cx, &view);
    let flushed = saves(&sent);
    assert_eq!(
        flushed.len(),
        1,
        "the unsent body is flushed behind the upload"
    );
    assert_eq!(flushed[0].1.attachments(), [stored_png("shot.png")]);
    assert_eq!(flushed[0].1.text().as_str(), "recalled");
}
