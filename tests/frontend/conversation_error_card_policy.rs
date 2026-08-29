//! Direct, dependency-free coverage for the conversation error-card policy.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/conversation_error_card_policy.rs"]
mod conversation_error_card_policy;

use conversation_error_card_policy::{
    CatalogErrorDefinition, ConversationErrorCardCommand, ConversationErrorCardInput,
    CopyCodeResult, CopyCodeState, diagnostic_detail, present_conversation_error_card,
};

fn input<'a>(
    definition: CatalogErrorDefinition<'a>,
    code: &'a str,
    captured_detail: Option<&'a str>,
    projected_detail: Option<&'a str>,
    formatted_reset_label: Option<&'a str>,
) -> ConversationErrorCardInput<'a> {
    ConversationErrorCardInput::new(
        definition,
        code,
        captured_detail,
        projected_detail,
        formatted_reset_label,
    )
}

#[test]
fn known_catalog_definition_supplies_title_and_summary_without_a_local_catalog() {
    let card = present_conversation_error_card(input(
        CatalogErrorDefinition::new("Usage limit reached", "The usage window has ended."),
        "AE-PROVIDER-201",
        None,
        None,
        None,
    ));

    assert_eq!(card.title, "Usage limit reached");
    assert_eq!(card.summary, "The usage window has ended.");
    assert_eq!(card.code, "AE-PROVIDER-201");
}

#[test]
fn unknown_catalog_fallback_still_keeps_the_supplied_code_visible_and_copyable() {
    let code = "AE-PROVIDER-999";
    let card = present_conversation_error_card(input(
        CatalogErrorDefinition::new(
            "Unexpected engine failure",
            "The engine reported an unrecognized failure.",
        ),
        code,
        Some("provider detail"),
        None,
        None,
    ));

    assert!(card.code_visible);
    assert!(card.is_code_visible());
    assert_eq!(card.code, code);
    assert_eq!(
        card.copy_command,
        ConversationErrorCardCommand::copy_code(code)
    );
    assert_eq!(card.copy_command.code(), code);
    assert_eq!(card.accessible_label(), "Copy error code AE-PROVIDER-999");
    assert_eq!(card.diagnostic(), Some("provider detail"));
}

#[test]
fn captured_detail_wins_but_projected_detail_fills_an_absent_capture() {
    assert_eq!(
        diagnostic_detail(Some("captured"), Some("projected")),
        Some("captured")
    );
    assert_eq!(
        diagnostic_detail(None, Some("projected")),
        Some("projected")
    );

    let captured_wins = present_conversation_error_card(input(
        CatalogErrorDefinition::new("Title", "Summary"),
        "AE-RUN-301",
        Some("captured"),
        Some("projected"),
        None,
    ));
    assert_eq!(captured_wins.detail(), Some("captured"));

    let projected_fills = present_conversation_error_card(input(
        CatalogErrorDefinition::new("Title", "Summary"),
        "AE-RUN-301",
        None,
        Some("projected"),
        None,
    ));
    assert_eq!(projected_fills.detail(), Some("projected"));
}

#[test]
fn diagnostic_line_is_omitted_only_when_both_detail_values_are_absent() {
    let absent = present_conversation_error_card(input(
        CatalogErrorDefinition::new("Title", "Summary"),
        "AE-RUN-301",
        None,
        None,
        None,
    ));
    assert_eq!(absent.diagnostic, None);

    let captured_empty = present_conversation_error_card(input(
        CatalogErrorDefinition::new("Title", "Summary"),
        "AE-RUN-301",
        Some(""),
        Some("projected"),
        None,
    ));
    assert_eq!(captured_empty.diagnostic, Some(""));

    let projected_empty = present_conversation_error_card(input(
        CatalogErrorDefinition::new("Title", "Summary"),
        "AE-RUN-301",
        None,
        Some(""),
        None,
    ));
    assert_eq!(projected_empty.diagnostic, Some(""));
}

#[test]
fn reset_sentence_is_appended_only_for_a_supplied_formatted_label() {
    let without_reset = present_conversation_error_card(input(
        CatalogErrorDefinition::new("Title", "Summary"),
        "AE-RUN-301",
        None,
        None,
        None,
    ));
    assert_eq!(without_reset.summary, "Summary");

    let with_reset = present_conversation_error_card(input(
        CatalogErrorDefinition::new("Title", "Summary"),
        "AE-RUN-301",
        None,
        None,
        Some("Aug 28, 2026, 4:30 PM"),
    ));
    assert_eq!(with_reset.summary, "Summary Resets Aug 28, 2026, 4:30 PM.");

    // Presence, rather than a policy-side non-empty check, mirrors the
    // source's `resets_label !== undefined` branch.
    let empty_label = present_conversation_error_card(input(
        CatalogErrorDefinition::new("Title", "Summary"),
        "AE-RUN-301",
        None,
        None,
        Some(""),
    ));
    assert_eq!(empty_label.summary, "Summary Resets .");
}

#[test]
fn copy_result_and_state_are_typed_and_have_no_clipboard_side_effect() {
    assert_eq!(CopyCodeState::default(), CopyCodeState::Idle);
    assert_eq!(
        CopyCodeState::from_result(CopyCodeResult::Succeeded),
        CopyCodeState::Success
    );
    assert_eq!(
        CopyCodeState::from(CopyCodeResult::Failed),
        CopyCodeState::Failure
    );
    assert!(CopyCodeState::Success.is_success());
    assert!(!CopyCodeState::Success.is_failure());
    assert!(CopyCodeState::Failure.is_failure());
    assert!(!CopyCodeState::Failure.is_success());
}
