//! Focused parity coverage for the dependency-free runtime fixture helpers.

#[path = "../../modules/frontend/src/runtime_fixture_support.rs"]
mod runtime_fixture_support;

use runtime_fixture_support::{
    DEFAULT_FIXTURE_JOURNAL_SEQUENCE, FIXTURE_FAILURE_CODE, FIXTURE_FAILURE_PROTOCOL_CODE,
    FIXTURE_FAILURE_RETRYABLE, FIXTURE_RECEIPT_STATUS, FixtureClientError, FixturePreviewTarget,
    fixture_failure, fixture_preview_target, fixture_receipt,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreviewFields {
    label: String,
    routes: Vec<String>,
}

#[test]
fn fixture_failures_preserve_empty_and_unicode_messages_with_exact_fields() {
    let cases = ["", "fixture message — ❌\nwith spacing"];

    for message in cases {
        let failure = fixture_failure(message);

        assert_eq!(
            failure,
            FixtureClientError {
                cause: None,
                code: FIXTURE_FAILURE_CODE,
                message: message.to_owned(),
                protocol_code: FIXTURE_FAILURE_PROTOCOL_CODE,
                retryable: FIXTURE_FAILURE_RETRYABLE,
            }
        );
    }
}

#[test]
fn default_and_override_receipts_preserve_ids_and_status() {
    let default_receipt = fixture_receipt("", None);
    assert_eq!(default_receipt.command_id, "");
    assert_eq!(
        default_receipt.journal_sequence,
        DEFAULT_FIXTURE_JOURNAL_SEQUENCE
    );
    assert_eq!(default_receipt.status, FIXTURE_RECEIPT_STATUS);

    let unicode_id = "命令-🚀\nexact";
    let override_receipt = fixture_receipt(unicode_id, Some(0));
    assert_eq!(override_receipt.command_id, unicode_id);
    assert_eq!(override_receipt.journal_sequence, 0);
    assert_eq!(override_receipt.status, FIXTURE_RECEIPT_STATUS);
}

#[test]
fn preview_lookup_returns_the_first_duplicate_and_preserves_fields_and_order() {
    let targets = vec![
        FixturePreviewTarget::new(
            "duplicate",
            PreviewFields {
                label: "first".to_owned(),
                routes: vec!["/first".to_owned(), "/second".to_owned()],
            },
        ),
        FixturePreviewTarget::new(
            "other",
            PreviewFields {
                label: "other".to_owned(),
                routes: vec!["/other".to_owned()],
            },
        ),
        FixturePreviewTarget::new(
            "duplicate",
            PreviewFields {
                label: "second".to_owned(),
                routes: vec!["/later".to_owned()],
            },
        ),
    ];

    let found = fixture_preview_target(&targets, "duplicate").expect("duplicate target exists");

    assert_eq!(found, &targets[0]);
    assert_eq!(found.fields.label, "first");
    assert_eq!(found.fields.routes, ["/first", "/second"]);
    assert_eq!(
        targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        ["duplicate", "other", "duplicate"]
    );
}

#[test]
fn preview_lookup_matches_unicode_ids_exactly() {
    let target = FixturePreviewTarget::new("目标-🚀", "opaque fields".to_owned());
    let targets = [target];

    assert_eq!(
        fixture_preview_target(&targets, "目标-🚀").map(|value| value.fields.as_str()),
        Ok("opaque fields")
    );
    assert!(fixture_preview_target(&targets, "目标-🚀 ").is_err());
}

#[test]
fn empty_targets_and_misses_return_the_exact_typed_failure() {
    let empty: Vec<FixturePreviewTarget<String>> = Vec::new();
    let empty_error = fixture_preview_target(&empty, "missing").expect_err("empty lookup fails");
    assert_eq!(
        empty_error,
        fixture_failure("Unknown fixture preview target: missing")
    );

    let targets = [FixturePreviewTarget::new("known", "fields".to_owned())];
    let missing_id = "不存在-🚀";
    let missing_error =
        fixture_preview_target(&targets, missing_id).expect_err("unknown lookup fails");
    assert_eq!(
        missing_error.message,
        "Unknown fixture preview target: 不存在-🚀"
    );
    assert_eq!(missing_error.code, "protocol");
    assert_eq!(missing_error.protocol_code, "fixture_not_found");
    assert!(!missing_error.retryable);
    assert!(missing_error.cause.is_none());
}
