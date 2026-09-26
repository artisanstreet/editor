//! Transport-thread child for the Forge-managed engine installs: the status
//! read, the vendor version listing, and version changes (follow `latest`,
//! hold a version, roll back).
//!
//! Every command is answered by exactly one [`EngineInstallsEvent`]. After
//! the first read the Forge pushes status changes as host state.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    ChangeEngineVersion, EngineInstallSnapshot, EngineVersionList, ListEngineVersions,
    ReadEngineInstalls,
};

use super::{
    ExpectedResponse, FrameFactory, NativeTransportEvent, Query, ResponsePayload, ServiceFailure,
    ServiceFailureStage, ServiceRuntime, SyncSender, publish, query_request,
};

/// One engine-install command sent from the application thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineInstallsCommand {
    /// Read every managed engine's status.
    Read,
    /// List the vendor's versions of one engine.
    ListVersions(String),
    /// Select a version, follow `latest`, or roll back one engine.
    Change(ChangeEngineVersion),
}

/// One engine-install result returned by the service child.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineInstallsEvent {
    /// The status snapshot after a read or a queued change, or the failure.
    Snapshot(Result<EngineInstallSnapshot, ServiceFailure>),
    /// One engine's version listing, or the failure.
    Versions {
        engine_id: String,
        result: Result<EngineVersionList, ServiceFailure>,
    },
}

/// The response shape one engine-install request accepts.
#[derive(Clone)]
pub(super) enum EngineInstallsExpectation {
    Snapshot,
    Versions(String),
}

impl EngineInstallsExpectation {
    /// Whether `payload` is the exact answer to this request.
    pub(super) fn accepts(&self, payload: &ResponsePayload) -> bool {
        match (self, payload) {
            (Self::Snapshot, ResponsePayload::EngineInstalls(_)) => true,
            (Self::Versions(engine_id), ResponsePayload::EngineVersions(list)) => {
                list.engine_id() == engine_id
            }
            _ => false,
        }
    }
}

/// Handles one engine-install command on the authenticated service runtime.
pub(super) async fn handle_engine_installs_command(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: EngineInstallsCommand,
) -> Result<(), ServiceFailure> {
    let (query, expectation) = match command {
        EngineInstallsCommand::Read => (
            Query::ReadEngineInstalls(ReadEngineInstalls),
            EngineInstallsExpectation::Snapshot,
        ),
        EngineInstallsCommand::ListVersions(engine_id) => (
            Query::ListEngineVersions(ListEngineVersions {
                engine_id: engine_id.clone(),
            }),
            EngineInstallsExpectation::Versions(engine_id),
        ),
        EngineInstallsCommand::Change(change) => (
            Query::ChangeEngineVersion(change),
            EngineInstallsExpectation::Snapshot,
        ),
    };
    let answer = Box::pin(runtime.request(
        frames,
        query_request(query),
        ExpectedResponse::EngineInstalls(expectation.clone()),
    ))
    .await
    .map_err(ServiceFailure::from);
    let event = match expectation {
        EngineInstallsExpectation::Snapshot => {
            EngineInstallsEvent::Snapshot(answer.and_then(|payload| match payload {
                ResponsePayload::EngineInstalls(snapshot) => Ok(snapshot),
                _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
            }))
        }
        EngineInstallsExpectation::Versions(engine_id) => EngineInstallsEvent::Versions {
            engine_id,
            result: answer.and_then(|payload| match payload {
                ResponsePayload::EngineVersions(list) => Ok(list),
                _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
            }),
        },
    };
    publish(events, NativeTransportEvent::EngineInstalls(event))
}
