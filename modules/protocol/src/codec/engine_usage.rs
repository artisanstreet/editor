//! Engine account-usage snapshot encode.

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
