//! Where an engine compacts its context, reported with run usage.
//!
//! Ported from the Editor's `context_auto_compaction` policy (itself the
//! Rust port of `modules/frontend/src/lib/context-usage/auto-compaction.ts`)
//! so the Forge decides the threshold and the Editor only paints it. The
//! threshold belongs to the run that reported the reading: its engine and
//! model, never a thread's next launch policy.

#![forbid(unsafe_code)]

use artisan_domain::RunUsageReport;

/// Codex's documented compaction threshold as a percentage of its window.
const CODEX_COMPACTION_PERCENT: u64 = 90;

/// Claude Sonnet 5's documented default compaction capacity in tokens.
const CLAUDE_SONNET_5_COMPACTION_TOKENS: u64 = 967_000;

/// Returns the context size, in tokens, at which the engine that reported
/// `report` compacts, or `None` when no documented policy applies (the
/// window boundary is then the only limit) or the report carries no window.
///
/// Native engines report their engine id as the provider route, so the route
/// names the engine; a provider route of the managed `OpenCode` engine
/// matches none of them. Model comparisons are exact and case-sensitive.
#[must_use]
pub(crate) fn compaction_at_tokens(report: &RunUsageReport) -> Option<u64> {
    let window = report
        .context_window_tokens()
        .filter(|window| *window > 0)?;
    match report.provider_route_id().as_str() {
        "codex" => Some(window.saturating_mul(CODEX_COMPACTION_PERCENT) / 100),
        "claude" if report.model_id().as_str() == "claude-sonnet-5" => {
            Some(CLAUDE_SONNET_5_COMPACTION_TOKENS.min(window))
        }
        "claude" => Some(window),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use artisan_domain::{
        EngineModelId, EngineRouteId, RunId, RunUsageBasis, RunUsageReportInput, ThreadId,
        UnixMillis,
    };

    use super::*;

    fn report(route: &str, model: &str, window: Option<u64>) -> RunUsageReport {
        RunUsageReport::new(RunUsageReportInput {
            run_id: RunId::parse("run-1").expect("run"),
            thread_id: ThreadId::parse("thread-1").expect("thread"),
            provider_session_id: "session".to_owned(),
            source_sequence: 1,
            model_id: EngineModelId::parse(model).expect("model"),
            provider_route_id: EngineRouteId::parse(route).expect("route"),
            variant_id: None,
            basis: RunUsageBasis::Delta,
            provider_turn_id: None,
            input_tokens: None,
            cached_input_tokens: None,
            output_tokens: None,
            context_tokens: Some(1),
            context_window_tokens: window,
            observed_at: UnixMillis::from_millis(1),
        })
        .expect("report")
    }

    #[test]
    fn codex_compacts_at_ninety_percent_of_its_window() {
        assert_eq!(
            compaction_at_tokens(&report("codex", "gpt-5.6-sol", Some(200_000))),
            Some(180_000)
        );
    }

    #[test]
    fn claude_sonnet_5_caps_its_capacity_and_other_claude_models_use_the_window() {
        assert_eq!(
            compaction_at_tokens(&report("claude", "claude-sonnet-5", Some(1_000_000))),
            Some(967_000)
        );
        assert_eq!(
            compaction_at_tokens(&report("claude", "claude-sonnet-5", Some(200_000))),
            Some(200_000)
        );
        assert_eq!(
            compaction_at_tokens(&report("claude", "claude-fable-5[1m]", Some(1_000_000))),
            Some(1_000_000)
        );
        // The special case is exact: a suffixed or recased id is not it.
        assert_eq!(
            compaction_at_tokens(&report("claude", "claude-sonnet-5[1m]", Some(1_000_000))),
            Some(1_000_000)
        );
    }

    #[test]
    fn undocumented_engines_and_missing_windows_report_no_threshold() {
        assert_eq!(
            compaction_at_tokens(&report("grok", "grok-4.6", Some(200_000))),
            None
        );
        assert_eq!(
            compaction_at_tokens(&report("openrouter", "some-model", Some(200_000))),
            None
        );
        assert_eq!(
            compaction_at_tokens(&report("codex", "gpt-5.6-sol", None)),
            None
        );
    }
}
