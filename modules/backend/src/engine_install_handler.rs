//! Request-handler adapter for the Forge-managed engine installs.
//!
//! The engine manager owns every install decision; this leaf turns its
//! snapshot, version listing, and queued changes into correlated protocol
//! responses. The manager rides on the account-usage service, which already
//! carries every engine's host state to connections.

#![forbid(unsafe_code)]

use artisan_domain::{EngineInstallSnapshot, EngineVersionChange, Query, RequestId};
use artisan_protocol::{ErrorCode, ErrorDetail, ProtocolFailure, ResponsePayload, ServerResponse};

use crate::account_usage_service::AccountUsageService;
use crate::engine_manager::{EngineManager, EngineManagerError};

const UNAVAILABLE_DETAIL: &str = "engine management is unavailable on this Forge";

impl AccountUsageService {
    /// Attaches the Forge's engine manager so its statuses are pushed beside
    /// usage.
    #[must_use]
    pub(crate) fn with_engine_manager(self, manager: EngineManager) -> Self {
        let _ = self.engines.set(manager);
        self
    }

    /// Returns the engine manager, when this Forge runs one.
    pub(crate) fn engine_manager(&self) -> Option<&EngineManager> {
        self.engines.get()
    }

    /// Returns the current engine install snapshot.
    pub(crate) fn engine_installs(&self) -> Option<EngineInstallSnapshot> {
        self.engine_manager().map(EngineManager::snapshot)
    }
}

/// Answers the engine-install read, version listing, and version change.
pub(crate) async fn answer(
    service: Option<&AccountUsageService>,
    request_id: &RequestId,
    query: &Query,
) -> Result<ServerResponse, ProtocolFailure> {
    let Some(manager) = service.and_then(AccountUsageService::engine_manager) else {
        return Err(failure(
            ErrorCode::UnsupportedFeature,
            UNAVAILABLE_DETAIL,
            request_id,
        ));
    };
    let payload = match query {
        Query::ReadEngineInstalls(_) => Ok(ResponsePayload::EngineInstalls(manager.snapshot())),
        Query::ListEngineVersions(request) => manager
            .list_versions(&request.engine_id)
            .await
            .map(ResponsePayload::EngineVersions),
        Query::ChangeEngineVersion(request) => match &request.change {
            EngineVersionChange::Select(selection) => manager.select(&request.engine_id, selection),
            EngineVersionChange::Rollback => manager.rollback(&request.engine_id),
        }
        .map(ResponsePayload::EngineInstalls),
        _ => Err(EngineManagerError::Unavailable),
    };
    payload
        .map(|payload| ServerResponse {
            request_id: request_id.clone(),
            payload,
        })
        .map_err(|error| {
            let code = match error {
                EngineManagerError::UnknownEngine | EngineManagerError::InvalidSelection => {
                    ErrorCode::InvalidInput
                }
                EngineManagerError::Unavailable | EngineManagerError::Operation(_) => {
                    ErrorCode::Internal
                }
            };
            failure(code, error.reason(), request_id)
        })
}

fn failure(code: ErrorCode, detail: &str, request_id: &RequestId) -> ProtocolFailure {
    ProtocolFailure {
        code,
        detail: ErrorDetail::parse(detail)
            .unwrap_or_else(|_| ErrorDetail::parse(UNAVAILABLE_DETAIL).expect("bounded detail")),
        retryable: matches!(code, ErrorCode::Internal),
        request_id: Some(request_id.clone()),
    }
}
