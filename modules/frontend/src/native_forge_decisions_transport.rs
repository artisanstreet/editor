//! Transport-thread child for the business decisions the Forge sends as
//! data: the scope-free host catalog with the Forge's account readiness
//! applied.
//!
//! Every command is answered by exactly one [`ForgeDecisionEvent`].

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::ReadHostCatalog;

use crate::native_model_catalog::NativeModelCatalog;

use super::*;

/// One Forge-decision command sent from the application thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForgeDecisionCommand {
    /// Read the scope-free host catalog.
    ReadHostCatalog,
}

/// One Forge-decision result returned by the service child.
#[derive(Clone, Debug, PartialEq)]
pub enum ForgeDecisionEvent {
    /// The decoded host catalog, or the read failure.
    HostCatalog(Result<Box<NativeModelCatalog>, ServiceFailure>),
}

/// The response shape one Forge-decision request accepts.
#[derive(Clone)]
pub(super) enum ForgeDecisionExpectation {
    HostCatalog,
}

impl ForgeDecisionExpectation {
    /// Whether `payload` is the exact answer to this request.
    pub(super) const fn accepts(&self, payload: &ResponsePayload) -> bool {
        matches!(
            (self, payload),
            (Self::HostCatalog, ResponsePayload::HostCatalog(_))
        )
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
    };
    publish(events, NativeTransportEvent::ForgeDecision(event))
}
