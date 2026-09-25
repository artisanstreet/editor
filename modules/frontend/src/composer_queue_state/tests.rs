use super::*;
use artisan_domain::{
    CommandReceipt, FailedMessageListing, QueuedMessageListOrder, QueuedMessageListing,
    ReceiptDisposition, RunUsageReportInput, UnixMillis,
};

fn thread(value: &str) -> ThreadId {
    ThreadId::parse(value).expect("thread id")
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).expect("request id")
}

fn message(value: &str) -> MessageId {
    MessageId::parse(value).expect("message id")
}

fn summary(
    thread_id: &ThreadId,
    id: &str,
    state: QueuedMessageState,
) -> artisan_domain::QueuedMessageSummary {
    artisan_domain::QueuedMessageSummary {
        message_id: message(id),
        thread_id: thread_id.clone(),
        original_request_id: request(&format!("request-{id}")),
        text: Some(AuthoredText::parse(id).expect("text")),
        attachments: Vec::new(),
        accepted_at: UnixMillis::EPOCH,
        last_error: None,
        state,
        engine: Some(EngineId::Codex),
    }
}

fn failed_summary(thread_id: &ThreadId, id: &str) -> FailedMessageSummary {
    FailedMessageSummary {
        message_id: message(id),
        thread_id: thread_id.clone(),
        original_request_id: request(&format!("request-{id}")),
        text: Some(AuthoredText::parse("hello").expect("text")),
        attachments: Vec::new(),
        accepted_at: UnixMillis::from_millis(300),
        failed_at: UnixMillis::from_millis(500),
        reason: DispatchError::parse("engine profile unavailable".to_owned()).expect("reason"),
        retryable: true,
    }
}

fn outbox(
    thread_id: &ThreadId,
    queued: Vec<artisan_domain::QueuedMessageSummary>,
    failed: Vec<FailedMessageSummary>,
) -> MessageOutbox {
    MessageOutbox::new(
        QueuedMessageListing::new(
            thread_id.clone(),
            QueuedMessageListOrder::OldestFirst,
            COMPOSER_QUEUE_PAGE_LIMIT,
            queued.len() as u64,
            queued,
        )
        .expect("queued listing"),
        FailedMessageListing::new(
            thread_id.clone(),
            COMPOSER_QUEUE_PAGE_LIMIT,
            failed.len() as u64,
            failed,
        )
        .expect("failed listing"),
    )
    .expect("outbox")
}

fn withdrawal(
    request_id: &str,
    command: &WithdrawQueuedMessageCommand,
    outcome: QueuedMessageWithdrawalOutcome,
) -> QueuedMessageWithdrawalResult {
    QueuedMessageWithdrawalResult {
        receipt: CommandReceipt {
            request_id: request(request_id),
            disposition: ReceiptDisposition::Accepted,
        },
        thread_id: command.thread_id.clone(),
        message_id: command.message_id.clone(),
        original_request_id: command.original_request_id.clone(),
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
fn outbox_installs_forge_rows_and_reports_only_later_arrivals() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 4);
    let arrivals = state
        .apply_outbox(&outbox(
            &thread_id,
            vec![summary(&thread_id, "one", QueuedMessageState::Dispatching)],
            vec![failed_summary(&thread_id, "failed")],
        ))
        .expect("outbox");
    assert!(
        arrivals.is_empty(),
        "rows present at mount are not arrivals"
    );
    let [entry] = state.entries() else {
        panic!("one queued row");
    };
    assert_eq!(entry.state(), QueuedMessageState::Dispatching);
    assert_eq!(entry.engine(), Some(EngineId::Codex));
    let [failure] = state.failed_entries() else {
        panic!("one failed row");
    };
    assert!(failure.retryable());
    assert_eq!(failure.target().message_id, message("failed"));

    let arrivals = state
        .apply_outbox(&outbox(
            &thread_id,
            vec![
                summary(&thread_id, "one", QueuedMessageState::Dispatching),
                summary(&thread_id, "two", QueuedMessageState::Queued),
            ],
            Vec::new(),
        ))
        .expect("outbox");
    assert_eq!(arrivals, vec![message("two")]);
    assert!(
        state.failed_entries().is_empty(),
        "the Forge dropped the failure"
    );

    // A message that reached the transcript simply leaves the outbox.
    state
        .apply_outbox(&outbox(
            &thread_id,
            vec![summary(&thread_id, "two", QueuedMessageState::Queued)],
            Vec::new(),
        ))
        .expect("outbox");
    assert_eq!(state.entries().len(), 1);
    assert_eq!(state.entries()[0].message_id(), &message("two"));
}

#[test]
fn outbox_for_another_thread_or_with_reused_ids_is_rejected_whole() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 4);
    state
        .apply_outbox(&outbox(
            &thread_id,
            vec![summary(&thread_id, "one", QueuedMessageState::Queued)],
            Vec::new(),
        ))
        .expect("outbox");
    let other = thread("thread-b");
    assert_eq!(
        state.apply_outbox(&outbox(&other, Vec::new(), Vec::new())),
        Err(OutboxRejection::WrongThread)
    );
    let mut first = summary(&thread_id, "a", QueuedMessageState::Queued);
    let second = summary(&thread_id, "b", QueuedMessageState::Queued);
    first.original_request_id = second.original_request_id.clone();
    assert_eq!(
        state.apply_outbox(&outbox(&thread_id, vec![first, second], Vec::new())),
        Err(OutboxRejection::DuplicateCommandId)
    );
    assert_eq!(state.entries().len(), 1, "the previous outbox stays");
}

#[test]
fn scope_change_drops_the_outbox_until_the_new_thread_pushes_its_own() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 4);
    state
        .apply_outbox(&outbox(
            &thread_id,
            vec![summary(&thread_id, "one", QueuedMessageState::Queued)],
            vec![failed_summary(&thread_id, "failed")],
        ))
        .expect("outbox");
    let next = thread("thread-b");
    state.set_scope(Some(next.clone()), 5);
    assert!(state.entries().is_empty());
    assert!(state.failed_entries().is_empty());
    let arrivals = state
        .apply_outbox(&outbox(
            &next,
            vec![summary(&next, "two", QueuedMessageState::Queued)],
            Vec::new(),
        ))
        .expect("outbox");
    assert!(arrivals.is_empty());
    assert_eq!(state.entries()[0].identity().generation(), 5);
}

#[test]
fn exact_withdrawal_receipt_ownership_rejects_wrong_target_and_accepts_duplicate() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 9);
    state
        .apply_outbox(&outbox(
            &thread_id,
            vec![summary(&thread_id, "one", QueuedMessageState::Queued)],
            Vec::new(),
        ))
        .expect("outbox");
    let identity = state.entries()[0].identity().clone();
    let command = state
        .begin_discard(&identity, request("withdraw-one"))
        .expect("discard intent");
    assert!(!command.recall_to_draft);
    let error = state
        .accept_withdrawal_result(withdrawal(
            "withdraw-other",
            &command,
            QueuedMessageWithdrawalOutcome::Withdrawn,
        ))
        .expect_err("wrong request must not settle");
    assert_eq!(error.reason, WithdrawalReceiptRejection::TargetMismatch);
    assert!(state.pending_withdrawal().is_some());

    let result = withdrawal(
        "withdraw-one",
        &command,
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
fn edit_recalls_the_payload_into_the_forge_draft_and_locks_until_answered() {
    let thread_id = thread("thread-a");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 2);
    state
        .apply_outbox(&outbox(
            &thread_id,
            vec![
                summary(&thread_id, "queued", QueuedMessageState::Queued),
                summary(&thread_id, "starting", QueuedMessageState::Dispatching),
            ],
            Vec::new(),
        ))
        .expect("outbox");
    let dispatching = state.entries()[1].identity().clone();
    assert_eq!(
        state.begin_edit(&dispatching, request("withdraw-late")),
        Err(QueueIntentError::StaleRow),
        "a dispatching row can no longer be withdrawn"
    );
    let identity = state.entries()[0].identity().clone();
    let command = state
        .begin_edit(&identity, request("withdraw-edit"))
        .expect("edit intent");
    assert!(
        command.recall_to_draft,
        "the Forge owns the recalled payload"
    );
    assert!(state.edit_pending());
    assert_eq!(
        state.accept_withdrawal_result(withdrawal(
            "withdraw-edit",
            &command,
            QueuedMessageWithdrawalOutcome::Withdrawn,
        )),
        Ok(WithdrawalReceiptDisposition::Recalled)
    );
    assert!(!state.edit_pending());
    assert_eq!(state.status(), QueueStatus::RecalledToDraft);
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
        compaction_at_tokens: None,
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
        compaction_at_tokens: None,
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
        compaction_at_tokens: None,
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
        compaction_at_tokens: None,
    };
    let (next_usage_read, _) = state.begin_usage_read().expect("next usage read");
    let error = state
        .accept_usage_result(wrong_thread, &next_usage_read, "Model A".to_owned())
        .expect_err("wrong thread must be fenced");
    assert_eq!(error.reason, UsageRejection::WrongThread);
}

#[test]
fn pushed_usage_updates_the_live_run_without_a_read_and_keeps_every_fence() {
    let thread_id = thread("thread-a");
    let run_id = RunId::parse("run-a").expect("run id");
    let mut state = ComposerQueueState::new();
    state.set_scope(Some(thread_id.clone()), 2);
    assert!(state.begin_usage_scope(
        thread_id.clone(),
        2,
        run_id.clone(),
        EngineModelId::parse("model-a").expect("model"),
        EngineRouteId::parse("route-a").expect("route"),
        None,
    ));
    let pushed = |sequence, run: &RunId| RunUsageResult {
        thread_id: thread_id.clone(),
        run_id: run.clone(),
        report: Some(report(&thread_id, run, sequence, "model-a", "route-a")),
        compaction_at_tokens: None,
    };
    assert_eq!(
        state.accept_pushed_usage(pushed(2, &run_id), "Model A".to_owned()),
        Ok(UsageResultDisposition::Updated)
    );
    let older = state
        .accept_pushed_usage(pushed(1, &run_id), "Model A".to_owned())
        .expect_err("an older push never replaces a newer report");
    assert_eq!(older.reason, UsageRejection::StaleSequence);
    let other = RunId::parse("run-b").expect("run id");
    let foreign = state
        .accept_pushed_usage(pushed(3, &other), "Model A".to_owned())
        .expect_err("another run's usage is not this scope's");
    assert_eq!(foreign.reason, UsageRejection::WrongRun);
    assert!(state.reporting_usage_for(Some(run_id.as_str())).is_some());
}
