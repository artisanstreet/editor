//! Focused parity tests for dependency-free startup-failure classification.

#![allow(dead_code)]

#[path = "../../modules/backend/src/agent_orchestrator_start_failure_policy.rs"]
mod agent_orchestrator_start_failure_policy;

use agent_orchestrator_start_failure_policy::{
    ENGINE_START_FAILED_ARTISAN_CODE, ENGINE_UNAVAILABLE_ARTISAN_CODE, ENGINE_UNAVAILABLE_TAG,
    GENERIC_START_FAILURE_MESSAGE, INTERRUPTED_START_FAILURE_MESSAGE, MAX_DETAIL_UTF16_CODE_UNITS,
    MAX_TAG_LENGTH, StartFailure, StartFailureCauseObservation, StartFailureKind,
    TaggedFailureObservation, classify_tagged_failure, start_failure_from_cause,
};

fn tagged(
    tag: &str,
    message: Option<&str>,
    artisan_code: Option<&str>,
) -> TaggedFailureObservation {
    TaggedFailureObservation::new(
        tag,
        message.map(str::to_owned),
        artisan_code.map(str::to_owned),
    )
}

fn failure(diagnostic: &str, tagged_failure: Option<TaggedFailureObservation>) -> StartFailure {
    start_failure_from_cause(StartFailureCauseObservation::failed(
        diagnostic,
        tagged_failure,
    ))
}

fn error_ref(result: &StartFailure) -> &agent_orchestrator_start_failure_policy::EngineErrorRef {
    result
        .error_ref
        .as_ref()
        .expect("non-interrupt error reference")
}

#[test]
fn startup_kind_vocabulary_is_exhaustive_and_exact() {
    let expected = [
        (StartFailureKind::Configuration, "configuration"),
        (StartFailureKind::EngineError, "engine_error"),
        (StartFailureKind::Interrupted, "interrupted"),
        (StartFailureKind::Timeout, "timeout"),
    ];

    assert_eq!(
        StartFailureKind::ALL.as_slice(),
        &expected.map(|(kind, _)| kind)
    );
    for (kind, spelling) in expected {
        assert_eq!(kind.as_str(), spelling);
        assert_eq!(kind.to_string(), spelling);
    }
}

#[test]
fn unclassified_failure_uses_generic_code_and_retains_diagnostic() {
    let result = failure("pretty cause with a private path", None);

    assert_eq!(result.kind, StartFailureKind::EngineError);
    assert_eq!(result.message, GENERIC_START_FAILURE_MESSAGE);
    assert_eq!(
        result.diagnostic.as_deref(),
        Some("pretty cause with a private path")
    );
    assert_eq!(
        error_ref(&result),
        &agent_orchestrator_start_failure_policy::EngineErrorRef {
            artisan_code: ENGINE_START_FAILED_ARTISAN_CODE.to_owned(),
            detail: None,
            provider_code: None,
        }
    );
}

#[test]
fn valid_tags_preserve_exact_provider_code_and_classified_message() {
    let result = failure(
        "cause",
        Some(tagged(
            "Adapter.Failure-2",
            Some("  first\t second\n"),
            None,
        )),
    );

    assert_eq!(
        result.message,
        "Engine startup failed before the native session became ready (Adapter.Failure-2: first second)."
    );
    assert_eq!(
        error_ref(&result).artisan_code,
        ENGINE_START_FAILED_ARTISAN_CODE
    );
    assert_eq!(
        error_ref(&result).provider_code.as_deref(),
        Some("Adapter.Failure-2")
    );
    assert_eq!(error_ref(&result).detail.as_deref(), Some("first second"));
}

#[test]
fn tag_grammar_accepts_all_edges_and_rejects_rewrites() {
    let valid = [
        "A",
        "z",
        "A0",
        "A_B.c-9",
        &format!("A{}", "x".repeat(MAX_TAG_LENGTH - 1)),
    ];
    for tag in valid {
        let classification = classify_tagged_failure(&tagged(tag, None, None))
            .expect("tag should match the source grammar");
        assert_eq!(classification.name, tag);
    }

    let too_long = format!("A{}", "x".repeat(MAX_TAG_LENGTH));
    let invalid = [
        "",
        "0A",
        "_A",
        "-A",
        ".A",
        "A B",
        "A/B",
        "A\n",
        "Ä",
        too_long.as_str(),
    ];
    for tag in invalid {
        assert_eq!(
            classify_tagged_failure(&tagged(tag, Some("detail"), Some("adapter-code"))),
            None,
            "tag={tag:?}"
        );
        let result = failure(
            "retained diagnostic",
            Some(tagged(tag, Some("detail"), Some("adapter-code"))),
        );
        assert_eq!(result.message, GENERIC_START_FAILURE_MESSAGE);
        assert_eq!(
            error_ref(&result).artisan_code,
            ENGINE_START_FAILED_ARTISAN_CODE
        );
        assert_eq!(error_ref(&result).provider_code, None);
        assert_eq!(error_ref(&result).detail, None);
        assert_eq!(result.diagnostic.as_deref(), Some("retained diagnostic"));
    }
}

#[test]
fn empty_and_whitespace_only_messages_have_no_detail_or_colon() {
    for message in [None, Some(""), Some(" \t\n\u{feff}\u{3000}")] {
        let result = failure("cause", Some(tagged("Failure", message, None)));
        assert_eq!(
            result.message,
            "Engine startup failed before the native session became ready (Failure)."
        );
        assert_eq!(error_ref(&result).detail, None);
        assert_eq!(error_ref(&result).provider_code.as_deref(), Some("Failure"));
    }
}

#[test]
fn javascript_whitespace_is_collapsed_and_trimmed() {
    let message = "\u{feff}\u{2003}alpha\u{000b}\u{2028}\u{3000}beta\u{00a0}\u{2029}";
    let result = failure("cause", Some(tagged("Whitespace", Some(message), None)));

    assert_eq!(error_ref(&result).detail.as_deref(), Some("alpha beta"));
    assert_eq!(
        result.message,
        "Engine startup failed before the native session became ready (Whitespace: alpha beta)."
    );
}

#[test]
fn detail_is_bounded_after_normalization_by_utf16_units() {
    let exact = "x".repeat(MAX_DETAIL_UTF16_CODE_UNITS);
    let over = "x".repeat(MAX_DETAIL_UTF16_CODE_UNITS + 1);

    let exact_result = failure("cause", Some(tagged("Bound", Some(&exact), None)));
    let over_result = failure("cause", Some(tagged("Bound", Some(&over), None)));

    assert_eq!(
        error_ref(&exact_result).detail.as_deref(),
        Some(exact.as_str())
    );
    assert_eq!(
        error_ref(&over_result).detail.as_deref(),
        Some(exact.as_str())
    );
    assert_eq!(
        error_ref(&over_result)
            .detail
            .as_deref()
            .expect("bounded detail")
            .encode_utf16()
            .count(),
        MAX_DETAIL_UTF16_CODE_UNITS
    );
}

#[test]
fn detail_cutoff_preserves_previously_collapsed_boundary_without_retrimming() {
    let message = format!("{} \t tail", "x".repeat(MAX_DETAIL_UTF16_CODE_UNITS - 1));
    let result = failure("cause", Some(tagged("Boundary", Some(&message), None)));
    let detail = error_ref(&result).detail.as_deref().expect("detail");

    assert_eq!(detail.len(), MAX_DETAIL_UTF16_CODE_UNITS);
    assert!(detail.ends_with(' '));
}

#[test]
fn supplementary_scalars_are_counted_as_two_utf16_units_without_replacement() {
    let exact = "🦀".repeat(MAX_DETAIL_UTF16_CODE_UNITS / 2);
    let over = format!("{exact}🦀");
    let edge = format!("{}🦀", "x".repeat(MAX_DETAIL_UTF16_CODE_UNITS - 1));

    let exact_result = failure("cause", Some(tagged("Unicode", Some(&exact), None)));
    let over_result = failure("cause", Some(tagged("Unicode", Some(&over), None)));
    let edge_result = failure("cause", Some(tagged("Unicode", Some(&edge), None)));

    assert_eq!(
        error_ref(&exact_result).detail.as_deref(),
        Some(exact.as_str())
    );
    assert_eq!(
        error_ref(&over_result).detail.as_deref(),
        Some(exact.as_str())
    );
    assert_eq!(
        error_ref(&edge_result).detail.as_deref(),
        Some("x".repeat(MAX_DETAIL_UTF16_CODE_UNITS - 1).as_str())
    );
    for result in [exact_result, over_result, edge_result] {
        let detail = error_ref(&result).detail.as_deref().expect("detail");
        assert!(detail.encode_utf16().count() <= MAX_DETAIL_UTF16_CODE_UNITS);
        assert!(!detail.contains('\u{fffd}'));
    }
}

#[test]
fn adapter_code_wins_over_both_fallbacks_verbatim() {
    let custom = failure(
        "cause",
        Some(tagged(
            ENGINE_UNAVAILABLE_TAG,
            Some("detail"),
            Some("AE-CUSTOM-999"),
        )),
    );
    let empty = failure(
        "cause",
        Some(tagged(ENGINE_UNAVAILABLE_TAG, None, Some(""))),
    );

    assert_eq!(error_ref(&custom).artisan_code, "AE-CUSTOM-999");
    assert_eq!(error_ref(&empty).artisan_code, "");
}

#[test]
fn engine_unavailable_tag_has_special_fallback_and_other_tags_do_not() {
    let unavailable = failure("cause", Some(tagged(ENGINE_UNAVAILABLE_TAG, None, None)));
    let generic = failure("cause", Some(tagged("OtherFailure", None, None)));

    assert_eq!(
        error_ref(&unavailable).artisan_code,
        ENGINE_UNAVAILABLE_ARTISAN_CODE
    );
    assert_eq!(
        error_ref(&generic).artisan_code,
        ENGINE_START_FAILED_ARTISAN_CODE
    );
    assert_eq!(
        error_ref(&unavailable).provider_code.as_deref(),
        Some(ENGINE_UNAVAILABLE_TAG)
    );
    assert_eq!(
        error_ref(&generic).provider_code.as_deref(),
        Some("OtherFailure")
    );
}

#[test]
fn interruption_precedes_tag_detail_and_diagnostic() {
    let observation = StartFailureCauseObservation::new(
        true,
        "diagnostic that must not escape",
        Some(tagged(
            ENGINE_UNAVAILABLE_TAG,
            Some("provider detail"),
            Some("AE-CUSTOM-999"),
        )),
    );
    let result = start_failure_from_cause(&observation);

    assert_eq!(result.kind, StartFailureKind::Interrupted);
    assert!(result.is_interrupted());
    assert_eq!(result.message, INTERRUPTED_START_FAILURE_MESSAGE);
    assert_eq!(result.diagnostic, None);
    assert_eq!(result.error_ref, None);
}

#[test]
fn non_interrupt_result_retains_empty_diagnostic_exactly() {
    let result = failure("", Some(tagged("Failure", None, None)));

    assert_eq!(result.diagnostic, Some(String::new()));
    assert_eq!(result.kind, StartFailureKind::EngineError);
}

#[test]
fn convenience_constructors_preserve_typed_observations() {
    let interrupted = StartFailureCauseObservation::interrupted();
    assert!(interrupted.interrupts_only);
    assert_eq!(interrupted.diagnostic, "");
    assert_eq!(interrupted.tagged_failure, None);

    let tagged_failure = TaggedFailureObservation::tag_only("Tag")
        .with_message("message")
        .with_artisan_code("code");
    let failed = StartFailureCauseObservation::failed("diagnostic", Some(tagged_failure.clone()));
    assert!(!failed.interrupts_only);
    assert_eq!(failed.diagnostic, "diagnostic");
    assert_eq!(failed.tagged_failure, Some(tagged_failure));
}
