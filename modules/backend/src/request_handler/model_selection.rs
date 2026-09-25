//! Model selections and send admission, decided by the Forge.
//!
//! The Editor names the model it shows (a [`CatalogSelection`]); the Forge
//! resolves it against the catalog it serves and the thread's saved
//! configuration (see [`crate::engine_selection`]). A draft submission is
//! admitted here before it is queued: a still-starting run refuses it, a
//! selection that changes the thread's configuration must be runnable and is
//! saved first, and a send whose engine matches the live run steers into it.
//! Every refusal is typed data with a message the Editor shows as it is.
//!
//! The decisions are pure functions over the catalog, the saved
//! configuration, the live run, and the account readiness; the handler
//! methods only gather those facts and apply the plan.

use artisan_catalog::{NativeModelCatalog, NativeModelPolicy};
use artisan_database::SetThreadEngineConfigInput;
use artisan_domain::{
    CatalogSelection, EngineConfigRevision, EngineConfigUpdatePrecondition, EngineId,
    EngineProfileId, EngineReadiness, EngineRunConfig, ModelSelectionResolution, RequestId,
    ResolveModelSelection, RunId, SubmissionRefusal, SubmissionRefusalKind, SubmitComposerDraft,
    ThreadId,
};
use artisan_protocol::{ProtocolFailure, ResponsePayload, RunLiveStatus, ServerResponse};

use super::failures::{outcome, repository_failure};
use super::{RequestHandler, origin_clock_failure};

const CATALOG_UNAVAILABLE: &str =
    "The host model catalog is unavailable right now. Your draft is preserved; try again.";
const NO_SELECTION: &str = "Select a model before sending. Your draft is preserved.";
const RUN_STARTING: &str =
    "The current run is still starting. Wait before sending another message.";
const SETTINGS_RACED: &str =
    "The thread's model settings changed while sending. Your draft is preserved; send again.";
const CATALOG_REJECTED: &str = "This model is unavailable in the host catalog right now. Your draft is preserved; pick another model or try again.";

/// A selection resolved against the Forge's catalog.
pub(super) struct ResolvedSelection {
    catalog: NativeModelCatalog,
    policy: NativeModelPolicy,
    config: EngineRunConfig,
}

impl ResolvedSelection {
    /// The configuration the selection resolved to.
    pub(super) fn into_config(self) -> EngineRunConfig {
        self.config
    }
}

/// How a draft submission is admitted.
pub(super) enum SubmissionAdmission {
    /// Queue it on `engine`, steering into `steer_run_id` when named. A
    /// revision already submitted is admitted without an engine: its replay
    /// answers the first submission.
    Admitted {
        steer_run_id: Option<RunId>,
        engine: Option<EngineId>,
    },
    /// Refuse it; nothing is queued.
    Refused(SubmissionRefusal),
}

/// What admitting a send requires, decided from its facts.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum SubmissionPlan {
    /// Refuse the send.
    Refuse(SubmissionRefusal),
    /// Admit it on `engine`, first saving `save` when it is a new
    /// configuration.
    Admit {
        /// Configuration to save before queueing, when the send changes it.
        save: Option<Box<EngineRunConfig>>,
        /// Engine the message runs on.
        engine: EngineId,
    },
}

/// Resolves a selection against `catalog` and the thread's `previous`
/// configuration.
///
/// # Errors
///
/// Returns an invalid-selection refusal naming why the catalog cannot build
/// it.
pub(super) fn resolve_in_catalog(
    catalog: NativeModelCatalog,
    selection: &CatalogSelection,
    previous: Option<&EngineRunConfig>,
) -> Result<ResolvedSelection, SubmissionRefusal> {
    let invalid = |reason: &str| {
        refusal(
            SubmissionRefusalKind::InvalidSelection,
            &format!("{reason}. Your draft is preserved; choose the model again or pick another."),
        )
    };
    let policy =
        crate::engine_selection::policy_from_selection(&catalog, selection).map_err(invalid)?;
    let config =
        crate::engine_selection::config_for_policy(&catalog, &policy, previous).map_err(invalid)?;
    Ok(ResolvedSelection {
        catalog,
        policy,
        config,
    })
}

/// Plans one send from its facts.
///
/// A still-starting run refuses every send. Without a selection the thread's
/// saved configuration runs it (none refuses). A selection that resolves to
/// the saved configuration is admitted as it is, even while its engine is
/// not ready: the Forge holds the message until it can run and says why on
/// the outbox row. A selection that changes the configuration must be
/// runnable in the served catalog, and its configuration is saved first; an
/// unrunnable one is refused with the engine's readiness reason.
pub(super) fn plan_submission(
    live_status: Option<RunLiveStatus>,
    saved: Option<&EngineRunConfig>,
    resolved: Option<Result<ResolvedSelection, SubmissionRefusal>>,
    readiness: impl Fn(&str) -> Option<EngineReadiness>,
) -> SubmissionPlan {
    if live_status == Some(RunLiveStatus::Queued) {
        return SubmissionPlan::Refuse(refusal(SubmissionRefusalKind::RunStarting, RUN_STARTING));
    }
    let resolved = match resolved {
        None => {
            return match saved {
                Some(saved) => SubmissionPlan::Admit {
                    save: None,
                    engine: saved.selection().engine_id(),
                },
                None => SubmissionPlan::Refuse(refusal(
                    SubmissionRefusalKind::NoSelection,
                    NO_SELECTION,
                )),
            };
        }
        Some(Err(refusal)) => return SubmissionPlan::Refuse(refusal),
        Some(Ok(resolved)) => resolved,
    };
    let engine = resolved.config.selection().engine_id();
    if saved == Some(&resolved.config) {
        return SubmissionPlan::Admit { save: None, engine };
    }
    if resolved.catalog.admit_policy(&resolved.policy).is_err() {
        let reason = readiness(&resolved.policy.engine_id)
            .filter(|readiness| !readiness.is_ready())
            .and_then(|readiness| readiness.reason().map(str::to_owned));
        return SubmissionPlan::Refuse(match reason {
            Some(reason) => refusal(
                SubmissionRefusalKind::EngineNotReady,
                &format!(
                    "{reason} Your draft is preserved; send again once it is ready, or review the engine in Settings."
                ),
            ),
            None => refusal(SubmissionRefusalKind::EngineNotReady, CATALOG_REJECTED),
        });
    }
    SubmissionPlan::Admit {
        save: Some(Box::new(resolved.config)),
        engine,
    }
}

/// The live run a send on `engine` steers into: a running or waiting run on
/// the same engine. Anything else is a fresh send queued behind it.
pub(super) fn steer_target(
    live: Option<(RunId, RunLiveStatus, EngineId)>,
    engine: EngineId,
) -> Option<RunId> {
    live.and_then(|(run_id, status, run_engine)| {
        (matches!(status, RunLiveStatus::Running | RunLiveStatus::Waiting) && run_engine == engine)
            .then_some(run_id)
    })
}

impl RequestHandler {
    /// Answers a selection resolution: the configuration the Forge would run
    /// for the thread, or why it cannot. Nothing is saved.
    pub(super) async fn resolve_model_selection_outcome(
        &self,
        request_id: &RequestId,
        query: &ResolveModelSelection,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let saved = self
            .repository
            .read_thread_engine_settings(&query.thread_id)
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        let resolution = self
            .resolve_selection(
                &query.thread_id,
                &query.selection,
                saved
                    .as_ref()
                    .map(artisan_database::ThreadEngineSettings::config),
            )
            .await
            .map(|resolved| resolved.config);
        Ok(outcome(
            request_id,
            ResponsePayload::ModelSelectionResolved(ModelSelectionResolution {
                thread_id: query.thread_id.clone(),
                selection: query.selection.clone(),
                outcome: resolution,
            }),
        ))
    }

    /// Admits one draft submission, saving the selection's configuration
    /// when it changes the thread's.
    ///
    /// A revision that was already submitted is admitted unchanged so its
    /// replay answers the first submission's message.
    pub(super) async fn admit_submission(
        &self,
        request_id: &RequestId,
        submit: &SubmitComposerDraft,
    ) -> Result<SubmissionAdmission, ProtocolFailure> {
        let thread = &submit.thread_id;
        if self
            .repository
            .composer_draft_submitted(thread, submit.draft_revision)
            .await
            .map_err(|error| repository_failure(&error, request_id))?
        {
            return Ok(SubmissionAdmission::Admitted {
                steer_run_id: None,
                engine: None,
            });
        }
        let live = self.live_run(thread).await;
        let saved = self
            .repository
            .read_thread_engine_settings(thread)
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        let saved_config = saved
            .as_ref()
            .map(artisan_database::ThreadEngineSettings::config);
        let resolved = match (&submit.selection, &live) {
            // A starting run refuses before any catalog work.
            (_, Some((_, RunLiveStatus::Queued, _))) | (None, _) => None,
            (Some(selection), _) => Some(
                self.resolve_selection(thread, selection, saved_config)
                    .await,
            ),
        };
        let plan = plan_submission(
            live.as_ref().map(|(_, status, _)| *status),
            saved_config,
            resolved,
            |engine| {
                self.account_usage
                    .as_ref()
                    .and_then(|usage| usage.readiness(engine))
            },
        );
        let engine = match plan {
            SubmissionPlan::Refuse(refusal) => return Ok(SubmissionAdmission::Refused(refusal)),
            SubmissionPlan::Admit { save, engine } => {
                if let Some(config) = save {
                    let precondition = saved
                        .as_ref()
                        .map_or(EngineConfigUpdatePrecondition::Unconfigured, |saved| {
                            EngineConfigUpdatePrecondition::Exact(saved.revision())
                        });
                    if !self
                        .save_resolved(request_id, thread, precondition, &config)
                        .await?
                    {
                        return Ok(SubmissionAdmission::Refused(refusal(
                            SubmissionRefusalKind::InvalidSelection,
                            SETTINGS_RACED,
                        )));
                    }
                }
                engine
            }
        };
        Ok(SubmissionAdmission::Admitted {
            steer_run_id: steer_target(live, engine),
            engine: Some(engine),
        })
    }

    /// The thread's configuration revision after an admitted submission;
    /// admission leaves the thread configured, so it exists.
    pub(super) async fn engine_config_revision(
        &self,
        request_id: &RequestId,
        thread: &ThreadId,
    ) -> Result<EngineConfigRevision, ProtocolFailure> {
        self.repository
            .read_thread_engine_settings(thread)
            .await
            .map_err(|error| repository_failure(&error, request_id))?
            .map(|saved| saved.revision())
            .ok_or_else(|| {
                super::failures::typed_failure(
                    artisan_protocol::ErrorCode::Internal,
                    "an admitted submission's thread has no engine configuration",
                    false,
                    request_id,
                )
            })
    }

    async fn resolve_selection(
        &self,
        thread: &ThreadId,
        selection: &CatalogSelection,
        previous: Option<&EngineRunConfig>,
    ) -> Result<ResolvedSelection, SubmissionRefusal> {
        let profile = selection.profile_id.clone().unwrap_or_else(|| {
            EngineProfileId::parse(crate::engine_selection::NATIVE_DEFAULT_PROFILE_ID)
                .expect("the native default profile is a valid profile id")
        });
        let catalog = crate::composer_catalog_handler::served_catalog(
            self.composer_catalog.as_ref(),
            self.account_usage.as_deref(),
            &self.repository,
            thread,
            &profile,
        )
        .await
        .map_err(|_| refusal(SubmissionRefusalKind::InvalidSelection, CATALOG_UNAVAILABLE))?;
        resolve_in_catalog(catalog, selection, previous)
    }

    /// Saves a configuration the Forge resolved for a send. Returns `false`
    /// when another writer changed the thread's configuration first.
    async fn save_resolved(
        &self,
        request_id: &RequestId,
        thread: &ThreadId,
        precondition: EngineConfigUpdatePrecondition,
        config: &EngineRunConfig,
    ) -> Result<bool, ProtocolFailure> {
        let accepted_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let save_id = RequestId::mint("forge-engine-save")
            .map_err(|_| super::forged_identity_failure("engine configuration save", request_id))?;
        match self
            .repository
            .set_thread_engine_config(SetThreadEngineConfigInput {
                request_id: save_id,
                thread_id: thread.clone(),
                precondition,
                config: config.clone(),
                accepted_at,
            })
            .await
        {
            Ok(_) => {
                // The selection a send saved is the user's latest choice.
                self.remember_default_engine_config(config).await;
                Ok(true)
            }
            Err(artisan_database::RepositoryError::EngineConfigRevisionConflict { .. }) => {
                Ok(false)
            }
            Err(error) => Err(repository_failure(&error, request_id)),
        }
    }

    /// The thread's live run with its status and engine, when one is
    /// registered and its lifecycle is live.
    async fn live_run(&self, thread: &ThreadId) -> Option<(RunId, RunLiveStatus, EngineId)> {
        let run_id = self.run_cancellation.as_ref()?.active_run(thread).ok()??;
        let (lifecycle, engine) = self
            .repository
            .read_assistant_run_status(thread, &run_id)
            .await
            .ok()??;
        let status = super::queries::run_live_status(&lifecycle)?;
        Some((run_id, status, engine))
    }
}

fn refusal(kind: SubmissionRefusalKind, message: &str) -> SubmissionRefusal {
    SubmissionRefusal::new(kind, message).unwrap_or_else(|_| {
        SubmissionRefusal::new(
            kind,
            "The Forge refused this send. Your draft is preserved.",
        )
        .expect("the fallback refusal message is valid")
    })
}

#[cfg(test)]
#[path = "model_selection_tests.rs"]
mod tests;
