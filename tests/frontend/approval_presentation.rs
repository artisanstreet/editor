//! Dependency-free parity tests for the approval presentation policy.
//!
//! The tables cover every protocol kind and state, optional request fields,
//! legacy prompt handling, UI icon intent, and the runtime fallbacks that are
//! reachable when a decoded value is newer or malformed.

#[path = "../../modules/frontend/src/approval_presentation.rs"]
mod approval_presentation;

use approval_presentation::{
    ApprovalItem, ApprovalKind, ApprovalPresentation, ApprovalRequest, ApprovalState,
    get_approval_presentation, is_protocol_plumbing, legacy_description,
};

fn optional(value: Option<&str>) -> Option<String> {
    value.map(str::to_owned)
}

fn command_request(
    command: Option<&str>,
    cwd: Option<&str>,
    reason: Option<&str>,
) -> ApprovalRequest {
    ApprovalRequest::command(optional(command), optional(cwd), optional(reason))
}

fn file_change_request(reason: Option<&str>) -> ApprovalRequest {
    ApprovalRequest::file_change(optional(reason))
}

fn action_request(reason: Option<&str>) -> ApprovalRequest {
    ApprovalRequest::action(optional(reason))
}

fn item(prompt: &str, state: &str, request: Option<ApprovalRequest>) -> ApprovalItem {
    ApprovalItem::new(prompt, ApprovalState::from_raw(state), request)
}

#[test]
fn protocol_plumbing_table_matches_the_typescript_regex() {
    let cases = [
        ("item/abc/requestApproval", true),
        ("item/a/b/c/requestApproval", true),
        ("item/a/requestApproval for tool", true),
        ("item/a/requestApproval  for   tool", true),
        ("item/a/requestApproval\tfor\ttool", true),
        ("item/a/requestApproval\u{00a0}for\u{feff}tool", true),
        ("item/a/requestApproval for tool/with-slash", true),
        ("item/a/requestApproval/requestApproval", true),
        ("item/a/requestApproval for", false),
        ("item/a/requestApproval for ", false),
        ("item/a/requestApproval for tool extra", false),
        ("item/a/requestApproval extra", false),
        ("item/a/requestApprovalFor tool", false),
        ("item/a/requestApproval\u{0085}for\u{0085}tool", false),
        ("item/requestApproval", false),
        ("item//requestApproval", false),
        ("item/a\n/requestApproval", false),
        ("item/a\r/requestApproval", false),
        ("item/a\u{2028}/requestApproval", false),
        ("item/a/requestApproval for tool\n", false),
        ("item/a/requestApproval for tool\u{0085}", true),
        ("not-item/abc/requestApproval", false),
        ("", false),
    ];

    for (input, expected) in cases {
        assert_eq!(
            is_protocol_plumbing(input),
            expected,
            "protocol-plumbing mismatch for {input:?}"
        );
    }
}

#[test]
fn legacy_description_uses_ecmascript_trim_and_filters_only_plumbing() {
    let empty_ecmascript_whitespace = [
        "",
        " \t\n\r\u{000b}\u{000c}",
        "\u{00a0}\u{1680}\u{2000}\u{2028}\u{2029}",
        "\u{202f}\u{205f}\u{3000}\u{feff}",
    ];
    for prompt in empty_ecmascript_whitespace {
        assert_eq!(
            legacy_description(prompt),
            None,
            "empty prompt should have no description: {prompt:?}"
        );
    }

    let cases = [
        ("  hello  ", Some("hello")),
        ("\u{feff}hello\u{00a0}", Some("hello")),
        ("  hello\nsecond line  ", Some("hello\nsecond line")),
        ("  item/abc/requestApproval  ", None),
        ("item/a/requestApproval for tool", None),
        (" item//requestApproval ", Some("item//requestApproval")),
        (
            "item/a/requestApproval for",
            Some("item/a/requestApproval for"),
        ),
        ("\u{0085}hello\u{0085}", Some("\u{0085}hello\u{0085}")),
    ];
    for (prompt, expected) in cases {
        assert_eq!(
            legacy_description(prompt).as_deref(),
            expected,
            "legacy description mismatch for {prompt:?}"
        );
    }
}

#[test]
fn raw_kind_and_state_parsing_is_exact_and_preserves_unknown_values() {
    let kind_cases = [
        ("command", ApprovalKind::Command),
        ("file_change", ApprovalKind::FileChange),
        ("action", ApprovalKind::Action),
        ("Command", ApprovalKind::Unknown(String::from("Command"))),
        (" future ", ApprovalKind::Unknown(String::from(" future "))),
    ];
    for (raw, expected) in kind_cases {
        let kind = ApprovalKind::from_raw(raw);
        assert_eq!(kind, expected, "kind classification mismatch for {raw:?}");
        assert_eq!(kind.as_raw(), raw);
    }

    let state_cases = [
        ("requested", ApprovalState::Requested),
        ("approved", ApprovalState::Approved),
        ("rejected", ApprovalState::Rejected),
        ("cancelled", ApprovalState::Cancelled),
        ("Approved", ApprovalState::Unknown(String::from("Approved"))),
        (" future ", ApprovalState::Unknown(String::from(" future "))),
    ];
    for (raw, expected) in state_cases {
        let state = ApprovalState::from_raw(raw);
        assert_eq!(state, expected, "state classification mismatch for {raw:?}");
        assert_eq!(state.as_raw(), raw);
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn every_known_kind_and_state_matches_the_exact_presentation_matrix() {
    struct KindCase {
        raw_kind: &'static str,
        approve_label: &'static str,
        noun: &'static str,
        default_title: &'static str,
        reason: &'static str,
        command: Option<&'static str>,
        cwd: Option<&'static str>,
    }

    let kinds = [
        KindCase {
            raw_kind: "command",
            approve_label: "Run command",
            noun: "Command",
            default_title: "Run this command?",
            reason: "Run the test suite",
            command: Some("pnpm test"),
            cwd: Some("C:\\workspace"),
        },
        KindCase {
            raw_kind: "file_change",
            approve_label: "Apply changes",
            noun: "Changes",
            default_title: "Apply these changes?",
            reason: "Apply the generated fixes",
            command: None,
            cwd: None,
        },
        KindCase {
            raw_kind: "action",
            approve_label: "Approve",
            noun: "Action",
            default_title: "Allow this action?",
            reason: "Connect the provider",
            command: None,
            cwd: None,
        },
    ];
    let states = [
        ("requested", "default"),
        ("approved", "approved"),
        ("rejected", "denied"),
        ("cancelled", "cancelled"),
    ];

    for kind in kinds {
        for (raw_state, title_suffix) in states {
            let request = match kind.raw_kind {
                "command" => Some(command_request(kind.command, kind.cwd, Some(kind.reason))),
                "file_change" => Some(file_change_request(Some(kind.reason))),
                "action" => Some(action_request(Some(kind.reason))),
                _ => unreachable!("the table contains only known kinds"),
            };
            let presentation = get_approval_presentation(&item(
                "legacy prompt that is ignored when reason exists",
                raw_state,
                request,
            ));

            let expected_title = match title_suffix {
                "default" => kind.default_title,
                "approved" => match kind.noun {
                    "Command" => "Command approved",
                    "Changes" => "Changes approved",
                    "Action" => "Action approved",
                    _ => unreachable!("the table contains only known nouns"),
                },
                "denied" => match kind.noun {
                    "Command" => "Command denied",
                    "Changes" => "Changes denied",
                    "Action" => "Action denied",
                    _ => unreachable!("the table contains only known nouns"),
                },
                "cancelled" => match kind.noun {
                    "Command" => "Command cancelled",
                    "Changes" => "Changes cancelled",
                    "Action" => "Action cancelled",
                    _ => unreachable!("the table contains only known nouns"),
                },
                _ => unreachable!("the table contains only known title suffixes"),
            };
            let expected_description = (raw_state == "requested").then_some(kind.reason);

            assert_eq!(
                presentation.approve_label, kind.approve_label,
                "approve label mismatch for {}/{}",
                kind.raw_kind, raw_state
            );
            assert_eq!(
                presentation.kind.as_raw(),
                kind.raw_kind,
                "kind mismatch for {}/{}",
                kind.raw_kind,
                raw_state
            );
            assert_eq!(
                presentation.title, expected_title,
                "title mismatch for {}/{}",
                kind.raw_kind, raw_state
            );
            assert_eq!(
                presentation.description.as_deref(),
                expected_description,
                "description mismatch for {}/{}",
                kind.raw_kind,
                raw_state
            );
            assert_eq!(
                presentation.command.as_deref(),
                kind.command,
                "command mismatch for {}/{}",
                kind.raw_kind,
                raw_state
            );
            assert_eq!(
                presentation.cwd.as_deref(),
                kind.cwd,
                "cwd mismatch for {}/{}",
                kind.raw_kind,
                raw_state
            );
        }
    }
}

#[test]
fn command_optional_fields_are_preserved_independently() {
    let cases = [
        (Some("echo hi"), Some("/workspace")),
        (Some("echo hi"), None),
        (None, Some("/workspace")),
        (None, None),
    ];
    for (command, cwd) in cases {
        let presentation = get_approval_presentation(&item(
            "reason from prompt",
            "requested",
            Some(command_request(command, cwd, None)),
        ));
        assert_eq!(presentation.command.as_deref(), command);
        assert_eq!(presentation.cwd.as_deref(), cwd);
        assert_eq!(presentation.kind, ApprovalKind::Command);
    }
}

#[test]
fn reason_precedence_matches_nullish_coalescing_and_preserves_empty_reason() {
    let cases = [
        ("Legacy prompt", None, Some("Legacy prompt")),
        (
            "Legacy prompt",
            Some("Explicit reason"),
            Some("Explicit reason"),
        ),
        ("Legacy prompt", Some(""), Some("")),
        ("item/a/requestApproval", None, None),
        (
            "item/a/requestApproval",
            Some("Explicit reason"),
            Some("Explicit reason"),
        ),
    ];

    for (prompt, reason, expected) in cases {
        let presentation = get_approval_presentation(&item(
            prompt,
            "requested",
            Some(command_request(None, None, reason)),
        ));
        assert_eq!(presentation.description.as_deref(), expected);
    }

    let non_requested = get_approval_presentation(&item(
        "Legacy prompt",
        "approved",
        Some(command_request(None, None, Some("reason"))),
    ));
    assert_eq!(non_requested.description, None);
}

#[test]
fn action_fallback_covers_missing_requests_and_exact_default_description() {
    let cases = [
        ("requested", "Allow this action?", Some("Do the thing")),
        ("approved", "Action approved", None),
        ("rejected", "Action denied", None),
        ("cancelled", "Action cancelled", None),
    ];

    for (state, expected_title, expected_description) in cases {
        let presentation = get_approval_presentation(&item("Do the thing", state, None));
        assert_eq!(presentation.approve_label, "Approve");
        assert_eq!(presentation.kind, ApprovalKind::Action);
        assert_eq!(presentation.title, expected_title);
        assert_eq!(presentation.description.as_deref(), expected_description);
        assert_eq!(presentation.command, None);
        assert_eq!(presentation.cwd, None);
    }

    let defaulted = get_approval_presentation(&item("   ", "requested", None));
    assert_eq!(
        defaulted.description.as_deref(),
        Some("Artisan needs your approval before it can continue.")
    );

    let explicit_action = get_approval_presentation(&item(
        "legacy",
        "requested",
        Some(action_request(Some("explicit action reason"))),
    ));
    assert_eq!(
        explicit_action.description.as_deref(),
        Some("explicit action reason")
    );
}

#[test]
fn unknown_kind_normalizes_to_action_and_drops_command_details() {
    let request = ApprovalRequest::new(
        ApprovalKind::from_raw("future_kind"),
        Some(String::from("do not expose")),
        Some(String::from("/do-not-expose")),
        Some(String::from("Future action reason")),
    );
    let presentation = get_approval_presentation(&item("legacy", "requested", Some(request)));

    assert_eq!(presentation.approve_label, "Approve");
    assert_eq!(presentation.kind, ApprovalKind::Action);
    assert_eq!(presentation.title, "Allow this action?");
    assert_eq!(
        presentation.description.as_deref(),
        Some("Future action reason")
    );
    assert_eq!(presentation.command, None);
    assert_eq!(presentation.cwd, None);
}

#[test]
fn unknown_state_uses_the_typescript_cancelled_fallback() {
    let state = ApprovalState::from_raw("future_state");
    let presentation = get_approval_presentation(&ApprovalItem::new(
        "legacy reason",
        state.clone(),
        Some(command_request(Some("echo hi"), None, Some("reason"))),
    ));

    assert_eq!(presentation.title, "Command cancelled");
    assert_eq!(presentation.description, None);
    assert_eq!(presentation.command.as_deref(), Some("echo hi"));
    assert_eq!(ApprovalPresentation::status_icon(&state), "circle-x");
}

#[test]
fn kind_and_state_icon_intent_matches_the_svelte_call_site() {
    let kind_cases = [
        (ApprovalKind::Command, "terminal-2"),
        (ApprovalKind::FileChange, "file-diff"),
        (ApprovalKind::Action, "file-diff"),
        (ApprovalKind::from_raw("future_kind"), "file-diff"),
    ];
    for (kind, expected_icon) in kind_cases {
        assert_eq!(kind.icon_name(), expected_icon);
    }

    let state_cases = [
        (ApprovalState::Requested, "circle-x"),
        (ApprovalState::Approved, "circle-check"),
        (ApprovalState::Rejected, "circle-x"),
        (ApprovalState::Cancelled, "circle-x"),
        (ApprovalState::from_raw("future_state"), "circle-x"),
    ];
    for (state, expected_icon) in state_cases {
        assert_eq!(state.status_icon_name(), expected_icon);
        assert_eq!(ApprovalPresentation::status_icon(&state), expected_icon);
    }

    let command = get_approval_presentation(&item(
        "reason",
        "requested",
        Some(command_request(None, None, None)),
    ));
    let file_change = get_approval_presentation(&item(
        "reason",
        "requested",
        Some(file_change_request(None)),
    ));
    let action = get_approval_presentation(&item("reason", "requested", None));
    assert_eq!(command.icon_name(), "terminal-2");
    assert_eq!(file_change.icon_name(), "file-diff");
    assert_eq!(action.icon_name(), "file-diff");
}

#[test]
fn multiline_unicode_legacy_prompts_remain_intact_after_trimming() {
    let presentation = get_approval_presentation(&item(
        "  héllo 🦀\nsecond line  ",
        "requested",
        Some(command_request(None, None, None)),
    ));
    assert_eq!(
        presentation.description.as_deref(),
        Some("héllo 🦀\nsecond line")
    );
}

#[test]
fn presentation_is_pure_and_deterministic() {
    let first = item(
        "hello",
        "requested",
        Some(command_request(Some("ls"), Some("/"), Some("reason"))),
    );
    let second = first.clone();
    assert_eq!(
        get_approval_presentation(&first),
        get_approval_presentation(&second)
    );
}
