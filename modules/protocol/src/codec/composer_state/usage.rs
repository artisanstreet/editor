//! Recalled run-usage results, reports, and their bound encoding.

#![forbid(unsafe_code)]

use super::helpers::*;
use super::*;
/// Encodes a run-usage result with explicit optional wrappers for all
/// optional text and numeric fields.
pub fn encode_run_usage_result(
    mut builder: composer_state_capnp::run_usage_result::Builder<'_>,
    value: &RunUsageResult,
) -> Result<(), ComposerStateCodecError> {
    if let Some(report) = &value.report
        && (report.thread_id() != &value.thread_id || report.run_id() != &value.run_id)
    {
        return Err(ComposerStateCodecError::StateValue {
            field: "response.runUsage.report",
        });
    }
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_run_id(value.run_id.as_str());
    if let Some(report) = &value.report {
        encode_run_usage_report(builder.reborrow().init_report(), report);
    }
    Ok(())
}

/// Decodes a run-usage result and preserves `None` separately from
/// `Some(0)` in every optional numeric field.
pub fn decode_run_usage_result(
    value: composer_state_capnp::run_usage_result::Reader<'_>,
) -> Result<RunUsageResult, ComposerStateCodecError> {
    let thread_id = parse_thread_id(
        read_text(value.get_thread_id(), "response.runUsage.threadId")?,
        "response.runUsage.threadId",
    )?;
    let run_id = parse_run_id(
        read_text(value.get_run_id(), "response.runUsage.runId")?,
        "response.runUsage.runId",
    )?;
    let report = if value.has_report() {
        Some(decode_run_usage_report(
            value.get_report()?,
            &thread_id,
            &run_id,
        )?)
    } else {
        None
    };
    RunUsageResult::new(thread_id, run_id, report).map_err(|_| {
        ComposerStateCodecError::StateValue {
            field: "response.runUsage.report",
        }
    })
}

/// Validates a usage result against the exact authenticated read scope.
pub fn validate_run_usage_scope(
    query: &ReadRunUsage,
    result: &RunUsageResult,
) -> Result<(), ComposerStateCodecError> {
    if query.thread_id != result.thread_id {
        return Err(ComposerStateCodecError::ScopeMismatch {
            field: "runUsage.threadId",
        });
    }
    if query.run_id != result.run_id {
        return Err(ComposerStateCodecError::ScopeMismatch {
            field: "runUsage.runId",
        });
    }
    Ok(())
}

fn encode_run_usage_report(
    mut builder: composer_state_capnp::run_usage_report::Builder<'_>,
    value: &RunUsageReport,
) {
    builder.set_provider_session_id(value.provider_session_id());
    builder.set_source_sequence(value.source_sequence());
    builder.set_model_id(value.model_id().as_str());
    builder.set_provider_route_id(value.provider_route_id().as_str());
    encode_optional_text(
        builder.reborrow().init_variant_id(),
        value.variant_id().map(EngineVariantId::as_str),
    );
    builder.set_basis(encode_usage_basis(value.basis()));
    encode_optional_text(
        builder.reborrow().init_provider_turn_id(),
        value.provider_turn_id(),
    );
    encode_optional_u64(builder.reborrow().init_input_tokens(), value.input_tokens());
    encode_optional_u64(
        builder.reborrow().init_cached_input_tokens(),
        value.cached_input_tokens(),
    );
    encode_optional_u64(
        builder.reborrow().init_output_tokens(),
        value.output_tokens(),
    );
    encode_optional_u64(
        builder.reborrow().init_context_tokens(),
        value.context_tokens(),
    );
    encode_optional_u64(
        builder.reborrow().init_context_window_tokens(),
        value.context_window_tokens(),
    );
    builder.set_observed_at_millis(value.observed_at().as_millis());
}

fn decode_run_usage_report(
    value: composer_state_capnp::run_usage_report::Reader<'_>,
    thread_id: &ThreadId,
    run_id: &RunId,
) -> Result<RunUsageReport, ComposerStateCodecError> {
    let provider_session_id = read_text(
        value.get_provider_session_id(),
        "response.runUsage.report.providerSessionId",
    )?;
    let source_sequence = value.get_source_sequence();
    let model_id = parse_model_id(
        read_text(value.get_model_id(), "response.runUsage.report.modelId")?,
        "response.runUsage.report.modelId",
    )?;
    let provider_route_id = parse_route_id(
        read_text(
            value.get_provider_route_id(),
            "response.runUsage.report.providerRouteId",
        )?,
        "response.runUsage.report.providerRouteId",
    )?;
    let variant_id = match decode_optional_text(
        value.get_variant_id()?,
        "response.runUsage.report.variantId",
    )? {
        Some(value) => Some(parse_variant_id(
            value,
            "response.runUsage.report.variantId",
        )?),
        None => None,
    };
    let basis = decode_usage_basis(value.get_basis(), "response.runUsage.report.basis")?;
    let provider_turn_id = decode_optional_text(
        value.get_provider_turn_id()?,
        "response.runUsage.report.providerTurnId",
    )?;
    let input_tokens = decode_optional_u64(
        value.get_input_tokens()?,
        "response.runUsage.report.inputTokens",
    )?;
    let cached_input_tokens = decode_optional_u64(
        value.get_cached_input_tokens()?,
        "response.runUsage.report.cachedInputTokens",
    )?;
    let output_tokens = decode_optional_u64(
        value.get_output_tokens()?,
        "response.runUsage.report.outputTokens",
    )?;
    let context_tokens = decode_optional_u64(
        value.get_context_tokens()?,
        "response.runUsage.report.contextTokens",
    )?;
    let context_window_tokens = decode_optional_u64(
        value.get_context_window_tokens()?,
        "response.runUsage.report.contextWindowTokens",
    )?;
    RunUsageReport::new(RunUsageReportInput {
        run_id: run_id.clone(),
        thread_id: thread_id.clone(),
        provider_session_id,
        source_sequence,
        model_id,
        provider_route_id,
        variant_id,
        basis,
        provider_turn_id,
        input_tokens,
        cached_input_tokens,
        output_tokens,
        context_tokens,
        context_window_tokens,
        observed_at: UnixMillis::from_millis(value.get_observed_at_millis()),
    })
    .map_err(|_| ComposerStateCodecError::Usage {
        field: "response.runUsage.report",
    })
}

fn encode_usage_basis(value: RunUsageBasis) -> composer_state_capnp::RunUsageBasis {
    match value {
        RunUsageBasis::Delta => composer_state_capnp::RunUsageBasis::Delta,
        RunUsageBasis::Cumulative => composer_state_capnp::RunUsageBasis::Cumulative,
        RunUsageBasis::Unknown => composer_state_capnp::RunUsageBasis::Unknown,
    }
}

fn decode_usage_basis(
    value: Result<composer_state_capnp::RunUsageBasis, capnp::NotInSchema>,
    field: &'static str,
) -> Result<RunUsageBasis, ComposerStateCodecError> {
    match value.map_err(|source| ComposerStateCodecError::UnknownEnum {
        field,
        value: source.0,
    })? {
        composer_state_capnp::RunUsageBasis::Delta => Ok(RunUsageBasis::Delta),
        composer_state_capnp::RunUsageBasis::Cumulative => Ok(RunUsageBasis::Cumulative),
        composer_state_capnp::RunUsageBasis::Unknown => Ok(RunUsageBasis::Unknown),
    }
}
