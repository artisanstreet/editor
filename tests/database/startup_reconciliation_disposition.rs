use artisan_database::entities::{
    self, AssistantRunLifecycle, ConversationPatchKind, DispatchState, EntityLifecycle, RenderPhase,
};
use artisan_database::{
    AssistantChange, BindRunProvider, CheckpointUpdate, ClaimMessageDispatch,
    ClaimedMessageDispatch, ProviderBindingBytes, QueueFirstMessageInput, Repository,
    RunLaunchCredentials, RunStartKey, SetThreadEngineConfigInput, SqliteConfig,
    StartupReconciliationDisposition, StartupReconciliationDispositionError,
    StartupReconciliationDispositionOutcome, StartupReconciliationQuery, StartupRunLifecycle,
    connect,
};
use artisan_domain::{
    ApprovalMode, AssistantBody, AssistantMessagePhase, ByteLimit, CountLimit, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, ItemId, MessageBody, MessageId, NetworkAccess,
    OpenCode2Selection, PatchId, PermissionId, ProjectId, RequestId, RunId, ThreadId, ThreadTitle,
    TurnId, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

const OWNER_BYTES: [u8; 32] = [0xa1; 32];
const LEASE_BYTES: [u8; 32] = [0xb2; 32];
const CLAIM_TOKEN_BYTES: [u8; 32] = [0xc3; 32];
const DISPATCH_OWNER_BYTE: u8 = 0x11;

const THREAD_CREATED_AT_MS: i64 = 10;
const ACCEPTED_AT_MS: i64 = 50;
const CLAIMED_AT_MS: i64 = 100;
const OPERATED_AT_MS: i64 = 150;
const BOUND_AT_MS: i64 = 200;
const BATCH_AT_MS: i64 = 250;
const LEASE_EXPIRES_AT_MS: i64 = 600;

const RUN_ERROR_CODE: &str = "startup_reconciliation_unknown_outcome";
const RUN_ERROR_MESSAGE: &str =
    "startup reconciliation interrupted with unknown outcome; provider state may have progressed";
const DISPATCH_REASON: &str = "startup reconciliation: unknown outcome after lease expiry";

async fn memory_database() -> (DatabaseConnection, Repository) {
    let database = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("memory database should open");
    migrate_to_current(&database).await.expect("migrate");
    (database.clone(), Repository::new(database))
}

fn fixture_engine_config() -> EngineRunConfig {
    let one = FiniteMillis::new(1).expect("one millisecond is valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).expect("attempt budget is valid"),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit is valid"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit is valid"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit is valid"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness line limit is valid"),
        max_header_count: CountLimit::new(8).expect("header count is valid"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer limit is valid"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr limit is valid"),
        observation_capacity: CountLimit::new(16).expect("observation capacity is valid"),
    })
    .expect("runtime relationships are valid");
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-reconcile").expect("permission id is valid"),
        EngineAgentId::parse("agent-reconcile").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-reconcile").expect("profile id is valid"),
            EngineModelId::parse("model-reconcile").expect("model id is valid"),
            EngineRouteId::parse("route-reconcile").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

async fn seed_project_and_thread(
    database: &DatabaseConnection,
    repository: &Repository,
    thread_id: &str,
) {
    let project = entities::attached_project::ActiveModel {
        project_id: Set("project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(1),
    };
    let _ = entities::attached_project::Entity::insert(project)
        .exec(database)
        .await;
    let _ = repository
        .create_thread(artisan_database::CreateThreadInput {
            request_id: RequestId::parse(format!("req-{thread_id}")).expect("req"),
            thread_id: ThreadId::parse(thread_id).expect("tid"),
            project_id: ProjectId::parse("project-1").expect("pid"),
            title: ThreadTitle::parse("Thread").expect("title"),
            created_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
            updated_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await;
    let thread = ThreadId::parse(thread_id).expect("tid");
    if repository
        .read_thread_engine_settings(&thread)
        .await
        .expect("engine configuration should read")
        .is_none()
    {
        repository
            .set_thread_engine_config(SetThreadEngineConfigInput {
                request_id: RequestId::parse(format!("engine-{thread_id}")).expect("request id"),
                thread_id: thread,
                precondition: EngineConfigUpdatePrecondition::Unconfigured,
                config: fixture_engine_config(),
                accepted_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
            })
            .await
            .expect("engine configuration should create");
    }
}

#[allow(clippy::too_many_lines)]
async fn queue_claim_launch(
    repository: &Repository,
    _database: &DatabaseConnection,
    thread_id: &str,
    message_id: &str,
    run_id: &str,
    turn_id: &str,
) -> (
    ClaimedMessageDispatch,
    artisan_database::LaunchedRunReceipt,
    RunStartKey,
    RunLaunchCredentials,
) {
    let queue = QueueFirstMessageInput {
        request_id: RequestId::parse(format!("req-{message_id}")).expect("req"),
        message_id: MessageId::parse(message_id).expect("mid"),
        thread_id: ThreadId::parse(thread_id).expect("tid"),
        body: MessageBody::parse("hello").expect("body"),
        accepted_at: UnixMillis::from_millis(ACCEPTED_AT_MS),
    };
    repository.queue_first_message(queue).await.expect("queue");
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: artisan_database::DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
            claimed_at: UnixMillis::from_millis(CLAIMED_AT_MS),
            lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
        })
        .await
        .expect("claim")
        .expect("claimed");
    let run = RunId::parse(run_id).expect("run");
    let turn = TurnId::parse(turn_id).expect("turn");
    let item = ItemId::parse(format!("item-{run_id}")).expect("item");
    let p1 = PatchId::parse(format!("patch-{run_id}-a")).expect("p");
    let p2 = PatchId::parse(format!("patch-{run_id}-b")).expect("p");
    let mut bytes = [0u8; 32];
    for (idx, byte) in run_id.bytes().cycle().take(32).enumerate() {
        bytes[idx] = byte ^ 0x5a;
    }
    let len_u8 = u8::try_from(run_id.len()).unwrap_or(255);
    bytes[0] = bytes[0].wrapping_add(len_u8);
    let start_key = RunStartKey::new(bytes);
    let creds = RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, CLAIM_TOKEN_BYTES);
    let engine_settings = repository
        .read_thread_engine_settings(&ThreadId::parse(thread_id).expect("tid"))
        .await
        .expect("engine configuration should read")
        .expect("engine configuration should be present");
    let outcome = repository
        .launch_claimed_run(artisan_database::LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run,
            turn_id: &turn,
            item_id: &item,
            first_patch_id: &p1,
            second_patch_id: &p2,
            operated_at: UnixMillis::from_millis(OPERATED_AT_MS),
            run_start_key: &start_key,
            credentials: &creds,
            engine_settings: &engine_settings,
        })
        .await
        .expect("launch");
    let artisan_database::LaunchClaimedRunOutcome::Started(receipt) = outcome else {
        panic!("started");
    };
    (claimed, receipt, start_key, creds)
}

async fn bind_running(
    repository: &Repository,
    claimed: &ClaimedMessageDispatch,
    receipt: &artisan_database::LaunchedRunReceipt,
    start_key: &RunStartKey,
    creds: &RunLaunchCredentials,
) -> artisan_database::BoundRunReceipt {
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let outcome = repository
        .bind_run_provider(BindRunProvider {
            claimed,
            receipt,
            run_start_key: start_key,
            credentials: creds,
            expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
            bound_at: UnixMillis::from_millis(BOUND_AT_MS),
            binding_version: 2,
            binding_bytes: &binding,
        })
        .await
        .expect("bind");
    match outcome {
        artisan_database::BindRunProviderOutcome::Bound(r)
        | artisan_database::BindRunProviderOutcome::AlreadyBound(r) => r,
    }
}

struct RunningBatchFixtures<'a> {
    repository: &'a Repository,
    claimed: &'a ClaimedMessageDispatch,
    receipt: &'a artisan_database::LaunchedRunReceipt,
    bound: &'a artisan_database::BoundRunReceipt,
    start_key: &'a RunStartKey,
    creds: &'a RunLaunchCredentials,
}

async fn commit_running_item(
    fixtures: &RunningBatchFixtures<'_>,
    item_id: &ItemId,
    patch_turn: &PatchId,
    patch_item: &PatchId,
) {
    let body = AssistantBody::parse("hello assistant").expect("body");
    fixtures
        .repository
        .commit_run_batch(artisan_database::CommitRunBatch {
            scope: artisan_database::RunBatchScope {
                claimed: fixtures.claimed,
                launched: fixtures.receipt,
                bound: fixtures.bound,
                run_start_key: fixtures.start_key,
                credentials: fixtures.creds,
                expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
                expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
            },
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_AT_MS),
            activate_turn_patch_id: Some(patch_turn),
            changes: &[AssistantChange::Start {
                item_id,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: patch_item,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("commit batch");
}

async fn fetch_all(database: &DatabaseConnection) -> AllRows {
    async fn all<E>(db: &DatabaseConnection) -> Vec<E::Model>
    where
        E: EntityTrait,
    {
        E::find().all(db).await.expect("rows")
    }
    let mut dispatches = all::<entities::message_dispatch::Entity>(database).await;
    let mut runs = all::<entities::assistant_run::Entity>(database).await;
    let mut turns = all::<entities::conversation_turn::Entity>(database).await;
    let mut items = all::<entities::conversation_item::Entity>(database).await;
    let mut patches = all::<entities::conversation_patch::Entity>(database).await;
    let mut states = all::<entities::conversation_state::Entity>(database).await;
    dispatches.sort_by(|a, b| a.message_id.cmp(&b.message_id));
    runs.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    turns.sort_by(|a, b| a.turn_id.cmp(&b.turn_id));
    items.sort_by(|a, b| a.item_id.cmp(&b.item_id));
    patches.sort_by_key(|a| a.sequence);
    states.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
    AllRows {
        dispatches,
        runs,
        turns,
        items,
        patches,
        states,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AllRows {
    dispatches: Vec<entities::MessageDispatch>,
    runs: Vec<entities::AssistantRun>,
    turns: Vec<entities::ConversationTurn>,
    items: Vec<entities::ConversationItem>,
    patches: Vec<entities::ConversationPatch>,
    states: Vec<entities::ConversationState>,
}

// ---------------------------------------------------------------------------
// 1. expired launching / no-item pair: run interrupted, dispatch failed, turn interrupted with one patch; binding remains absent
// ---------------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn expired_launching_no_item_single_turn_patch() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_claimed, _receipt, _sk, _creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;

    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("query");
    let candidates = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("candidates");
    assert_eq!(candidates.len(), 1);
    let candidate = candidates
        .iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    assert_eq!(candidate.lifecycle, StartupRunLifecycle::Launching);
    assert!(candidate.assistant_item_id.is_none());

    let before = fetch_all(&database).await;
    let before_run = before
        .runs
        .iter()
        .find(|r| r.run_id == "run-1")
        .expect("run");
    assert_eq!(before_run.lifecycle, AssistantRunLifecycle::Launching);
    assert!(before_run.provider_binding.is_none());
    assert!(before_run.provider_binding_version.is_none());
    let before_turn = before
        .turns
        .iter()
        .find(|t| t.turn_id == "turn-1")
        .expect("turn");
    let before_state = before
        .states
        .iter()
        .find(|s| s.thread_id == "thread-1")
        .expect("state");
    let before_last_seq = before_state.last_patch_sequence;
    let before_turn_rev = before_turn.revision;

    let operated_at = UnixMillis::from_millis(LEASE_EXPIRES_AT_MS);
    let turn_patch = PatchId::parse("turn-patch-1").expect("p");
    let outcome = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate,
            operated_at,
            turn_patch_id: &turn_patch,
            item_patch_id: None,
        })
        .await
        .expect("dispose");
    assert!(matches!(
        outcome,
        StartupReconciliationDispositionOutcome::Interrupted(_)
    ));

    let after = fetch_all(&database).await;
    // Run: interrupted, owner/lease/claim cleared, binding retained absent, error pair, terminal null, updated_at advanced.
    let run = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-1")
        .expect("run");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Interrupted);
    assert!(run.owner.is_none());
    assert!(run.lease.is_none());
    assert!(run.claim_token.is_none());
    assert!(run.provider_binding.is_none());
    assert!(run.provider_binding_version.is_none());
    assert!(run.provider_bound_at_ms.is_none());
    assert_eq!(run.error_code.as_deref(), Some(RUN_ERROR_CODE));
    assert_eq!(run.error_message.as_deref(), Some(RUN_ERROR_MESSAGE));
    assert_eq!(run.terminal_at_ms, None);
    assert_eq!(run.updated_at_ms, LEASE_EXPIRES_AT_MS);
    assert!(run.updated_at_ms >= before_run.updated_at_ms);
    assert_eq!(run.generation, 1);
    assert_eq!(run.thread_id, "thread-1");
    assert_eq!(run.origin_message_id, "message-1");
    assert_eq!(run.origin_turn_id, "turn-1");

    // Dispatch: failed, cleared lease, reason, updated.
    let dispatch = after
        .dispatches
        .iter()
        .find(|d| d.message_id == "message-1")
        .expect("dispatch");
    assert_eq!(dispatch.state, DispatchState::Failed);
    assert!(dispatch.lease_owner.is_none());
    assert!(dispatch.lease_expires_at_ms.is_none());
    assert_eq!(dispatch.last_error.as_deref(), Some(DISPATCH_REASON));
    assert_eq!(dispatch.updated_at_ms, LEASE_EXPIRES_AT_MS);

    // Turn: interrupted, revision +1, updated.
    let turn = after
        .turns
        .iter()
        .find(|t| t.turn_id == "turn-1")
        .expect("turn");
    assert_eq!(turn.lifecycle, EntityLifecycle::Interrupted);
    assert_eq!(turn.revision, before_turn_rev + 1);
    assert_eq!(turn.updated_at_ms, LEASE_EXPIRES_AT_MS);
    assert_eq!(turn.ordinal, before_turn.ordinal);
    assert_eq!(turn.created_at_ms, before_turn.created_at_ms);

    // State: last_patch_sequence +1, updated.
    let state = after
        .states
        .iter()
        .find(|s| s.thread_id == "thread-1")
        .expect("state");
    assert_eq!(state.last_patch_sequence, before_last_seq + 1);
    assert_eq!(state.updated_at_ms, LEASE_EXPIRES_AT_MS);
    assert_eq!(
        state.next_renderer_ordinal,
        before_state.next_renderer_ordinal
    );

    // Patch: exactly one turn_lifecycle patch, consecutive sequence.
    let new_patches: Vec<_> = after
        .patches
        .iter()
        .filter(|p| p.patch_id == "turn-patch-1")
        .collect();
    assert_eq!(new_patches.len(), 1);
    let patch = new_patches[0];
    assert_eq!(patch.kind, ConversationPatchKind::TurnLifecycle);
    assert_eq!(patch.lifecycle, Some(EntityLifecycle::Interrupted));
    assert_eq!(patch.thread_id, "thread-1");
    assert_eq!(patch.turn_id.as_deref(), Some("turn-1"));
    assert!(patch.item_id.is_none());
    assert_eq!(patch.sequence, before_last_seq + 1);
    assert_eq!(patch.revision, before_turn_rev + 1);
    assert_eq!(patch.recorded_at_ms, LEASE_EXPIRES_AT_MS);
    assert!(patch.ordinal.is_none());
    assert!(patch.body.is_none() && patch.fragment.is_none());

    // Binding remains absent, unrelated rows unchanged: only one patch added.
    assert_eq!(after.patches.len(), before.patches.len() + 1);
    // No item was created.
    assert_eq!(after.items.len(), before.items.len());
}

// ---------------------------------------------------------------------------
// 2. expired running/assistant-item pair: run/dispatch/turn/item sealed in one transaction with two ordered lifecycle patches and provider binding retained
// ---------------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn expired_running_with_item_two_patches_binding_retained() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (claimed, receipt, sk, creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let bound = bind_running(&repository, &claimed, &receipt, &sk, &creds).await;
    let assistant_item = ItemId::parse("assistant-1").expect("aid");
    let p_turn_act = PatchId::parse("p-turn-act").expect("p");
    let p_item_start = PatchId::parse("p-item-start").expect("p");
    commit_running_item(
        &RunningBatchFixtures {
            repository: &repository,
            claimed: &claimed,
            receipt: &receipt,
            bound: &bound,
            start_key: &sk,
            creds: &creds,
        },
        &assistant_item,
        &p_turn_act,
        &p_item_start,
    )
    .await;

    let before = fetch_all(&database).await;
    let before_run = before
        .runs
        .iter()
        .find(|r| r.run_id == "run-1")
        .expect("run")
        .clone();
    let before_turn_rev = before
        .turns
        .iter()
        .find(|t| t.turn_id == "turn-1")
        .unwrap()
        .revision;
    let before_item_rev = before
        .items
        .iter()
        .find(|i| i.item_id == "assistant-1")
        .unwrap()
        .revision;
    let before_item_body = before
        .items
        .iter()
        .find(|i| i.item_id == "assistant-1")
        .unwrap()
        .body
        .clone();
    let before_item_phase = before
        .items
        .iter()
        .find(|i| i.item_id == "assistant-1")
        .unwrap()
        .phase
        .clone();
    let before_state_seq = before
        .states
        .iter()
        .find(|s| s.thread_id == "thread-1")
        .unwrap()
        .last_patch_sequence;

    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let candidates = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("candidates");
    let candidate = candidates
        .iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    assert_eq!(candidate.lifecycle, StartupRunLifecycle::Running);
    assert_eq!(
        candidate.assistant_item_id.as_ref().map(ItemId::as_str),
        Some("assistant-1")
    );

    let operated_at = UnixMillis::from_millis(LEASE_EXPIRES_AT_MS);
    let turn_patch = PatchId::parse("turn-patch-run").expect("p");
    let item_patch = PatchId::parse("item-patch-run").expect("p");
    let outcome = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate,
            operated_at,
            turn_patch_id: &turn_patch,
            item_patch_id: Some(&item_patch),
        })
        .await
        .expect("dispose");
    assert!(matches!(
        outcome,
        StartupReconciliationDispositionOutcome::Interrupted(_)
    ));

    let after = fetch_all(&database).await;

    // Run retains binding.
    let run = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-1")
        .expect("run");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Interrupted);
    assert!(run.owner.is_none() && run.lease.is_none() && run.claim_token.is_none());
    assert_eq!(run.provider_binding, before_run.provider_binding);
    assert_eq!(
        run.provider_binding_version,
        before_run.provider_binding_version
    );
    assert_eq!(run.provider_bound_at_ms, before_run.provider_bound_at_ms);
    assert_eq!(run.error_code.as_deref(), Some(RUN_ERROR_CODE));
    assert_eq!(run.error_message.as_deref(), Some(RUN_ERROR_MESSAGE));
    assert_eq!(run.terminal_at_ms, None);
    assert_eq!(run.updated_at_ms, LEASE_EXPIRES_AT_MS);

    // Dispatch failed.
    let dispatch = after
        .dispatches
        .iter()
        .find(|d| d.message_id == "message-1")
        .expect("dispatch");
    assert_eq!(dispatch.state, DispatchState::Failed);
    assert!(dispatch.lease_owner.is_none() && dispatch.lease_expires_at_ms.is_none());
    assert_eq!(dispatch.last_error.as_deref(), Some(DISPATCH_REASON));
    assert_eq!(dispatch.updated_at_ms, LEASE_EXPIRES_AT_MS);

    // Turn interrupted revision +1.
    let turn = after
        .turns
        .iter()
        .find(|t| t.turn_id == "turn-1")
        .expect("turn");
    assert_eq!(turn.lifecycle, EntityLifecycle::Interrupted);
    assert_eq!(turn.revision, before_turn_rev + 1);
    assert_eq!(turn.updated_at_ms, LEASE_EXPIRES_AT_MS);

    // Item interrupted, bodies/phase/ordinal preserved, revision +1.
    let item = after
        .items
        .iter()
        .find(|i| i.item_id == "assistant-1")
        .expect("item");
    assert_eq!(item.lifecycle, EntityLifecycle::Interrupted);
    assert_eq!(item.revision, before_item_rev + 1);
    assert_eq!(item.updated_at_ms, LEASE_EXPIRES_AT_MS);
    assert_eq!(item.body, before_item_body);
    assert_eq!(item.phase, before_item_phase);
    assert_eq!(item.ordinal, 2);
    assert_eq!(item.run_id.as_deref(), Some("run-1"));
    assert_eq!(item.phase, Some(RenderPhase::Final));

    // Patches: two consecutive sequences, correct payloads.
    let state = after
        .states
        .iter()
        .find(|s| s.thread_id == "thread-1")
        .expect("state");
    assert_eq!(state.last_patch_sequence, before_state_seq + 2);
    assert_eq!(state.updated_at_ms, LEASE_EXPIRES_AT_MS);

    let tp = after
        .patches
        .iter()
        .find(|p| p.patch_id == "turn-patch-run")
        .expect("turn patch");
    let ip = after
        .patches
        .iter()
        .find(|p| p.patch_id == "item-patch-run")
        .expect("item patch");
    assert_eq!(tp.kind, ConversationPatchKind::TurnLifecycle);
    assert_eq!(tp.lifecycle, Some(EntityLifecycle::Interrupted));
    assert_eq!(tp.revision, before_turn_rev + 1);
    assert_eq!(tp.recorded_at_ms, LEASE_EXPIRES_AT_MS);
    assert_eq!(ip.kind, ConversationPatchKind::ItemLifecycle);
    assert_eq!(ip.lifecycle, Some(EntityLifecycle::Interrupted));
    assert_eq!(ip.revision, before_item_rev + 1);
    assert_eq!(ip.recorded_at_ms, LEASE_EXPIRES_AT_MS);
    // Consecutive.
    let mut seqs = [tp.sequence, ip.sequence];
    seqs.sort_unstable();
    assert_eq!(seqs[1], seqs[0] + 1);
    assert!(seqs[0] == before_state_seq + 1 && seqs[1] == before_state_seq + 2);
}

// ---------------------------------------------------------------------------
// 3. unexpired equality boundary rejected/left byte-for-byte unchanged
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unexpired_boundary_rejected_unchanged() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_claimed, _receipt, _sk, _creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;

    // Operate at one ms before expiry -> not eligible.
    let operated_at = UnixMillis::from_millis(LEASE_EXPIRES_AT_MS - 1);
    let query = StartupReconciliationQuery::new(operated_at, 10).expect("q");
    let candidates = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("candidates");
    assert_eq!(candidates.len(), 0);

    // Directly attempt disposition with candidate snapshot at expiry but operated at expiry-1 (unexpired) -> SkippedMoved.
    let candidate_at_expiry = {
        let q = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
            .expect("q");
        let cands = repository
            .list_startup_reconciliation_candidates(q)
            .await
            .expect("cands");
        cands
            .into_vec()
            .into_iter()
            .find(|c| c.run_id.as_str() == "run-1")
            .expect("candidate")
    };
    let before = fetch_all(&database).await;
    let outcome = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate_at_expiry,
            operated_at,
            turn_patch_id: &PatchId::parse("turn-patch-unexp").expect("p"),
            item_patch_id: None,
        })
        .await
        .expect("dispose unexpired");
    assert!(matches!(
        outcome,
        StartupReconciliationDispositionOutcome::SkippedMoved
    ));
    let after = fetch_all(&database).await;
    assert_eq!(before, after, "unexpired call must leave bytes unchanged");
}

// ---------------------------------------------------------------------------
// 4. stale run snapshot and stale dispatch snapshot each return SkippedMoved with no partial mutation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stale_run_snapshot_skipped_no_partial() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_claimed, _receipt, _sk, _creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let mut candidate = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("cands")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    // Make stale run_updated_at (decrement by 1)
    candidate.run_updated_at = UnixMillis::from_millis(candidate.run_updated_at.as_millis() - 1);
    let before = fetch_all(&database).await;
    let outcome = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &PatchId::parse("turn-patch-stale-run").expect("p"),
            item_patch_id: None,
        })
        .await
        .expect("stale run");
    assert!(matches!(
        outcome,
        StartupReconciliationDispositionOutcome::SkippedMoved
    ));
    assert_eq!(before, fetch_all(&database).await);
}

#[tokio::test]
async fn stale_dispatch_snapshot_skipped_no_partial() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_claimed, _receipt, _sk, _creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let mut candidate = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("cands")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    candidate.dispatch_updated_at =
        UnixMillis::from_millis(candidate.dispatch_updated_at.as_millis() - 1);
    let before = fetch_all(&database).await;
    let outcome = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &PatchId::parse("turn-patch-stale-dispatch").expect("p"),
            item_patch_id: None,
        })
        .await
        .expect("stale dispatch");
    assert!(matches!(
        outcome,
        StartupReconciliationDispositionOutcome::SkippedMoved
    ));
    assert_eq!(before, fetch_all(&database).await);
}

// ---------------------------------------------------------------------------
// 5. idempotent identical replay does not duplicate patches/counters
// ---------------------------------------------------------------------------

#[tokio::test]
async fn idempotent_replay_no_duplicate() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_claimed, _receipt, _sk, _creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let candidate = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("cands")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    let turn_patch = PatchId::parse("turn-patch-replay").expect("p");
    let first = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &turn_patch,
            item_patch_id: None,
        })
        .await
        .expect("first");
    assert!(matches!(
        first,
        StartupReconciliationDispositionOutcome::Interrupted(_)
    ));
    let after_first = fetch_all(&database).await;
    let second = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &turn_patch,
            item_patch_id: None,
        })
        .await
        .expect("second");
    assert!(matches!(
        second,
        StartupReconciliationDispositionOutcome::AlreadyInterrupted(_)
    ));
    let after_second = fetch_all(&database).await;
    assert_eq!(
        after_first, after_second,
        "replay must not duplicate patches/counters"
    );
}

// With item pair as well
#[tokio::test]
async fn idempotent_replay_with_item_no_duplicate() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (claimed, receipt, sk, creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let bound = bind_running(&repository, &claimed, &receipt, &sk, &creds).await;
    let assistant_item = ItemId::parse("assistant-1").expect("aid");
    let p_turn_act = PatchId::parse("p-turn-act2").expect("p");
    let p_item_start = PatchId::parse("p-item-start2").expect("p");
    commit_running_item(
        &RunningBatchFixtures {
            repository: &repository,
            claimed: &claimed,
            receipt: &receipt,
            bound: &bound,
            start_key: &sk,
            creds: &creds,
        },
        &assistant_item,
        &p_turn_act,
        &p_item_start,
    )
    .await;
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let candidate = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("cands")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    let turn_patch = PatchId::parse("turn-patch-replay2").expect("p");
    let item_patch = PatchId::parse("item-patch-replay2").expect("p");
    let first = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &turn_patch,
            item_patch_id: Some(&item_patch),
        })
        .await
        .expect("first");
    assert!(matches!(
        first,
        StartupReconciliationDispositionOutcome::Interrupted(_)
    ));
    let after_first = fetch_all(&database).await;
    let second = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &turn_patch,
            item_patch_id: Some(&item_patch),
        })
        .await
        .expect("second");
    assert!(matches!(
        second,
        StartupReconciliationDispositionOutcome::AlreadyInterrupted(_)
    ));
    assert_eq!(after_first, fetch_all(&database).await);
}

// ---------------------------------------------------------------------------
// 6. mismatched item/patch input, duplicate patch identity, and counter/revision overflow fail typed and roll back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mismatched_item_input_fails_typed_and_rolls_back() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_claimed, _receipt, _sk, _creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let candidate = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("cands")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    // candidate has no item but we supply item patch => IdentityConflict
    let before = fetch_all(&database).await;
    let err = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &PatchId::parse("turn-patch-mismatch").expect("p"),
            item_patch_id: Some(&PatchId::parse("item-patch-mismatch").expect("p")),
        })
        .await
        .expect_err("mismatched item input should fail");
    assert!(matches!(
        err,
        StartupReconciliationDispositionError::IdentityConflict { .. }
    ));
    assert_eq!(before, fetch_all(&database).await);

    // Now create running item candidate and omit item patch => also IdentityConflict
    let (database2, repository2) = memory_database().await;
    seed_project_and_thread(&database2, &repository2, "thread-1").await;
    let (claimed, receipt, sk, creds) = queue_claim_launch(
        &repository2,
        &database2,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let bound = bind_running(&repository2, &claimed, &receipt, &sk, &creds).await;
    let assistant_item = ItemId::parse("assistant-1").expect("aid");
    commit_running_item(
        &RunningBatchFixtures {
            repository: &repository2,
            claimed: &claimed,
            receipt: &receipt,
            bound: &bound,
            start_key: &sk,
            creds: &creds,
        },
        &assistant_item,
        &PatchId::parse("p-turn-act3").expect("p"),
        &PatchId::parse("p-item-start3").expect("p"),
    )
    .await;
    let cand2 = repository2
        .list_startup_reconciliation_candidates(
            StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
                .expect("q"),
        )
        .await
        .expect("cands2")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate2");
    let before2 = fetch_all(&database2).await;
    let err2 = repository2
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &cand2,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &PatchId::parse("turn-patch-mismatch2").expect("p"),
            item_patch_id: None,
        })
        .await
        .expect_err("missing item patch should fail");
    assert!(matches!(
        err2,
        StartupReconciliationDispositionError::IdentityConflict { .. }
    ));
    assert_eq!(before2, fetch_all(&database2).await);
}

#[tokio::test]
async fn duplicate_patch_identity_fails_typed_and_rolls_back() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (claimed, receipt, sk, creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let bound = bind_running(&repository, &claimed, &receipt, &sk, &creds).await;
    let assistant_item = ItemId::parse("assistant-1").expect("aid");
    commit_running_item(
        &RunningBatchFixtures {
            repository: &repository,
            claimed: &claimed,
            receipt: &receipt,
            bound: &bound,
            start_key: &sk,
            creds: &creds,
        },
        &assistant_item,
        &PatchId::parse("p-turn-dup").expect("p"),
        &PatchId::parse("p-item-dup").expect("p"),
    )
    .await;
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let candidate = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("cands")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    let before = fetch_all(&database).await;
    let dup = PatchId::parse("dup-patch").expect("p");
    let err = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &dup,
            item_patch_id: Some(&dup),
        })
        .await
        .expect_err("duplicate patch identity should fail");
    assert!(matches!(
        err,
        StartupReconciliationDispositionError::PatchConflict { .. }
    ));
    assert_eq!(before, fetch_all(&database).await);
}

#[tokio::test]
async fn patch_sequence_overflow_fails_typed_and_rolls_back() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_claimed, _receipt, _sk, _creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    // Force last_patch_sequence to i64::MAX
    let state = entities::conversation_state::Entity::find_by_id("thread-1")
        .one(&database)
        .await
        .expect("q")
        .expect("state");
    let mut active: entities::conversation_state::ActiveModel = state.into();
    active.last_patch_sequence = Set(i64::MAX);
    active.update(&database).await.expect("update");
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let candidate = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("cands")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    let before = fetch_all(&database).await;
    let err = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &PatchId::parse("turn-patch-overflow").expect("p"),
            item_patch_id: None,
        })
        .await
        .expect_err("patch sequence overflow should fail");
    assert!(matches!(
        err,
        StartupReconciliationDispositionError::CounterOverflow { .. }
    ));
    assert_eq!(before, fetch_all(&database).await);
}

#[tokio::test]
async fn revision_overflow_fails_typed_and_rolls_back() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_claimed, _receipt, _sk, _creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    // Force turn revision to i64::MAX
    let turn = entities::conversation_turn::Entity::find_by_id("turn-1")
        .one(&database)
        .await
        .expect("q")
        .expect("turn");
    let mut active: entities::conversation_turn::ActiveModel = turn.into();
    active.revision = Set(i64::MAX);
    active.update(&database).await.expect("update");
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let candidate = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("cands")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    let before = fetch_all(&database).await;
    let err = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &PatchId::parse("turn-patch-rev-overflow").expect("p"),
            item_patch_id: None,
        })
        .await
        .expect_err("revision overflow should fail");
    assert!(matches!(
        err,
        StartupReconciliationDispositionError::CounterOverflow { .. }
    ));
    assert_eq!(before, fetch_all(&database).await);
}

// Pre-existing patch identity collides durably -> PatchConflict
#[tokio::test]
async fn existing_patch_identity_fails_typed() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_claimed, _receipt, _sk, _creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    // Manually insert a patch that will collide with disposition's turn patch.
    let state = entities::conversation_state::Entity::find_by_id("thread-1")
        .one(&database)
        .await
        .expect("q")
        .expect("state");
    let colliding = PatchId::parse("colliding-patch").expect("p");
    entities::conversation_patch::ActiveModel {
        patch_id: Set(colliding.as_str().to_owned()),
        thread_id: Set("thread-1".to_owned()),
        sequence: Set(state.last_patch_sequence + 10),
        kind: Set(ConversationPatchKind::TurnLifecycle),
        revision: Set(1),
        recorded_at_ms: Set(999),
        turn_id: Set(Some("turn-1".to_owned())),
        item_id: Set(None),
        ordinal: Set(None),
        lifecycle: Set(Some(EntityLifecycle::Interrupted)),
        item_kind: Set(None),
        run_id: Set(None),
        phase: Set(None),
        body: Set(None),
        fragment: Set(None),
        entity_created_at_ms: Set(None),
        entity_updated_at_ms: Set(None),
    }
    .insert(&database)
    .await
    .expect("insert colliding");
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let candidate = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("cands")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    let before = fetch_all(&database).await;
    let err = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &colliding,
            item_patch_id: None,
        })
        .await
        .expect_err("existing patch should fail");
    assert!(matches!(
        err,
        StartupReconciliationDispositionError::PatchConflict { .. }
    ));
    assert_eq!(before, fetch_all(&database).await);
}

// ---------------------------------------------------------------------------
// 7. terminal runs and queued/leased dispatches are never touched
// ---------------------------------------------------------------------------

#[tokio::test]
async fn terminal_runs_never_touched() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (claimed, receipt, sk, creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let bound = bind_running(&repository, &claimed, &receipt, &sk, &creds).await;
    let assistant_item = ItemId::parse("assistant-1").expect("aid");
    commit_running_item(
        &RunningBatchFixtures {
            repository: &repository,
            claimed: &claimed,
            receipt: &receipt,
            bound: &bound,
            start_key: &sk,
            creds: &creds,
        },
        &assistant_item,
        &PatchId::parse("p-turn-term").expect("p"),
        &PatchId::parse("p-item-term").expect("p"),
    )
    .await;
    // Complete the run terminally.
    let body = AssistantBody::parse("final").expect("body");
    repository
        .complete_run(artisan_database::CompleteRun {
            scope: artisan_database::RunBatchScope {
                claimed: &claimed,
                launched: &receipt,
                bound: &bound,
                run_start_key: &sk,
                credentials: &creds,
                expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
                expected_updated_at: UnixMillis::from_millis(BATCH_AT_MS),
            },
            operated_at: UnixMillis::from_millis(BATCH_AT_MS + 10),
            item_id: &assistant_item,
            expected_revision: artisan_domain::Revision::new(0),
            body: &body,
            phase: AssistantMessagePhase::Final,
            item_patch_id: &PatchId::parse("p-complete-item").expect("p"),
            turn_patch_id: &PatchId::parse("p-complete-turn").expect("p"),
        })
        .await
        .expect("complete");
    // Run is now completed (terminal). Create a fake candidate that would try to dispose it - but discovery should omit terminal.
    let query =
        StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS + 1000), 10)
            .expect("q");
    let candidates = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("candidates");
    assert_eq!(candidates.len(), 0, "terminal run must not be candidate");

    // Even if we forge a candidate with same ids but lifecycle Completed, disposition should SkippedMoved and leave rows unchanged.
    let before = fetch_all(&database).await;
    let fake_candidate = artisan_database::StartupReconciliationCandidate {
        run_id: RunId::parse("run-1").expect("run"),
        thread_id: ThreadId::parse("thread-1").expect("tid"),
        message_id: MessageId::parse("message-1").expect("mid"),
        turn_id: TurnId::parse("turn-1").expect("turn"),
        generation: 1,
        lifecycle: StartupRunLifecycle::Running, // lie: say running but actually completed
        lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
        run_updated_at: UnixMillis::from_millis(BATCH_AT_MS + 10),
        dispatch_updated_at: UnixMillis::from_millis(BATCH_AT_MS + 10),
        assistant_item_id: Some(assistant_item.clone()),
    };
    let outcome = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &fake_candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS + 1000),
            turn_patch_id: &PatchId::parse("turn-patch-term").expect("p"),
            item_patch_id: Some(&PatchId::parse("item-patch-term").expect("p")),
        })
        .await
        .expect("fake dispose");
    assert!(matches!(
        outcome,
        StartupReconciliationDispositionOutcome::SkippedMoved
    ));
    assert_eq!(before, fetch_all(&database).await);
}

#[tokio::test]
async fn queued_and_leased_dispatches_never_touched() {
    let (database, repository) = memory_database().await;
    // queued dispatch (no claim)
    seed_project_and_thread(&database, &repository, "thread-q").await;
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("req-q").expect("req"),
            message_id: MessageId::parse("message-q").expect("mid"),
            thread_id: ThreadId::parse("thread-q").expect("tid"),
            body: MessageBody::parse("queued").expect("body"),
            accepted_at: UnixMillis::from_millis(ACCEPTED_AT_MS),
        })
        .await
        .expect("queue");
    // leased dispatch (claimed but not launched)
    seed_project_and_thread(&database, &repository, "thread-l").await;
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("req-l").expect("req"),
            message_id: MessageId::parse("message-l").expect("mid"),
            thread_id: ThreadId::parse("thread-l").expect("tid"),
            body: MessageBody::parse("leased").expect("body"),
            accepted_at: UnixMillis::from_millis(ACCEPTED_AT_MS),
        })
        .await
        .expect("queue");
    let _leased = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: artisan_database::DispatchLeaseOwner::new([0x22; 32]),
            claimed_at: UnixMillis::from_millis(CLAIMED_AT_MS),
            lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
        })
        .await
        .expect("claim")
        .expect("leased");

    // Ensure no candidate for queued/leased
    let query =
        StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS + 1000), 10)
            .expect("q");
    let candidates = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("candidates");
    for c in &candidates {
        assert_ne!(c.message_id.as_str(), "message-q");
        assert_ne!(c.message_id.as_str(), "message-l");
    }

    // Forge fake candidate pointing at queued dispatch -> should be SkippedMoved
    let before = fetch_all(&database).await;
    let fake_queued = artisan_database::StartupReconciliationCandidate {
        run_id: RunId::parse("run-fake-q").expect("run"),
        thread_id: ThreadId::parse("thread-q").expect("tid"),
        message_id: MessageId::parse("message-q").expect("mid"),
        turn_id: TurnId::parse("turn-fake-q").expect("turn"),
        generation: 1,
        lifecycle: StartupRunLifecycle::Launching,
        lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
        run_updated_at: UnixMillis::from_millis(OPERATED_AT_MS),
        dispatch_updated_at: UnixMillis::from_millis(CLAIMED_AT_MS),
        assistant_item_id: None,
    };
    let outcome_q = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &fake_queued,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &PatchId::parse("turn-patch-q").expect("p"),
            item_patch_id: None,
        })
        .await
        .expect("queued fake");
    assert!(matches!(
        outcome_q,
        StartupReconciliationDispositionOutcome::SkippedMoved
    ));

    let fake_leased = artisan_database::StartupReconciliationCandidate {
        run_id: RunId::parse("run-fake-l").expect("run"),
        thread_id: ThreadId::parse("thread-l").expect("tid"),
        message_id: MessageId::parse("message-l").expect("mid"),
        turn_id: TurnId::parse("turn-fake-l").expect("turn"),
        generation: 1,
        lifecycle: StartupRunLifecycle::Launching,
        lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
        run_updated_at: UnixMillis::from_millis(OPERATED_AT_MS),
        dispatch_updated_at: UnixMillis::from_millis(CLAIMED_AT_MS),
        assistant_item_id: None,
    };
    let outcome_l = repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &fake_leased,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &PatchId::parse("turn-patch-l").expect("p"),
            item_patch_id: None,
        })
        .await
        .expect("leased fake");
    assert!(matches!(
        outcome_l,
        StartupReconciliationDispositionOutcome::SkippedMoved
    ));
    assert_eq!(before, fetch_all(&database).await);
}

// ---------------------------------------------------------------------------
// Additional exact row assertions for error bounds and unchanged unrelated rows
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_bounds_and_provider_binding_bytes_preserved() {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (claimed, receipt, sk, creds) = queue_claim_launch(
        &repository,
        &database,
        "thread-1",
        "message-1",
        "run-1",
        "turn-1",
    )
    .await;
    let bound = bind_running(&repository, &claimed, &receipt, &sk, &creds).await;
    // Capture binding bytes before disposition.
    let before_run = entities::assistant_run::Entity::find_by_id("run-1")
        .one(&database)
        .await
        .expect("q")
        .expect("run");
    let binding_bytes = before_run.provider_binding.clone().expect("binding");
    let binding_version = before_run.provider_binding_version.expect("version");
    let bound_at = before_run.provider_bound_at_ms.expect("bound_at");

    let assistant_item = ItemId::parse("assistant-1").expect("aid");
    commit_running_item(
        &RunningBatchFixtures {
            repository: &repository,
            claimed: &claimed,
            receipt: &receipt,
            bound: &bound,
            start_key: &sk,
            creds: &creds,
        },
        &assistant_item,
        &PatchId::parse("p-turn-err").expect("p"),
        &PatchId::parse("p-item-err").expect("p"),
    )
    .await;
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), 10)
        .expect("q");
    let candidate = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("cands")
        .into_vec()
        .into_iter()
        .find(|c| c.run_id.as_str() == "run-1")
        .expect("candidate");
    repository
        .dispose_expired_startup_candidate(StartupReconciliationDisposition {
            candidate: &candidate,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            turn_patch_id: &PatchId::parse("turn-patch-err").expect("p"),
            item_patch_id: Some(&PatchId::parse("item-patch-err").expect("p")),
        })
        .await
        .expect("dispose");

    let after_run = entities::assistant_run::Entity::find_by_id("run-1")
        .one(&database)
        .await
        .expect("q")
        .expect("run");
    assert_eq!(
        after_run.provider_binding.as_ref().unwrap().as_slice(),
        binding_bytes.as_slice()
    );
    assert_eq!(after_run.provider_binding_version, Some(binding_version));
    assert_eq!(after_run.provider_bound_at_ms, Some(bound_at));
    assert_eq!(
        after_run.error_code.as_deref().unwrap().len(),
        RUN_ERROR_CODE.len()
    );
    assert!(after_run.error_code.as_deref().unwrap().len() <= 128);
    assert!(after_run.error_message.as_deref().unwrap().len() <= 1024);
    assert!(!after_run.error_message.as_deref().unwrap().is_empty());
    let after_dispatch = entities::message_dispatch::Entity::find_by_id("message-1")
        .one(&database)
        .await
        .expect("q")
        .expect("dispatch");
    assert!(after_dispatch.last_error.as_deref().unwrap().len() <= 4096);

    // Create unrelated thread/message and ensure untouched.
    seed_project_and_thread(&database, &repository, "thread-2").await;
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("req-unrelated").expect("req"),
            message_id: MessageId::parse("message-unrelated").expect("mid"),
            thread_id: ThreadId::parse("thread-2").expect("tid"),
            body: MessageBody::parse("unrelated").expect("body"),
            accepted_at: UnixMillis::from_millis(ACCEPTED_AT_MS + 100),
        })
        .await
        .expect("queue unrelated");
    let unrelated_dispatch = entities::message_dispatch::Entity::find_by_id("message-unrelated")
        .one(&database)
        .await
        .expect("q")
        .expect("dispatch");
    assert_eq!(unrelated_dispatch.state, DispatchState::Queued);
}
