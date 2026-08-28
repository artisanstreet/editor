//! Focused tests for the dependency-free usage-interruption model policy.

#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../../modules/backend/src/usage_interruption_model_policy.rs"]
mod usage_interruption_model_policy;

use usage_interruption_model_policy::{
    DecodedUsageInterruptionAlternatives, MAX_USAGE_EVIDENCE_TEXT_UTF16_UNITS,
    UsageInterruptionAlternative, UsageInterruptionDecodeError, UsageInterruptionModelPolicy,
    UsageInterruptionRow, UsageInterruptionSnapshot, decode_usage_interruption_row,
    sanitise_usage_evidence_text, sanitize_usage_evidence_text,
};

fn utf16_units(value: &str) -> usize {
    value.encode_utf16().count()
}

fn empty_row() -> UsageInterruptionRow {
    UsageInterruptionRow {
        affected_model_id: None,
        alternatives: Ok(Vec::new()),
        auto_continue: false,
        cancelled_at: None,
        continuation_command_id: None,
        continued_at: None,
        created_at: "created-required".to_owned(),
        failed_at: None,
        interruption_id: "interruption-required".to_owned(),
        limit_id: None,
        limit_label: None,
        limit_scope: "unknown".to_owned(),
        provider_code: None,
        resets_at: None,
        resume_not_before: None,
        revision: 0,
        source_agent_id: "source-agent-required".to_owned(),
        source_engine_id: "source-engine-required".to_owned(),
        source_model_id: None,
        source_run_id: "source-run-required".to_owned(),
        state: "awaiting_decision".to_owned(),
        target_engine_id: None,
        target_model_id: None,
        target_run_id: None,
        thread_id: "thread-required".to_owned(),
        updated_at: "updated-required".to_owned(),
    }
}

fn alternative() -> UsageInterruptionAlternative {
    UsageInterruptionAlternative::new(
        "Alternative Display",
        "alternative-engine",
        "alternative-model",
        "2026-08-28T12:00:00.000Z",
    )
}

fn present_row() -> UsageInterruptionRow {
    UsageInterruptionRow {
        affected_model_id: Some("affected-model".to_owned()),
        alternatives: Ok(vec![alternative()]),
        auto_continue: true,
        cancelled_at: Some("2026-08-28T12:01:00.000Z".to_owned()),
        continuation_command_id: Some("continue-command".to_owned()),
        continued_at: Some("2026-08-28T12:02:00.000Z".to_owned()),
        created_at: "2026-08-28T12:03:00.000Z".to_owned(),
        failed_at: Some("2026-08-28T12:04:00.000Z".to_owned()),
        interruption_id: "interruption-id".to_owned(),
        limit_id: Some("limit-id".to_owned()),
        limit_label: Some("limit-label".to_owned()),
        limit_scope: "model".to_owned(),
        provider_code: Some("AE-PROVIDER-201".to_owned()),
        resets_at: Some("2026-08-28T12:05:00.000Z".to_owned()),
        resume_not_before: Some("2026-08-28T12:06:00.000Z".to_owned()),
        revision: 27,
        source_agent_id: "source-agent".to_owned(),
        source_engine_id: "source-engine".to_owned(),
        source_model_id: Some("source-model".to_owned()),
        source_run_id: "source-run".to_owned(),
        state: "launching".to_owned(),
        target_engine_id: Some("target-engine".to_owned()),
        target_model_id: Some("target-model".to_owned()),
        target_run_id: Some("target-run".to_owned()),
        thread_id: "thread-id".to_owned(),
        updated_at: "2026-08-28T12:07:00.000Z".to_owned(),
    }
}

#[test]
fn all_null_projection_preserves_required_fields_and_keeps_optionals_absent() {
    let row = empty_row();
    let projected = decode_usage_interruption_row(&row).expect("valid empty alternatives");

    assert_eq!(
        projected,
        UsageInterruptionSnapshot {
            affected_model_id: None,
            alternatives: Vec::new(),
            auto_continue: false,
            cancelled_at: None,
            continuation_command_id: None,
            continued_at: None,
            created_at: "created-required".to_owned(),
            failed_at: None,
            interruption_id: "interruption-required".to_owned(),
            limit_id: None,
            limit_label: None,
            limit_scope: "unknown".to_owned(),
            provider_code: None,
            resets_at: None,
            resume_not_before: None,
            revision: 0,
            source_agent_id: "source-agent-required".to_owned(),
            source_engine_id: "source-engine-required".to_owned(),
            source_model_id: None,
            source_run_id: "source-run-required".to_owned(),
            state: "awaiting_decision".to_owned(),
            target_engine_id: None,
            target_model_id: None,
            target_run_id: None,
            thread_id: "thread-required".to_owned(),
            updated_at: "updated-required".to_owned(),
        }
    );
}

#[test]
fn all_present_projection_custodies_every_field_and_alternative() {
    let row = present_row();
    let expected_alternative = alternative();
    let projected = decode_usage_interruption_row(&row).expect("valid alternatives");

    assert_eq!(
        projected,
        UsageInterruptionSnapshot {
            affected_model_id: Some("affected-model".to_owned()),
            alternatives: vec![expected_alternative],
            auto_continue: true,
            cancelled_at: Some("2026-08-28T12:01:00.000Z".to_owned()),
            continuation_command_id: Some("continue-command".to_owned()),
            continued_at: Some("2026-08-28T12:02:00.000Z".to_owned()),
            created_at: "2026-08-28T12:03:00.000Z".to_owned(),
            failed_at: Some("2026-08-28T12:04:00.000Z".to_owned()),
            interruption_id: "interruption-id".to_owned(),
            limit_id: Some("limit-id".to_owned()),
            limit_label: Some("limit-label".to_owned()),
            limit_scope: "model".to_owned(),
            provider_code: Some("AE-PROVIDER-201".to_owned()),
            resets_at: Some("2026-08-28T12:05:00.000Z".to_owned()),
            resume_not_before: Some("2026-08-28T12:06:00.000Z".to_owned()),
            revision: 27,
            source_agent_id: "source-agent".to_owned(),
            source_engine_id: "source-engine".to_owned(),
            source_model_id: Some("source-model".to_owned()),
            source_run_id: "source-run".to_owned(),
            state: "launching".to_owned(),
            target_engine_id: Some("target-engine".to_owned()),
            target_model_id: Some("target-model".to_owned()),
            target_run_id: Some("target-run".to_owned()),
            thread_id: "thread-id".to_owned(),
            updated_at: "2026-08-28T12:07:00.000Z".to_owned(),
        }
    );
}

#[test]
fn required_fields_are_copied_without_normalization_or_invented_values() {
    let row = UsageInterruptionRow {
        alternatives: Ok(Vec::new()),
        auto_continue: true,
        created_at: "  created with spaces  ".to_owned(),
        interruption_id: "interruption with spaces".to_owned(),
        limit_scope: "provider-specific-scope".to_owned(),
        revision: 9,
        source_agent_id: "agent with spaces".to_owned(),
        source_engine_id: "engine with spaces".to_owned(),
        source_run_id: "source run".to_owned(),
        state: "future-state".to_owned(),
        thread_id: "thread with spaces".to_owned(),
        updated_at: " updated ".to_owned(),
        ..empty_row()
    };
    let projected = decode_usage_interruption_row(&row).expect("valid alternatives");

    assert_eq!(projected.auto_continue, row.auto_continue);
    assert_eq!(projected.created_at, row.created_at);
    assert_eq!(projected.interruption_id, row.interruption_id);
    assert_eq!(projected.limit_scope, row.limit_scope);
    assert_eq!(projected.revision, row.revision);
    assert_eq!(projected.source_agent_id, row.source_agent_id);
    assert_eq!(projected.source_engine_id, row.source_engine_id);
    assert_eq!(projected.source_run_id, row.source_run_id);
    assert_eq!(projected.state, row.state);
    assert_eq!(projected.thread_id, row.thread_id);
    assert_eq!(projected.updated_at, row.updated_at);
}

#[test]
fn malformed_alternatives_are_an_explicit_decode_failure() {
    let row = UsageInterruptionRow {
        alternatives: Err(UsageInterruptionDecodeError::MalformedAlternatives),
        ..empty_row()
    };

    assert_eq!(
        decode_usage_interruption_row(&row),
        Err(UsageInterruptionDecodeError::MalformedAlternatives)
    );
    assert_eq!(
        UsageInterruptionDecodeError::MalformedAlternatives.to_string(),
        "usage interruption alternatives failed to decode"
    );
}

#[test]
fn all_c0_controls_and_del_are_replaced_by_one_space_each() {
    let mut input = String::from("prefix");
    let mut expected = String::from("prefix");
    for code in 0_u32..=0x1F {
        input.push(char::from_u32(code).expect("C0 scalar"));
        input.push('x');
        expected.push(' ');
        expected.push('x');
    }
    input.push('\u{007F}');
    input.push_str("suffix");
    expected.push(' ');
    expected.push_str("suffix");

    assert_eq!(sanitise_usage_evidence_text(Some(&input)), Some(expected));
}

#[test]
fn sanitization_trims_after_control_replacement() {
    assert_eq!(
        sanitise_usage_evidence_text(Some(" \n\t provider evidence \r ")),
        Some("provider evidence".to_owned())
    );
    assert_eq!(
        sanitise_usage_evidence_text(Some("left\n\tmiddle\r\tright")),
        Some("left  middle  right".to_owned())
    );
}

#[test]
fn sanitization_observes_bmp_utf16_unit_boundaries() {
    let below = "a".repeat(255);
    let exact = "a".repeat(MAX_USAGE_EVIDENCE_TEXT_UTF16_UNITS);
    let over = format!("{exact}b");

    assert_eq!(
        sanitise_usage_evidence_text(Some(&below)).as_deref(),
        Some(below.as_str())
    );
    assert_eq!(
        sanitise_usage_evidence_text(Some(&exact)).as_deref(),
        Some(exact.as_str())
    );
    assert_eq!(
        sanitise_usage_evidence_text(Some(&over)).as_deref(),
        Some(exact.as_str())
    );
    assert_eq!(
        sanitise_usage_evidence_text(Some(&over))
            .expect("bounded evidence")
            .encode_utf16()
            .count(),
        256
    );
}

#[test]
fn sanitization_observes_non_bmp_utf16_unit_boundaries() {
    let at_255 = format!("{}🙂", "a".repeat(253));
    let at_256 = format!("{}🙂", "a".repeat(254));
    let at_257 = format!("{}🙂", "a".repeat(255));

    assert_eq!(utf16_units(&at_255), 255);
    assert_eq!(utf16_units(&at_256), 256);
    assert_eq!(utf16_units(&at_257), 257);
    assert_eq!(sanitise_usage_evidence_text(Some(&at_255)), Some(at_255));
    assert_eq!(sanitise_usage_evidence_text(Some(&at_256)), Some(at_256));
    assert_eq!(
        sanitise_usage_evidence_text(Some(&at_257)),
        Some("a".repeat(255))
    );
}

#[test]
fn sanitization_observes_mixed_utf16_unit_boundaries() {
    let prefix = "🙂".repeat(127);
    let at_255 = format!("{prefix}a");
    let at_256 = format!("{prefix}ab");
    let at_257 = format!("{prefix}abc");

    assert_eq!(utf16_units(&at_255), 255);
    assert_eq!(utf16_units(&at_256), 256);
    assert_eq!(utf16_units(&at_257), 257);
    assert_eq!(sanitise_usage_evidence_text(Some(&at_255)), Some(at_255));
    assert_eq!(sanitise_usage_evidence_text(Some(&at_256)), Some(at_256));
    assert_eq!(
        sanitise_usage_evidence_text(Some(&at_257)),
        Some(format!("{prefix}ab"))
    );
}

#[test]
fn one_unit_left_before_emoji_does_not_split_the_scalar() {
    let input = format!("{}🙂tail", "a".repeat(255));
    let cleaned = sanitise_usage_evidence_text(Some(&input)).expect("bounded evidence");

    assert_eq!(cleaned, "a".repeat(255));
    assert_eq!(utf16_units(&cleaned), 255);
}

#[test]
fn empty_cleaned_evidence_is_absent() {
    assert_eq!(sanitise_usage_evidence_text(None), None);
    assert_eq!(sanitise_usage_evidence_text(Some("")), None);
    assert_eq!(sanitise_usage_evidence_text(Some(" \n\t\r\u{007F}")), None);
}

#[test]
fn non_ascii_evidence_is_preserved_and_bounded_by_utf16_units() {
    let input = "ø界🙂".repeat(100);
    let expected = "ø界🙂".repeat(64);
    let cleaned = sanitise_usage_evidence_text(Some(&input));

    assert_eq!(cleaned, Some(expected));
    assert_eq!(cleaned.as_deref().map(utf16_units), Some(256));
}

#[test]
fn repeated_evaluation_is_independent_and_facades_agree() {
    let row = present_row();
    let policy = UsageInterruptionModelPolicy::new();
    let first = decode_usage_interruption_row(&row).expect("valid alternatives");
    let second = decode_usage_interruption_row(&row).expect("valid alternatives");

    assert_eq!(policy, UsageInterruptionModelPolicy);
    assert_eq!(first, second);
    assert_eq!(
        UsageInterruptionModelPolicy::project(&row),
        Ok(first.clone())
    );
    assert_eq!(row, present_row());

    let evidence = Some("  model evidence\n");
    assert_eq!(
        sanitise_usage_evidence_text(evidence),
        sanitize_usage_evidence_text(evidence)
    );
    assert_eq!(
        UsageInterruptionModelPolicy::sanitise_evidence_text(evidence),
        sanitise_usage_evidence_text(evidence)
    );
}

#[test]
fn decoded_alternatives_type_is_explicitly_result_based() {
    let valid: DecodedUsageInterruptionAlternatives = Ok(vec![alternative()]);
    let invalid: DecodedUsageInterruptionAlternatives =
        Err(UsageInterruptionDecodeError::MalformedAlternatives);

    assert!(valid.is_ok());
    assert!(invalid.is_err());
}
