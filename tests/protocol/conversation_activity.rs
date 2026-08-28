//! Exhaustive, dependency-free coverage for conversation activity presentation.
//!
//! The tables below mirror the complete executable oracle in
//! `modules/protocol/src/conversation-activity.ts`: aliases and precedence,
//! stable copy, count grammar, legacy tool labels, JavaScript normalization,
//! and subagent identity grouping.

#![forbid(unsafe_code)]

#[path = "../../modules/protocol/src/conversation_activity.rs"]
mod conversation_activity;

use conversation_activity::{
    ConversationActivityCategory, ConversationActivityGroupMember,
    ConversationActivityGroupPresentation, ConversationActivityPresentation,
    ConversationActivityPresentationInput, ConversationActivitySubagent, ConversationLifecycle,
    get_conversation_activity_category, get_conversation_activity_category_label,
    get_conversation_activity_count_label, get_conversation_activity_group_presentation,
    get_conversation_activity_presentation,
};

fn input(
    kind: &str,
    label: &str,
    status: ConversationLifecycle,
) -> ConversationActivityPresentationInput {
    ConversationActivityPresentationInput::new(kind, label, status, None)
}

fn named_input(
    kind: &str,
    label: &str,
    status: ConversationLifecycle,
    agent_id: &str,
    display_name: &str,
) -> ConversationActivityPresentationInput {
    ConversationActivityPresentationInput::new(
        kind,
        label,
        status,
        Some(ConversationActivitySubagent::new(agent_id, display_name)),
    )
}

fn member(kind: &str) -> ConversationActivityGroupMember {
    ConversationActivityGroupMember::new(kind, None)
}

fn named_member(kind: &str, agent_id: &str, display_name: &str) -> ConversationActivityGroupMember {
    ConversationActivityGroupMember::new(
        kind,
        Some(ConversationActivitySubagent::new(agent_id, display_name)),
    )
}

const ALL_LIFECYCLES: [ConversationLifecycle; 8] = [
    ConversationLifecycle::Pending,
    ConversationLifecycle::Streaming,
    ConversationLifecycle::Active,
    ConversationLifecycle::Waiting,
    ConversationLifecycle::Completed,
    ConversationLifecycle::Failed,
    ConversationLifecycle::Interrupted,
    ConversationLifecycle::Cancelled,
];

const ALL_CATEGORIES: [ConversationActivityCategory; 16] = [
    ConversationActivityCategory::AppInspect,
    ConversationActivityCategory::Command,
    ConversationActivityCategory::Database,
    ConversationActivityCategory::Diff,
    ConversationActivityCategory::FileDelete,
    ConversationActivityCategory::FileEdit,
    ConversationActivityCategory::FileRead,
    ConversationActivityCategory::FileSearch,
    ConversationActivityCategory::GitStatus,
    ConversationActivityCategory::Integration,
    ConversationActivityCategory::Other,
    ConversationActivityCategory::Subagent,
    ConversationActivityCategory::Test,
    ConversationActivityCategory::Tool,
    ConversationActivityCategory::Typecheck,
    ConversationActivityCategory::WebSearch,
];

fn category_input(category: ConversationActivityCategory) -> (&'static str, &'static str) {
    match category {
        ConversationActivityCategory::AppInspect => ("preview", "ignored"),
        ConversationActivityCategory::Command => ("command", "ignored"),
        ConversationActivityCategory::Database => ("database", "ignored"),
        ConversationActivityCategory::Diff => ("diff", "ignored"),
        ConversationActivityCategory::FileDelete => ("file.delete", "ignored"),
        ConversationActivityCategory::FileEdit => ("file.edit", "ignored"),
        ConversationActivityCategory::FileRead => ("file.read", "ignored"),
        ConversationActivityCategory::FileSearch => ("file.list", "ignored"),
        ConversationActivityCategory::GitStatus => ("git.status", "ignored"),
        ConversationActivityCategory::Integration => ("integration", "ignored"),
        ConversationActivityCategory::Other => ("unrecognized.kind", "verbatim fallback"),
        ConversationActivityCategory::Subagent => ("subagent", "ignored"),
        ConversationActivityCategory::Test => ("test", "ignored"),
        ConversationActivityCategory::Tool => ("tool", "Tool"),
        ConversationActivityCategory::Typecheck => ("typecheck", "ignored"),
        ConversationActivityCategory::WebSearch => ("web.search", "ignored"),
    }
}

#[test]
fn every_category_has_the_exact_foreground_label() {
    let cases = [
        (ConversationActivityCategory::AppInspect, "App"),
        (ConversationActivityCategory::Command, "Command"),
        (ConversationActivityCategory::Database, "Database"),
        (ConversationActivityCategory::Diff, "Changes"),
        (ConversationActivityCategory::FileDelete, "Files"),
        (ConversationActivityCategory::FileEdit, "Files"),
        (ConversationActivityCategory::FileRead, "Files"),
        (ConversationActivityCategory::FileSearch, "Files"),
        (ConversationActivityCategory::GitStatus, "Git"),
        (ConversationActivityCategory::Integration, "Integrations"),
        (ConversationActivityCategory::Other, "Tools"),
        (ConversationActivityCategory::Subagent, "Subagents"),
        (ConversationActivityCategory::Test, "Tests"),
        (ConversationActivityCategory::Tool, "Tools"),
        (ConversationActivityCategory::Typecheck, "Types"),
        (ConversationActivityCategory::WebSearch, "Web"),
    ];

    for (category, expected) in cases {
        assert_eq!(get_conversation_activity_category_label(category), expected);
    }
}

#[test]
fn every_category_alias_and_condition_is_classified() {
    let cases = [
        // Command aliases.
        ("terminal", ConversationActivityCategory::Command),
        ("command", ConversationActivityCategory::Command),
        ("shell", ConversationActivityCategory::Command),
        ("bash", ConversationActivityCategory::Command),
        ("exec", ConversationActivityCategory::Command),
        (
            "run-terminal-command",
            ConversationActivityCategory::Command,
        ),
        ("SHELL_TOOL", ConversationActivityCategory::Command),
        // File-read aliases.
        ("file", ConversationActivityCategory::FileRead),
        ("read", ConversationActivityCategory::FileRead),
        ("read.file", ConversationActivityCategory::FileRead),
        ("file.read", ConversationActivityCategory::FileRead),
        ("workspace.read", ConversationActivityCategory::FileRead),
        ("documents.read", ConversationActivityCategory::FileRead),
        ("FILE_READ", ConversationActivityCategory::FileRead),
        // File-delete aliases.
        ("file.delete", ConversationActivityCategory::FileDelete),
        (
            "workspace.file.delete",
            ConversationActivityCategory::FileDelete,
        ),
        ("FILE_DELETE", ConversationActivityCategory::FileDelete),
        // File-edit aliases.
        ("write", ConversationActivityCategory::FileEdit),
        ("edit", ConversationActivityCategory::FileEdit),
        ("apply", ConversationActivityCategory::FileEdit),
        ("file.edit", ConversationActivityCategory::FileEdit),
        ("file.write", ConversationActivityCategory::FileEdit),
        ("workspace.edit", ConversationActivityCategory::FileEdit),
        ("workspace.write", ConversationActivityCategory::FileEdit),
        ("apply.patch", ConversationActivityCategory::FileEdit),
        ("FILE-WRITE", ConversationActivityCategory::FileEdit),
        // File-search aliases.
        ("workspace.search", ConversationActivityCategory::FileSearch),
        ("file.list", ConversationActivityCategory::FileSearch),
        ("grep", ConversationActivityCategory::FileSearch),
        ("glob", ConversationActivityCategory::FileSearch),
        ("find", ConversationActivityCategory::FileSearch),
        ("ripgrep", ConversationActivityCategory::FileSearch),
        ("WORKSPACE_SEARCH", ConversationActivityCategory::FileSearch),
        // Web-search aliases.
        ("search", ConversationActivityCategory::WebSearch),
        ("web.search", ConversationActivityCategory::WebSearch),
        ("fetch", ConversationActivityCategory::WebSearch),
        ("WEB-SEARCH", ConversationActivityCategory::WebSearch),
        // Remaining exact semantic aliases.
        ("test", ConversationActivityCategory::Test),
        ("unit-test", ConversationActivityCategory::Test),
        ("typescript", ConversationActivityCategory::Typecheck),
        ("typecheck", ConversationActivityCategory::Typecheck),
        ("git.status", ConversationActivityCategory::GitStatus),
        ("git-status", ConversationActivityCategory::GitStatus),
        ("diff", ConversationActivityCategory::Diff),
        ("unified-diff", ConversationActivityCategory::Diff),
        ("database", ConversationActivityCategory::Database),
        ("database-query", ConversationActivityCategory::Database),
        ("preview", ConversationActivityCategory::AppInspect),
        ("browser", ConversationActivityCategory::AppInspect),
        ("ui.inspect", ConversationActivityCategory::AppInspect),
        ("accessibility", ConversationActivityCategory::AppInspect),
        ("subagent", ConversationActivityCategory::Subagent),
        ("agent.activity", ConversationActivityCategory::Subagent),
        ("mcp", ConversationActivityCategory::Integration),
        ("integration", ConversationActivityCategory::Integration),
        ("tool", ConversationActivityCategory::Tool),
        ("plugin", ConversationActivityCategory::Tool),
        // Unrecognized values are not trimmed or rejected.
        ("", ConversationActivityCategory::Other),
        ("  read  ", ConversationActivityCategory::Other),
        ("provider-specific", ConversationActivityCategory::Other),
        ("🦀", ConversationActivityCategory::Other),
    ];

    for (kind, expected) in cases {
        assert_eq!(
            get_conversation_activity_category(kind),
            expected,
            "category mismatch for {kind:?}"
        );
    }
}

#[test]
fn category_precedence_matches_the_source_order() {
    let cases = [
        ("terminal-file-read", ConversationActivityCategory::Command),
        ("command-preview", ConversationActivityCategory::Command),
        (
            "workspace-read-test",
            ConversationActivityCategory::FileRead,
        ),
        ("file-delete-read", ConversationActivityCategory::FileRead),
        ("file-edit-read", ConversationActivityCategory::FileRead),
        (
            "workspace-search-test",
            ConversationActivityCategory::FileSearch,
        ),
        ("web-search-test", ConversationActivityCategory::WebSearch),
        ("typescript-test", ConversationActivityCategory::Test),
        ("git-status-diff", ConversationActivityCategory::GitStatus),
        ("database-preview", ConversationActivityCategory::Database),
        ("browser-subagent", ConversationActivityCategory::AppInspect),
        (
            "subagent-integration",
            ConversationActivityCategory::Subagent,
        ),
        (
            "mcp-integration-tool",
            ConversationActivityCategory::Integration,
        ),
        ("tool-plugin", ConversationActivityCategory::Tool),
    ];

    for (kind, expected) in cases {
        assert_eq!(
            get_conversation_activity_category(kind),
            expected,
            "for {kind:?}"
        );
    }
}

#[test]
fn every_category_count_has_exact_zero_one_and_many_copy() {
    let cases = [
        (
            ConversationActivityCategory::AppInspect,
            [
                "ran 0 app inspections",
                "inspected the app",
                "ran 2 app inspections",
            ],
        ),
        (
            ConversationActivityCategory::Command,
            ["ran 0 commands", "ran a command", "ran 2 commands"],
        ),
        (
            ConversationActivityCategory::Database,
            [
                "ran 0 database inspections",
                "inspected the database",
                "ran 2 database inspections",
            ],
        ),
        (
            ConversationActivityCategory::Diff,
            ["reviewed 0 diffs", "reviewed changes", "reviewed 2 diffs"],
        ),
        (
            ConversationActivityCategory::FileDelete,
            ["deleted 0 files", "deleted a file", "deleted 2 files"],
        ),
        (
            ConversationActivityCategory::FileEdit,
            ["edited 0 files", "edited a file", "edited 2 files"],
        ),
        (
            ConversationActivityCategory::FileRead,
            ["read 0 files", "read a file", "read 2 files"],
        ),
        (
            ConversationActivityCategory::FileSearch,
            ["searched 0 files", "searched files", "searched 2 files"],
        ),
        (
            ConversationActivityCategory::GitStatus,
            [
                "ran 0 Git status checks",
                "checked Git status",
                "ran 2 Git status checks",
            ],
        ),
        (
            ConversationActivityCategory::Integration,
            [
                "used 0 integrations",
                "used an integration",
                "used 2 integrations",
            ],
        ),
        (
            ConversationActivityCategory::Other,
            ["used 0 tools", "used a tool", "used 2 tools"],
        ),
        (
            ConversationActivityCategory::Subagent,
            [
                "talked to 0 subagents",
                "talked to a subagent",
                "talked to 2 subagents",
            ],
        ),
        (
            ConversationActivityCategory::Test,
            ["ran 0 test runs", "ran tests", "ran 2 test runs"],
        ),
        (
            ConversationActivityCategory::Tool,
            ["used 0 tools", "used a tool", "used 2 tools"],
        ),
        (
            ConversationActivityCategory::Typecheck,
            ["ran 0 type checks", "checked types", "ran 2 type checks"],
        ),
        (
            ConversationActivityCategory::WebSearch,
            [
                "ran 0 web searches",
                "searched the web",
                "ran 2 web searches",
            ],
        ),
    ];

    for (category, expected) in cases {
        for (count, expected_label) in [0_usize, 1, 2].into_iter().zip(expected) {
            assert_eq!(
                get_conversation_activity_count_label(category, count),
                expected_label,
                "count mismatch for {category:?}, {count}"
            );
        }
    }
}

#[test]
fn every_category_and_lifecycle_combination_has_exact_row_copy() {
    let cases = [
        (
            ConversationActivityCategory::AppInspect,
            "Inspecting the app",
            "Inspected the app",
            "App inspection failed",
        ),
        (
            ConversationActivityCategory::Command,
            "Running a command",
            "Ran a command",
            "Command failed",
        ),
        (
            ConversationActivityCategory::Database,
            "Inspecting the database",
            "Inspected the database",
            "Database inspection failed",
        ),
        (
            ConversationActivityCategory::Diff,
            "Reviewing changes",
            "Reviewed changes",
            "Change review failed",
        ),
        (
            ConversationActivityCategory::FileDelete,
            "Deleting a file",
            "Deleted a file",
            "File delete failed",
        ),
        (
            ConversationActivityCategory::FileEdit,
            "Editing a file",
            "Edited a file",
            "File edit failed",
        ),
        (
            ConversationActivityCategory::FileRead,
            "Reading a file",
            "Read a file",
            "File read failed",
        ),
        (
            ConversationActivityCategory::FileSearch,
            "Searching files",
            "Searched files",
            "File search failed",
        ),
        (
            ConversationActivityCategory::GitStatus,
            "Checking Git status",
            "Checked Git status",
            "Git status failed",
        ),
        (
            ConversationActivityCategory::Integration,
            "Using an integration",
            "Used an integration",
            "Integration failed",
        ),
        (
            ConversationActivityCategory::Other,
            "verbatim fallback",
            "verbatim fallback",
            "verbatim fallback",
        ),
        (
            ConversationActivityCategory::Subagent,
            "Talking to a subagent",
            "Talked to a subagent",
            "Subagent work failed",
        ),
        (
            ConversationActivityCategory::Test,
            "Running tests",
            "Ran tests",
            "Tests failed",
        ),
        (
            ConversationActivityCategory::Tool,
            "Using a tool",
            "Used a tool",
            "Tool failed",
        ),
        (
            ConversationActivityCategory::Typecheck,
            "Checking types",
            "Checked types",
            "Type check failed",
        ),
        (
            ConversationActivityCategory::WebSearch,
            "Searching the web",
            "Searched the web",
            "Web search failed",
        ),
    ];

    for (category, active, completed, failed) in cases {
        let (kind, label) = category_input(category);
        for status in ALL_LIFECYCLES {
            let expected = match status {
                ConversationLifecycle::Completed => completed,
                ConversationLifecycle::Failed
                | ConversationLifecycle::Interrupted
                | ConversationLifecycle::Cancelled => failed,
                ConversationLifecycle::Pending
                | ConversationLifecycle::Streaming
                | ConversationLifecycle::Active
                | ConversationLifecycle::Waiting => active,
            };
            assert_eq!(
                get_conversation_activity_presentation(&input(kind, label, status)).label,
                expected,
                "presentation mismatch for {category:?}, {status:?}"
            );
        }
    }
}

#[test]
fn non_subagent_grouping_counts_rows_and_uses_count_copy() {
    for category in ALL_CATEGORIES {
        if category == ConversationActivityCategory::Subagent {
            continue;
        }
        for count in [0_usize, 1, 2] {
            let activities = (0..count)
                .map(|_| member("arbitrary.kind"))
                .collect::<Vec<_>>();
            let expected = ConversationActivityGroupPresentation {
                count,
                label: get_conversation_activity_count_label(category, count),
            };
            assert_eq!(
                get_conversation_activity_group_presentation(category, &activities),
                expected,
                "group mismatch for {category:?}, {count}"
            );
        }
    }
}

#[test]
fn subagent_row_copy_preserves_the_special_interrupted_behavior() {
    let statuses = [
        (ConversationLifecycle::Pending, "Talking to Ada"),
        (ConversationLifecycle::Streaming, "Talking to Ada"),
        (ConversationLifecycle::Active, "Talking to Ada"),
        (ConversationLifecycle::Waiting, "Talking to Ada"),
        (ConversationLifecycle::Completed, "Talked to Ada"),
        (ConversationLifecycle::Failed, "Ada's work failed"),
        // The source's named-subagent branch checks only failed/cancelled;
        // interrupted therefore remains present tense unlike a nameless row.
        (ConversationLifecycle::Interrupted, "Talking to Ada"),
        (ConversationLifecycle::Cancelled, "Ada's work failed"),
    ];

    for (status, expected) in statuses {
        assert_eq!(
            get_conversation_activity_presentation(&named_input(
                "subagent", "ignored", status, "agent-1", "Ada",
            ))
            .label,
            expected,
            "named subagent mismatch for {status:?}"
        );
    }
}

#[test]
fn back_compat_reclassifies_legacy_tool_labels_before_presenting() {
    let cases = [
        ("tool", "read", ConversationActivityCategory::FileRead),
        ("tool", "FILE_READ", ConversationActivityCategory::FileRead),
        ("tool", "write", ConversationActivityCategory::FileEdit),
        ("unknown", "search", ConversationActivityCategory::WebSearch),
        (
            "unknown",
            "typescript",
            ConversationActivityCategory::Typecheck,
        ),
        (
            "unknown",
            "workspace-search",
            ConversationActivityCategory::FileSearch,
        ),
    ];

    for (kind, label, category) in cases {
        let expected = match category {
            ConversationActivityCategory::FileRead => "Reading a file",
            ConversationActivityCategory::FileEdit => "Edited a file",
            ConversationActivityCategory::WebSearch => "Searching the web",
            ConversationActivityCategory::Typecheck => "Checking types",
            ConversationActivityCategory::FileSearch => "Searching files",
            ConversationActivityCategory::Tool => "Using plugin",
            _ => unreachable!("table contains only reclassified categories"),
        };
        let status = if category == ConversationActivityCategory::FileEdit {
            ConversationLifecycle::Completed
        } else {
            ConversationLifecycle::Active
        };
        assert_eq!(
            get_conversation_activity_presentation(&input(kind, label, status)).label,
            expected,
            "legacy label {label:?}"
        );
    }

    // A tool-shaped label does not replace an initially unknown category; the
    // source only reclassifies labels to specific non-tool semantics.
    assert_eq!(
        get_conversation_activity_presentation(&input(
            "unknown",
            "plugin",
            ConversationLifecycle::Active,
        ))
        .label,
        "plugin"
    );
}

#[test]
fn exact_tool_and_tools_labels_use_the_generic_category_copy() {
    for label in ["Tool", "Tools"] {
        let cases = [
            (ConversationLifecycle::Pending, "Using a tool"),
            (ConversationLifecycle::Completed, "Used a tool"),
            (ConversationLifecycle::Failed, "Tool failed"),
            (ConversationLifecycle::Interrupted, "Tool failed"),
        ];
        for (status, expected) in cases {
            assert_eq!(
                get_conversation_activity_presentation(&input("tool", label, status)).label,
                expected,
                "special label {label:?}, {status:?}"
            );
        }
    }

    // The exception is exact and case-sensitive; lowercase `tool` is a
    // generic named-tool label instead.
    assert_eq!(
        get_conversation_activity_presentation(&input(
            "tool",
            "tool",
            ConversationLifecycle::Active
        ))
        .label,
        "Using tool"
    );
}

#[test]
fn generic_tool_copy_uses_all_lifecycle_states_and_normalizes_its_name() {
    let cases = [
        (ConversationLifecycle::Pending, "Using foo bar baz"),
        (ConversationLifecycle::Streaming, "Using foo bar baz"),
        (ConversationLifecycle::Active, "Using foo bar baz"),
        (ConversationLifecycle::Waiting, "Using foo bar baz"),
        (ConversationLifecycle::Completed, "Used foo bar baz"),
        (ConversationLifecycle::Failed, "Foo bar baz failed"),
        (ConversationLifecycle::Interrupted, "Foo bar baz failed"),
        (ConversationLifecycle::Cancelled, "Foo bar baz failed"),
    ];

    for (status, expected) in cases {
        assert_eq!(
            get_conversation_activity_presentation(&input(
                "plugin",
                "  foo..bar__-\t baz  ",
                status,
            ))
            .label,
            expected,
            "generic tool mismatch for {status:?}"
        );
    }
}

#[test]
fn empty_tool_names_fall_back_to_the_generic_tool_copy() {
    for label in ["", " \t\r\n\u{00a0}\u{feff} "] {
        assert_eq!(
            get_conversation_activity_presentation(&input(
                "tool",
                label,
                ConversationLifecycle::Active,
            ))
            .label,
            "Using a tool",
            "empty tool-name fallback for {label:?}"
        );
    }
}

#[test]
fn javascript_tool_name_normalization_handles_whitespace_separators_and_unicode() {
    let cases = [
        (
            "  foo - \t _ . bar  ",
            ConversationLifecycle::Active,
            "Using foo bar",
        ),
        (
            "\u{feff}Foo\u{feff}",
            ConversationLifecycle::Active,
            "Using Foo",
        ),
        (
            "\u{0085}Foo\u{0085}",
            ConversationLifecycle::Active,
            "Using \u{0085}Foo\u{0085}",
        ),
        ("._-", ConversationLifecycle::Active, "Using  "),
        ("._-", ConversationLifecycle::Completed, "Used  "),
        ("._-", ConversationLifecycle::Failed, "  failed"),
        ("é_tool", ConversationLifecycle::Failed, "É tool failed"),
        ("ß_tool", ConversationLifecycle::Failed, "SS tool failed"),
        ("𐐨_tool", ConversationLifecycle::Failed, "𐐨 tool failed"),
    ];

    for (label, status, expected) in cases {
        assert_eq!(
            get_conversation_activity_presentation(&input("plugin", label, status)).label,
            expected,
            "normalized tool label {label:?}"
        );
    }
}

#[test]
fn unknown_labels_are_returned_verbatim_without_validation_or_normalization() {
    let labels = [
        "",
        "  provider-specific  ",
        "\u{feff}☃\u{00a0}",
        "\u{0085}not-trimmed\u{0085}",
        "工具 / provider\\kind",
    ];

    for label in labels {
        for status in ALL_LIFECYCLES {
            assert_eq!(
                get_conversation_activity_presentation(
                    &input("provider-only-kind", label, status,)
                )
                .label,
                label,
                "fallback label changed for {label:?}, {status:?}"
            );
        }
    }
}

#[test]
fn subagent_grouping_matches_javascript_map_identity_and_name_rules() {
    let cases = [
        (
            "empty",
            Vec::new(),
            ConversationActivityGroupPresentation {
                count: 0,
                label: "talked to 0 subagents".to_owned(),
            },
        ),
        (
            "anonymous",
            vec![member("subagent")],
            ConversationActivityGroupPresentation {
                count: 1,
                label: "talked to a subagent".to_owned(),
            },
        ),
        (
            "two anonymous",
            vec![member("one"), member("two")],
            ConversationActivityGroupPresentation {
                count: 2,
                label: "talked to 2 subagents".to_owned(),
            },
        ),
        (
            "one named",
            vec![named_member("subagent", "a", "Ada")],
            ConversationActivityGroupPresentation {
                count: 1,
                label: "talked to Ada".to_owned(),
            },
        ),
        (
            "same identity last name wins",
            vec![
                named_member("first", "a", "Ada"),
                named_member("second", "a", "Grace"),
            ],
            ConversationActivityGroupPresentation {
                count: 1,
                label: "talked to Grace".to_owned(),
            },
        ),
        (
            "same identity remains one map entry",
            vec![
                named_member("first", "a", "Ada"),
                named_member("second", "a", "Grace"),
                named_member("third", "b", "Lin"),
            ],
            ConversationActivityGroupPresentation {
                count: 2,
                label: "talked to 2 subagents".to_owned(),
            },
        ),
        (
            "same display name but different identities",
            vec![
                named_member("first", "a", "Same"),
                named_member("second", "b", "Same"),
            ],
            ConversationActivityGroupPresentation {
                count: 2,
                label: "talked to 2 subagents".to_owned(),
            },
        ),
        (
            "named and anonymous",
            vec![named_member("named", "a", "Ada"), member("anonymous")],
            ConversationActivityGroupPresentation {
                count: 2,
                label: "talked to 2 subagents".to_owned(),
            },
        ),
        (
            "empty identity is still named",
            vec![named_member("empty-id", "", "No ID")],
            ConversationActivityGroupPresentation {
                count: 1,
                label: "talked to No ID".to_owned(),
            },
        ),
        (
            "empty display name is not absent",
            vec![named_member("empty-name", "a", "")],
            ConversationActivityGroupPresentation {
                count: 1,
                label: "talked to ".to_owned(),
            },
        ),
        (
            "unicode name",
            vec![named_member("unicode", "工具", "Élodie 🛠")],
            ConversationActivityGroupPresentation {
                count: 1,
                label: "talked to Élodie 🛠".to_owned(),
            },
        ),
    ];

    for (name, activities, expected) in cases {
        assert_eq!(
            get_conversation_activity_group_presentation(
                ConversationActivityCategory::Subagent,
                &activities,
            ),
            expected,
            "subagent group mismatch: {name}"
        );
    }
}

#[test]
fn owned_inputs_and_outputs_do_not_borrow_callers() {
    let presentation = {
        let kind = String::from("unknown");
        let label = String::from("caller-owned 🌍");
        let value = ConversationActivityPresentationInput::new(
            kind,
            label,
            ConversationLifecycle::Active,
            None,
        );
        get_conversation_activity_presentation(&value)
    };
    assert_eq!(
        presentation,
        ConversationActivityPresentation {
            label: "caller-owned 🌍".to_owned()
        }
    );

    let subagent = ConversationActivitySubagent::new("agent", "Ada");
    assert_eq!(subagent.agent_id, "agent");
    assert_eq!(subagent.display_name, "Ada");
}
