//! The composer as a view of its Forge-owned draft.

#![forbid(unsafe_code)]

use artisan_domain::{
    AuthoredText, ComposerAttachmentDigest, ComposerAttachmentRef, ComposerDraft,
    ComposerDraftRevision, ComposerDraftScope, ImageMimeType, ProjectId, ThreadId, UnixMillis,
};
use gpui::TestAppContext;
use sha2::{Digest as _, Sha256};

use super::NativeComposer;
use super::native_composer_attachments::ComposerAttachment;
use super::tests::{RECALL_PRIMARY_PNG, ready_attachment};

fn thread(name: &str) -> ComposerDraftScope {
    ComposerDraftScope::Thread(ThreadId::parse(name).unwrap())
}

fn stored_png(name: &str) -> ComposerAttachmentRef {
    ComposerAttachmentRef::new(
        ComposerAttachmentDigest::new(Sha256::digest(RECALL_PRIMARY_PNG).into()),
        ImageMimeType::Png,
        name,
        u32::try_from(RECALL_PRIMARY_PNG.len()).unwrap(),
    )
    .unwrap()
}

fn forge_draft(text: &str, attachments: Vec<ComposerAttachmentRef>) -> ComposerDraft {
    ComposerDraft::new(
        ComposerDraftRevision::new(3).unwrap(),
        AuthoredText::parse(text).unwrap(),
        attachments,
        UnixMillis::from_millis(1),
    )
    .unwrap()
}

#[gpui::test]
fn switching_threads_shows_each_forge_draft_and_keeps_undo_local(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.update(|_, app| {
        view.update(app, |composer, cx| {
            composer.switch_thread("one", false, cx);
            assert!(composer.awaiting_forge_draft());
            let before = composer.draft_change();
            composer.replace_range(0..0, "first draft", None, cx);
            assert_ne!(composer.draft_change(), before, "typing is a draft change");
            assert!(!composer.awaiting_forge_draft());

            composer.switch_thread("two", false, cx);
            assert_eq!(composer.draft_scope(), Some(thread("two")));
            assert_eq!(composer.state.draft(), "", "the view waits for the Forge");
            assert!(composer.undo.is_empty(), "undo never crosses threads");
            let change = composer.draft_change();
            assert!(
                composer
                    .apply_forge_draft(&thread("one"), &forge_draft("stale", Vec::new()), cx)
                    .is_empty()
            );
            assert_eq!(
                composer.state.draft(),
                "",
                "another scope's draft is ignored"
            );
            composer.apply_forge_draft(&thread("two"), &forge_draft("stored two", Vec::new()), cx);
            assert_eq!(composer.state.draft(), "stored two");
            assert_eq!(
                composer.draft_change(),
                change,
                "showing the Forge draft saves nothing"
            );
            assert!(composer.undo.is_empty());
        });
    });
}

#[gpui::test]
fn local_typing_wins_over_a_late_forge_draft(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.update(|_, app| {
        view.update(app, |composer, cx| {
            composer.switch_thread("late", false, cx);
            composer.replace_range(0..0, "typed first", None, cx);
            composer.apply_forge_draft(&thread("late"), &forge_draft("older", Vec::new()), cx);
            assert_eq!(composer.state.draft(), "typed first");
            assert_eq!(composer.draft_body().text, "typed first");
        });
    });
}

#[gpui::test]
fn carrying_a_draft_moves_it_and_releases_its_source_scope(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.update(|_, app| {
        view.update(app, |composer, cx| {
            composer.switch_thread("project:alpha", false, cx);
            composer.replace_range(0..0, "Move this draft", None, cx);
            composer
                .attachments
                .push(ready_attachment("image", &[1, 2]));
            assert!(composer.has_unsent_draft());
            let change = composer.draft_change();
            composer.switch_thread("destination", true, cx);
            assert_eq!(composer.state.draft(), "Move this draft");
            assert_eq!(composer.attachments.len(), 1);
            assert_ne!(
                composer.draft_change(),
                change,
                "the new thread saves the draft"
            );
            assert_eq!(composer.draft_scope(), Some(thread("destination")));
            assert_eq!(
                composer.take_released_draft_scope(),
                Some(ComposerDraftScope::Project(
                    ProjectId::parse("alpha").unwrap()
                ))
            );
            assert_eq!(composer.take_released_draft_scope(), None);
        });
    });
}

#[gpui::test]
fn uploaded_images_join_the_draft_body_in_tray_order(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.update(|_, app| {
        view.update(app, |composer, cx| {
            composer.switch_thread("images", false, cx);
            composer
                .attachments
                .push(ready_attachment("first", &[1, 2]));
            composer
                .attachments
                .push(ready_attachment("second", &[3, 4]));
            let unstored = composer.unstored_attachments();
            assert_eq!(unstored.len(), 2);
            assert_eq!(unstored[0].1.bytes(), &[1, 2]);
            assert!(composer.draft_body().attachments.is_empty());

            let second = stored_png("second.png");
            let change = composer.draft_change();
            composer.mark_attachment_stored(&thread("images"), "second", second.clone(), cx);
            assert_ne!(composer.draft_change(), change);
            assert_eq!(composer.draft_body().attachments, vec![second]);
            assert_eq!(composer.unstored_attachments().len(), 1);
            // A reference for another scope's composer is ignored.
            composer.mark_attachment_stored(&thread("other"), "first", stored_png("x.png"), cx);
            assert_eq!(composer.unstored_attachments().len(), 1);
        });
    });
}

#[gpui::test]
fn forge_draft_images_restore_from_their_stored_bytes(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    let reference = stored_png("shot.png");
    let change = cx.update(|_, app| {
        view.update(app, |composer, cx| {
            composer.switch_thread("restored", false, cx);
            let digests = composer.apply_forge_draft(
                &thread("restored"),
                &forge_draft("see", vec![reference.clone()]),
                cx,
            );
            assert_eq!(digests, vec![*reference.digest()]);
            assert_eq!(composer.draft_body().attachments, vec![reference.clone()]);
            composer.restore_stored_attachment(
                &thread("restored"),
                reference.digest(),
                ImageMimeType::Png,
                RECALL_PRIMARY_PNG,
                cx,
            );
            composer.draft_change()
        })
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let composer = view.read(app);
        assert!(
            composer
                .attachments
                .iter()
                .all(ComposerAttachment::is_ready)
        );
        assert_eq!(
            composer.attachments[0].bytes.as_deref().map(Vec::as_slice),
            Some(RECALL_PRIMARY_PNG)
        );
        assert_eq!(composer.draft_body().attachments, vec![reference]);
        assert_eq!(
            composer.draft_change(),
            change,
            "restored bytes change nothing stored"
        );
        assert!(composer.unstored_attachments().is_empty());
    });
}
