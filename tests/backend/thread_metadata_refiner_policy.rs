//! Focused parity tests for the dependency-free thread metadata refiner.

#![allow(dead_code)]

#[path = "../../modules/backend/src/thread_metadata_refiner_policy.rs"]
mod thread_metadata_refiner_policy;

use std::str::FromStr;

use thread_metadata_refiner_policy::{
    MAX_CONTEXT_ITEMS, MAX_CONTEXT_TEXT_LENGTH, ThreadMetadataProjection, ThreadMetadataRefinement,
    ThreadMetadataRefinementTrigger, ThreadMetadataRefinementTriggerParseError,
    ThreadMetadataRefinerInput, ThreadMetadataRefinerPolicy, bound_thread_metadata_refiner_input,
    javascript_utf16_code_units, refine_thread_metadata,
};

fn projection(
    title: &str,
    current_goal: Option<&str>,
    title_locked: bool,
) -> ThreadMetadataProjection {
    ThreadMetadataProjection::new(title, current_goal.map(str::to_owned), title_locked)
}

#[allow(clippy::too_many_arguments)]
fn input(
    trigger: ThreadMetadataRefinementTrigger,
    current_goal: Option<&str>,
    title: &str,
    title_locked: bool,
    recent_assistant_text: &[&str],
    recent_user_text: &[&str],
    recent_activity: &[&str],
    recent_files: &[&str],
    recent_artifacts: &[&str],
) -> ThreadMetadataRefinerInput {
    ThreadMetadataRefinerInput {
        projection: projection(title, current_goal, title_locked),
        trigger,
        recent_assistant_text: recent_assistant_text
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        recent_user_text: recent_user_text
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        recent_activity: recent_activity
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        recent_files: recent_files
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        recent_artifacts: recent_artifacts
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
    }
}

fn empty_input(trigger: ThreadMetadataRefinementTrigger) -> ThreadMetadataRefinerInput {
    input(
        trigger,
        None,
        "Existing title",
        false,
        &[],
        &[],
        &[],
        &[],
        &[],
    )
}

#[test]
fn trigger_vocabulary_is_exact_and_exhaustive() {
    let cases = [
        (
            ThreadMetadataRefinementTrigger::AssistantMessage,
            "assistant_message",
        ),
        (ThreadMetadataRefinementTrigger::UserMessage, "user_message"),
        (ThreadMetadataRefinementTrigger::RunStarted, "run_started"),
        (
            ThreadMetadataRefinementTrigger::RunCompleted,
            "run_completed",
        ),
        (ThreadMetadataRefinementTrigger::RunFailed, "run_failed"),
    ];

    assert_eq!(
        ThreadMetadataRefinementTrigger::ALL,
        cases.map(|(trigger, _)| trigger)
    );
    for (trigger, spelling) in cases {
        assert_eq!(trigger.as_str(), spelling);
        assert_eq!(trigger.to_string(), spelling);
        assert_eq!(
            ThreadMetadataRefinementTrigger::parse(spelling),
            Ok(trigger)
        );
        assert_eq!(
            spelling.parse::<ThreadMetadataRefinementTrigger>(),
            Ok(trigger)
        );
        assert_eq!(
            ThreadMetadataRefinementTrigger::from_str(spelling),
            Ok(trigger)
        );
    }
}

#[test]
fn trigger_parser_rejects_rewritten_values() {
    for spelling in [
        "ASSISTANT_MESSAGE",
        " user_message",
        "run_failed ",
        "unknown",
    ] {
        assert_eq!(
            ThreadMetadataRefinementTrigger::parse(spelling),
            Err(ThreadMetadataRefinementTriggerParseError),
        );
    }
}

#[test]
fn bounds_are_inclusive_at_seven_eight_and_nine_entries() {
    let seven = ["one", "two", "three", "four", "five", "six", "seven"];
    let eight = [
        "one", "two", "three", "four", "five", "six", "seven", "eight",
    ];
    let nine = [
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];

    for (values, expected) in [
        (&seven[..], seven.to_vec()),
        (&eight[..], eight.to_vec()),
        (&nine[..], nine[1..].to_vec()),
    ] {
        let raw = input(
            ThreadMetadataRefinementTrigger::UserMessage,
            None,
            "title",
            false,
            values,
            values,
            values,
            values,
            values,
        );
        let bounded = bound_thread_metadata_refiner_input(&raw);
        assert_eq!(bounded.recent_user_text, expected);
        assert_eq!(bounded.recent_assistant_text, bounded.recent_user_text);
        assert_eq!(bounded.recent_activity, bounded.recent_user_text);
        assert_eq!(bounded.recent_files, bounded.recent_user_text);
        assert_eq!(bounded.recent_artifacts, bounded.recent_user_text);
    }
    assert_eq!(MAX_CONTEXT_ITEMS, 8);
}

#[test]
fn last_eight_raw_entries_are_selected_before_trim_and_empty_filtering() {
    let raw = input(
        ThreadMetadataRefinementTrigger::UserMessage,
        None,
        "title",
        false,
        &["assistant-1", "assistant-2"],
        &[
            "old-entry",
            "  ",
            "user-3",
            "user-4",
            "user-5",
            "user-6",
            "user-7",
            "user-8",
            "user-9",
        ],
        &["activity-1", "activity-2"],
        &[
            "old-file", " ", "file-3", "file-4", "file-5", "file-6", "file-7", "file-8", "file-9",
        ],
        &["artifact-1", "artifact-2"],
    );
    let bounded = bound_thread_metadata_refiner_input(&raw);

    assert_eq!(
        bounded.recent_user_text,
        [
            "user-3", "user-4", "user-5", "user-6", "user-7", "user-8", "user-9"
        ]
        .map(str::to_owned),
    );
    assert_eq!(
        bounded.recent_files,
        [
            "file-3", "file-4", "file-5", "file-6", "file-7", "file-8", "file-9"
        ]
        .map(str::to_owned),
    );
}

#[test]
fn every_evidence_category_has_its_own_ordered_retention_window() {
    let raw = input(
        ThreadMetadataRefinementTrigger::AssistantMessage,
        None,
        "title",
        false,
        &[" a1 ", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9"],
        &[" u1 ", "u2"],
        &[" activity-1 ", "activity-2"],
        &[" f1 ", "f2"],
        &[" artifact-1 ", "artifact-2"],
    );
    let bounded = bound_thread_metadata_refiner_input(&raw);

    assert_eq!(
        bounded.recent_assistant_text,
        ["a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9"].map(str::to_owned)
    );
    assert_eq!(bounded.recent_user_text, ["u1", "u2"].map(str::to_owned));
    assert_eq!(
        bounded.recent_activity,
        ["activity-1", "activity-2"].map(str::to_owned)
    );
    assert_eq!(bounded.recent_files, ["f1", "f2"].map(str::to_owned));
    assert_eq!(
        bounded.recent_artifacts,
        ["artifact-1", "artifact-2"].map(str::to_owned)
    );
}

#[test]
fn trim_matches_ecmascript_whitespace_and_filters_only_empty_results() {
    let raw = input(
        ThreadMetadataRefinementTrigger::UserMessage,
        None,
        "title",
        false,
        &[],
        &[
            "\u{FEFF}\t\n  hello\u{2003}\u{00A0}world\u{FEFF}",
            "\u{0085}",
            "\u{200B}",
        ],
        &[],
        &[],
        &[],
    );
    let bounded = bound_thread_metadata_refiner_input(&raw);

    assert_eq!(
        bounded.recent_user_text,
        ["hello\u{2003}\u{00A0}world", "\u{0085}", "\u{200B}"].map(str::to_owned),
    );

    let blanks = input(
        ThreadMetadataRefinementTrigger::UserMessage,
        None,
        "title",
        false,
        &[],
        &["", " \t\n", "\u{FEFF}"],
        &[],
        &[],
        &[],
    );
    assert!(
        bound_thread_metadata_refiner_input(&blanks)
            .recent_user_text
            .is_empty()
    );
}

#[test]
fn text_bound_is_inclusive_in_utf16_units() {
    let below = "x".repeat(MAX_CONTEXT_TEXT_LENGTH - 1);
    let exact = "x".repeat(MAX_CONTEXT_TEXT_LENGTH);
    let above = "x".repeat(MAX_CONTEXT_TEXT_LENGTH + 1);
    let raw = input(
        ThreadMetadataRefinementTrigger::UserMessage,
        None,
        "title",
        false,
        &[],
        &[below.as_str(), exact.as_str(), above.as_str()],
        &[],
        &[],
        &[],
    );
    let bounded = bound_thread_metadata_refiner_input(&raw);

    assert_eq!(bounded.recent_user_text[0], below);
    assert_eq!(bounded.recent_user_text[1], exact);
    assert_eq!(
        bounded.recent_user_text[2],
        "x".repeat(MAX_CONTEXT_TEXT_LENGTH)
    );
    assert!(
        bounded
            .recent_user_text
            .iter()
            .all(|value| javascript_utf16_code_units(value) <= MAX_CONTEXT_TEXT_LENGTH)
    );
    assert_eq!(MAX_CONTEXT_TEXT_LENGTH, 500);
}

#[test]
fn astral_text_counts_as_two_units_and_is_never_split() {
    let exactly_250 = "🦀".repeat(250);
    let above = format!("{}x", "🦀".repeat(250));
    let raw = input(
        ThreadMetadataRefinementTrigger::UserMessage,
        None,
        "title",
        false,
        &[],
        &[exactly_250.as_str(), above.as_str()],
        &[],
        &[],
        &[],
    );
    let bounded = bound_thread_metadata_refiner_input(&raw);

    assert_eq!(bounded.recent_user_text[0], exactly_250);
    assert_eq!(bounded.recent_user_text[1], exactly_250);
    assert_eq!(
        javascript_utf16_code_units(&bounded.recent_user_text[1]),
        500
    );
    assert!(bounded.recent_user_text[1].is_char_boundary(bounded.recent_user_text[1].len()));
}

#[test]
fn refinement_uses_latest_user_then_latest_file_then_current_title() {
    let user_wins = input(
        ThreadMetadataRefinementTrigger::RunFailed,
        Some("old goal"),
        "current title",
        false,
        &["assistant must not win"],
        &["first user", " latest user "],
        &["activity must not win"],
        &["first file", " latest file "],
        &["artifact must not win"],
    );
    let expected_user = ThreadMetadataRefinement {
        current_goal: Some("latest user".to_owned()),
        mentioned_projects: None,
        rename_suggestion: Some("latest user".to_owned()),
        title: Some("latest user".to_owned()),
    };
    assert_eq!(refine_thread_metadata(&user_wins), expected_user);

    let file_wins = input(
        ThreadMetadataRefinementTrigger::RunStarted,
        Some("old goal"),
        "current title",
        false,
        &[],
        &[],
        &[],
        &["first file", " latest file "],
        &[],
    );
    assert_eq!(
        refine_thread_metadata(&file_wins),
        ThreadMetadataRefinement {
            current_goal: Some("old goal".to_owned()),
            mentioned_projects: None,
            rename_suggestion: Some("latest file".to_owned()),
            title: Some("latest file".to_owned()),
        },
    );

    let title_wins = empty_input(ThreadMetadataRefinementTrigger::AssistantMessage);
    assert_eq!(
        refine_thread_metadata(&title_wins),
        ThreadMetadataRefinement {
            current_goal: None,
            mentioned_projects: None,
            rename_suggestion: Some("Existing title".to_owned()),
            title: Some("Existing title".to_owned()),
        },
    );
}

#[test]
fn existing_current_goal_is_retained_only_without_user_text() {
    let retained = empty_input(ThreadMetadataRefinementTrigger::RunCompleted);
    assert_eq!(refine_thread_metadata(&retained).current_goal, None);

    let with_goal = input(
        ThreadMetadataRefinementTrigger::RunCompleted,
        Some("keep this goal"),
        "title",
        false,
        &[],
        &[],
        &[],
        &[],
        &[],
    );
    assert_eq!(
        refine_thread_metadata(&with_goal).current_goal,
        Some("keep this goal".to_owned()),
    );

    let replaced = input(
        ThreadMetadataRefinementTrigger::UserMessage,
        Some("old goal"),
        "title",
        false,
        &[],
        &["new goal"],
        &[],
        &[],
        &[],
    );
    assert_eq!(
        refine_thread_metadata(&replaced).current_goal,
        Some("new goal".to_owned()),
    );

    let empty_goal = input(
        ThreadMetadataRefinementTrigger::RunCompleted,
        Some(""),
        "title",
        false,
        &[],
        &[],
        &[],
        &[],
        &[],
    );
    assert_eq!(refine_thread_metadata(&empty_goal).current_goal, None);
}

#[test]
fn title_lock_suppresses_only_title_and_keeps_rename_suggestion() {
    let locked = input(
        ThreadMetadataRefinementTrigger::UserMessage,
        Some("old goal"),
        "manual title",
        true,
        &[],
        &["suggested title"],
        &[],
        &["file fallback"],
        &[],
    );

    let refinement = refine_thread_metadata(&locked);
    assert_eq!(
        refinement.rename_suggestion,
        Some("suggested title".to_owned())
    );
    assert_eq!(refinement.title, None);
    assert_eq!(refinement.current_goal, Some("suggested title".to_owned()));
    assert_eq!(refinement.mentioned_projects, None);
}

#[test]
fn no_mentioned_projects_are_invented_and_input_categories_remain_unchanged() {
    let raw = input(
        ThreadMetadataRefinementTrigger::UserMessage,
        None,
        "title",
        false,
        &["assistant"],
        &["user"],
        &["activity"],
        &["file"],
        &["artifact"],
    );
    let before = raw.clone();
    let refinement = ThreadMetadataRefinerPolicy::refine(&raw);

    assert_eq!(refinement.mentioned_projects, None);
    assert_eq!(raw, before);
}

#[test]
fn all_triggers_share_the_same_pure_refinement_result() {
    let results = ThreadMetadataRefinementTrigger::ALL.map(|trigger| {
        refine_thread_metadata(&input(
            trigger,
            Some("goal"),
            "title",
            false,
            &["assistant"],
            &["user"],
            &["activity"],
            &["file"],
            &["artifact"],
        ))
    });

    assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn policy_value_and_free_functions_are_equivalent() {
    let raw = input(
        ThreadMetadataRefinementTrigger::RunStarted,
        Some("goal"),
        "title",
        false,
        &[" a "],
        &[" u "],
        &[" activity "],
        &[" f "],
        &[" artifact "],
    );

    assert_eq!(
        ThreadMetadataRefinerPolicy::bound_input(&raw),
        bound_thread_metadata_refiner_input(&raw),
    );
    assert_eq!(
        ThreadMetadataRefinerPolicy::refine(&raw),
        refine_thread_metadata(&raw),
    );
    assert_eq!(
        ThreadMetadataRefinerPolicy::new(),
        ThreadMetadataRefinerPolicy
    );
}
