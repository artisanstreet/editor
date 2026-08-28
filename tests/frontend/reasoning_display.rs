//! Exhaustive parity and presentation-policy coverage for the native
//! reasoning-display leaf (`modules/frontend/src/reasoning_display.rs`),
//! ported from `modules/frontend/src/lib/engine/reasoning-display.ts` and
//! `modules/catalog/src/schema.ts`.

use artisan_frontend::reasoning_display::{
    PolicyView, ReasoningDisplay, model_reasoning_display, policy_reasoning_display,
    should_show_live_summary,
};
use std::str::FromStr;

const ALL_CANONICAL: [(&str, ReasoningDisplay); 2] = [
    ("summary", ReasoningDisplay::Summary),
    ("trace", ReasoningDisplay::Trace),
];

#[test]
fn every_recognized_value_round_trips_through_canonical_string() {
    for (canonical, display) in ALL_CANONICAL {
        assert_eq!(display.as_str(), canonical);
        assert_eq!(display.label(), canonical);
        assert_eq!(display.to_string(), canonical);
        assert_eq!(
            ReasoningDisplay::from_canonical_str(canonical),
            Some(display)
        );
        assert_eq!(ReasoningDisplay::from_str(canonical).unwrap(), display);
        assert_eq!(format!("{display}"), canonical);
    }
    assert_eq!(
        ReasoningDisplay::from_str("summary").unwrap(),
        ReasoningDisplay::Summary
    );
    assert_eq!(
        ReasoningDisplay::from_str("trace").unwrap(),
        ReasoningDisplay::Trace
    );
}

#[test]
fn all_contains_exactly_the_recognized_values_in_canonical_order() {
    assert_eq!(
        ReasoningDisplay::ALL,
        [ReasoningDisplay::Summary, ReasoningDisplay::Trace]
    );
    assert_eq!(ReasoningDisplay::ALL.len(), ALL_CANONICAL.len());
    for (index, (_, display)) in ALL_CANONICAL.iter().enumerate() {
        assert_eq!(&ReasoningDisplay::ALL[index], display);
    }
    // No duplicates.
    assert_ne!(ReasoningDisplay::Summary, ReasoningDisplay::Trace);
    assert_ne!(
        ReasoningDisplay::Summary.as_str(),
        ReasoningDisplay::Trace.as_str()
    );
}

#[test]
fn default_is_summary_and_rank_reflects_default() {
    assert_eq!(ReasoningDisplay::default(), ReasoningDisplay::Summary);
    assert_eq!(ReasoningDisplay::Summary.rank(), 0);
    assert_eq!(ReasoningDisplay::Trace.rank(), 1);
    assert!(ReasoningDisplay::Summary.rank() < ReasoningDisplay::Trace.rank());
}

#[test]
fn exact_display_strings_are_canonical_and_distinct() {
    assert_eq!(ReasoningDisplay::Summary.as_str(), "summary");
    assert_eq!(ReasoningDisplay::Trace.as_str(), "trace");
    assert_eq!(ReasoningDisplay::Summary.label(), "summary");
    assert_eq!(ReasoningDisplay::Trace.label(), "trace");
    assert_ne!(
        ReasoningDisplay::Summary.label(),
        ReasoningDisplay::Trace.label()
    );
    // Descriptions are distinct and non-empty.
    assert!(!ReasoningDisplay::Summary.description().is_empty());
    assert!(!ReasoningDisplay::Trace.description().is_empty());
    assert_ne!(
        ReasoningDisplay::Summary.description(),
        ReasoningDisplay::Trace.description()
    );
}

#[test]
fn canonical_parser_is_exact_and_rejects_aliases_and_casing() {
    // Exact parser rejects every casing and whitespace variant.
    let rejected = [
        "Summary",
        "SUMMARY",
        "sUmMaRy",
        "Trace",
        "TRACE",
        "tRaCe",
        " summary",
        "summary ",
        " summary ",
        "\ttrace\n",
        "",
        " ",
        "summaries",
        "traces",
        "summery",
        "tracing",
    ];
    for input in rejected {
        assert_eq!(
            ReasoningDisplay::from_canonical_str(input),
            None,
            "exact parser must reject '{input}'"
        );
        assert!(
            ReasoningDisplay::from_str(input).is_err(),
            "FromStr must reject '{input}'"
        );
    }
}

#[test]
fn normalized_parser_accepts_trimmed_ascii_case_variants() {
    let cases: [(&str, ReasoningDisplay); 10] = [
        ("summary", ReasoningDisplay::Summary),
        ("Summary", ReasoningDisplay::Summary),
        ("SUMMARY", ReasoningDisplay::Summary),
        ("  summary  ", ReasoningDisplay::Summary),
        ("\tSummary\n", ReasoningDisplay::Summary),
        ("trace", ReasoningDisplay::Trace),
        ("Trace", ReasoningDisplay::Trace),
        ("TRACE", ReasoningDisplay::Trace),
        ("  trace ", ReasoningDisplay::Trace),
        ("\nTRACE\t", ReasoningDisplay::Trace),
    ];
    for (input, expected) in cases {
        assert_eq!(
            ReasoningDisplay::parse_normalized(input),
            Some(expected),
            "normalized parser failed for '{input}'"
        );
    }
    // Normalized parser still rejects unknowns, even with trimming/casing.
    for input in [
        "", " ", "unknown", "summery", "tracing", "summary!", "trace!",
    ] {
        assert_eq!(
            ReasoningDisplay::parse_normalized(input),
            None,
            "normalized parser must reject '{input}'"
        );
    }
}

#[test]
fn unknown_canonical_inputs_produce_typed_error_preserving_input() {
    let err = ReasoningDisplay::from_str("unknown").unwrap_err();
    assert_eq!(err.to_string(), "unknown reasoning display 'unknown'");
    let err2 = ReasoningDisplay::from_str("SUMMARY").unwrap_err();
    assert_eq!(err2.to_string(), "unknown reasoning display 'SUMMARY'");
    assert_ne!(err, err2);
}

#[test]
fn model_reasoning_display_defaults_absent_to_summary() {
    assert_eq!(model_reasoning_display(None), ReasoningDisplay::Summary);
    assert_eq!(
        model_reasoning_display(Some(ReasoningDisplay::Summary)),
        ReasoningDisplay::Summary
    );
    assert_eq!(
        model_reasoning_display(Some(ReasoningDisplay::Trace)),
        ReasoningDisplay::Trace
    );
}

#[test]
fn policy_reasoning_display_absent_policy_is_summary() {
    assert_eq!(
        policy_reasoning_display(None, None),
        ReasoningDisplay::Summary
    );
    assert_eq!(
        policy_reasoning_display(None, Some(ReasoningDisplay::Trace)),
        ReasoningDisplay::Summary
    );
    assert_eq!(
        policy_reasoning_display(None, Some(ReasoningDisplay::Summary)),
        ReasoningDisplay::Summary
    );
}

#[test]
fn policy_reasoning_display_absent_model_is_summary_even_with_resolved() {
    let policy = Some(PolicyView::new(Some("codex"), None));
    assert_eq!(
        policy_reasoning_display(policy, None),
        ReasoningDisplay::Summary
    );
    assert_eq!(
        policy_reasoning_display(policy, Some(ReasoningDisplay::Trace)),
        ReasoningDisplay::Summary
    );
    let policy_no_engine = Some(PolicyView::new(None, None));
    assert_eq!(
        policy_reasoning_display(policy_no_engine, Some(ReasoningDisplay::Trace)),
        ReasoningDisplay::Summary
    );
}

#[test]
fn policy_reasoning_display_unresolvable_model_is_summary() {
    let policy = Some(PolicyView::new(Some("codex"), Some("unknown-model")));
    assert_eq!(
        policy_reasoning_display(policy, None),
        ReasoningDisplay::Summary
    );
    let policy2 = Some(PolicyView::new(Some("cursor"), Some("glm-5.2-unknown")));
    assert_eq!(
        policy_reasoning_display(policy2, None),
        ReasoningDisplay::Summary
    );
}

#[test]
fn policy_reasoning_display_resolved_value_is_returned_verbatim() {
    let summary_policy = Some(PolicyView::new(Some("codex"), Some("gpt-5.6-sol")));
    assert_eq!(
        policy_reasoning_display(summary_policy, Some(ReasoningDisplay::Summary)),
        ReasoningDisplay::Summary
    );
    let trace_policy = Some(PolicyView::new(Some("cursor"), Some("kimi-k3")));
    assert_eq!(
        policy_reasoning_display(trace_policy, Some(ReasoningDisplay::Trace)),
        ReasoningDisplay::Trace
    );
    // Engine id does not change the pure policy: model presence + resolved wins.
    let no_engine = Some(PolicyView::new(None, Some("kimi-k3")));
    assert_eq!(
        policy_reasoning_display(no_engine, Some(ReasoningDisplay::Trace)),
        ReasoningDisplay::Trace
    );
}

#[test]
fn policy_selection_and_availability_parity_with_ts_leaf() {
    // Table mirrors TS: policy?.model === undefined => summary,
    // definition === undefined => summary, else model_reasoning_display(def).
    let cases: [(
        Option<PolicyView<'_>>,
        Option<ReasoningDisplay>,
        ReasoningDisplay,
    ); 7] = [
        (None, None, ReasoningDisplay::Summary),
        (
            None,
            Some(ReasoningDisplay::Trace),
            ReasoningDisplay::Summary,
        ),
        (
            Some(PolicyView::new(Some("codex"), None)),
            Some(ReasoningDisplay::Trace),
            ReasoningDisplay::Summary,
        ),
        (
            Some(PolicyView::new(Some("codex"), Some("gpt-5.6-sol"))),
            None,
            ReasoningDisplay::Summary,
        ),
        (
            Some(PolicyView::new(Some("codex"), Some("gpt-5.6-sol"))),
            Some(ReasoningDisplay::Summary),
            ReasoningDisplay::Summary,
        ),
        (
            Some(PolicyView::new(Some("cursor"), Some("kimi-k3"))),
            Some(ReasoningDisplay::Trace),
            ReasoningDisplay::Trace,
        ),
        (
            Some(PolicyView::new(Some("cursor"), Some("glm-5.2"))),
            Some(ReasoningDisplay::Trace),
            ReasoningDisplay::Trace,
        ),
    ];
    for (policy, resolved, expected) in cases {
        assert_eq!(
            policy_reasoning_display(policy, resolved),
            expected,
            "policy={policy:?} resolved={resolved:?}"
        );
    }
}

#[test]
fn live_summary_visibility_matches_thread_workspace_branch() {
    assert!(should_show_live_summary(ReasoningDisplay::Summary));
    assert!(!should_show_live_summary(ReasoningDisplay::Trace));
    assert!(ReasoningDisplay::Summary.can_show_live_summary());
    assert!(!ReasoningDisplay::Trace.can_show_live_summary());
}

#[test]
fn is_helpers_are_mutually_exclusive_and_exhaustive() {
    for display in ReasoningDisplay::ALL {
        assert_ne!(display.is_summary(), display.is_trace());
        assert!(display.is_summary() || display.is_trace());
        assert_eq!(display.is_summary(), display == ReasoningDisplay::Summary);
        assert_eq!(display.is_trace(), display == ReasoningDisplay::Trace);
    }
}

#[test]
fn no_fast_effort_distinction_leaks_into_reasoning_display() {
    // The leaf must stay pure presentation policy: it knows summary vs trace,
    // not speed tiers or thinking effort. Verify the API surface contains no
    // speed/effort naming and the two variants remain exactly the catalog pair.
    let variants = [ReasoningDisplay::Summary, ReasoningDisplay::Trace];
    assert_eq!(variants.len(), 2);
    for variant in variants {
        let debug = format!("{variant:?}");
        assert!(
            !debug.to_ascii_lowercase().contains("fast"),
            "debug must not mention fast: {debug}"
        );
        assert!(
            !debug.to_ascii_lowercase().contains("effort"),
            "debug must not mention effort: {debug}"
        );
        let desc = variant.description().to_ascii_lowercase();
        assert!(!desc.contains("fast"), "description must not mention fast");
    }
}
