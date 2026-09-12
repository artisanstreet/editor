//! Typed error mappings from provider/transport failures to operation errors.

use super::super::catalog::CatalogError;
use super::super::http::{HealthError, PromptError, ResumeError};
use super::super::readiness::ReadinessError;
use super::super::stream::StreamError;
use super::core::EngineOperationError;
pub(super) fn map_prompt_error(error: PromptError) -> EngineOperationError {
    match error {
        PromptError::Shutdown => EngineOperationError::Shutdown,
        PromptError::Cancelled => EngineOperationError::Cancelled,
        PromptError::Timeout => EngineOperationError::Deadline,
        _ => EngineOperationError::ProviderRequestFailed,
    }
}

pub(super) fn map_resume_error(error: ResumeError) -> EngineOperationError {
    match error {
        ResumeError::Shutdown => EngineOperationError::Shutdown,
        ResumeError::Cancelled => EngineOperationError::Cancelled,
        ResumeError::Timeout => EngineOperationError::Deadline,
        _ => EngineOperationError::ProviderRequestFailed,
    }
}

pub(super) fn map_stream_error(error: StreamError) -> EngineOperationError {
    match error {
        StreamError::Shutdown => EngineOperationError::Shutdown,
        StreamError::Cancelled => EngineOperationError::Cancelled,
        StreamError::Timeout => EngineOperationError::Deadline,
        _ => EngineOperationError::StreamFailed,
    }
}

pub(super) fn map_readiness_error(error: ReadinessError) -> EngineOperationError {
    match error {
        ReadinessError::Deadline => EngineOperationError::Deadline,
        ReadinessError::Cancelled => EngineOperationError::Cancelled,
        ReadinessError::Shutdown => EngineOperationError::Shutdown,
        other => EngineOperationError::ReadinessFailed(other),
    }
}

pub(super) fn map_health_error(error: HealthError) -> EngineOperationError {
    match error {
        HealthError::Timeout => EngineOperationError::Deadline,
        HealthError::Cancelled => EngineOperationError::Cancelled,
        HealthError::Shutdown => EngineOperationError::Shutdown,
        HealthError::IncompatibleVersion => EngineOperationError::IncompatibleVersion,
        other => EngineOperationError::HealthFailed(other),
    }
}

pub(super) fn map_catalog_error(error: CatalogError) -> EngineOperationError {
    match error {
        CatalogError::Shutdown => EngineOperationError::Shutdown,
        CatalogError::Cancelled => EngineOperationError::Cancelled,
        CatalogError::Timeout => EngineOperationError::Deadline,
        other => EngineOperationError::CatalogFailed(other),
    }
}
