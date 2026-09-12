use super::*;
use artisan_domain::{CommandReceipt, ReceiptDisposition, RunUsageReportInput};

fn thread(value: &str) -> ThreadId {
    ThreadId::parse(value).expect("thread id")
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).expect("request id")
}

fn message(value: &str) -> MessageId {
    MessageId::parse(value).expect("message id")
}

fn listing(
    thread_id: &ThreadId,
    rows: Vec<artisan_domain::QueuedMessageSummary>,
) -> QueuedMessageListing {
    QueuedMessageListing::new(
        thread_id.clone(),
        QueuedMessageListOrder::OldestFirst,
        COMPOSER_QUEUE_PAGE_LIMIT,
        rows.len() as u64,
        rows,
    )
    .expect("valid listing")
}

fn summary(
    thread_id: &ThreadId,
    id: &str,
    text: Option<&str>,
) -> artisan_domain::QueuedMessageSummary {
    artisan_domain::QueuedMessageSummary {
        message_id: message(id),
        thread_id: thread_id.clone(),
        original_request_id: request(&format!("request-{id}")),
        text: text.map(|value| AuthoredText::parse(value).expect("text")),
        attachments: Vec::new(),
        accepted_at: UnixMillis::EPOCH,
        last_error: None,
    }
}

fn withdrawal(
    request_id: &str,
    thread_id: &ThreadId,
    message_id: &MessageId,
    original_request_id: &RequestId,
    outcome: QueuedMessageWithdrawalOutcome,
) -> QueuedMessageWithdrawalResult {
    QueuedMessageWithdrawalResult {
        receipt: CommandReceipt {
            request_id: request(request_id),
            disposition: ReceiptDisposition::Accepted,
        },
        thread_id: thread_id.clone(),
        message_id: message_id.clone(),
        original_request_id: original_request_id.clone(),
        accepted_at: UnixMillis::EPOCH,
        outcome,
    }
}

fn report(
    thread_id: &ThreadId,
    run_id: &RunId,
    source_sequence: u64,
    model_id: &str,
    route_id: &str,
) -> RunUsageReport {
    RunUsageReport::new(RunUsageReportInput {
        run_id: run_id.clone(),
        thread_id: thread_id.clone(),
        provider_session_id: "session-1".to_owned(),
        source_sequence,
        model_id: EngineModelId::parse(model_id).expect("model id"),
        provider_route_id: EngineRouteId::parse(route_id).expect("route id"),
        variant_id: None,
        basis: artisan_domain::RunUsageBasis::Delta,
        provider_turn_id: None,
        input_tokens: Some(0),
        cached_input_tokens: None,
        output_tokens: Some(4),
        context_tokens: Some(0),
        context_window_tokens: Some(100),
        observed_at: UnixMillis::EPOCH,
    })
    .expect("usage report")
}

#[test]
fn queue_refresh_is_bounded_and_does_not_overlap() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 4);
    let token = state
        .begin_queue_refresh(true, true, false, false, true)
        .expect("forced refresh");
    assert!(
        state
            .begin_queue_refresh(true, true, false, false, true)
            .is_none()
    );
    assert_eq!(token.generation(), 4);
    state
        .apply_queue_listing(&token, &listing(&thread_id, Vec::new()))
        .expect("bounded empty listing");
    assert!(!state.queue_refresh_in_flight());
    assert!(
        state
            .begin_queue_refresh(true, true, false, false, false)
            .is_none()
    );
}

#[test]
fn exact_withdrawal_receipt_ownership_rejects_wrong_target_and_accepts_duplicate() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 9);
    let token = state
        .begin_queue_refresh(true, true, false, false, true)
        .expect("refresh");
    state
        .apply_queue_listing(
            &token,
            &listing(&thread_id, vec![summary(&thread_id, "one", Some("keep"))]),
        )
        .expect("listing");
    let identity = state.entries()[0].identity().clone();
    let command = state
        .begin_discard(&identity, request("withdraw-one"))
        .expect("discard intent");
    let wrong = withdrawal(
        "withdraw-other",
        &thread_id,
        command.message_id(),
        command.original_request_id(),
        QueuedMessageWithdrawalOutcome::Withdrawn,
    );
    let error = state
        .accept_withdrawal_result(wrong)
        .expect_err("wrong request must not settle");
    assert_eq!(error.reason, WithdrawalReceiptRejection::TargetMismatch);
    assert!(state.pending_withdrawal().is_some());

    let result = withdrawal(
        "withdraw-one",
        &thread_id,
        command.message_id(),
        command.original_request_id(),
        QueuedMessageWithdrawalOutcome::Withdrawn,
    );
    assert_eq!(
        state.accept_withdrawal_result(result.clone()),
        Ok(WithdrawalReceiptDisposition::Discarded)
    );
    assert_eq!(
        state.accept_withdrawal_result(result),
        Ok(WithdrawalReceiptDisposition::DuplicateHandled)
    );
}

#[test]
fn stale_usage_is_rejected_without_reinterpreting_the_reporting_model() {
    let thread_id = thread("thread-a");
    let run_id = RunId::parse("run-a").expect("run id");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 3);
    assert!(state.begin_usage_scope(
        thread_id.clone(),
        3,
        run_id.clone(),
        EngineModelId::parse("model-a").expect("model"),
        EngineRouteId::parse("route-a").expect("route"),
        None,
    ));
    let (usage_read, _) = state.begin_usage_read().expect("usage read");
    let fresh = RunUsageResult {
        thread_id: thread_id.clone(),
        run_id: run_id.clone(),
        report: Some(report(&thread_id, &run_id, 2, "model-a", "route-a")),
    };
    assert_eq!(
        state.accept_usage_result(fresh, &usage_read, "Model A".to_owned()),
        Ok(UsageResultDisposition::Updated)
    );
    let (next_usage_read, _) = state.begin_usage_read().expect("next usage read");
    let older = RunUsageResult {
        thread_id: thread_id.clone(),
        run_id: run_id.clone(),
        report: Some(report(&thread_id, &run_id, 1, "model-a", "route-a")),
    };
    let error = state
        .accept_usage_result(older, &next_usage_read, "newly selected model".to_owned())
        .expect_err("older source sequence must be rejected");
    assert_eq!(error.reason, UsageRejection::StaleSequence);
    let usage = state
        .reporting_usage_for(Some(run_id.as_str()))
        .expect("last report remains");
    assert_eq!(usage.model_id, "model-a");
    assert_eq!(usage.model_name, "Model A");
    assert_eq!(usage.context_tokens, Some(0));
    assert_eq!(usage.context_window_tokens, Some(100));
}

#[test]
fn absent_usage_fields_remain_absent_and_scope_mismatch_is_rejected() {
    let thread_id = thread("thread-a");
    let run_id = RunId::parse("run-a").expect("run id");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 1);
    assert!(state.begin_usage_scope(
        thread_id.clone(),
        1,
        run_id.clone(),
        EngineModelId::parse("model-a").expect("model"),
        EngineRouteId::parse("route-a").expect("route"),
        None,
    ));
    let absent = RunUsageResult {
        thread_id: thread_id.clone(),
        run_id: run_id.clone(),
        report: None,
    };
    let (usage_read, _) = state.begin_usage_read().expect("usage read");
    assert_eq!(
        state.accept_usage_result(absent, &usage_read, "Model A".to_owned()),
        Ok(UsageResultDisposition::NoReport)
    );
    assert!(state.reporting_usage_for(Some(run_id.as_str())).is_none());
    let wrong_thread = RunUsageResult {
        thread_id: thread("thread-b"),
        run_id,
        report: None,
    };
    let (next_usage_read, _) = state.begin_usage_read().expect("next usage read");
    let error = state
        .accept_usage_result(wrong_thread, &next_usage_read, "Model A".to_owned())
        .expect_err("wrong thread must be fenced");
    assert_eq!(error.reason, UsageRejection::WrongThread);
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one race scenario drives the full withdrawal, typing, restore-rejection, and explicit-retry sequence; splitting it would hide the causal ordering"
)]
fn withdrawn_payload_survives_a_typing_race_for_explicit_retry(cx: &mut gpui::TestAppContext) {
    let (composer, cx) =
        cx.add_window_view(|_, cx| crate::native_composer::NativeComposer::new(cx));
    let thread_id = thread("thread-a");
    let target = cx.update(|_, app| {
        composer.update(app, |composer, composer_cx| {
            composer.switch_thread("thread-a", false, composer_cx);
            composer
                .capture_recall_target()
                .expect("empty composer target")
        })
    });

    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 2);
    let refresh = state
        .begin_queue_refresh(true, true, false, false, true)
        .expect("queue refresh");
    state
        .apply_queue_listing(
            &refresh,
            &listing(&thread_id, vec![summary(&thread_id, "a", Some("queued"))]),
        )
        .expect("queue listing");
    let identity = state.entries()[0].identity().clone();
    let command = state
        .begin_edit(&identity, request("withdraw-a"), target)
        .expect("edit intent");
    assert_eq!(
        state
            .accept_withdrawal_result(withdrawal(
                "withdraw-a",
                &thread_id,
                command.message_id(),
                command.original_request_id(),
                QueuedMessageWithdrawalOutcome::Withdrawn,
            ))
            .expect("withdrawal receipt"),
        WithdrawalReceiptDisposition::EditNeedsRead(ReadRecalledMessage::new(
            thread_id.clone(),
            command.message_id().clone(),
            command.original_request_id().clone(),
        ))
    );
    assert_eq!(
        state.next_recalled_message_read(),
        Some(ReadRecalledMessage::new(
            thread_id.clone(),
            command.message_id().clone(),
            command.original_request_id().clone(),
        ))
    );

    let retry_query = ReadRecalledMessage::new(
        thread_id.clone(),
        command.message_id().clone(),
        command.original_request_id().clone(),
    );
    assert!(!state.can_retry_recalled_read());
    assert!(state.mark_recalled_read_failed(&retry_query));
    assert!(state.can_retry_recalled_read());
    assert_eq!(state.next_recalled_message_read(), Some(retry_query));
    assert!(!state.can_retry_recalled_read());
    assert!(state.next_recalled_message_read().is_none());

    let payload = artisan_domain::QueueMessagePayload::text_only("queued text").expect("payload");
    let recalled = RecalledMessageResult::new(
        thread_id,
        command.message_id().clone(),
        command.original_request_id().clone(),
        Some(payload),
    )
    .expect("recalled result");
    assert_eq!(
        state
            .accept_recalled_message(recalled)
            .expect("read result"),
        RecalledMessageDisposition::PayloadReady
    );
    let candidate = state
        .take_restore_candidate()
        .expect("candidate is owned by the state");
    let RecallRestoreCandidate {
        identity,
        thread_id,
        message_id,
        original_request_id,
        target,
        payload,
    } = candidate;

    cx.update(|_, app| {
        composer.update(app, |composer, composer_cx| {
            composer.set_draft("user started typing");
            composer_cx.notify();
        });
    });
    let result = cx.update(|_, app| {
        composer.update(app, |composer, composer_cx| {
            composer.restore_recalled_payload(&target, payload, composer_cx)
        })
    });
    let payload = result.expect_err("typing must not be overwritten");
    state
        .retain_restore_candidate(RecallRestoreCandidate::new(
            identity,
            thread_id,
            message_id,
            original_request_id,
            target,
            payload,
        ))
        .expect("the unchanged payload remains available");
    assert!(state.can_retry_restore());
}

fn failed_summary(thread_id: &ThreadId, id: &str) -> artisan_domain::FailedMessageSummary {
    artisan_domain::FailedMessageSummary {
        message_id: message(id),
        thread_id: thread_id.clone(),
        original_request_id: request(&format!("request-{id}")),
        text: Some(AuthoredText::parse("hello").expect("text")),
        attachments: Vec::new(),
        accepted_at: UnixMillis::from_millis(300),
        failed_at: UnixMillis::from_millis(500),
        reason: artisan_domain::DispatchError::parse(
            "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue".to_owned(),
        )
        .expect("diagnostic"),
    }
}

fn failed_listing(
    thread_id: &ThreadId,
    rows: Vec<artisan_domain::FailedMessageSummary>,
) -> artisan_domain::FailedMessageListing {
    artisan_domain::FailedMessageListing::new(
        thread_id.clone(),
        COMPOSER_QUEUE_PAGE_LIMIT,
        rows.len() as u64,
        rows,
    )
    .expect("valid failed listing")
}

#[test]
fn failed_refresh_installs_exact_rows_without_touching_queue_fence() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 6);
    assert!(
        state
            .begin_failed_refresh(true, true, false, false)
            .is_none(),
        "slow poll discovers nothing while no failure exists"
    );
    let token = state
        .begin_failed_refresh(true, true, false, true)
        .expect("forced failed refresh");
    assert!(state.failed_refresh_in_flight());
    assert!(
        state
            .begin_queue_refresh(true, true, false, false, true)
            .is_some(),
        "the queued fence stays independent"
    );
    state
        .apply_failed_listing(
            &token,
            &failed_listing(&thread_id, vec![failed_summary(&thread_id, "one")]),
        )
        .expect("failed page");
    assert!(!state.failed_refresh_in_flight());
    assert_eq!(state.failed_total_count(), 1);
    let [entry] = state.failed_entries() else {
        panic!("exactly one failed row should be installed");
    };
    assert_eq!(entry.message_id(), &message("one"));
    assert_eq!(entry.thread_id(), &thread_id);
    assert_eq!(entry.card_text(), "hello");
    assert!(!entry.has_attachments());
    assert_eq!(
        entry.reason(),
        "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue"
    );
    assert_eq!(entry.identity().generation(), 6);
    let token = state
        .begin_failed_refresh(true, true, false, false)
        .expect("slow poll continues while failures persist");
    assert!(state.finish_failed_refresh(&token));
}

#[test]
fn failed_listing_rejects_stale_wrong_thread_and_duplicate_rows() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 6);
    let token = state
        .begin_failed_refresh(true, true, false, true)
        .expect("failed refresh");
    state.set_scope(Some(thread_id.clone()), 7);
    assert_eq!(
        state.apply_failed_listing(&token, &failed_listing(&thread_id, Vec::new())),
        Err(FailedListingRejection::StaleRefresh)
    );

    let other = thread("thread-b");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(other.clone()), 6);
    let token = state
        .begin_failed_refresh(true, true, false, true)
        .expect("failed refresh");
    assert_eq!(
        state.apply_failed_listing(&token, &failed_listing(&thread_id, Vec::new())),
        Err(FailedListingRejection::WrongThread)
    );

    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 6);
    let token = state
        .begin_failed_refresh(true, true, false, true)
        .expect("failed refresh");
    let mut first = failed_summary(&thread_id, "one");
    let mut second = failed_summary(&thread_id, "two");
    second.message_id = message("two");
    second.original_request_id = first.original_request_id.clone();
    first.message_id = message("one");
    assert_eq!(
        state.apply_failed_listing(&token, &failed_listing(&thread_id, vec![first, second])),
        Err(FailedListingRejection::DuplicateCommandId)
    );
}

#[test]
fn failed_scope_change_clears_failed_rows_and_fence() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 6);
    let token = state
        .begin_failed_refresh(true, true, false, true)
        .expect("failed refresh");
    state
        .apply_failed_listing(
            &token,
            &failed_listing(&thread_id, vec![failed_summary(&thread_id, "one")]),
        )
        .expect("failed page");
    assert_eq!(state.failed_entries().len(), 1);
    state.set_scope(Some(thread("thread-b")), 7);
    assert!(state.failed_entries().is_empty());
    assert_eq!(state.failed_total_count(), 0);
    assert!(!state.failed_refresh_in_flight());
}

#[test]
fn taken_up_rows_leave_the_lip_while_still_listed() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 4);
    let token = state
        .begin_queue_refresh(true, true, false, false, true)
        .expect("forced refresh");
    state
        .apply_queue_listing(
            &token,
            &listing(
                &thread_id,
                vec![
                    summary(&thread_id, "message-a", Some("first")),
                    summary(&thread_id, "message-b", Some("second")),
                ],
            ),
        )
        .expect("queued page");
    assert_eq!(state.pending_lip_rows().len(), 2);
    // The transcript echo for the first row retires it even though the
    // next listing still carries it: the lip yields to the transcript.
    assert!(state.mark_taken_up(&message("message-a")));
    assert!(!state.mark_taken_up(&message("message-a")));
    let rows = state.pending_lip_rows();
    assert_eq!(
        rows.iter()
            .map(|row| row.command_id.as_str())
            .collect::<Vec<_>>(),
        ["request-message-b"]
    );
    // A later listing that still carries the echoed row cannot revive it.
    let token = state
        .begin_queue_refresh(true, true, false, false, true)
        .expect("forced refresh");
    state
        .apply_queue_listing(
            &token,
            &listing(
                &thread_id,
                vec![
                    summary(&thread_id, "message-a", Some("first")),
                    summary(&thread_id, "message-b", Some("second")),
                ],
            ),
        )
        .expect("queued page");
    assert_eq!(state.pending_lip_rows().len(), 1);
}

#[test]
fn take_up_set_prunes_against_the_listing_while_retired_history_holds() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 4);
    let token = state
        .begin_queue_refresh(true, true, false, false, true)
        .expect("forced refresh");
    state
        .apply_queue_listing(
            &token,
            &listing(
                &thread_id,
                vec![summary(&thread_id, "message-a", Some("first"))],
            ),
        )
        .expect("queued page");
    assert!(state.mark_taken_up(&message("message-a")));
    // The row leaves the page: the set entry is pruned because it
    // cannot lip, while the finite history keeps the echo retired.
    let token = state
        .begin_queue_refresh(true, true, false, false, true)
        .expect("forced refresh");
    state
        .apply_queue_listing(&token, &listing(&thread_id, Vec::new()))
        .expect("empty page");
    assert!(state.pending_lip_rows().is_empty());
    // A dispatcher requeue re-lists the echoed row: it must not re-lip
    // beside its transcript twin.
    let token = state
        .begin_queue_refresh(true, true, false, false, true)
        .expect("forced refresh");
    state
        .apply_queue_listing(
            &token,
            &listing(
                &thread_id,
                vec![summary(&thread_id, "message-a", Some("first"))],
            ),
        )
        .expect("queued page");
    assert!(state.pending_lip_rows().is_empty());
    assert!(!state.mark_taken_up(&message("message-a")));
}

#[test]
fn echo_watches_retire_per_source_id_and_clear_on_scope_change() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 4);
    assert_eq!(state.echo_watch_count(), 0);
    state.stage_echo_watch(message("message-a"), None, Some("Codex".to_owned()));
    state.stage_echo_watch(message("message-b"), None, None);
    assert_eq!(state.echo_watch_count(), 2);
    let watch = state
        .echo_watch_for(&message("message-a"))
        .expect("staged watch");
    assert_eq!(watch.message_id(), &message("message-a"));
    assert_eq!(watch.engine_label(), Some("Codex"));
    // Unknown ids match nothing and change nothing.
    assert!(state.echo_watch_for(&message("message-x")).is_none());
    assert!(!state.clear_echo_watch_for(&message("message-x")));
    // Retiring one watch leaves the other staged.
    assert!(state.mark_taken_up(&message("message-a")));
    assert!(!state.mark_taken_up(&message("message-a")));
    assert!(state.clear_echo_watch_for(&message("message-a")));
    assert!(!state.clear_echo_watch_for(&message("message-a")));
    assert_eq!(state.echo_watch_count(), 1);
    assert!(state.echo_watch_for(&message("message-b")).is_some());
    // A fresh watch dies with its thread scope.
    state.set_scope(Some(thread("thread-b")), 5);
    assert_eq!(state.echo_watch_count(), 0);
    assert!(state.pending_lip_rows().is_empty());
}

#[test]
fn echo_watch_replay_replaces_and_overflow_evicts_oldest() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 4);
    state.stage_echo_watch(message("message-a"), None, Some("Old".to_owned()));
    // A replayed receipt for the same message replaces, not duplicates.
    state.stage_echo_watch(message("message-a"), None, Some("New".to_owned()));
    assert_eq!(state.echo_watch_count(), 1);
    assert_eq!(
        state
            .echo_watch_for(&message("message-a"))
            .expect("replaced watch")
            .engine_label(),
        Some("New")
    );
    for index in 0..ECHO_WATCH_LIMIT {
        state.stage_echo_watch(message(&format!("message-{index}")), None, None);
    }
    assert_eq!(state.echo_watch_count(), ECHO_WATCH_LIMIT);
    // The oldest entry (message-a, then message-0) evicted first.
    assert!(state.echo_watch_for(&message("message-a")).is_none());
}
