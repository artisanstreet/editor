use super::*;
use sea_orm::ConnectionTrait;

#[tokio::test]
async fn crash_recovers_five_messages_preserves_completed_text_and_replays_atomically() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (claimed, receipt, key, credentials) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let bound = bind_running(&repository, &claimed, &receipt, &key, &credentials).await;
    let ids: Vec<_> = (0..6)
        .map(|i| ItemId::parse(format!("assistant-{i}")).unwrap())
        .collect();
    let starts: Vec<_> = (0..6)
        .map(|i| PatchId::parse(format!("start-{i}")).unwrap())
        .collect();
    let body = AssistantBody::parse("preserved output").unwrap();
    let changes: Vec<_> = ids
        .iter()
        .zip(&starts)
        .map(|(item_id, patch_id)| AssistantChange::Start {
            item_id,
            patch_id,
            phase: AssistantMessagePhase::Final,
            body: &body,
        })
        .collect();
    repository
        .commit_run_batch(artisan_database::CommitRunBatch {
            scope: artisan_database::RunBatchScope {
                claimed: &claimed,
                launched: &receipt,
                bound: &bound,
                run_start_key: &key,
                credentials: &credentials,
                expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
                expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
            },
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_AT_MS),
            activate_turn_patch_id: Some(&PatchId::parse("activate").unwrap()),
            changes: &changes,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .unwrap();
    database
        .execute_unprepared(
            "UPDATE conversation_items SET lifecycle='completed' WHERE item_id='assistant-5'",
        )
        .await
        .unwrap();
    let before = fetch_all(&database).await;
    let now = UnixMillis::from_millis(LEASE_EXPIRES_AT_MS);
    let candidates = repository
        .list_startup_reconciliation_candidates(StartupReconciliationQuery::new(now, 1).unwrap())
        .await
        .unwrap();
    let candidate = &candidates[0];
    assert_eq!(candidate.assistant_item_ids.len(), 5);
    let turn_patch = PatchId::parse("recovery-turn").unwrap();
    let patches: Vec<_> = (0..5)
        .map(|i| PatchId::parse(format!("recovery-item-{i}")).unwrap())
        .collect();
    let command = || StartupReconciliationDisposition {
        candidate,
        operated_at: now,
        turn_patch_id: &turn_patch,
        item_patch_ids: &patches,
    };
    assert!(matches!(
        repository
            .dispose_expired_startup_candidate(command())
            .await
            .unwrap(),
        StartupReconciliationDispositionOutcome::Interrupted(_)
    ));
    let after = fetch_all(&database).await;
    assert_eq!(after.patches.len(), before.patches.len() + 6);
    for old in &before.items {
        let new = after
            .items
            .iter()
            .find(|i| i.item_id == old.item_id)
            .unwrap();
        assert_eq!(new.body, old.body);
        if old.lifecycle == EntityLifecycle::Completed {
            assert_eq!(new, old);
        } else {
            assert_eq!(new.lifecycle, EntityLifecycle::Interrupted);
            assert_eq!(new.revision, old.revision + 1);
        }
    }
    assert!(matches!(
        repository
            .dispose_expired_startup_candidate(command())
            .await
            .unwrap(),
        StartupReconciliationDispositionOutcome::AlreadyInterrupted(_)
    ));
    assert_eq!(fetch_all(&database).await, after);
}

#[tokio::test]
async fn unresolved_expired_run_blocks_only_its_own_thread() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    // A queued follow-up in the unresolved run's thread, older than healthy work.
    entities::message::ActiveModel {
        message_id: Set("follow-up".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(1),
        body: Set("continue".to_owned()),
        accepted_at_ms: Set(51),
    }
    .insert(&database)
    .await
    .unwrap();
    database.execute_unprepared("INSERT INTO message_dispatches (message_id, correlation_id, state, attempt_count, queued_at_ms, available_at_ms, updated_at_ms) VALUES ('follow-up','follow-up-request','queued',0,51,51,51)").await.unwrap();
    seed_project_and_thread(&database, &repository, "thread-2").await;
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("healthy-request").unwrap(),
            message_id: MessageId::parse("healthy-message").unwrap(),
            thread_id: ThreadId::parse("thread-2").unwrap(),
            body: MessageBody::parse("healthy").unwrap(),
            accepted_at: UnixMillis::from_millis(60),
        })
        .await
        .unwrap();
    let claim = || ClaimMessageDispatch {
        owner: artisan_database::DispatchLeaseOwner::new([9; 32]),
        claimed_at: UnixMillis::from_millis(700),
        lease_expires_at: UnixMillis::from_millis(900),
    };
    assert_eq!(
        repository
            .claim_next_message_dispatch(claim())
            .await
            .unwrap()
            .unwrap()
            .message_id
            .as_str(),
        "healthy-message"
    );
    assert!(
        repository
            .claim_next_message_dispatch(claim())
            .await
            .unwrap()
            .is_none()
    );
}
