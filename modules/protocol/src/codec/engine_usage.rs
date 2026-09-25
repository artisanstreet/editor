//! Engine account-usage snapshot encode and decode, including the Forge's
//! per-engine readiness verdict.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn encode_engine_usage_window_kind(
    kind: EngineUsageWindowKind,
) -> artisan_capnp::EngineUsageWindowKind {
    match kind {
        EngineUsageWindowKind::Session => artisan_capnp::EngineUsageWindowKind::Session,
        EngineUsageWindowKind::Weekly => artisan_capnp::EngineUsageWindowKind::Weekly,
        EngineUsageWindowKind::Monthly => artisan_capnp::EngineUsageWindowKind::Monthly,
        EngineUsageWindowKind::Unknown => artisan_capnp::EngineUsageWindowKind::Unknown,
    }
}

pub(crate) fn encode_engine_usage_authentication(
    state: EngineUsageAuthentication,
) -> artisan_capnp::EngineUsageAuthentication {
    match state {
        EngineUsageAuthentication::Authenticated => {
            artisan_capnp::EngineUsageAuthentication::Authenticated
        }
        EngineUsageAuthentication::Unauthenticated => {
            artisan_capnp::EngineUsageAuthentication::Unauthenticated
        }
        EngineUsageAuthentication::Unknown => artisan_capnp::EngineUsageAuthentication::Unknown,
    }
}

pub(crate) fn encode_quota_surface(surface: QuotaSurface) -> artisan_capnp::QuotaSurface {
    match surface {
        QuotaSurface::Supported => artisan_capnp::QuotaSurface::Supported,
        QuotaSurface::Unknown => artisan_capnp::QuotaSurface::Unknown,
        QuotaSurface::Unsupported => artisan_capnp::QuotaSurface::Unsupported,
    }
}

pub(crate) fn encode_engine_usage_window(
    mut builder: artisan_capnp::engine_usage_window::Builder<'_>,
    window: &EngineUsageWindow,
) {
    builder.set_id(window.id());
    builder.set_kind(encode_engine_usage_window_kind(window.kind()));
    builder.set_label(window.label().unwrap_or(""));
    builder.set_percent_used(window.percent_used());
    builder.set_resets_at(window.resets_at().unwrap_or(""));
    builder.set_window_minutes(window.window_minutes().unwrap_or(0));
}

pub(crate) fn encode_engine_usage_report(
    mut builder: artisan_capnp::engine_usage_report::Builder<'_>,
    report: &EngineUsageReport,
) -> Result<(), ProtocolEncodeError> {
    builder.set_engine_id(report.engine_id());
    builder.set_display_name(report.display_name());
    builder.set_authentication(encode_engine_usage_authentication(
        report.authentication().state(),
    ));
    builder.set_auth_reason(report.authentication().reason().unwrap_or(""));
    builder.set_account_email(report.account_email().unwrap_or(""));
    match report.quota_surface() {
        None => builder.reborrow().init_quota_surface().set_absent(()),
        Some(surface) => builder
            .reborrow()
            .init_quota_surface()
            .set_present(encode_quota_surface(surface)),
    }
    builder.set_failure(report.failure().unwrap_or(""));
    let mut readiness = builder.reborrow().init_readiness();
    readiness.set_verdict(match report.readiness().verdict() {
        EngineReadinessVerdict::NotReady => artisan_capnp::EngineReadinessVerdict::NotReady,
        EngineReadinessVerdict::Ready => artisan_capnp::EngineReadinessVerdict::Ready,
        EngineReadinessVerdict::NeedsSignIn => artisan_capnp::EngineReadinessVerdict::NeedsSignIn,
        EngineReadinessVerdict::Checking => artisan_capnp::EngineReadinessVerdict::Checking,
    });
    readiness.set_reason(report.readiness().reason().unwrap_or(""));
    let mut windows = builder.init_windows(list_length(
        "response.accountUsage.windows",
        report.windows().len(),
    )?);
    for (index, window) in report.windows().iter().enumerate() {
        encode_engine_usage_window(
            windows
                .reborrow()
                .get(list_index("response.accountUsage.windows", index)?),
            window,
        );
    }
    Ok(())
}

pub(crate) fn encode_engine_usage_snapshot(
    mut builder: artisan_capnp::engine_usage_snapshot::Builder<'_>,
    snapshot: &EngineUsageSnapshot,
) -> Result<(), ProtocolEncodeError> {
    builder.set_fetched_at(snapshot.fetched_at());
    let mut engines = builder.init_engines(list_length(
        "response.accountUsage.engines",
        snapshot.engines().len(),
    )?);
    for (index, engine) in snapshot.engines().iter().enumerate() {
        encode_engine_usage_report(
            engines
                .reborrow()
                .get(list_index("response.accountUsage.engines", index)?),
            engine,
        )?;
    }
    Ok(())
}

pub(crate) fn decode_engine_usage_authentication(
    state: artisan_capnp::EngineUsageAuthentication,
) -> EngineUsageAuthentication {
    match state {
        artisan_capnp::EngineUsageAuthentication::Authenticated => {
            EngineUsageAuthentication::Authenticated
        }
        artisan_capnp::EngineUsageAuthentication::Unauthenticated => {
            EngineUsageAuthentication::Unauthenticated
        }
        artisan_capnp::EngineUsageAuthentication::Unknown => EngineUsageAuthentication::Unknown,
    }
}

pub(crate) fn decode_quota_surface(surface: artisan_capnp::QuotaSurface) -> QuotaSurface {
    match surface {
        artisan_capnp::QuotaSurface::Supported => QuotaSurface::Supported,
        artisan_capnp::QuotaSurface::Unknown => QuotaSurface::Unknown,
        artisan_capnp::QuotaSurface::Unsupported => QuotaSurface::Unsupported,
    }
}

pub(crate) fn optional_wire_text(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

pub(crate) fn decode_engine_usage_window(
    window: engine_usage_window::Reader<'_>,
) -> Result<EngineUsageWindow, ProtocolDecodeError> {
    let window_minutes = match window.get_window_minutes() {
        0 => None,
        minutes => Some(minutes),
    };
    Ok(EngineUsageWindow::new(
        read_text(window.get_id(), "response.accountUsage.window.id")?,
        decode_engine_usage_window_kind(window.get_kind()?),
        optional_wire_text(read_text(
            window.get_label(),
            "response.accountUsage.window.label",
        )?),
        window.get_percent_used(),
        optional_wire_text(read_text(
            window.get_resets_at(),
            "response.accountUsage.window.resetsAt",
        )?),
        window_minutes,
    )?)
}

pub(crate) fn decode_engine_usage_report(
    report: engine_usage_report::Reader<'_>,
) -> Result<EngineUsageReport, ProtocolDecodeError> {
    let authentication = EngineUsageAuth::new(
        decode_engine_usage_authentication(report.get_authentication()?),
        optional_wire_text(read_text(
            report.get_auth_reason(),
            "response.accountUsage.authReason",
        )?),
    )?;
    let quota_surface = match report.get_quota_surface().which()? {
        engine_usage_report::quota_surface::Which::Absent(()) => None,
        engine_usage_report::quota_surface::Which::Present(surface) => {
            Some(decode_quota_surface(surface?))
        }
    };
    let encoded_windows = report.get_windows()?;
    let count = encoded_windows.len() as usize;
    if count > ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE {
        return Err(EngineUsageError::TooManyWindows {
            count,
            maximum: ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE,
        }
        .into());
    }
    let mut windows = Vec::with_capacity(count);
    for encoded_window in encoded_windows {
        windows.push(decode_engine_usage_window(encoded_window)?);
    }
    Ok(EngineUsageReport::new(
        optional_wire_text(read_text(
            report.get_account_email(),
            "response.accountUsage.accountEmail",
        )?),
        authentication,
        read_text(
            report.get_display_name(),
            "response.accountUsage.displayName",
        )?,
        read_text(report.get_engine_id(), "response.accountUsage.engineId")?,
        optional_wire_text(read_text(
            report.get_failure(),
            "response.accountUsage.failure",
        )?),
        quota_surface,
        windows,
    )?
    .with_readiness(decode_engine_readiness(report)?))
}

fn decode_engine_readiness(
    report: engine_usage_report::Reader<'_>,
) -> Result<EngineReadiness, ProtocolDecodeError> {
    if !report.has_readiness() {
        return Ok(EngineReadiness::not_ready());
    }
    let readiness = report.get_readiness()?;
    let verdict = match readiness.get_verdict()? {
        artisan_capnp::EngineReadinessVerdict::NotReady => EngineReadinessVerdict::NotReady,
        artisan_capnp::EngineReadinessVerdict::Ready => EngineReadinessVerdict::Ready,
        artisan_capnp::EngineReadinessVerdict::NeedsSignIn => EngineReadinessVerdict::NeedsSignIn,
        artisan_capnp::EngineReadinessVerdict::Checking => EngineReadinessVerdict::Checking,
    };
    let reason = optional_wire_text(read_text(
        readiness.get_reason(),
        "response.accountUsage.readiness.reason",
    )?);
    Ok(EngineReadiness::new(verdict, reason)?)
}

pub(crate) fn decode_engine_usage_snapshot(
    value: engine_usage_snapshot::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let fetched_at = read_text(value.get_fetched_at(), "response.accountUsage.fetchedAt")?;
    let encoded_engines = value.get_engines()?;
    let count = encoded_engines.len() as usize;
    if count > ENGINE_USAGE_ENGINES_MAX {
        return Err(EngineUsageError::TooManyEngines {
            count,
            maximum: ENGINE_USAGE_ENGINES_MAX,
        }
        .into());
    }
    let mut engines = Vec::with_capacity(count);
    for encoded_engine in encoded_engines {
        engines.push(decode_engine_usage_report(encoded_engine)?);
    }
    Ok(ResponsePayload::AccountUsage(EngineUsageSnapshot::new(
        engines, fetched_at,
    )?))
}
