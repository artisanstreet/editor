//! Sending from the new-thread screen ("What should we build in …?"): Send
//! submits the project's new-task draft by revision, the Forge answers the
//! thread it created, and the Editor opens it. A send whose answer was lost
//! names the same revision again, and Send never silently does nothing.

use super::*;
use crate::native_application::NativeRoute;

fn new_task_project() -> ProjectId {
    ProjectId::parse("new-task-editor").expect("project")
}

fn project_scope() -> ComposerDraftScope {
    ComposerDraftScope::Project(new_task_project())
}

/// The new-thread screen of a project without an open thread, its composer
/// showing the project's draft with `text` typed into it.
fn new_thread_screen(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    text: &str,
    sink: NativeTestCommandSink,
) {
    application.test_command_sink = Some(sink);
    let projects =
        ProjectListing::new(vec![project(new_task_project().as_str(), "editor")]).unwrap();
    application.handle_projects(&projects, cx);
    application.handle_empty_threads(&new_task_project(), cx);
    application.navigate(
        NativeRoute::NewThread {
            project: Some(new_task_project()),
        },
        cx,
    );
    application
        .composer
        .update(cx, |composer, cx| composer.type_at_end(text, cx));
    application.observe_composer_change(cx);
}

fn send_is_offered(application: &NativeApplication, cx: &gpui::App) -> bool {
    application.composer.read(cx).send_ready()
        && application.composer_controls.read(cx).snapshot().send_ready
}

/// The Forge stores the project draft's latest save at `revision`.
fn ack_project_save(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    revision: u64,
) {
    let sequence = application
        .composer_drafts
        .sent
        .borrow()
        .iter()
        .filter_map(|command| match command {
            ComposerDraftCommand::Save { sequence, command }
                if command.scope() == &project_scope() =>
            {
                Some(*sequence)
            }
            _ => None,
        })
        .next_back()
        .expect("the project draft is saved before it is sent");
    application.receive_draft_event(
        ComposerDraftEvent::Saved {
            scope: project_scope(),
            sequence,
            revision: ComposerDraftRevision::new(revision).expect("revision"),
        },
        cx,
    );
}

fn flight_request(application: &NativeApplication) -> RequestId {
    application
        .message_flight
        .as_ref()
        .expect("a send in flight")
        .request_id
        .clone()
}

/// Answers the refresh of the project's listing the Editor asked for, with
/// `thread` listed.
fn list_created_thread(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    thread: &ThreadId,
) {
    let (project, generation) = application
        .sidebar_threads
        .pending
        .clone()
        .expect("the project's listing is read again");
    assert_eq!(project, new_task_project());
    let listing = ThreadListing::new(vec![super::super::thread(
        thread.as_str(),
        project.as_str(),
        "New task",
    )])
    .unwrap();
    application.receive_refreshed_threads(&project, generation, Ok(listing), cx);
}

#[gpui::test]
fn send_on_the_new_thread_screen_submits_the_project_draft_and_opens_its_thread(
    cx: &mut TestAppContext,
) {
    let created = ThreadId::parse("created-by-send").expect("thread");
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            new_thread_screen(application, cx, "build the parser", sink);
            assert!(
                send_is_offered(application, cx),
                "Send is enabled on the new-thread screen once text is typed"
            );

            application.begin_message_submission(cx);
            assert!(
                application.message_flight.is_some(),
                "Send on the new-thread screen is not a no-op"
            );
            assert!(
                submissions(&commands.borrow()).is_empty(),
                "nothing is sent before the Forge stores the body"
            );
            ack_project_save(application, cx, 12);
            let sent = submissions(&commands.borrow());
            assert_eq!(sent.len(), 1, "the project's draft is submitted");
            assert_eq!(sent[0].scope, project_scope());
            assert_eq!(
                sent[0].draft_revision,
                ComposerDraftRevision::new(12).expect("revision")
            );
            let request_id = flight_request(application);

            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    request_id.as_str(),
                    &created,
                    "first-message",
                    ReceiptDisposition::Accepted,
                )),
                cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(cx).draft(), "");
            list_created_thread(application, cx, &created);
            assert_eq!(application.selected_thread.as_ref(), Some(&created));
            assert_eq!(
                application.route(),
                &NativeRoute::Thread {
                    project: new_task_project(),
                    thread: created.clone(),
                }
            );
        });
    });
}

#[gpui::test]
fn a_new_task_resent_after_a_lost_answer_names_the_same_revision(cx: &mut TestAppContext) {
    let created = ThreadId::parse("created-once").expect("thread");
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            new_thread_screen(application, cx, "send me once", sink);
            application.begin_message_submission(cx);
            ack_project_save(application, cx, 21);
            let first = flight_request(application);
            // The connection drops before the Forge's answer arrives.
            application.handle_service_event(
                NativeTransportEvent::MessageFailed {
                    scope: project_scope(),
                    request_id: first.clone(),
                    failure: message_failure(),
                },
                cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(cx).draft(), "send me once");
            assert!(send_is_offered(application, cx));

            application.begin_message_submission(cx);
            let second = flight_request(application);
            assert_ne!(first, second);
            // The Forge answers the repeated revision with the thread and
            // message the lost attempt created.
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    second.as_str(),
                    &created,
                    "message-once",
                    ReceiptDisposition::Duplicate,
                )),
                cx,
            );
            assert!(application.message_flight.is_none());
            list_created_thread(application, cx, &created);
            assert_eq!(application.selected_thread.as_ref(), Some(&created));
        });
    });
    let sent = submissions(&commands.borrow());
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|submit| submit.scope == project_scope()));
    assert_eq!(sent[0].draft_revision, sent[1].draft_revision);
}

#[gpui::test]
fn send_without_a_project_says_why_and_keeps_the_draft(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.handle_projects(&ProjectListing::new(Vec::new()).unwrap(), cx);
            application.navigate(NativeRoute::NewThread { project: None }, cx);
            application
                .composer
                .update(cx, |composer, cx| composer.type_at_end("orphan idea", cx));
            application.observe_composer_change(cx);
            assert!(application.message_submission_is_admissible(cx));

            application.begin_message_submission(cx);
            assert!(application.message_flight.is_none());
            assert!(
                application
                    .message_failure_note
                    .as_deref()
                    .is_some_and(|note| note.contains("Choose a project")),
                "Send says why it could not send"
            );
            assert_eq!(application.composer.read(cx).draft(), "orphan idea");
        });
    });
    assert!(submissions(&commands.borrow()).is_empty());
}
