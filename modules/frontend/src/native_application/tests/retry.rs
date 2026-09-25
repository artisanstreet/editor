//! Failed sends and Forge-owned retry.
//!
//! A request the Forge never accepted keeps its draft in the composer and is
//! sent again as a new request; the Editor keeps no copy to replay. A message
//! the Forge accepted and then failed is retried by identity: the Forge
//! dispatches its stored payload again and reports the row state in the
//! pushed outbox, which is the only source of pending and failed rows.
use super::*;
use crate::native_transport_service::{ComposerStateCommand, ComposerStateEvent};
use artisan_domain::{
    FailedMessageRetried, FailedMessageRetryOutcome, FailedMessageTarget, MessageId,
    QueuedMessageState,
};

fn pending_rows(application: &NativeApplication, cx: &gpui::App) -> Vec<(String, String)> {
    application
        .conversation_host
        .as_ref()
        .expect("message host")
        .read(cx)
        .surface()
        .read(cx)
        .pending_message_rows()
        .iter()
        .map(|row| (row.message_id.clone(), row.status.clone()))
        .collect()
}

fn failed_target(thread_id: &ThreadId) -> FailedMessageTarget {
    FailedMessageTarget {
        thread_id: thread_id.clone(),
        message_id: MessageId::parse("message-1").expect("message"),
        original_request_id: request("queue-1"),
    }
}

#[gpui::test]
fn failed_send_keeps_the_draft_and_offers_no_local_retry(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-thread").expect("thread");
    let body = "  exact retry body\n  ";
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id.clone(),
                body,
                sink,
            );
            install_configured_engine_settings(application, application_cx);
            application.begin_message_submission(application_cx);
            assert_eq!(application.composer.read(application_cx).draft(), "");
            assert!(
                pending_rows(application, application_cx).is_empty(),
                "the row appears only when the Forge's outbox carries it"
            );
            let request_id = application
                .message_flight
                .as_ref()
                .expect("admitted flight")
                .request_id
                .clone();
            application.handle_service_event(
                NativeTransportEvent::MessageFailed {
                    thread_id: thread_id.clone(),
                    request_id,
                    failure: message_failure(),
                },
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(application_cx).draft(), body);
            let controls = application.composer_controls.read(application_cx);
            let failure = controls
                .snapshot()
                .failure
                .as_ref()
                .expect("failure banner");
            assert!(!failure.retryable, "sending again is the user's Send");
        });
    });
    cx.run_until_parked();
    assert_eq!(queued_messages(&commands.borrow()).len(), 1);
    cx.update(|_, app| assert!(!view.read(app).composer.read(app).is_submitting()));
}

#[gpui::test]
fn forge_retry_names_only_the_failed_message(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-forge-thread").expect("thread");
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id.clone(),
                "unrelated draft",
                sink,
            );
            seed_failed_entry(application, &thread_id, 3);
            application.sync_composer_controls(application_cx);
            let controls = application.composer_controls.read(application_cx);
            let rows = &controls.snapshot().failed_dispatches;
            assert_eq!(rows.len(), 1);
            assert!(rows[0].retryable);
            assert!(application.handle_queue_control(
                &NativeComposerControlsEvent::RetryFailedDispatch {
                    command_id: "queue-1".to_owned(),
                    generation: 3,
                },
                application_cx,
            ));
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "unrelated draft",
                "a Forge retry never touches the draft"
            );
        });
    });
    let commands = commands.borrow();
    assert!(queued_messages(&commands).is_empty());
    let retries = commands
        .iter()
        .filter_map(|command| match command {
            NativeTransportCommand::ComposerState(ComposerStateCommand::RetryFailedMessage {
                command,
                ..
            }) => Some(command),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(retries.len(), 1);
    assert_eq!(retries[0].target, failed_target(&thread_id));
    assert!(retries[0].request_id.as_str().starts_with("native-retry-"));
}

#[gpui::test]
fn stale_retry_generation_sends_nothing(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-stale-generation").expect("thread");
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(application, application_cx, thread_id.clone(), "", sink);
            seed_failed_entry(application, &thread_id, 3);
            application.handle_queue_control(
                &NativeComposerControlsEvent::RetryFailedDispatch {
                    command_id: "queue-1".to_owned(),
                    generation: 2,
                },
                application_cx,
            );
        });
    });
    assert!(commands.borrow().is_empty());
}

#[gpui::test]
fn not_retryable_answer_points_to_a_new_chat(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-refused-thread").expect("thread");
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(application, application_cx, thread_id.clone(), "", sink);
            seed_failed_entry(application, &thread_id, 3);
            let answer = |outcome| ComposerStateEvent::FailedMessageRetried {
                generation: 3,
                result: FailedMessageRetried {
                    request_id: request("native-retry-1"),
                    target: failed_target(&thread_id),
                    outcome,
                },
            };
            application.handle_composer_state_event(
                answer(FailedMessageRetryOutcome::Requeued),
                application_cx,
            );
            assert!(application.message_failure.is_none());
            application.handle_composer_state_event(
                answer(FailedMessageRetryOutcome::NotRetryable),
                application_cx,
            );
            assert!(application.message_failure.is_some());
            assert!(
                application
                    .message_failure_note
                    .as_deref()
                    .is_some_and(|note| note.contains("new chat"))
            );
        });
    });
}

#[gpui::test]
fn forge_outbox_rows_are_the_same_after_a_restart(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-outbox-thread").expect("thread");
    let other = ThreadId::parse("retry-outbox-other").expect("thread");
    let mut waiting = queued_summary(&thread_id, "message-b", QueuedMessageState::Queued);
    waiting.last_error =
        Some(artisan_domain::DispatchError::parse("engine not ready".to_owned()).expect("reason"));
    let outbox = message_outbox(
        &thread_id,
        vec![
            queued_summary(&thread_id, "message-a", QueuedMessageState::Dispatching),
            waiting,
        ],
        vec![failed_summary(&thread_id, false)],
    );
    let expected = vec![
        ("message-a".to_owned(), "Starting…".to_owned()),
        (
            "message-b".to_owned(),
            "Waiting: engine not ready".to_owned(),
        ),
    ];
    // Two application instances stand in for the Editor before and after a
    // restart: neither has rows until the Forge pushes its outbox.
    for _ in 0..2 {
        let (view, window_cx) = cx.add_window_view(signed_in_test_application);
        let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
        let other_outbox = message_outbox(
            &other,
            vec![queued_summary(
                &other,
                "message-x",
                QueuedMessageState::Queued,
            )],
            Vec::new(),
        );
        let outbox = outbox.clone();
        let rows = window_cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id.clone(),
                    "",
                    sink,
                );
                assert!(pending_rows(application, application_cx).is_empty());
                application.handle_service_event(
                    NativeTransportEvent::MessageOutbox(other_outbox),
                    application_cx,
                );
                assert!(pending_rows(application, application_cx).is_empty());
                application.handle_service_event(
                    NativeTransportEvent::MessageOutbox(outbox),
                    application_cx,
                );
                let controls = application.composer_controls.read(application_cx);
                let failed = &controls.snapshot().failed_dispatches;
                assert_eq!(failed.len(), 1);
                assert!(!failed[0].retryable, "the Forge decides retryability");
                pending_rows(application, application_cx)
            })
        });
        assert_eq!(rows, expected);
    }
}

#[gpui::test]
fn delivered_message_leaves_the_transcript_tail_with_the_outbox(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-delivered-thread").expect("thread");
    let (view, cx) = cx.add_window_view(signed_in_test_application);
    let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(application, application_cx, thread_id.clone(), "", sink);
            application.handle_service_event(
                NativeTransportEvent::MessageOutbox(message_outbox(
                    &thread_id,
                    vec![queued_summary(
                        &thread_id,
                        "message-a",
                        QueuedMessageState::Queued,
                    )],
                    Vec::new(),
                )),
                application_cx,
            );
            assert_eq!(pending_rows(application, application_cx).len(), 1);
            application.handle_service_event(
                NativeTransportEvent::MessageOutbox(message_outbox(
                    &thread_id,
                    Vec::new(),
                    Vec::new(),
                )),
                application_cx,
            );
            assert!(pending_rows(application, application_cx).is_empty());
        });
    });
}

#[test]
fn message_failure_presentation_contains_only_redacted_stage_and_category() {
    let detail = message_status_detail(None, Some(NativeMessageFailure::new(message_failure())))
        .expect("failure detail");
    assert_eq!(detail, "Send failed: request (peer).");
    for secret in [
        "body text",
        "retry-request-id",
        "https://forge.invalid",
        "credential-value",
        "peer detail",
    ] {
        assert!(!detail.contains(secret), "failure detail leaked {secret}");
    }
}

#[gpui::test]
fn busy_and_stopped_admission_retains_the_draft_without_an_application_flight(
    cx: &mut TestAppContext,
) {
    let (view, _) = cx.add_window_view(test_application);
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            for (error, draft) in [
                (
                    super::CommandSendError::Busy,
                    "busy admission body".to_owned(),
                ),
                (
                    super::CommandSendError::Stopped,
                    "stopped admission body".to_owned(),
                ),
            ] {
                let (_, token) = application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft(draft.clone());
                        composer.begin_payload_submission()
                    })
                    .expect("begin");
                application.reject_message_submission(
                    token,
                    super::command_failure(error),
                    application_cx,
                );
                assert!(application.message_flight.is_none());
                assert_eq!(application.composer.read(application_cx).draft(), draft);
                assert!(!application.composer.read(application_cx).is_submitting());
            }
        });
    });
}

#[gpui::test]
fn accepted_and_duplicate_receipts_clear_only_the_matching_flight(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("forge-thread").expect("thread");
    let (view, _) = cx.add_window_view(test_application);
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_thread = Some(thread_id.clone());
            application.state = NativeViewState::Ready;
            let (_, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("first exact body");
                    composer.begin_payload_submission()
                })
                .expect("first begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: thread_id.clone(),
                request_id: artisan_domain::RequestId::parse("request-first").expect("request"),
                token,
            });
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    "request-first",
                    &thread_id,
                    "message-first",
                    ReceiptDisposition::Accepted,
                )),
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(application_cx).draft(), "");
            assert_eq!(
                application
                    .message_receipt
                    .as_ref()
                    .map(|receipt| receipt.disposition),
                Some(ReceiptDisposition::Accepted)
            );

            let (_, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("second exact body");
                    composer.begin_payload_submission()
                })
                .expect("second begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: thread_id.clone(),
                request_id: artisan_domain::RequestId::parse("request-second").expect("request"),
                token,
            });
            application.composer.update(application_cx, |composer, _| {
                composer.set_draft("newer draft while duplicate is pending");
            });
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    "request-second",
                    &thread_id,
                    "message-second",
                    ReceiptDisposition::Duplicate,
                )),
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "newer draft while duplicate is pending"
            );
            assert_eq!(
                application
                    .message_receipt
                    .as_ref()
                    .map(|receipt| receipt.disposition),
                Some(ReceiptDisposition::Duplicate)
            );
        });
    });
}

#[gpui::test]
fn stale_queue_results_do_not_clear_a_newer_draft(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("forge-thread").expect("thread");
    let (view, _) = cx.add_window_view(test_application);
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_thread = Some(thread_id.clone());
            application.state = NativeViewState::Ready;
            let (_, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("newer draft");
                    composer.begin_payload_submission()
                })
                .expect("begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: thread_id.clone(),
                request_id: artisan_domain::RequestId::parse("request-newer").expect("request"),
                token,
            });
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    "request-stale",
                    &thread_id,
                    "message-stale",
                    ReceiptDisposition::Accepted,
                )),
                application_cx,
            );
            assert!(application.message_flight.is_some());
            assert_eq!(application.composer.read(application_cx).draft(), "");
        });
    });
}

#[gpui::test]
fn queue_failure_and_service_stop_retain_the_draft(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("forge-thread").expect("thread");
    let (view, _) = cx.add_window_view(test_application);
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_thread = Some(thread_id.clone());
            application.state = NativeViewState::Ready;
            let (_, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("retained queue body");
                    composer.begin_payload_submission()
                })
                .expect("begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: thread_id.clone(),
                request_id: artisan_domain::RequestId::parse("request-failure").expect("request"),
                token,
            });
            application.handle_service_event(
                NativeTransportEvent::MessageFailed {
                    thread_id: thread_id.clone(),
                    request_id: artisan_domain::RequestId::parse("request-failure")
                        .expect("request"),
                    failure: ServiceFailure {
                        stage: super::ServiceFailureStage::Request,
                        category: super::ServiceFailureCategory::Peer,
                    },
                },
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "retained queue body"
            );
            assert!(application.message_failure.is_some());

            let (_, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("retained on stop");
                    composer.begin_payload_submission()
                })
                .expect("second begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: thread_id.clone(),
                request_id: artisan_domain::RequestId::parse("request-stop").expect("request"),
                token,
            });
            application.handle_service_event(
                NativeTransportEvent::Stopped(ServiceStopStatus::Clean),
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "retained on stop"
            );
        });
    });
}

#[gpui::test]
fn real_thread_transition_and_shutdown_retain_and_clear_old_presentation(cx: &mut TestAppContext) {
    let old_thread = ThreadId::parse("old-thread").expect("thread");
    let (view, _) = cx.add_window_view(test_application);
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_thread = Some(old_thread.clone());
            application.state = NativeViewState::Ready;
            let (_, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("transition body");
                    composer.begin_payload_submission()
                })
                .expect("begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: old_thread.clone(),
                request_id: artisan_domain::RequestId::parse("request-transition")
                    .expect("request"),
                token,
            });
            application.message_receipt = Some(first_receipt(
                "request-old",
                &old_thread,
                "message-old",
                ReceiptDisposition::Accepted,
            ));
            application.message_failure = Some(NativeMessageFailure::new(ServiceFailure {
                stage: super::ServiceFailureStage::Request,
                category: super::ServiceFailureCategory::Peer,
            }));
            application.retire_host(application_cx);
            assert!(application.message_flight.is_none());
            assert_eq!(application.selected_thread, None);
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "transition body"
            );
            assert!(application.message_receipt.is_none());
            assert!(application.message_failure.is_none());

            application.selected_thread = Some(old_thread.clone());
            application.state = NativeViewState::Ready;
            let (_, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("shutdown body");
                    composer.begin_payload_submission()
                })
                .expect("shutdown begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: old_thread,
                request_id: artisan_domain::RequestId::parse("request-shutdown").expect("request"),
                token,
            });
            application.prepare_shutdown(application_cx);
            assert!(application.message_flight.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "shutdown body"
            );
            assert!(application.message_receipt.is_none());
            assert!(application.message_failure.is_none());
        });
    });
}
