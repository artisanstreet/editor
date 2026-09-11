//! Request-handler adapters for the native composer catalog and favorites.
//!
//! Catalog discovery stays in [`super::composer_catalog_service`]. This leaf
//! combines its typed result with the latest durable favorites, passes the
//! complete value through the shared catalog bridge/wire encoder, and maps
//! bounded failures to the existing protocol vocabulary. No provider payload,
//! path, executable, or catalog diagnostic is formatted here.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_database::{
    ModelFavoritesRepositoryError, Repository, SetModelFavoriteInput, SetModelFavoriteResult,
};
use artisan_domain::{
    EngineProfileId, ReadComposerCatalog, RequestId, SetModelFavorite, ThreadId, UnixMillis,
};
use artisan_protocol::{
    CatalogSnapshotWire, ComposerCatalogResult, ErrorCode, ErrorDetail,
    ModelFavoritesSnapshot as ProtocolModelFavoritesSnapshot, ProtocolFailure, ResponsePayload,
    ServerResponse, SetModelFavoriteReceipt,
};

use crate::composer_catalog_service::{ComposerCatalogService, ComposerCatalogServiceError};

const CATALOG_UNAVAILABLE_DETAIL: &str = "composer catalog service is unavailable";
const CATALOG_THREAD_UNKNOWN_DETAIL: &str = "composer catalog thread is unknown";
const CATALOG_PROJECT_UNKNOWN_DETAIL: &str = "composer catalog project is unknown";
const CATALOG_PERSISTENCE_DETAIL: &str = "composer catalog persistence is unavailable";
const CATALOG_PROFILE_DETAIL: &str = "composer catalog engine profile is unavailable";
const CATALOG_SCOPE_DETAIL: &str = "composer catalog scope is unavailable";
const CATALOG_SCOPE_MISMATCH_DETAIL: &str = "composer catalog scope did not match the request";
const CATALOG_BUSY_DETAIL: &str = "composer catalog service is busy";
const CATALOG_OWNER_UNAVAILABLE_DETAIL: &str = "composer catalog owner is unavailable";
const CATALOG_TIMEOUT_DETAIL: &str = "composer catalog discovery timed out";
const CATALOG_DISCOVERY_DETAIL: &str = "composer catalog discovery failed";
const CATALOG_INVALID_DETAIL: &str = "composer catalog response is invalid";
const CATALOG_STALE_DETAIL: &str = "model favorite catalog revision is stale";
const CATALOG_MODEL_UNKNOWN_DETAIL: &str = "model favorite is not in the current catalog";
const FAVORITES_PERSISTENCE_DETAIL: &str = "model favorites persistence is unavailable";
const FAVORITES_CORRUPT_DETAIL: &str = "model favorites state is invalid";
const FAVORITES_CONFLICT_DETAIL: &str = "model favorite request conflicts with an existing request";

/// Finite backend failure for one composer catalog/favorites request.
///
/// Variants intentionally carry no user payload, path, provider response, or
/// database diagnostic. The request handler turns these categories into the
/// stable protocol failure contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ComposerCatalogHandlerError {
    /// The process did not inject the catalog capability.
    CapabilityUnavailable,
    /// The requested thread is absent.
    ThreadUnknown,
    /// The thread's attached project is absent.
    ProjectUnknown,
    /// The authoritative thread/project scope could not be read.
    CatalogPersistence,
    /// Durable favorites could not be read or written due to a transient
    /// database operation.
    FavoritesPersistence,
    /// Durable favorites violate a bounded schema/domain invariant.
    FavoritesInvalid,
    /// A favorite request id was reused for a different persisted payload.
    IdempotencyConflict,
    /// The requested profile cannot be resolved by the certified authority.
    ProfileUnavailable,
    /// The authoritative catalog scope could not be constructed.
    ScopeUnavailable,
    /// The owner returned a different scope than the request asked for.
    ScopeMismatch,
    /// The bounded owner admission queue is full.
    Busy,
    /// The owner is down or its admission channel is closed.
    OwnerUnavailable,
    /// The bounded discovery operation elapsed.
    TimedOut,
    /// The owner returned a typed unsuccessful operation result.
    DiscoveryFailed,
    /// The bridge or shared wire encoder rejected the typed result.
    InvalidCatalog,
    /// The favorite command observed an older catalog revision.
    StaleCatalog,
    /// The favorite command named no catalog model.
    UnknownModel,
}

/// Answers one scoped composer-catalog query.
pub(crate) async fn read_composer_catalog(
    service: Option<&ComposerCatalogService>,
    repository: &Repository,
    request_id: &RequestId,
    query: &ReadComposerCatalog,
) -> Result<ServerResponse, ProtocolFailure> {
    let catalog = current_catalog(service, repository, query.thread_id(), query.profile_id())
        .await
        .map_err(|error| protocol_failure(error, request_id))?;
    let result = catalog_response(
        query.thread_id().clone(),
        query.profile_id().clone(),
        &catalog,
    )
    .map_err(|error| protocol_failure(error, request_id))?;
    Ok(outcome(
        request_id,
        ResponsePayload::ComposerCatalog(result),
    ))
}

/// Answers the durable, pure model-favorites query.
pub(crate) async fn read_model_favorites(
    repository: &Repository,
    request_id: &RequestId,
) -> Result<ServerResponse, ProtocolFailure> {
    let snapshot = repository
        .read_model_favorites()
        .await
        .map_err(|error| protocol_failure(favorites_error(&error), request_id))?;
    Ok(outcome(
        request_id,
        ResponsePayload::ModelFavorites(ProtocolModelFavoritesSnapshot::from_domain(&snapshot)),
    ))
}

/// Performs the non-durable admission checks for one favorite command.
///
/// The caller must perform durable receipt lookup before entering this
/// function. A persisted removal is deliberately accepted without discovery:
/// it may name a model that has disappeared from the current provider
/// catalog. Every addition, and every removal that is not already persisted,
/// must match the current authoritative scoped revision and model identity.
pub(crate) async fn prepare_model_favorite(
    service: Option<&ComposerCatalogService>,
    repository: &Repository,
    command: &SetModelFavorite,
) -> Result<(), ComposerCatalogHandlerError> {
    let favorites = repository
        .read_model_favorites()
        .await
        .map_err(|error| favorites_error(&error))?;
    if !command.favorite() && favorites.contains(command.model_id()) {
        return Ok(());
    }

    let catalog = current_catalog(
        service,
        repository,
        command.thread_id(),
        command.profile_id(),
    )
    .await?;
    validate_current_catalog(&catalog, command)
}

/// Persists a previously admitted favorite command and builds its protocol
/// receipt. The repository owns the immediate transaction and durable replay.
pub(crate) async fn persist_model_favorite(
    repository: &Repository,
    command: &SetModelFavorite,
    accepted_at: UnixMillis,
    request_id: &RequestId,
) -> Result<ServerResponse, ProtocolFailure> {
    let result = repository
        .set_model_favorite(SetModelFavoriteInput {
            request_id: command.request_id().clone(),
            model_id: command.model_id().clone(),
            favorite: command.favorite(),
            accepted_at,
        })
        .await
        .map_err(|error| protocol_failure(favorites_error(&error), request_id))?;
    Ok(favorite_response(request_id, command, &result))
}

/// Converts an exact repository replay into the same protocol receipt used by
/// a newly accepted favorite mutation.
#[must_use]
pub(crate) fn favorite_response(
    request_id: &RequestId,
    command: &SetModelFavorite,
    result: &SetModelFavoriteResult,
) -> ServerResponse {
    outcome(
        request_id,
        ResponsePayload::ModelFavoriteSet(SetModelFavoriteReceipt {
            request_id: result.request_id().clone(),
            model_id: command.model_id().clone(),
            favorite: command.favorite(),
            disposition: result.disposition(),
            snapshot: ProtocolModelFavoritesSnapshot::from_domain(result.snapshot()),
        }),
    )
}

/// Maps a favorites repository error without including its database detail.
#[must_use]
pub(crate) fn favorites_error(
    error: &ModelFavoritesRepositoryError,
) -> ComposerCatalogHandlerError {
    match error {
        ModelFavoritesRepositoryError::IdempotencyConflict { .. } => {
            ComposerCatalogHandlerError::IdempotencyConflict
        }
        ModelFavoritesRepositoryError::Database { .. } => {
            ComposerCatalogHandlerError::FavoritesPersistence
        }
        ModelFavoritesRepositoryError::MissingState
        | ModelFavoritesRepositoryError::CorruptData { .. }
        | ModelFavoritesRepositoryError::CardinalityExceeded { .. }
        | ModelFavoritesRepositoryError::RevisionExhausted { .. }
        | ModelFavoritesRepositoryError::SnapshotTooLarge { .. }
        | ModelFavoritesRepositoryError::Invariant { .. } => {
            ComposerCatalogHandlerError::FavoritesInvalid
        }
    }
}

async fn current_catalog(
    service: Option<&ComposerCatalogService>,
    repository: &Repository,
    thread: &ThreadId,
    profile: &EngineProfileId,
) -> Result<artisan_catalog::NativeModelCatalog, ComposerCatalogHandlerError> {
    let service = service.ok_or(ComposerCatalogHandlerError::CapabilityUnavailable)?;
    let result = service
        .discover(thread, profile)
        .await
        .map_err(service_error)?;
    let favorites = repository
        .read_model_favorites()
        .await
        .map_err(|error| favorites_error(&error))?;
    let discovery = crate::model_discovery::discovery_bundle().await;
    crate::native_model_catalog::from_catalog_result_with_discovery(result, &discovery, &favorites)
        .map_err(|_| ComposerCatalogHandlerError::InvalidCatalog)
}

fn service_error(error: ComposerCatalogServiceError) -> ComposerCatalogHandlerError {
    match error {
        ComposerCatalogServiceError::ThreadUnknown => ComposerCatalogHandlerError::ThreadUnknown,
        ComposerCatalogServiceError::ProjectUnknown => ComposerCatalogHandlerError::ProjectUnknown,
        ComposerCatalogServiceError::PersistenceUnavailable => {
            ComposerCatalogHandlerError::CatalogPersistence
        }
        ComposerCatalogServiceError::ProfileUnavailable => {
            ComposerCatalogHandlerError::ProfileUnavailable
        }
        ComposerCatalogServiceError::ScopeUnavailable => {
            ComposerCatalogHandlerError::ScopeUnavailable
        }
        ComposerCatalogServiceError::Busy => ComposerCatalogHandlerError::Busy,
        ComposerCatalogServiceError::Unavailable => ComposerCatalogHandlerError::OwnerUnavailable,
        ComposerCatalogServiceError::TimedOut => ComposerCatalogHandlerError::TimedOut,
        ComposerCatalogServiceError::DiscoveryFailed => {
            ComposerCatalogHandlerError::DiscoveryFailed
        }
        ComposerCatalogServiceError::ScopeMismatch => ComposerCatalogHandlerError::ScopeMismatch,
    }
}

fn validate_current_catalog(
    catalog: &artisan_catalog::NativeModelCatalog,
    command: &SetModelFavorite,
) -> Result<(), ComposerCatalogHandlerError> {
    let scope = catalog
        .scope
        .as_ref()
        .ok_or(ComposerCatalogHandlerError::InvalidCatalog)?;
    if scope.profile_id != command.profile_id().as_str() {
        return Err(ComposerCatalogHandlerError::ScopeMismatch);
    }
    if catalog.catalog_revision != command.catalog_revision().as_str() {
        return Err(ComposerCatalogHandlerError::StaleCatalog);
    }
    if catalog
        .manifest
        .model(command.model_id().as_str())
        .is_none()
    {
        return Err(ComposerCatalogHandlerError::UnknownModel);
    }
    Ok(())
}

fn catalog_response(
    thread_id: ThreadId,
    profile_id: EngineProfileId,
    catalog: &artisan_catalog::NativeModelCatalog,
) -> Result<ComposerCatalogResult, ComposerCatalogHandlerError> {
    let bytes = artisan_catalog::wire::encode_catalog(catalog)
        .map_err(|_| ComposerCatalogHandlerError::InvalidCatalog)?;
    let snapshot =
        CatalogSnapshotWire::new(bytes).map_err(|_| ComposerCatalogHandlerError::InvalidCatalog)?;
    ComposerCatalogResult::new(thread_id, profile_id, snapshot)
        .map_err(|_| ComposerCatalogHandlerError::InvalidCatalog)
}

/// Converts one finite catalog/favorites category to a correlated protocol
/// failure without formatting untrusted payloads.
pub(crate) fn protocol_failure(
    error: ComposerCatalogHandlerError,
    request_id: &RequestId,
) -> ProtocolFailure {
    let (code, detail, retryable) = match error {
        ComposerCatalogHandlerError::CapabilityUnavailable => (
            ErrorCode::UnsupportedFeature,
            CATALOG_UNAVAILABLE_DETAIL,
            false,
        ),
        ComposerCatalogHandlerError::ThreadUnknown => (
            ErrorCode::ThreadUnknown,
            CATALOG_THREAD_UNKNOWN_DETAIL,
            false,
        ),
        ComposerCatalogHandlerError::ProjectUnknown => (
            ErrorCode::ProjectUnknown,
            CATALOG_PROJECT_UNKNOWN_DETAIL,
            false,
        ),
        ComposerCatalogHandlerError::CatalogPersistence => {
            (ErrorCode::Internal, CATALOG_PERSISTENCE_DETAIL, true)
        }
        ComposerCatalogHandlerError::FavoritesPersistence => {
            (ErrorCode::Internal, FAVORITES_PERSISTENCE_DETAIL, true)
        }
        ComposerCatalogHandlerError::FavoritesInvalid => {
            (ErrorCode::Internal, FAVORITES_CORRUPT_DETAIL, false)
        }
        ComposerCatalogHandlerError::IdempotencyConflict => (
            ErrorCode::IdempotencyConflict,
            FAVORITES_CONFLICT_DETAIL,
            false,
        ),
        ComposerCatalogHandlerError::ProfileUnavailable => {
            (ErrorCode::InvalidInput, CATALOG_PROFILE_DETAIL, false)
        }
        ComposerCatalogHandlerError::ScopeUnavailable => {
            (ErrorCode::InvalidInput, CATALOG_SCOPE_DETAIL, false)
        }
        ComposerCatalogHandlerError::ScopeMismatch => (
            ErrorCode::InvalidInput,
            CATALOG_SCOPE_MISMATCH_DETAIL,
            false,
        ),
        ComposerCatalogHandlerError::Busy => (ErrorCode::Internal, CATALOG_BUSY_DETAIL, true),
        ComposerCatalogHandlerError::OwnerUnavailable => {
            (ErrorCode::Internal, CATALOG_OWNER_UNAVAILABLE_DETAIL, true)
        }
        ComposerCatalogHandlerError::TimedOut => {
            (ErrorCode::Internal, CATALOG_TIMEOUT_DETAIL, true)
        }
        ComposerCatalogHandlerError::DiscoveryFailed => {
            (ErrorCode::Internal, CATALOG_DISCOVERY_DETAIL, true)
        }
        ComposerCatalogHandlerError::InvalidCatalog => {
            (ErrorCode::Internal, CATALOG_INVALID_DETAIL, false)
        }
        ComposerCatalogHandlerError::StaleCatalog => {
            (ErrorCode::InvalidInput, CATALOG_STALE_DETAIL, false)
        }
        ComposerCatalogHandlerError::UnknownModel => {
            (ErrorCode::InvalidInput, CATALOG_MODEL_UNKNOWN_DETAIL, false)
        }
    };
    ProtocolFailure {
        code,
        detail: ErrorDetail::parse(detail).expect("catalog failure detail is bounded"),
        retryable,
        request_id: Some(request_id.clone()),
    }
}

fn outcome(request_id: &RequestId, payload: ResponsePayload) -> ServerResponse {
    ServerResponse {
        request_id: request_id.clone(),
        payload,
    }
}

#[cfg(test)]
mod tests {
    use artisan_catalog::{NativeCatalogRuntime, NativeCatalogScope, NativeModelCatalog};
    use artisan_domain::{CatalogRevision, ModelFavoriteId, SetModelFavorite};

    use super::*;

    fn catalog(profile_id: &str, revision: &str) -> NativeModelCatalog {
        let offline = NativeModelCatalog::offline().expect("bundled catalog is valid");
        NativeModelCatalog::from_manifest(
            offline.manifest,
            NativeCatalogRuntime {
                catalog_revision: Some(revision.to_owned()),
                scope: Some(NativeCatalogScope {
                    profile_id: profile_id.to_owned(),
                    working_directory: "C:/workspace".to_owned(),
                    workspace_trust: "safe".to_owned(),
                }),
                ..Default::default()
            },
        )
    }

    fn command(
        profile_id: &str,
        revision: &str,
        model_id: &str,
        favorite: bool,
    ) -> SetModelFavorite {
        SetModelFavorite::new(
            RequestId::parse("favorite-request").expect("request id is valid"),
            ThreadId::parse("thread-catalog").expect("thread id is valid"),
            EngineProfileId::parse(profile_id).expect("profile id is valid"),
            CatalogRevision::parse(revision).expect("catalog revision is valid"),
            ModelFavoriteId::parse(model_id).expect("model id is valid"),
            favorite,
        )
    }

    #[test]
    fn favorite_admission_requires_exact_revision_scope_and_model_identity() {
        let native = catalog("profile-main", "catalog-7");
        let model_id = native
            .manifest
            .models
            .first()
            .expect("bundled catalog has a model")
            .id
            .clone();

        assert_eq!(
            validate_current_catalog(
                &native,
                &command("profile-main", "catalog-7", &model_id, true),
            ),
            Ok(())
        );
        assert_eq!(
            validate_current_catalog(
                &native,
                &command("profile-main", "catalog-6", &model_id, true),
            ),
            Err(ComposerCatalogHandlerError::StaleCatalog)
        );
        assert_eq!(
            validate_current_catalog(
                &native,
                &command("profile-other", "catalog-7", &model_id, true),
            ),
            Err(ComposerCatalogHandlerError::ScopeMismatch)
        );
        assert_eq!(
            validate_current_catalog(
                &native,
                &command("profile-main", "catalog-7", "not-in-catalog", true),
            ),
            Err(ComposerCatalogHandlerError::UnknownModel)
        );
    }

    #[test]
    fn service_and_stale_removal_failures_are_bounded_and_classified() {
        let request_id = RequestId::parse("failure-request").expect("request id is valid");
        let unavailable = protocol_failure(
            ComposerCatalogHandlerError::CapabilityUnavailable,
            &request_id,
        );
        assert_eq!(unavailable.code, ErrorCode::UnsupportedFeature);
        assert!(!unavailable.retryable);
        let busy = protocol_failure(ComposerCatalogHandlerError::Busy, &request_id);
        assert_eq!(busy.code, ErrorCode::Internal);
        assert!(busy.retryable);
        let stale = protocol_failure(ComposerCatalogHandlerError::StaleCatalog, &request_id);
        assert_eq!(stale.code, ErrorCode::InvalidInput);
        assert!(!stale.retryable);
        assert!(format!("{unavailable:?}").len() < 256);
    }
}
