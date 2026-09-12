//! Claim loading, launch, admission, binding, and custody for one message.
//!
//! `execute_claim` is the single linear pipeline the dispatch loop drives:
//! read the queued payload and captured engine settings, resolve the engine
//! launch, register cancellation, launch the durable run, admit the owner
//! turn, bind the provider session, and consume the bound turn to terminal
//! settlement. Every early exit requeues or fails the claim through the same
//! bounded database commands; custody is returned to the loop only when an
//! owner child is still unresolved.

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
    CLAUDE_ENGINE_ID, CODEX_ENGINE_ID, CURSOR_ENGINE_ID, GROK_ENGINE_ID, HERMES_ENGINE_ID,
    OPENCODE2_ENGINE_ID,
};
use crate::{
    SystemCommandOrigin,
    engine_owner::cursor::CursorLaunch,
    engine_owner::grok::GrokLaunch,
    engine_owner::operation::AcceptedTurn,
    engine_owner::{
        EngineClaudeTurnInput, EngineCodexTurnInput, EngineContinuation, EngineCursorTurnInput,
        EngineGrokTurnInput, EngineHermesTurnInput, EngineTurnInput,
        hermes::{VerifiedHermesLaunch, resolve_service_executable},
    },
    lifecycle_control::ActivityLease,
    run_cancellation::RunCancellationLease,
    run_interaction::{RunInteractionAck, RunInteractionEnvelope},
};

use super::dispatch_policy::{
    LaunchAuthority, PromptAuthorization, SettingsLoadDecision, classify_launch_result,
    classify_settings_load, continuation_incompatible_reason, continuation_unavailable_reason,
    is_permanent_configuration_error, prompt_authorization_after_binding,
};
use super::dispatch_support::{
    add_duration, at_or_after, mint_item_id, mint_patch_id, mint_run_capabilities, mint_run_id,
    mint_turn_id, wall_clock,
};
use super::turn::{TurnConsumptionContext, consume_turn, is_unresolved_reap};
use super::{
    BoundClaim, ClaimCustody, ClaimExecution, ClaimIds, ClaimLaunchMode, LaunchedClaim,
    LoadedClaim, NativeRunDispatcherConfig, PROVIDER_BINDING_VERSION, PreparedClaim,
    ResolvedLaunch, RetainedActivity, binding_bytes_vec, binding_matches_bytes,
    drive_turn_with_lease_heartbeat,
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
    let loaded = load_claim(context, launch_mode).await?;
    let ids = match mint_claim_ids(
        loaded.context.origin,
        loaded.context.claimed.updated_at,
        &loaded.launch,
    ) {
        Ok(ids) => ids,
        Err(reason) => {
            loaded.context.requeue(reason).await;
            return None;
        }
    };
    let continuation = match resolve_continuation(&loaded, &ids).await {
        Ok(continuation) => continuation,
        Err(reason) => {
            loaded.context.fail(reason).await;
            return None;
        }
    };
    let Ok(cancellation) = loaded
        .context
        .cancellation
        .register(loaded.payload.thread_id.clone(), ids.run_id.clone())
    else {
        loaded.context.requeue("run cancellation unavailable").await;
        return None;
    };
    let launched = launch_claim(loaded, ids, cancellation, continuation).await?;
    let (prepared, custody) = admit_claim(launched).await;
    let Some(prepared) = prepared else {
        return match custody {
            ClaimCustody::Released => None,
            ClaimCustody::Retained(cancellation) => Some(RetainedActivity {
                _activity: activity_lease,
                _cancellation: cancellation,
            }),
        };
    };
    let (bound, custody) = bind_claim(prepared).await;
    let Some(bound) = bound else {
        return match custody {
            ClaimCustody::Released => None,
            ClaimCustody::Retained(cancellation) => Some(RetainedActivity {
                _activity: activity_lease,
                _cancellation: cancellation,
            }),
        };
    };
    match Box::pin(consume_bound_claim(bound)).await {
        ClaimCustody::Released => None,
        ClaimCustody::Retained(cancellation) => Some(RetainedActivity {
            _activity: activity_lease,
            _cancellation: cancellation,
        }),
    }
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
                let Some(launch) =
                    resolve_codex_launch(context.database_path, selection.profile_id()).await
                else {
                    context.requeue("engine profile unavailable").await;
                    return None;
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
                let Some(launch) =
                    resolve_claude_launch(context.database_path, selection.profile_id()).await
                else {
                    context.requeue("engine profile unavailable").await;
                    return None;
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
                let Some(launch) = resolve_grok_launch(selection.profile_id()).await else {
                    context.requeue("engine profile unavailable").await;
                    return None;
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
        EngineSelection::Hermes(selection) => match launch_mode {
            ClaimLaunchMode::Configured => {
                let Some(launch) = resolve_hermes_launch(selection.profile_id()).await else {
                    context.requeue("engine profile unavailable").await;
                    return None;
                };
                ResolvedLaunch::Hermes(Box::new(launch))
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

#[expect(
    clippy::too_many_lines,
    reason = "sequential validation of one continuation payload; each early return maps to a distinct refusal"
)]
async fn resolve_continuation(
    claim: &LoadedClaim<'_>,
    ids: &ClaimIds,
) -> Result<Option<EngineContinuation>, &'static str> {
    #[cfg(test)]
    if matches!(&claim.launch, ResolvedLaunch::Fixture(_)) {
        return Ok(None);
    }
    // Codex resumes its durable provider thread: the lookup is scoped to the
    // codex engine tag and the selecting profile, and the owner reopens the
    // same thread through `thread/resume` only after the X3 gate (same
    // engine, explicit target model, CLI >= 0.145.0). Incompatible bindings
    // fail closed; a fresh thread starts only with no history.
    if matches!(&claim.launch, ResolvedLaunch::Codex(_)) {
        let EngineSelection::Codex(selection) = claim.settings.config().selection() else {
            return Err("engine unavailable");
        };
        let profile_id = selection.profile_id().clone();
        let lookup = claim
            .context
            .repository
            .read_session_continuation(SessionContinuationQuery {
                thread_id: claim.payload.thread_id.clone(),
                engine_id: EngineId::Codex,
                profile_id,
                exclude_run_id: Some(ids.run_id.clone()),
            })
            .await
            .map_err(|_| "provider continuation lookup failed")?;
        return match lookup {
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
        };
    }
    // Grok resumes its durable provider conversation: the lookup is scoped
    // to the grok engine tag and the selecting profile, and the owner
    // reopens the same conversation through `session/load` only after the
    // G3 gate (same engine, explicit target model, recorded CLI version).
    // Incompatible bindings fail closed; a fresh conversation starts only
    // with no history.
    if matches!(&claim.launch, ResolvedLaunch::Grok(_)) {
        let EngineSelection::Grok(selection) = claim.settings.config().selection() else {
            return Err("engine unavailable");
        };
        let profile_id = selection.profile_id().clone();
        let lookup = claim
            .context
            .repository
            .read_session_continuation(SessionContinuationQuery {
                thread_id: claim.payload.thread_id.clone(),
                engine_id: EngineId::Grok,
                profile_id,
                exclude_run_id: Some(ids.run_id.clone()),
            })
            .await
            .map_err(|_| "provider continuation lookup failed")?;
        return match lookup {
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
        };
    }
    // Claude resumes its durable native session: the lookup is scoped to the
    // Claude engine tag and the selecting profile, and the owner reopens the
    // same session through `--resume` only after the L3 gate (same engine,
    // explicit target model, CLI >= 2.1.220). Incompatible bindings fail
    // closed; a fresh session starts only with no history.
    if matches!(&claim.launch, ResolvedLaunch::Claude(_)) {
        let EngineSelection::Claude(selection) = claim.settings.config().selection() else {
            return Err("engine unavailable");
        };
        let profile_id = selection.profile_id().clone();
        let lookup = claim
            .context
            .repository
            .read_session_continuation(SessionContinuationQuery {
                thread_id: claim.payload.thread_id.clone(),
                engine_id: EngineId::Claude,
                profile_id,
                exclude_run_id: Some(ids.run_id.clone()),
            })
            .await
            .map_err(|_| "provider continuation lookup failed")?;
        return match lookup {
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
        };
    }
    // Cursor resumes its durable ACP session: the lookup is scoped to the
    // cursor engine tag and the selecting profile, and the owner reopens the
    // same session through `session/load` only after the C3 gate (same
    // engine, explicit target model, CLI >= 2026.08.11-e8db854). Incompatible
    // bindings fail closed; a fresh session starts only with no history.
    if matches!(&claim.launch, ResolvedLaunch::Cursor(_)) {
        let EngineSelection::Cursor(selection) = claim.settings.config().selection() else {
            return Err("engine unavailable");
        };
        let profile_id = selection.profile_id().clone();
        let lookup = claim
            .context
            .repository
            .read_session_continuation(SessionContinuationQuery {
                thread_id: claim.payload.thread_id.clone(),
                engine_id: EngineId::Cursor,
                profile_id,
                exclude_run_id: Some(ids.run_id.clone()),
            })
            .await
            .map_err(|_| "provider continuation lookup failed")?;
        return match lookup {
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
        };
    }
    // Hermes resumes its durable gateway session: the lookup is scoped to the
    // Hermes engine tag and the selecting profile, and the owner enforces the
    // original model selection after `session.resume` (mirroring
    // `CheckNativeContinuation`: compatible only on identical selection).
    if matches!(&claim.launch, ResolvedLaunch::Hermes(_)) {
        let EngineSelection::Hermes(selection) = claim.settings.config().selection() else {
            return Err("engine unavailable");
        };
        let profile_id = selection.profile_id().clone();
        let lookup = claim
            .context
            .repository
            .read_session_continuation(SessionContinuationQuery {
                thread_id: claim.payload.thread_id.clone(),
                engine_id: EngineId::Hermes,
                profile_id,
                exclude_run_id: Some(ids.run_id.clone()),
            })
            .await
            .map_err(|_| "provider continuation lookup failed")?;
        return match lookup {
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
        };
    }
    let EngineSelection::OpenCode2(selection) = claim.settings.config().selection() else {
        return Err("engine unavailable");
    };
    let profile_id = selection.profile_id().clone();
    let lookup = claim
        .context
        .repository
        .read_session_continuation(SessionContinuationQuery {
            thread_id: claim.payload.thread_id.clone(),
            engine_id: EngineId::OpenCode2,
            profile_id,
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
        | ResolvedLaunch::Grok(_)
        | ResolvedLaunch::Hermes(_) => (
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
    ids: ClaimIds,
    cancellation: RunCancellationLease,
    continuation: Option<EngineContinuation>,
) -> Option<LaunchedClaim<'_>> {
    let launch_result = launch_with_retry(
        loaded.context.repository,
        LaunchClaimedRun {
            claimed: &loaded.context.claimed,
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
        ResolvedLaunch::Hermes(launch) => context.owner.admit_hermes_turn(
            EngineHermesTurnInput {
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
        return (None, ClaimCustody::Released);
    };
    let preparation = tokio::select! {
        biased;
        result = turn.prepare() => Ok(result),
        () = run_cancel.wait() => Err(()),
    };
    let (session_result, cancellation_observed) = match preparation {
        Ok(result) => (result, run_cancel.is_cancelled()),
        Err(()) => {
            // The lease signal wins without dropping the AcceptedTurn. Keep
            // setup alive long enough to obtain the session needed for the
            // durable bind, then cancel before authorization. This preserves
            // a user-cancelled terminal path instead of leaving an unbound
            // launching row for interruption recovery.
            (turn.prepare().await, true)
        }
    };
    let Ok(session) = session_result else {
        let custody = if is_unresolved_reap(&turn.finish().await) {
            ClaimCustody::Retained(cancellation)
        } else {
            ClaimCustody::Released
        };
        return (None, custody);
    };
    if cancellation_observed {
        turn.cancel();
    }
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
    // `codex`, `claude`, `grok`, `cursor`, or `hermes`) with format 1 and the native thread identity from
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
        EngineSelection::Hermes(selection) => {
            (HERMES_ENGINE_ID, selection.profile_id().as_str().to_owned())
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
    let bind_command = || BindRunProvider {
        claimed: &context.claimed,
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
                EngineSelection::Hermes(_) => EngineId::Hermes,
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
    // A provider turn can legitimately outlive the original claim window (a
    // slow tool call, a long response). The heartbeat below keeps the dispatch
    // lease alive for the whole turn so the recovery sweep cannot reap work
    // this dispatcher still owns, and so the terminal settlement still passes
    // its lease fence instead of leaving an unknown-outcome run behind.
    let custody_unresolved = {
        let turn = consume_turn(
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
        );
        // Keep the dispatch lease alive for the whole turn: a slow tool call
        // or long response must not be reaped mid-flight, and the terminal
        // settlement must still pass its lease fence afterwards.
        Box::pin(drive_turn_with_lease_heartbeat(
            context.repository,
            context.origin,
            &context.claimed,
            context.config.claim_lease,
            turn,
        ))
        .await
    };
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

/// Resolves one Codex profile into a verified launch with a bounded
/// `--version` probe enforcing the minimum CLI at probe time.
///
/// Returns `None` when the executable is unavailable, the probe times out or
/// fails, or the version predates the minimum; the caller requeues the claim.
async fn resolve_codex_launch(
    database_path: &Path,
    profile_id: &artisan_domain::EngineProfileId,
) -> Option<VerifiedCodexLaunch> {
    let authority = NativeCodexAuthority::new();
    let executable = authority.resolve_executable().ok()?;
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(&executable)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    authority
        .resolve_launch(database_path, profile_id, &stdout)
        .ok()
}

/// Resolves one Claude profile into a verified launch with a bounded
/// `--version` probe enforcing the minimum CLI at probe time.
///
/// Returns `None` when the executable is unavailable, the probe times out or
/// fails, or the version predates the minimum; the caller requeues the claim.
async fn resolve_claude_launch(
    database_path: &Path,
    profile_id: &artisan_domain::EngineProfileId,
) -> Option<VerifiedClaudeLaunch> {
    let authority = NativeClaudeAuthority::new();
    let executable = authority.resolve_executable().ok()?;
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(&executable)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    authority
        .resolve_launch(database_path, profile_id, &stdout)
        .ok()
}

/// Resolves one Grok profile into a probe-certified launch with a bounded
/// `--version` probe parsed by the shared ACP row.
///
/// There is no verified-launch authority or minimum CLI for Grok in the
/// TypeScript evidence: the existing discovery resolves the executable, the
/// path must still be a regular file, and any parsed version seats the
/// launch. Returns `None` when the executable is unavailable, the probe
/// times out or fails, or no version parses; the caller requeues the claim.
async fn resolve_grok_launch(profile_id: &artisan_domain::EngineProfileId) -> Option<GrokLaunch> {
    let resolved = artisan_native_engine::grok::resolve_live()?;
    let executable = resolved.path().to_path_buf();
    if !executable.is_file() {
        return None;
    }
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(&executable)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let version = artisan_native_engine::grok::parse_grok_version(&stdout)?;
    Some(GrokLaunch::new(executable, profile_id.clone(), version))
}
/// Resolves one Hermes profile into a verified launch with a bounded
/// `--version` probe enforcing the minimum gateway at probe time.
///
/// Executable resolution follows discovery precedence (`HERMES_EXECUTABLE`,
/// installed local-app-data, `PATH`); authentication stays owned by the
/// installed Hermes profile and is never probed here.
///
/// Returns `None` when no executable resolves, the probe times out or fails,
/// or the version predates the minimum; the caller requeues the claim.
async fn resolve_hermes_launch(
    profile_id: &artisan_domain::EngineProfileId,
) -> Option<VerifiedHermesLaunch> {
    let resolved = resolve_service_executable()?;
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(&resolved)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let version = artisan_native_engine::hermes::parse_hermes_version(&stdout).ok()?;
    artisan_native_engine::hermes::check_minimum_version(&version).ok()?;
    VerifiedHermesLaunch::new(
        resolved,
        profile_id.as_str().to_owned(),
        version.to_string(),
    )
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
