//! Sending the composer's Forge draft by revision: Send waits for the save
//! that stores the body being sent, names that revision, and a send
//! repeated after a lost answer names the same revision, so the Forge
//! answers the message it already queued instead of queueing another.

use super::*;
use crate::native_transport_service::{ComposerDraftCommand, ComposerDraftEvent};
use artisan_domain::{ComposerDraftRevision, ComposerDraftScope, SubmitComposerDraft};

fn submissions(commands: &[NativeTransportCommand]) -> Vec<SubmitComposerDraft> {
    commands
        .iter()
        .filter_map(|command| match command {
            NativeTransportCommand::SubmitComposerDraft(command) => Some((**command).clone()),
            _ => None,
        })
        .collect()
}

/// Draft saves sent so far, as (sequence, text).
fn saves(application: &NativeApplication) -> Vec<(u64, String)> {
    application
        .composer_drafts
        .sent
        .borrow()
        .iter()
        .filter_map(|command| match command {
            ComposerDraftCommand::Save { sequence, command } => {
                Some((*sequence, command.text().as_str().to_owned()))
            }
            _ => None,
        })
        .collect()
}

/// A ready thread whose composer shows the Forge draft `text`.
fn ready_thread(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    thread_id: &ThreadId,
    text: &str,
    sink: NativeTestCommandSink,
) {
    install_ready_message_surface(application, cx, thread_id.clone(), "", sink);
    install_configured_engine_settings(application, cx);
    application.reopen_with_forge_draft(thread_id.as_str(), text, cx);
}

#[gpui::test]
fn resending_after_a_lost_answer_names_the_same_draft_revision(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("draft-send-lost").expect("thread");
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, commands) = command_sink([Ok(()), Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            ready_thread(application, cx, &thread_id, "send me once", sink);
            application.begin_message_submission(cx);
            let first = application
                .message_flight
                .as_ref()
                .expect("flight")
                .request_id
                .clone();
            // The connection drops before the Forge's answer arrives.
            application.handle_service_event(
                NativeTransportEvent::MessageFailed {
                    scope: artisan_domain::ComposerDraftScope::Thread(thread_id.clone()),
                    request_id: first.clone(),
                    failure: message_failure(),
                },
                cx,
            );
            assert_eq!(application.composer.read(cx).draft(), "send me once");

            application.begin_message_submission(cx);
            let second = application
                .message_flight
                .as_ref()
                .expect("flight")
                .request_id
                .clone();
            assert_ne!(first, second, "the request id only correlates one attempt");
            // The Forge answers the repeated revision with the message the
            // lost attempt queued.
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    second.as_str(),
                    &thread_id,
                    "message-once",
                    ReceiptDisposition::Duplicate,
                )),
                cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(cx).draft(), "");
        });
    });
    let sent = submissions(&commands.borrow());
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].draft_revision, sent[1].draft_revision);
    assert_ne!(sent[0].request_id, sent[1].request_id);
    assert_eq!(queued_messages(&commands.borrow()).len(), 2);
}

#[gpui::test]
fn send_waits_for_the_save_that_stores_its_body(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("draft-send-wait").expect("thread");
    let scope = ComposerDraftScope::Thread(thread_id.clone());
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            ready_thread(application, cx, &thread_id, "", sink);
            application
                .composer
                .update(cx, |composer, cx| composer.type_at_end("hello", cx));
            application.begin_message_submission(cx);
            assert!(application.message_flight.is_some(), "the send is admitted");
            assert!(
                submissions(&commands.borrow()).is_empty(),
                "nothing is sent before the Forge stores the body"
            );
            let (sequence, text) = saves(application).last().cloned().expect("save sent");
            assert_eq!(text, "hello");

            // Typed after Send: saved only once the send is on the wire.
            application
                .composer
                .update(cx, |composer, cx| composer.type_at_end("next", cx));
            application.sync_composer_draft(cx);
            assert_eq!(saves(application).len(), 1);

            let revision = ComposerDraftRevision::new(90).expect("revision");
            application.receive_draft_event(
                ComposerDraftEvent::Saved {
                    scope: scope.clone(),
                    sequence,
                    revision,
                },
                cx,
            );
            let sent = submissions(&commands.borrow());
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].draft_revision, revision);
            assert_eq!(sent[0].scope, ComposerDraftScope::Thread(thread_id.clone()));
            assert_eq!(
                saves(application).last().map(|(_, text)| text.as_str()),
                Some("next")
            );
        });
    });
}

#[gpui::test]
fn a_stale_answer_keeps_the_draft_and_saves_it_again(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("draft-send-stale").expect("thread");
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            ready_thread(application, cx, &thread_id, "hello", sink);
            application.begin_message_submission(cx);
            assert_eq!(submissions(&commands.borrow()).len(), 1);
            let request_id = application
                .message_flight
                .as_ref()
                .expect("flight")
                .request_id
                .clone();
            let before = saves(application).len();
            application.handle_service_event(
                NativeTransportEvent::MessageStale {
                    scope: artisan_domain::ComposerDraftScope::Thread(thread_id.clone()),
                    request_id,
                    current_revision: Some(ComposerDraftRevision::new(95).expect("revision")),
                },
                cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(cx).draft(), "hello");
            assert!(
                application
                    .message_failure_note
                    .as_deref()
                    .is_some_and(|note| note.contains("changed"))
            );
            let after = saves(application);
            assert_eq!(
                after.len(),
                before + 1,
                "the composer's text is saved again"
            );
            assert_eq!(after.last().map(|(_, text)| text.as_str()), Some("hello"));
        });
    });
}

#[path = "new_thread_send.rs"]
mod new_thread_send;
