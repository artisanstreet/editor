//! Forge-managed engine installs: the status read and push, the version
//! listing, and version changes.

#[allow(clippy::wildcard_imports)]
use super::*;

use artisan_domain::engine_install::{ENGINE_INSTALL_MAX_ENGINES, ENGINE_VERSION_LIST_MAX};
use artisan_domain::{
    ChangeEngineVersion, EngineInstallError, EngineInstallPhase, EngineInstallSnapshot,
    EngineInstallStatus, EngineIntegrity, EngineVersionChange, EngineVersionEntry,
    EngineVersionList, EngineVersionSelection, ListEngineVersions, ReadEngineInstalls,
};

use crate::artisan_capnp::{engine_install_snapshot, engine_version_change, engine_version_list};

/// Encodes the engine-install status read.
pub(crate) fn encode_read_engine_installs(mut builder: request::Builder<'_>) {
    builder.set_read_engine_installs(());
}

/// Encodes a version listing request.
pub(crate) fn encode_list_engine_versions(
    mut builder: request::Builder<'_>,
    request: &ListEngineVersions,
) {
    builder.set_list_engine_versions(&request.engine_id);
}

/// Encodes a version change request.
pub(crate) fn encode_change_engine_version(
    builder: request::Builder<'_>,
    request: &ChangeEngineVersion,
) {
    let mut change = builder.init_change_engine_version();
    change.set_engine_id(&request.engine_id);
    match &request.change {
        EngineVersionChange::Select(EngineVersionSelection::Latest) => change.set_latest(()),
        EngineVersionChange::Select(EngineVersionSelection::Version(version)) => {
            change.set_version(version);
        }
        EngineVersionChange::Rollback => change.set_rollback(()),
    }
}

/// Decodes the engine-install status read.
pub(crate) const fn decode_read_engine_installs() -> ClientRequest {
    ClientRequest::Query(Query::ReadEngineInstalls(ReadEngineInstalls))
}

/// Decodes a version listing request.
pub(crate) fn decode_list_engine_versions(
    engine_id: capnp::Result<capnp::text::Reader<'_>>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let engine_id = read_text(engine_id, "request.listEngineVersions")?;
    artisan_domain::engine_install::validate_engine_id(&engine_id).map_err(install_error)?;
    Ok(ClientRequest::Query(Query::ListEngineVersions(
        ListEngineVersions { engine_id },
    )))
}

/// Decodes a version change request.
pub(crate) fn decode_change_engine_version(
    value: engine_version_change::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let engine_id = read_text(
        value.get_engine_id(),
        "request.changeEngineVersion.engineId",
    )?;
    let change = match value.which()? {
        engine_version_change::Which::Latest(()) => {
            EngineVersionChange::Select(EngineVersionSelection::Latest)
        }
        engine_version_change::Which::Version(version) => {
            EngineVersionChange::Select(EngineVersionSelection::Version(read_text(
                version,
                "request.changeEngineVersion.version",
            )?))
        }
        engine_version_change::Which::Rollback(()) => EngineVersionChange::Rollback,
    };
    let request = ChangeEngineVersion { engine_id, change };
    request.validate().map_err(install_error)?;
    Ok(ClientRequest::Query(Query::ChangeEngineVersion(request)))
}

/// Encodes one install snapshot, shared by the answer and the push.
pub(crate) fn encode_engine_install_snapshot(
    builder: engine_install_snapshot::Builder<'_>,
    snapshot: &EngineInstallSnapshot,
) -> Result<(), ProtocolEncodeError> {
    let field = "engineInstalls.engines";
    let mut engines = builder.init_engines(list_length(field, snapshot.engines().len())?);
    for (index, status) in snapshot.engines().iter().enumerate() {
        let mut encoded = engines.reborrow().get(list_index(field, index)?);
        encoded.set_engine_id(&status.engine_id);
        encoded.set_phase(match status.phase {
            EngineInstallPhase::NotInstalled => artisan_capnp::EngineInstallPhase::NotInstalled,
            EngineInstallPhase::Installing => artisan_capnp::EngineInstallPhase::Installing,
            EngineInstallPhase::Ready => artisan_capnp::EngineInstallPhase::Ready,
            EngineInstallPhase::Failed => artisan_capnp::EngineInstallPhase::Failed,
            EngineInstallPhase::Unsupported => artisan_capnp::EngineInstallPhase::Unsupported,
        });
        encoded.set_active_version(status.active_version.as_deref().unwrap_or_default());
        encoded.set_held_version(status.held_version.as_deref().unwrap_or_default());
        encoded.set_latest_version(status.latest_version.as_deref().unwrap_or_default());
        encoded.set_pending_version(status.pending_version.as_deref().unwrap_or_default());
        encoded.set_rollback_version(status.rollback_version.as_deref().unwrap_or_default());
        encoded.set_has_progress(status.progress_percent.is_some());
        encoded.set_progress_percent(status.progress_percent.unwrap_or_default());
        encoded.set_reason(status.reason.as_deref().unwrap_or_default());
        encoded.set_overridden(status.overridden);
        encoded.set_integrity(match status.integrity {
            EngineIntegrity::VendorChecksum => artisan_capnp::EngineIntegrity::VendorChecksum,
            EngineIntegrity::TrustOnFirstDownload => {
                artisan_capnp::EngineIntegrity::TrustOnFirstDownload
            }
        });
        encoded.set_trusted_since(status.trusted_since.as_deref().unwrap_or_default());
        encoded.set_vendor_version_list(status.vendor_version_list);
    }
    Ok(())
}

/// Decodes one install snapshot after checking its bound.
pub(crate) fn decode_engine_install_snapshot(
    value: engine_install_snapshot::Reader<'_>,
) -> Result<EngineInstallSnapshot, ProtocolDecodeError> {
    let engines = value.get_engines()?;
    if engines.len() as usize > ENGINE_INSTALL_MAX_ENGINES {
        return Err(install_error(EngineInstallError::TooMany));
    }
    let optional = |value: capnp::Result<capnp::text::Reader<'_>>, field| {
        read_text(value, field).map(|text| (!text.is_empty()).then_some(text))
    };
    let statuses = engines
        .iter()
        .map(|status| {
            Ok(EngineInstallStatus {
                engine_id: read_text(status.get_engine_id(), "engineInstalls.engineId")?,
                phase: match status.get_phase()? {
                    artisan_capnp::EngineInstallPhase::NotInstalled => {
                        EngineInstallPhase::NotInstalled
                    }
                    artisan_capnp::EngineInstallPhase::Installing => EngineInstallPhase::Installing,
                    artisan_capnp::EngineInstallPhase::Ready => EngineInstallPhase::Ready,
                    artisan_capnp::EngineInstallPhase::Failed => EngineInstallPhase::Failed,
                    artisan_capnp::EngineInstallPhase::Unsupported => {
                        EngineInstallPhase::Unsupported
                    }
                },
                active_version: optional(status.get_active_version(), "engineInstalls.active")?,
                held_version: optional(status.get_held_version(), "engineInstalls.held")?,
                latest_version: optional(status.get_latest_version(), "engineInstalls.latest")?,
                pending_version: optional(status.get_pending_version(), "engineInstalls.pending")?,
                rollback_version: optional(
                    status.get_rollback_version(),
                    "engineInstalls.rollback",
                )?,
                progress_percent: status
                    .get_has_progress()
                    .then(|| status.get_progress_percent()),
                reason: optional(status.get_reason(), "engineInstalls.reason")?,
                overridden: status.get_overridden(),
                integrity: match status.get_integrity()? {
                    artisan_capnp::EngineIntegrity::VendorChecksum => {
                        EngineIntegrity::VendorChecksum
                    }
                    artisan_capnp::EngineIntegrity::TrustOnFirstDownload => {
                        EngineIntegrity::TrustOnFirstDownload
                    }
                },
                trusted_since: optional(status.get_trusted_since(), "engineInstalls.trustedSince")?,
                vendor_version_list: status.get_vendor_version_list(),
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;
    EngineInstallSnapshot::new(statuses).map_err(install_error)
}

/// Encodes one version listing.
pub(crate) fn encode_engine_version_list(
    mut builder: engine_version_list::Builder<'_>,
    list: &EngineVersionList,
) -> Result<(), ProtocolEncodeError> {
    builder.set_engine_id(list.engine_id());
    let field = "engineVersions.versions";
    let mut versions = builder.init_versions(list_length(field, list.versions().len())?);
    for (index, entry) in list.versions().iter().enumerate() {
        let mut encoded = versions.reborrow().get(list_index(field, index)?);
        encoded.set_version(&entry.version);
        encoded.set_installed(entry.installed);
        encoded.set_active(entry.active);
        encoded.set_below_floor(entry.below_floor);
    }
    Ok(())
}

/// Decodes one version listing after checking its bound.
pub(crate) fn decode_engine_version_list(
    value: engine_version_list::Reader<'_>,
) -> Result<EngineVersionList, ProtocolDecodeError> {
    let versions = value.get_versions()?;
    if versions.len() as usize > ENGINE_VERSION_LIST_MAX {
        return Err(install_error(EngineInstallError::TooMany));
    }
    let entries = versions
        .iter()
        .map(|entry| {
            Ok(EngineVersionEntry {
                version: read_text(entry.get_version(), "engineVersions.version")?,
                installed: entry.get_installed(),
                active: entry.get_active(),
                below_floor: entry.get_below_floor(),
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;
    EngineVersionList::new(
        read_text(value.get_engine_id(), "engineVersions.engineId")?,
        entries,
    )
    .map_err(install_error)
}

const fn install_error(source: EngineInstallError) -> ProtocolDecodeError {
    ProtocolDecodeError::EngineInstall { source }
}
