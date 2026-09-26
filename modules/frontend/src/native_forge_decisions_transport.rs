//! Transport-thread child for the business decisions the Forge sends as
//! data: the scope-free host catalog with the Forge's account readiness
//! applied, and the resolution of a model selection into the configuration
//! the Forge would run. The Forge's admission of a draft submission (a typed
//! refusal, or the configuration revision it was admitted under) arrives as
//! a [`ForgeDecisionEvent`] too.
//!
//! Every command is answered by exactly one [`ForgeDecisionEvent`].

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    CatalogSelection, EngineConfigRevision, EngineRunConfig, ManualEngineConfiguration,
    ReadHostCatalog, ResolveEngineConfiguration, ResolveModelSelection, SubmissionRefusal,
};

use crate::native_model_catalog::NativeModelCatalog;

use super::*;

/// One Forge-decision command sent from the application thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForgeDecisionCommand {
    /// Read the scope-free host catalog.
    ReadHostCatalog,
    /// Resolve a selection into the configuration the Forge would run.
    ResolveModelSelection(ResolveModelSelection),
    /// Build the configuration a manual settings draft describes.
    ResolveEngineConfiguration(ResolveEngineConfiguration),
}

/// One Forge-decision result returned by the service child.
#[derive(Clone, Debug, PartialEq)]
pub enum ForgeDecisionEvent {
    /// The decoded host catalog, or the read failure.
    HostCatalog(Result<Box<NativeModelCatalog>, ServiceFailure>),
    /// The Forge's resolution of a selection, or the request failure.
    ModelSelectionResolved {
        /// Thread the selection was resolved for.
        thread_id: ThreadId,
        /// The selection as asked.
        selection: CatalogSelection,
        /// The configuration or the Forge's refusal; or the request failure.
        result: Result<Result<Box<EngineRunConfig>, SubmissionRefusal>, ServiceFailure>,
    },
    /// The Forge built a manual settings draft into a configuration, or
    /// refused it.
    EngineConfigurationResolved {
        /// Thread the draft is for.
        thread_id: ThreadId,
        /// The draft as asked.
        configuration: ManualEngineConfiguration,
        /// The configuration or the Forge's refusal; or the request failure.
        result: Result<Result<Box<EngineRunConfig>, SubmissionRefusal>, ServiceFailure>,
    },
    /// The Forge refused a draft submission; nothing was queued.
    SendRefused {
        /// Draft scope of the send.
        scope: artisan_domain::ComposerDraftScope,
        /// The send's request identity.
        request_id: RequestId,
        /// The typed refusal with its message.
        refusal: SubmissionRefusal,
    },
    /// The Forge admitted a draft submission under this configuration
    /// revision of its thread.
    SendAdmitted {
        /// Thread of the send.
        thread_id: ThreadId,
        /// Configuration revision after admission.
        engine_config_revision: EngineConfigRevision,
    },
}

/// The response shape one Forge-decision request accepts.
#[derive(Clone)]
pub(super) enum ForgeDecisionExpectation {
    HostCatalog,
    Resolution(ResolveModelSelection),
    Configuration(ResolveEngineConfiguration),
}

impl ForgeDecisionExpectation {
    /// Whether `payload` is the exact answer to this request.
    pub(super) fn accepts(&self, payload: &ResponsePayload) -> bool {
        match (self, payload) {
            (Self::HostCatalog, ResponsePayload::HostCatalog(_)) => true,
            (Self::Resolution(query), ResponsePayload::ModelSelectionResolved(resolution)) => {
                resolution.thread_id == query.thread_id && resolution.selection == query.selection
            }
            (
                Self::Configuration(query),
                ResponsePayload::EngineConfigurationResolved(resolution),
            ) => {
                resolution.thread_id == query.thread_id
                    && resolution.configuration == query.configuration
            }
            _ => false,
        }
    }
}

/// Handles one Forge-decision command on the authenticated service runtime.
pub(super) async fn handle_forge_decision_command(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: ForgeDecisionCommand,
) -> Result<(), ServiceFailure> {
    let event = match command {
        ForgeDecisionCommand::ReadHostCatalog => {
            let result = runtime
                .request(
                    frames,
                    query_request(Query::ReadHostCatalog(ReadHostCatalog)),
                    ExpectedResponse::ForgeDecision(ForgeDecisionExpectation::HostCatalog),
                )
                .await
                .map_err(ServiceFailure::from)
                .and_then(|payload| match payload {
                    ResponsePayload::HostCatalog(snapshot) => snapshot
                        .decoded()
                        .map(Box::new)
                        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request)),
                    _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
                });
            ForgeDecisionEvent::HostCatalog(result)
        }
        ForgeDecisionCommand::ResolveModelSelection(query) => {
            let thread_id = query.thread_id.clone();
            let selection = query.selection.clone();
            let result = if runtime.known_threads.contains(&thread_id) {
                runtime
                    .request(
                        frames,
                        query_request(Query::ResolveModelSelection(query.clone())),
                        ExpectedResponse::ForgeDecision(ForgeDecisionExpectation::Resolution(
                            query,
                        )),
                    )
                    .await
                    .map_err(ServiceFailure::from)
                    .and_then(|payload| match payload {
                        ResponsePayload::ModelSelectionResolved(resolution) => {
                            Ok(resolution.outcome.map(Box::new))
                        }
                        _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
                    })
            } else {
                Err(ServiceFailure::invalid(ServiceFailureStage::Request))
            };
            ForgeDecisionEvent::ModelSelectionResolved {
                thread_id,
                selection,
                result,
            }
        }
        ForgeDecisionCommand::ResolveEngineConfiguration(query) => {
            let thread_id = query.thread_id.clone();
            let configuration = query.configuration.clone();
            let result = runtime
                .request(
                    frames,
                    query_request(Query::ResolveEngineConfiguration(query.clone())),
                    ExpectedResponse::ForgeDecision(ForgeDecisionExpectation::Configuration(query)),
                )
                .await
                .map_err(ServiceFailure::from)
                .and_then(|payload| match payload {
                    ResponsePayload::EngineConfigurationResolved(resolution) => {
                        Ok(resolution.outcome.map(Box::new))
                    }
                    _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
                });
            ForgeDecisionEvent::EngineConfigurationResolved {
                thread_id,
                configuration,
                result,
            }
        }
    };
    publish(events, NativeTransportEvent::ForgeDecision(event))
}
