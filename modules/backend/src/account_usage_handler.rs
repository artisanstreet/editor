//! Request-handler adapter for the native account-usage query.
//!
//! Provider fan-out, freshness, and failure isolation stay in
//! [`super::account_usage_service`]. This leaf turns the snapshot into the
//! correlated protocol response and maps the single missing-capability case
//! to the existing failure vocabulary. No provider payload, token, path, or
//! executable detail is formatted here.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{ReadAccountUsage, RequestId};
use artisan_protocol::{ErrorCode, ErrorDetail, ProtocolFailure, ResponsePayload, ServerResponse};

use crate::account_usage_service::AccountUsageService;

const ACCOUNT_USAGE_UNAVAILABLE_DETAIL: &str = "account usage service is unavailable";

/// Finite backend failure for one account-usage query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccountUsageHandlerError {
    /// The process did not inject the account-usage capability.
    CapabilityUnavailable,
}

/// Answers one account-usage query from the injected usage service.
///
/// Every selected engine always yields exactly one report; per-engine
/// failures stay inside the snapshot with explicit auth and surface states.
pub(crate) async fn read_account_usage(
    service: Option<&AccountUsageService>,
    request_id: &RequestId,
    query: &ReadAccountUsage,
) -> Result<ServerResponse, ProtocolFailure> {
    let Some(service) = service else {
        return Err(protocol_failure(
            AccountUsageHandlerError::CapabilityUnavailable,
            request_id,
        ));
    };
    Ok(ServerResponse {
        request_id: request_id.clone(),
        payload: ResponsePayload::AccountUsage(service.read(query).await),
    })
}

/// Converts the missing-capability case to a correlated protocol failure.
pub(crate) fn protocol_failure(
    error: AccountUsageHandlerError,
    request_id: &RequestId,
) -> ProtocolFailure {
    let AccountUsageHandlerError::CapabilityUnavailable = error;
    ProtocolFailure {
        code: ErrorCode::UnsupportedFeature,
        detail: ErrorDetail::parse(ACCOUNT_USAGE_UNAVAILABLE_DETAIL)
            .expect("account usage failure detail is bounded"),
        retryable: false,
        request_id: Some(request_id.clone()),
    }
}
