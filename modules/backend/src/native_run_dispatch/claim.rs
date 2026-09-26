//! Claim loading, launch, admission, binding, and custody for one message.
//!
//! `execute_claim` is the single linear pipeline the dispatch loop drives:
//! read the queued payload and captured engine settings, resolve the engine
//! launch, register cancellation, launch the durable run, admit the owner
//! turn, bind the provider session, and consume the bound turn to terminal
//! settlement, all under one continuous lease heartbeat. Every early exit
//! requeues or fails the claim through the same bounded database commands; a
//! provider that never starts fails its launched run with a typed reason.
//! Custody is returned to the loop only when an owner child is unresolved.

use std::path::Path;
use std::time::Duration;

use artisan_database::{
    BindRunProvider, BindRunProviderOutcome, ClaimedMessageDispatch, DispatchFailureReason,
    FailMessageDispatch, LaunchClaimedRun, LaunchClaimedRunOutcome, ProviderBindingBytes,
    Repository, RequeueMessageDispatch, RunBatchScope, SessionContinuationLookup,
    SessionContinuationQuery,
};
use artisan_domain::{EngineId, EngineSelection, UnixMillis};
#[cfg(test)]
use artisan_domain::{ItemId, PatchId, RunId, TurnId};
use artisan_native_engine::{
    NativeClaudeAuthority, NativeCodexAuthority, VerifiedClaudeLaunch, VerifiedCodexLaunch,
};
use artisan_transport::CancelHandle;

#[cfg(test)]
use crate::engine_owner::FixtureTurnInput;
use crate::engine_owner::consts::{
    CLAUDE_ENGINE_ID, CODEX_ENGINE_ID, CURSOR_ENGINE_ID, GROK_ENGINE_ID, OPENCODE2_ENGINE_ID,
};
use crate::{
    SystemCommandOrigin,
    engine_owner::cursor::CursorLaunch,
    engine_owner::grok::GrokLaunch,
    engine_owner::operation::AcceptedTurn,
    engine_owner::{
        EngineClaudeTurnInput, EngineCodexTurnInput, EngineContinuation, EngineCursorTurnInput,
        EngineGrokTurnInput, EngineTurnInput,
    },
    lifecycle_control::ActivityLease,
    run_cancellation::RunCancellationLease,
    run_interaction::{RunInteractionAck, RunInteractionEnvelope},
};

use super::claim_lease::{ClaimLease, drive_with_claim_lease};
use super::dispatch_policy::{
    LaunchAuthority, PromptAuthorization, SettingsLoadDecision, classify_launch_result,
    classify_settings_load, continuation_incompatible_reason, continuation_unavailable_reason,
    is_permanent_configuration_error, prompt_authorization_after_binding,
};
use super::dispatch_support::{
    add_duration, at_or_after, mint_item_id, mint_patch_id, mint_run_capabilities, mint_run_id,
    mint_turn_id, wall_clock,
};
use super::start_failure::{
    ProviderStart, StartFailure, await_provider_start, settle_unstarted_claim,
};
use super::turn::{TurnConsumptionContext, consume_turn, is_unresolved_reap};
use super::{
    BoundClaim, ClaimCustody, ClaimExecution, ClaimIds, ClaimLaunchMode, LaunchedClaim,
    LoadedClaim, NativeRunDispatcherConfig, PROVIDER_BINDING_VERSION, PreparedClaim,
    ResolvedLaunch, RetainedActivity, binding_bytes_vec, binding_matches_bytes,
};

pub(super) async fn execute_claim(
    context: ClaimExecution<'_>,
    launch_mode: ClaimLaunchMode,
    activity_lease: ActivityLease,
) -> Option<RetainedActivity> {
    if context.stop.is_cancelled() || context.process_cancel.is_cancelled() {
        context.requeue("dispatcher stopping").await;
        return None;
    }
    // One heartbeat keeps the lease alive from claim to settlement: provider
    // startup alone may outlast the lease, and an unrenewed lease is reaped
    // by live recovery as an unknown outcome while this dispatcher owns it.
    let (repository, origin, lease) = (context.repository, context.origin, context.lease);
    let claim = Box::pin(run_claim(context, launch_mode));
    match drive_with_claim_lease(repository, origin, lease, claim).await {
        ClaimCustody::Released => None,
        ClaimCustody::Retained(cancellation) => Some(RetainedActivity {
            _activity: activity_lease,
            _cancellation: cancellation,
        }),
    }
}

async fn run_claim(context: ClaimExecution<'_>, launch_mode: ClaimLaunchMode) -> ClaimCustody {
    let Some(loaded) = load_claim(context, launch_mode).await else {
        return ClaimCustody::Released;
    };
    let ids = match mint_claim_ids(
        loaded.context.origin,
        loaded.context.claimed.updated_at,
        &loaded.launch,
    ) {
        Ok(ids) => ids,
        Err(reason) => {
            loaded.context.requeue(reason).await;
            return ClaimCustody::Released;
        }
    };
    let continuation = match resolve_continuation(&loaded, &ids).await {
        Ok(continuation) => continuation,
        Err(reason) => {
            loaded.context.fail(reason).await;
            return ClaimCustody::Released;
        }
    };
    let Ok(cancellation) = loaded
        .context
        .cancellation
        .register_exclusive(loaded.payload.thread_id.clone(), ids.run_id.clone())
    else {
        loaded.context.requeue("run cancellation unavailable").await;
        return ClaimCustody::Released;
    };
    let Some(launched) = launch_claim(loaded, ids, cancellation, continuation).await else {
        return ClaimCustody::Released;
    };
    let (prepared, custody) = admit_claim(launched).await;
    let Some(prepared) = prepared else {
        return custody;
    };
    let (bound, custody) = bind_claim(prepared).await;
    let Some(bound) = bound else {
        return custody;
    };
    Box::pin(consume_bound_claim(bound)).await
}

#[expect(
    clippy::too_many_lines,
    reason = "linear claim load path; extraction would thread the full borrow set through helpers"
)]
async fn load_claim(
    context: ClaimExecution<'_>,
    launch_mode: ClaimLaunchMode,
) -> Option<LoadedClaim<'_>> {
    let Some(payload) = read_payload(context.repository, &context.claimed).await else {
        context.requeue("message payload unavailable").await;
        return None;
    };
    // Launch resolves from the settings snapshot captured with the
    // accepted command, never from current thread settings: a message
    // queued under one engine can never run on a later-selected engine.
    // Legacy rows without snapshots fall back to the current read with
    // that fallback marked; the launch-time snapshot check below still
    // fences drift.
    let settings = match context
        .repository
        .read_receipt_engine_settings(&payload.correlation_id)
        .await
    {
        Ok(Some(captured)) => captured,
        Ok(None) => match classify_settings_load(
            context
                .repository
                .read_thread_engine_settings(&payload.thread_id)
                .await,
        ) {
            SettingsLoadDecision::Ready(settings) => *settings,
            SettingsLoadDecision::Requeue(reason) => {
                context.requeue(reason).await;
                return None;
            }
            SettingsLoadDecision::Fail(reason) => {
                context.fail(reason).await;
                return None;
            }
        },
        Err(_) => {
            context.requeue("engine settings snapshot unreadable").await;
            return None;
        }
    };
    let project_root = match context
        .repository
        .read_thread_project_root(&payload.thread_id)
        .await
    {
        Ok(root) => root,
        Err(error) => {
            if is_permanent_configuration_error(&error) {
                context.fail("project root corrupt").await;
            } else {
                context.requeue("project root unavailable").await;
            }
            return None;
        }
    };
    // The settings fence fails closed per engine: OpenCode2 resolves through
    // the certified profile authority, Codex resolves through the Codex
    // launch authority with a bounded `--version` probe enforcing the minimum
    // CLI, Claude resolves through the Claude launch authority with a bounded
    // `--version` probe enforcing the minimum CLI, Grok resolves through the
    // existing Grok discovery with a bounded `--version` probe parsed by the
    // shared ACP row (no minimum CLI in the TypeScript evidence), Cursor has
    // no launch authority in C1 and requeues, and every other newly
    // representable engine requeues instead of running as another engine.
    let launch = match settings.config().selection() {
        EngineSelection::OpenCode2(selection) => match launch_mode {
            ClaimLaunchMode::Configured => {
                let profile_id = selection.profile_id();
                let Ok(launch) = context
                    .config
                    .authority
                    .resolve_profile_launch(context.database_path, profile_id)
                else {
                    context.requeue("engine profile unavailable").await;
                    return None;
                };
                ResolvedLaunch::Configured(Box::new(launch))
            }
            #[cfg(test)]
            ClaimLaunchMode::Fixture(fixture) => {
                let configured_profile = selection.profile_id();
                if configured_profile.as_str() != fixture.profile_id.as_str() {
                    context.requeue("engine profile unavailable").await;
                    return None;
                }
                ResolvedLaunch::Fixture(fixture)
            }
        },
        EngineSelection::Codex(selection) => match launch_mode {
            ClaimLaunchMode::Configured => {
                let resolved =
                    resolve_codex_launch(context.database_path, selection.profile_id()).await;
                let launch = match resolved {
                    Ok(launch) => launch,
                    Err(failure) => {
                        context.requeue(failure.reason()).await;
                        return None;
                    }
                };
                ResolvedLaunch::Codex(Box::new(launch))
            }
            #[cfg(test)]
            ClaimLaunchMode::Fixture(_) => {
                context.requeue("engine unavailable").await;
                return None;
            }
        },
        EngineSelection::Claude(selection) => match launch_mode {
            ClaimLaunchMode::Configured => {
                let resolved =
                    resolve_claude_launch(context.database_path, selection.profile_id()).await;
                let launch = match resolved {
                    Ok(launch) => launch,
                    Err(failure) => {
                        context.requeue(failure.reason()).await;
                        return None;
                    }
                };
                ResolvedLaunch::Claude(Box::new(launch))
            }
            #[cfg(test)]
            ClaimLaunchMode::Fixture(_) => {
                context.requeue("engine unavailable").await;
                return None;
            }
        },
        EngineSelection::Grok(selection) => match launch_mode {
            ClaimLaunchMode::Configured => {
                let resolved =
                    resolve_grok_launch(context.database_path, selection.profile_id()).await;
                let launch = match resolved {
                    Ok(launch) => launch,
                    Err(failure) => {
                        context.requeue(failure.reason()).await;
                        return None;
                    }
                };
                ResolvedLaunch::Grok(Box::new(launch))
            }
            #[cfg(test)]
            ClaimLaunchMode::Fixture(_) => {
                context.requeue("engine unavailable").await;
                return None;
            }
        },
        EngineSelection::Cursor(selection) => match launch_mode {
            ClaimLaunchMode::Configured => {
                // C1 owns the definition row but no launch authority yet: the
                // probe/authority packet resolves this. Requeue without
                // running as another engine.
                let Some(launch) =
                    resolve_cursor_launch(context.database_path, selection.profile_id())
                else {
                    context.requeue("engine profile unavailable").await;
                    return None;
                };
                ResolvedLaunch::Cursor(Box::new(launch))
            }
            #[cfg(test)]
            ClaimLaunchMode::Fixture(_) => {
                context.requeue("engine unavailable").await;
                return None;
            }
        },
    };
    Some(LoadedClaim {
        context,
        payload,
        settings,
        project_root,
        launch,
    })
}

/// Resolves the provider session the new run continues, if any.
///
/// Every engine resumes only its own durable provider session: the lookup is
/// scoped to the selected engine tag and profile, and the owner reopens the
/// session only after that engine's continuation gate (same engine, explicit
/// target model, minimum CLI). Incompatible or ambiguous history fails
/// closed; a fresh session starts only when the thread has no history, which
/// includes runs that never reached their provider.
async fn resolve_continuation(
    claim: &LoadedClaim<'_>,
    ids: &ClaimIds,
) -> Result<Option<EngineContinuation>, &'static str> {
    let selection = claim.settings.config().selection();
    let launch_engine = match &claim.launch {
        ResolvedLaunch::Configured(_) => EngineId::OpenCode2,
        ResolvedLaunch::Codex(_) => EngineId::Codex,
        ResolvedLaunch::Claude(_) => EngineId::Claude,
        ResolvedLaunch::Cursor(_) => EngineId::Cursor,
        ResolvedLaunch::Grok(_) => EngineId::Grok,
        #[cfg(test)]
        ResolvedLaunch::Fixture(_) => return Ok(None),
    };
    if selection.engine_id() != launch_engine {
        return Err("engine unavailable");
    }
    let lookup = claim
        .context
        .repository
        .read_session_continuation(SessionContinuationQuery {
            thread_id: claim.payload.thread_id.clone(),
            engine_id: launch_engine,
            profile_id: selection.profile_id().clone(),
            exclude_run_id: Some(ids.run_id.clone()),
        })
        .await
        .map_err(|_| "provider continuation lookup failed")?;
    match lookup {
        SessionContinuationLookup::NoHistory => Ok(None),
        SessionContinuationLookup::Usable(continuation) => {
            EngineContinuation::new(continuation.session_id.as_str().to_owned())
                .map(Some)
                .ok_or("provider continuation corrupt")
        }
        SessionContinuationLookup::Unavailable(unavailable) => {
            Err(continuation_unavailable_reason(&unavailable))
        }
        SessionContinuationLookup::Incompatible(incompatible) => {
            Err(continuation_incompatible_reason(&incompatible))
        }
    }
}

fn mint_claim_ids(
    origin: &SystemCommandOrigin,
    updated_at: UnixMillis,
    launch: &ResolvedLaunch,
) -> Result<ClaimIds, &'static str> {
    let (run_id, turn_id, item_id, first_patch_id, second_patch_id) = match launch {
        ResolvedLaunch::Configured(_)
        | ResolvedLaunch::Codex(_)
        | ResolvedLaunch::Claude(_)
        | ResolvedLaunch::Cursor(_)
        | ResolvedLaunch::Grok(_) => (
            mint_run_id(origin).ok_or("run identity unavailable")?,
            mint_turn_id(origin).ok_or("run identity unavailable")?,
            mint_item_id(origin).ok_or("run identity unavailable")?,
            mint_patch_id(origin).ok_or("run identity unavailable")?,
            mint_patch_id(origin).ok_or("run identity unavailable")?,
        ),
        #[cfg(test)]
        ResolvedLaunch::Fixture(_) => fixture_claim_ids()?,
    };
    let operated_at = at_or_after(origin, updated_at).ok_or("run clock unavailable")?;
    let (run_start_key, credentials) =
        mint_run_capabilities().ok_or("run capability unavailable")?;
    Ok(ClaimIds {
        run_id,
        turn_id,
        item_id,
        first_patch_id,
        second_patch_id,
        operated_at,
        run_start_key,
        credentials,
    })
}

#[cfg(test)]
fn fixture_claim_ids() -> Result<(RunId, TurnId, ItemId, PatchId, PatchId), &'static str> {
    Ok((
        RunId::parse("fixture-run").map_err(|_| "run identity unavailable")?,
        TurnId::parse("fixture-turn").map_err(|_| "run identity unavailable")?,
        ItemId::parse("fixture-user-item").map_err(|_| "run identity unavailable")?,
        PatchId::parse("fixture-launch-turn").map_err(|_| "run identity unavailable")?,
        PatchId::parse("fixture-launch-item").map_err(|_| "run identity unavailable")?,
    ))
}

pub(super) async fn launch_claim(
    loaded: LoadedClaim<'_>,
    mut ids: ClaimIds,
    cancellation: RunCancellationLease,
    continuation: Option<EngineContinuation>,
) -> Option<LaunchedClaim<'_>> {
    // Launch fences the exact lease window: hold it so the heartbeat cannot
    // move it mid-command, and stamp no earlier than a renewal while leased.
    let lease: &ClaimLease = loaded.context.lease;
    let mut window = lease.hold().await;
    ids.operated_at = ids.operated_at.max(window.updated_at);
    let launch_result = launch_with_retry(
        loaded.context.repository,
        LaunchClaimedRun {
            claimed: &window,
            run_id: &ids.run_id,
            turn_id: &ids.turn_id,
            item_id: &ids.item_id,
            first_patch_id: &ids.first_patch_id,
            second_patch_id: &ids.second_patch_id,
            operated_at: ids.operated_at,
            run_start_key: &ids.run_start_key,
            credentials: &ids.credentials,
            engine_settings: &loaded.settings,
        },
        loaded.context.config.max_command_retries,
    )
    .await;
    if matches!(launch_result, Ok(LaunchClaimedRunOutcome::Started(_))) {
        let lease_expires_at = window.lease_expires_at;
        ClaimLease::record(&mut window, lease_expires_at, ids.operated_at);
    }
    drop(window);
    let receipt = match classify_launch_result(&launch_result) {
        LaunchAuthority::Started => {
            // The Started receipt durably projected the user turn/item:
            // wake subscribers now so admission streams before provider
            // startup, which can lag by seconds behind the launch.
            let _ = loaded
                .context
                .config
                .conversation_commit_notifier()
                .publish(&loaded.payload.thread_id);
            match launch_result {
                Ok(LaunchClaimedRunOutcome::Started(receipt)) => receipt,
                _ => unreachable!("started launch authority has a started receipt"),
            }
        }
        // `AlreadyStarted` is durable replay information, never authority to
        // contact OpenCode. Leave the launching run for the recovery path;
        // creating another provider session here could duplicate an unknown
        // external effect from the original attempt.
        LaunchAuthority::Replay => return None,
        LaunchAuthority::Requeue => {
            loaded.context.requeue("run launch unavailable").await;
            return None;
        }
    };
    let LoadedClaim {
        context,
        payload,
        settings,
        project_root,
        launch,
    } = loaded;
    Some(LaunchedClaim {
        context,
        payload,
        settings,
        project_root,
        launch,
        continuation,
        ids,
        receipt,
        cancellation,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "admission is a linear sequence over one claim; helpers would need the full borrow set"
)]
async fn admit_claim(claim: LaunchedClaim<'_>) -> (Option<PreparedClaim<'_>>, ClaimCustody) {
    let LaunchedClaim {
        context,
        payload,
        settings,
        project_root,
        launch,
        continuation,
        ids,
        receipt,
        cancellation,
    } = claim;
    let attempt_budget = Duration::from_millis(settings.config().runtime().attempt_budget().get());
    let prompt_id = payload.message_id.as_str().to_owned();
    let prompt = payload.payload;
    let prompt_delivery = context.config.prompt_delivery.clone();
    let stream_after = context.config.stream_after;
    let control_capacity = context.config.queue_capacity.get();
    let run_cancel = cancellation.cancel_handle();
    let engine = settings.config().selection().engine_id();
    let turn_result = match launch {
        ResolvedLaunch::Configured(launch) => context.owner.admit_turn(
            EngineTurnInput {
                run_id: receipt.run_id.clone(),
                thread_id: payload.thread_id.clone(),
                project_root,
                prompt_id,
                prompt,
                settings: settings.clone(),
                launch: *launch,
                continuation,
                prompt_delivery,
                stream_after,
                control_capacity,
            },
            attempt_budget,
        ),
        ResolvedLaunch::Codex(launch) => context.owner.admit_codex_turn(
            EngineCodexTurnInput {
                run_id: receipt.run_id.clone(),
                thread_id: payload.thread_id.clone(),
                project_root,
                prompt_id,
                prompt,
                settings: settings.clone(),
                launch: *launch,
                continuation,
                prompt_delivery,
                stream_after,
                control_capacity,
            },
            attempt_budget,
        ),
        ResolvedLaunch::Claude(launch) => context.owner.admit_claude_turn(
            EngineClaudeTurnInput {
                run_id: receipt.run_id.clone(),
                thread_id: payload.thread_id.clone(),
                project_root,
                prompt_id,
                prompt,
                settings: settings.clone(),
                launch: *launch,
                continuation,
                prompt_delivery,
                stream_after,
                control_capacity,
            },
            attempt_budget,
        ),
        ResolvedLaunch::Grok(launch) => context.owner.admit_grok_turn(
            EngineGrokTurnInput {
                run_id: receipt.run_id.clone(),
                thread_id: payload.thread_id.clone(),
                project_root,
                prompt_id,
                prompt,
                settings: settings.clone(),
                launch: *launch,
                continuation,
                prompt_delivery,
                stream_after,
                control_capacity,
            },
            attempt_budget,
        ),
        ResolvedLaunch::Cursor(launch) => context.owner.admit_cursor_turn(
            EngineCursorTurnInput {
                run_id: receipt.run_id.clone(),
                thread_id: payload.thread_id.clone(),
                project_root,
                prompt_id,
                prompt,
                settings: settings.clone(),
                launch: *launch,
                continuation,
                prompt_delivery,
                stream_after,
                control_capacity,
            },
            attempt_budget,
        ),
        #[cfg(test)]
        ResolvedLaunch::Fixture(fixture) => context.owner.admit_fixture_turn(
            FixtureTurnInput {
                run_id: receipt.run_id.clone(),
                project_root,
                prompt_id,
                prompt,
                settings: settings.clone(),
                fixture,
                prompt_delivery,
                stream_after,
                control_capacity,
            },
            attempt_budget,
        ),
    };
    let Ok(mut turn) = turn_result else {
        let failure = StartFailure::NotAdmitted;
        settle_unstarted_claim(&context, &ids, &receipt, engine, &failure).await;
        return (None, ClaimCustody::Released);
    };
    let launch_deadline = context.config.launch_deadline;
    let session = match await_provider_start(&mut turn, &run_cancel, launch_deadline).await {
        ProviderStart::Started { session, stopped } => {
            if stopped {
                turn.cancel();
            }
            session
        }
        ProviderStart::Failed(failure) => {
            let unresolved = abandon_turn(turn, context.stop, context.process_cancel).await;
            settle_unstarted_claim(&context, &ids, &receipt, engine, &failure).await;
            let custody = if unresolved {
                ClaimCustody::Retained(cancellation)
            } else {
                ClaimCustody::Released
            };
            return (None, custody);
        }
    };
    (
        Some(PreparedClaim {
            context,
            ids,
            receipt,
            settings,
            turn,
            session,
            cancellation,
        }),
        ClaimCustody::Released,
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "binding is a linear sequence over one claim; helpers would need the full borrow set"
)]
async fn bind_claim(claim: PreparedClaim<'_>) -> (Option<BoundClaim<'_>>, ClaimCustody) {
    let PreparedClaim {
        context,
        ids,
        receipt,
        settings,
        mut turn,
        session,
        cancellation,
    } = claim;
    let run_cancel = cancellation.cancel_handle();
    if run_cancel.is_cancelled() {
        turn.cancel();
    }
    // Provider binding bytes carry the exact engine tag (`opencode2`,
    // `codex`, `claude`, `grok`, or `cursor`) with format 1 and the native
    // thread identity from
    // the app-server contract. A selection for any other engine abandons the
    // turn here instead of binding as a runnable engine.
    let (binding_engine, binding_profile) = match settings.config().selection() {
        EngineSelection::OpenCode2(selection) => (
            OPENCODE2_ENGINE_ID,
            selection.profile_id().as_str().to_owned(),
        ),
        EngineSelection::Codex(selection) => {
            (CODEX_ENGINE_ID, selection.profile_id().as_str().to_owned())
        }
        EngineSelection::Claude(selection) => {
            (CLAUDE_ENGINE_ID, selection.profile_id().as_str().to_owned())
        }
        EngineSelection::Cursor(selection) => {
            (CURSOR_ENGINE_ID, selection.profile_id().as_str().to_owned())
        }
        EngineSelection::Grok(selection) => {
            (GROK_ENGINE_ID, selection.profile_id().as_str().to_owned())
        }
    };
    let Some(raw_binding) = binding_bytes_vec(binding_engine, &binding_profile, session.session())
    else {
        let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
        return (
            None,
            if custody {
                ClaimCustody::Retained(cancellation)
            } else {
                ClaimCustody::Released
            },
        );
    };
    // Round-trip the bytes before binding: a tag/format/profile mismatch
    // requeues through abandonment instead of persisting a corrupt bind.
    if !binding_matches_bytes(
        &raw_binding,
        binding_engine,
        &binding_profile,
        session.session(),
    ) {
        let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
        return (
            None,
            if custody {
                ClaimCustody::Retained(cancellation)
            } else {
                ClaimCustody::Released
            },
        );
    }
    let Some(binding_bytes) = ProviderBindingBytes::new(raw_binding).ok() else {
        let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
        return (
            None,
            if custody {
                ClaimCustody::Retained(cancellation)
            } else {
                ClaimCustody::Released
            },
        );
    };
    let Some(bound_at) = at_or_after(context.origin, ids.operated_at) else {
        let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
        return (
            None,
            if custody {
                ClaimCustody::Retained(cancellation)
            } else {
                ClaimCustody::Released
            },
        );
    };
    // Bind fences the exact lease window the heartbeat renews.
    let lease: &ClaimLease = context.lease;
    let window = lease.hold().await;
    let bind_command = || BindRunProvider {
        claimed: &window,
        receipt: &receipt,
        run_start_key: &ids.run_start_key,
        credentials: &ids.credentials,
        expected_launch_at: ids.operated_at,
        bound_at,
        binding_version: PROVIDER_BINDING_VERSION,
        binding_bytes: &binding_bytes,
    };
    let bind_result = tokio::select! {
        biased;
        result = bind_with_retry(
            context.repository,
            bind_command(),
            context.config.max_command_retries,
        ) => result,
        () = run_cancel.wait() => {
            // Do not drop the AcceptedTurn while the durable bind is pending.
            // Cancel the provider operation, then finish the same idempotent
            // bind path so terminal settlement has an authenticated scope.
            turn.cancel();
            bind_with_retry(
                context.repository,
                bind_command(),
                context.config.max_command_retries,
            )
            .await
        }
    };
    drop(window);
    let (bound, already_bound) = match bind_result {
        Ok(BindRunProviderOutcome::Bound(receipt)) => (receipt, false),
        Ok(BindRunProviderOutcome::AlreadyBound(receipt)) => (receipt, true),
        Err(_) => {
            let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
            return (
                None,
                if custody {
                    ClaimCustody::Retained(cancellation)
                } else {
                    ClaimCustody::Released
                },
            );
        }
    };
    if run_cancel.is_cancelled() {
        turn.cancel();
    }
    let authorization_failed = match prompt_authorization_after_binding(already_bound) {
        PromptAuthorization::DoNotAuthorize => true,
        PromptAuthorization::Authorize => turn.authorize().is_err(),
    };
    if authorization_failed && !run_cancel.is_cancelled() {
        let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
        return (
            None,
            if custody {
                ClaimCustody::Retained(cancellation)
            } else {
                ClaimCustody::Released
            },
        );
    }
    if run_cancel.is_cancelled() {
        turn.cancel();
    }
    (
        Some(BoundClaim {
            context,
            ids,
            receipt,
            bound,
            bound_at,
            engine: match settings.config().selection() {
                EngineSelection::OpenCode2(_) => EngineId::OpenCode2,
                EngineSelection::Codex(_) => EngineId::Codex,
                EngineSelection::Claude(_) => EngineId::Claude,
                EngineSelection::Cursor(_) => EngineId::Cursor,
                EngineSelection::Grok(_) => EngineId::Grok,
            },
            turn,
            cancellation,
        }),
        ClaimCustody::Released,
    )
}

async fn consume_bound_claim(bound: BoundClaim<'_>) -> ClaimCustody {
    let BoundClaim {
        context,
        ids,
        receipt,
        bound,
        bound_at,
        engine,
        turn,
        cancellation,
    } = bound;
    let scope = RunBatchScope {
        claimed: &context.claimed,
        launched: &receipt,
        bound: &bound,
        run_start_key: &ids.run_start_key,
        credentials: &ids.credentials,
        expected_launch_at: ids.operated_at,
        expected_updated_at: bound_at,
    };
    let run_cancel = cancellation.cancel_handle();
    // Register mid-turn interaction routing for the live run. A registration
    // failure never kills the run: responses then answer `wrong_run` and the
    // client retries once registry pressure clears.
    let (interaction_lease, inbox) = match context
        .interactions
        .register(receipt.thread_id.clone(), receipt.run_id.clone())
    {
        Ok((lease, receiver)) => (Some(lease), Some(receiver)),
        Err(_) => (None, None),
    };
    let mut inbox = inbox;
    // The claim heartbeat keeps the lease alive for the whole turn, so a
    // slow tool call is not reaped and settlement passes its lease fence.
    let custody_unresolved = Box::pin(consume_turn(
        TurnConsumptionContext {
            repository: context.repository,
            config: context.config,
            origin: context.origin,
            stop: context.stop,
            process_cancel: context.process_cancel,
            run_cancel: run_cancel.as_ref(),
        },
        turn,
        scope,
        engine,
        inbox.as_mut(),
    ))
    .await;
    if let Some(receiver) = inbox.as_mut() {
        drain_interactions(receiver);
    }
    drop(interaction_lease);
    // Pending rows are per-run: the settle wipes them so decisions never leak
    // across runs. Receipts stay: replays must still answer `duplicate`.
    // Best-effort beside terminal settlement; the delete is idempotent.
    let _ = context
        .repository
        .settle_run_interactions(&receipt.run_id)
        .await;
    if custody_unresolved {
        ClaimCustody::Retained(cancellation)
    } else {
        ClaimCustody::Released
    }
}

/// Replies `wrong_run` to every response still queued for a settled run.
///
/// The run is gone, so nothing is stored: the client retries against the
/// owning run once it is live.
pub(super) fn drain_interactions(
    receiver: &mut tokio::sync::mpsc::Receiver<RunInteractionEnvelope>,
) {
    while let Ok(envelope) = receiver.try_recv() {
        let _ = envelope.respond.send(RunInteractionAck::WrongRun);
    }
}

async fn read_payload(
    repository: &Repository,
    claimed: &ClaimedMessageDispatch,
) -> Option<artisan_database::QueueMessageDispatchPayload> {
    repository
        .read_queue_message_dispatch_payload(&claimed.message_id)
        .await
        .ok()
        .flatten()
}

pub(super) async fn requeue_claim(
    repository: &Repository,
    claimed: ClaimedMessageDispatch,
    config: &NativeRunDispatcherConfig,
    origin: &SystemCommandOrigin,
    reason: &'static str,
) {
    let Some(operated_at) = wall_clock(origin) else {
        return;
    };
    let Some(available_at) = add_duration(operated_at, config.retry_backoff) else {
        return;
    };
    let Ok(reason) = DispatchFailureReason::parse(reason) else {
        return;
    };
    let _ = repository
        .requeue_message_dispatch(RequeueMessageDispatch {
            message_id: claimed.message_id,
            owner: claimed.owner,
            operated_at,
            available_at,
            reason,
        })
        .await;
}

pub(super) async fn fail_claim(
    repository: &Repository,
    claimed: ClaimedMessageDispatch,
    origin: &SystemCommandOrigin,
    reason: &'static str,
) {
    let Some(operated_at) = wall_clock(origin) else {
        return;
    };
    let Ok(reason) = DispatchFailureReason::parse(reason) else {
        return;
    };
    let _ = repository
        .fail_message_dispatch(FailMessageDispatch {
            message_id: claimed.message_id,
            owner: claimed.owner,
            operated_at,
            reason,
        })
        .await;
}

async fn launch_with_retry(
    repository: &Repository,
    command: LaunchClaimedRun<'_>,
    retries: std::num::NonZeroUsize,
) -> Result<LaunchClaimedRunOutcome, artisan_database::RunLaunchError> {
    let LaunchClaimedRun {
        claimed,
        run_id,
        turn_id,
        item_id,
        first_patch_id,
        second_patch_id,
        operated_at,
        run_start_key,
        credentials,
        engine_settings,
    } = command;
    let mut last_error = None;
    for _ in 0..retries.get() {
        match repository
            .launch_claimed_run(LaunchClaimedRun {
                claimed,
                run_id,
                turn_id,
                item_id,
                first_patch_id,
                second_patch_id,
                operated_at,
                run_start_key,
                credentials,
                engine_settings,
            })
            .await
        {
            Ok(outcome) => return Ok(outcome),
            Err(error) => last_error = Some(error),
        }
    }
    // `retries` is a `NonZeroUsize`, so the loop body ran at least once and
    // recorded the error above.
    Err(last_error.expect("positive retry count always records a result"))
}

async fn bind_with_retry(
    repository: &Repository,
    command: BindRunProvider<'_>,
    retries: std::num::NonZeroUsize,
) -> Result<BindRunProviderOutcome, artisan_database::RunBindingError> {
    let BindRunProvider {
        claimed,
        receipt,
        run_start_key,
        credentials,
        expected_launch_at,
        bound_at,
        binding_version,
        binding_bytes,
    } = command;
    let mut last_error = None;
    for _ in 0..retries.get() {
        match repository
            .bind_run_provider(BindRunProvider {
                claimed,
                receipt,
                run_start_key,
                credentials,
                expected_launch_at,
                bound_at,
                binding_version,
                binding_bytes,
            })
            .await
        {
            Ok(outcome) => return Ok(outcome),
            Err(error) => last_error = Some(error),
        }
    }
    // `retries` is a `NonZeroUsize`, so the loop body ran at least once and
    // recorded the error above.
    Err(last_error.expect("positive retry count always records a result"))
}

/// Why a managed engine's launch could not be resolved before a run.
///
/// Each cause maps to one fixed requeue reason: no path, OS message, or
/// probe output ever reaches the dispatch row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LaunchProbeFailure {
    /// The Forge-managed install could not be resolved.
    NotInstalled,
    /// The managed launch environment could not be built.
    EnvironmentUnavailable,
    /// The executable does not exist.
    ExecutableMissing,
    /// The operating system refused to execute it.
    ExecutableNotPermitted,
    /// `--version` did not answer within its bound.
    ProbeTimedOut,
    /// `--version` failed or answered unreadably.
    ProbeFailed,
    /// The answered version is unparseable or older than the minimum.
    VersionUnsupported,
    /// The launch authority rejected the installation.
    LaunchRejected,
}

impl LaunchProbeFailure {
    const fn reason(self) -> &'static str {
        match self {
            Self::NotInstalled => "engine is not installed",
            Self::EnvironmentUnavailable => "engine environment unavailable",
            Self::ExecutableMissing => "engine executable not found",
            Self::ExecutableNotPermitted => "engine executable is not permitted to run",
            Self::ProbeTimedOut => "engine version probe timed out",
            Self::ProbeFailed => "engine version probe failed",
            Self::VersionUnsupported => "engine version is not supported",
            Self::LaunchRejected => "engine installation failed verification",
        }
    }

    fn from_spawn(error: &std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => Self::ExecutableMissing,
            std::io::ErrorKind::PermissionDenied => Self::ExecutableNotPermitted,
            _ => Self::ProbeFailed,
        }
    }

    /// Classifies a Claude/Codex authority refusal (identical shapes).
    const fn from_authority(unsupported_version: bool, executable_unavailable: bool) -> Self {
        if unsupported_version {
            Self::VersionUnsupported
        } else if executable_unavailable {
            Self::ExecutableMissing
        } else {
            Self::LaunchRejected
        }
    }
}

/// Runs one bounded `--version` probe of the Forge-managed `engine` with its
/// managed environment. Returns the probed stdout, or the typed reason the
/// engine is not installed, cannot run, or did not answer.
async fn probe_managed_version(
    engine: artisan_native_engine::ManagedEngine,
    database_path: &Path,
) -> Result<(std::path::PathBuf, String), LaunchProbeFailure> {
    let target = artisan_native_engine::resolve_launch_target_in(engine, database_path, &|name| {
        std::env::var_os(name)
    })
    .map_err(|_| LaunchProbeFailure::NotInstalled)?;
    let environment = target
        .environment()
        .map_err(|_| LaunchProbeFailure::EnvironmentUnavailable)?;
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(target.executable())
            .arg("--version")
            .env_clear()
            .envs(environment)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| LaunchProbeFailure::ProbeTimedOut)?
    .map_err(|error| LaunchProbeFailure::from_spawn(&error))?;
    if !output.status.success() {
        return Err(LaunchProbeFailure::ProbeFailed);
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| LaunchProbeFailure::ProbeFailed)?;
    Ok((target.executable().to_path_buf(), stdout))
}

/// Resolves one Codex profile into a verified launch with a bounded
/// `--version` probe enforcing the minimum CLI at probe time.
///
/// Fails with the typed reason when the executable is unavailable, the probe
/// times out or fails, or the version predates the minimum; the caller
/// requeues the claim with that reason.
async fn resolve_codex_launch(
    database_path: &Path,
    profile_id: &artisan_domain::EngineProfileId,
) -> Result<VerifiedCodexLaunch, LaunchProbeFailure> {
    use artisan_native_engine::NativeCodexLaunchError as Refusal;
    let (_, stdout) =
        probe_managed_version(artisan_native_engine::ManagedEngine::Codex, database_path).await?;
    NativeCodexAuthority::new()
        .resolve_launch(database_path, profile_id, &stdout)
        .map_err(|error| {
            LaunchProbeFailure::from_authority(
                matches!(error, Refusal::VersionTooOld | Refusal::VersionUnparseable),
                matches!(error, Refusal::ExecutableUnavailable),
            )
        })
}

/// Resolves one Claude profile into a verified launch with a bounded
/// `--version` probe enforcing the minimum CLI at probe time.
///
/// Fails with the typed reason when the executable is unavailable, the probe
/// times out or fails, or the version predates the minimum; the caller
/// requeues the claim with that reason.
async fn resolve_claude_launch(
    database_path: &Path,
    profile_id: &artisan_domain::EngineProfileId,
) -> Result<VerifiedClaudeLaunch, LaunchProbeFailure> {
    use artisan_native_engine::NativeClaudeLaunchError as Refusal;
    let (_, stdout) =
        probe_managed_version(artisan_native_engine::ManagedEngine::Claude, database_path).await?;
    NativeClaudeAuthority::new()
        .resolve_launch(database_path, profile_id, &stdout)
        .map_err(|error| {
            LaunchProbeFailure::from_authority(
                matches!(error, Refusal::VersionTooOld | Refusal::VersionUnparseable),
                matches!(error, Refusal::ExecutableUnavailable),
            )
        })
}

/// Resolves one Grok profile into a probe-certified launch with a bounded
/// `--version` probe parsed by the shared ACP row: the Forge-managed Grok (or
/// its developer override) must answer with a parseable version. Fails with
/// the typed reason otherwise; the caller requeues the claim with it.
async fn resolve_grok_launch(
    database_path: &Path,
    profile_id: &artisan_domain::EngineProfileId,
) -> Result<GrokLaunch, LaunchProbeFailure> {
    let (executable, stdout) =
        probe_managed_version(artisan_native_engine::ManagedEngine::Grok, database_path).await?;
    let version = artisan_native_engine::grok::parse_grok_version(&stdout)
        .ok_or(LaunchProbeFailure::VersionUnsupported)?;
    Ok(GrokLaunch::new(executable, profile_id.clone(), version))
}
/// Resolves one Cursor profile into a C1 launch.
///
/// C1 owns the definition row but no launch authority yet: the probe and
/// verified-launch packet resolve this later. Always returns `None` so the
/// caller requeues the claim instead of running as another engine.
fn resolve_cursor_launch(
    _database_path: &Path,
    _profile_id: &artisan_domain::EngineProfileId,
) -> Option<CursorLaunch> {
    None
}

async fn abandon_turn(
    mut turn: AcceptedTurn,
    stop: &CancelHandle,
    process_cancel: &CancelHandle,
) -> bool {
    turn.cancel();
    let _ = drain_turn(&mut turn, stop, process_cancel).await;
    is_unresolved_reap(&turn.finish().await)
}

async fn drain_turn(
    turn: &mut AcceptedTurn,
    stop: &CancelHandle,
    process_cancel: &CancelHandle,
) -> bool {
    if stop.is_cancelled() || process_cancel.is_cancelled() {
        turn.cancel();
    }
    while turn.next_observation().await.is_some() {
        if stop.is_cancelled() || process_cancel.is_cancelled() {
            turn.cancel();
        }
    }
    true
}
