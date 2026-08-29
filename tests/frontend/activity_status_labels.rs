//! Exhaustive dependency-free coverage for conversation quiet-status labels.

#[path = "../../modules/frontend/src/activity_status_labels.rs"]
mod activity_status_labels;

use activity_status_labels::{
    ActiveWorkLabelInput, FALLBACK_THINKING_WORDS, ThinkingVisibilityState, active_work_label_for,
    advance_thinking_visibility, background_work_label_for, code_point_fnv1a, compacting_label_for,
    javascript_code_point_fnv1a, resolve_thinking_vocabulary, thinking_vocabulary_is_valid,
    thinking_word_at, thinking_word_for, waiting_label_for,
};

const TEST_WORDS: [&str; 5] = ["Alpha", "Beta", "Gamma", "Delta", "Epsilon"];

fn fallback_words_owned() -> Vec<String> {
    FALLBACK_THINKING_WORDS
        .iter()
        .map(|word| (*word).to_owned())
        .collect()
}

fn label_input<'a>(
    background_agent_names: &'a [&'a str],
    provider_responded: bool,
) -> ActiveWorkLabelInput<'a> {
    ActiveWorkLabelInput {
        awaiting_compaction: false,
        background_agent_names,
        engine_name: None,
        provider_responded,
        reasoning_summary: None,
        seed: "session-1",
        thinking_visibility_generation: 0,
        waiting_for_activity: false,
    }
}

#[test]
fn fallback_vocabulary_is_exact_and_invalid_input_is_repaired_as_a_whole() {
    assert_eq!(
        FALLBACK_THINKING_WORDS,
        [
            "Pondering",
            "Percolating",
            "Recombobulating",
            "Puttering",
            "Zesting",
        ]
    );

    let empty: [&str; 0] = [];
    let duplicate = ["Alpha", "Alpha", "Beta"];
    let empty_word = ["Alpha", ""];
    let whitespace_word = [" "];

    for vocabulary in [&empty[..], &duplicate[..], &empty_word[..]] {
        assert!(!thinking_vocabulary_is_valid(vocabulary));
        assert_eq!(
            resolve_thinking_vocabulary(vocabulary),
            fallback_words_owned()
        );
    }
    assert!(thinking_vocabulary_is_valid(&whitespace_word));
    assert_eq!(
        resolve_thinking_vocabulary(&whitespace_word),
        vec![" ".to_owned()]
    );

    let valid = ["Alpha", "Beta", "Gamma"];
    assert!(thinking_vocabulary_is_valid(&valid));
    assert_eq!(
        resolve_thinking_vocabulary(&valid),
        vec!["Alpha", "Beta", "Gamma"]
    );
}

#[test]
fn quiet_status_messages_preserve_every_exact_string_and_boundary() {
    let engine_cases = [
        (None, None),
        (
            Some("Claude"),
            Some("Waiting for Claude to respond…".to_owned()),
        ),
        (Some(""), Some("Waiting for  to respond…".to_owned())),
        (
            Some("引擎 🚀"),
            Some("Waiting for 引擎 🚀 to respond…".to_owned()),
        ),
    ];
    for (engine, expected) in engine_cases {
        assert_eq!(waiting_label_for(engine), expected);
    }

    assert_eq!(compacting_label_for(false), None);
    assert_eq!(
        compacting_label_for(true),
        Some("Compacting the conversation…")
    );

    let empty: [&str; 0] = [];
    let one = ["worker-a"];
    let two = ["worker-a", "worker-b"];
    let many = ["worker-a", "worker-b", "worker-c"];
    let four = ["worker-a", "worker-b", "worker-c", "worker-d"];
    let empty_name = [""];
    let two_with_empty_name = ["", "Ada"];
    let background_cases: &[(&[&str], Option<String>)] = &[
        (&empty, None),
        (&one, Some("Waiting for worker-a to finish…".to_owned())),
        (
            &two,
            Some("Waiting for worker-a and worker-b to finish…".to_owned()),
        ),
        (&many, Some("Waiting for 3 background agents…".to_owned())),
        (&four, Some("Waiting for 4 background agents…".to_owned())),
        (&empty_name, Some("Waiting for  to finish…".to_owned())),
        (
            &two_with_empty_name,
            Some("Waiting for  and Ada to finish…".to_owned()),
        ),
    ];
    for (names, expected) in background_cases {
        assert_eq!(background_work_label_for(names), *expected);
    }
}

#[test]
fn active_label_priority_live_activity_beats_everything() {
    let many_workers = ["worker-a", "worker-b", "worker-c"];
    let mut input = label_input(&many_workers, true);
    input.awaiting_compaction = true;
    input.engine_name = Some("Codex");
    input.reasoning_summary = Some("summarizing");
    input.thinking_visibility_generation = 4;
    input.waiting_for_activity = true;

    assert_eq!(active_work_label_for(input, &TEST_WORDS), "Waiting");
}

#[test]
fn active_label_priority_before_response_is_compaction_engine_then_word() {
    let no_workers: [&str; 0] = [];
    let many_workers = ["worker-a", "worker-b", "worker-c"];

    let mut compacting = label_input(&many_workers, false);
    compacting.awaiting_compaction = true;
    compacting.engine_name = Some("Codex");
    compacting.reasoning_summary = Some("summarizing");
    compacting.thinking_visibility_generation = 4;
    assert_eq!(
        active_work_label_for(compacting, &TEST_WORDS),
        "Compacting the conversation…"
    );

    let mut engine_wait = label_input(&many_workers, false);
    engine_wait.engine_name = Some("Codex");
    engine_wait.reasoning_summary = Some("summarizing");
    engine_wait.thinking_visibility_generation = 4;
    assert_eq!(
        active_work_label_for(engine_wait, &TEST_WORDS),
        "Waiting for Codex to respond…"
    );

    let mut thinking = label_input(&no_workers, false);
    thinking.reasoning_summary = Some("summarizing");
    thinking.thinking_visibility_generation = 4;
    let expected = thinking_word_for("session-1", 4, &TEST_WORDS);
    assert_eq!(active_work_label_for(thinking, &TEST_WORDS), expected);
}

#[test]
fn active_label_priority_after_response_is_background_summary_then_word() {
    let no_workers: [&str; 0] = [];
    let one_worker = ["worker-a"];
    let many_workers = ["worker-a", "worker-b", "worker-c"];
    let empty_name = [""];

    let mut background = label_input(&many_workers, true);
    background.engine_name = Some("Codex");
    background.reasoning_summary = Some("summarizing");
    background.thinking_visibility_generation = 4;
    assert_eq!(
        active_work_label_for(background, &TEST_WORDS),
        "Waiting for 3 background agents…"
    );

    let mut one_background = label_input(&one_worker, true);
    one_background.engine_name = Some("Codex");
    one_background.reasoning_summary = Some("summarizing");
    one_background.thinking_visibility_generation = 4;
    assert_eq!(
        active_work_label_for(one_background, &TEST_WORDS),
        "Waiting for worker-a to finish…"
    );

    let mut summary = label_input(&no_workers, true);
    summary.engine_name = Some("Codex");
    summary.reasoning_summary = Some("summarizing");
    summary.thinking_visibility_generation = 4;
    assert_eq!(active_work_label_for(summary, &TEST_WORDS), "summarizing");

    let mut empty_summary = label_input(&no_workers, true);
    empty_summary.engine_name = Some("Codex");
    empty_summary.reasoning_summary = Some("");
    empty_summary.thinking_visibility_generation = 4;
    assert_eq!(active_work_label_for(empty_summary, &TEST_WORDS), "");

    let mut thinking = label_input(&no_workers, true);
    thinking.engine_name = Some("Codex");
    thinking.thinking_visibility_generation = 4;
    let expected = thinking_word_for("session-1", 4, &TEST_WORDS);
    assert_eq!(active_work_label_for(thinking, &TEST_WORDS), expected);

    let mut empty_name_input = label_input(&empty_name, true);
    empty_name_input.engine_name = Some("Codex");
    empty_name_input.reasoning_summary = Some("summarizing");
    empty_name_input.thinking_visibility_generation = 4;
    assert_eq!(
        active_work_label_for(empty_name_input, &TEST_WORDS),
        "Waiting for  to finish…"
    );
}

#[test]
fn empty_engine_and_reasoning_are_distinct_from_absent_values() {
    let no_workers: [&str; 0] = [];

    let mut before_response = label_input(&no_workers, false);
    before_response.engine_name = Some("");
    assert_eq!(
        active_work_label_for(before_response, &TEST_WORDS),
        "Waiting for  to respond…"
    );

    let mut after_response = label_input(&no_workers, true);
    after_response.reasoning_summary = Some("");
    assert_eq!(active_work_label_for(after_response, &TEST_WORDS), "");

    after_response.reasoning_summary = None;
    assert_eq!(
        active_work_label_for(after_response, &TEST_WORDS),
        thinking_word_for("session-1", 0, &TEST_WORDS)
    );
}

#[test]
fn activity_wait_dominates_all_other_values() {
    let workers = ["", "Ada", "Faye"];
    let input = ActiveWorkLabelInput {
        awaiting_compaction: true,
        background_agent_names: &workers,
        engine_name: Some(""),
        provider_responded: false,
        reasoning_summary: Some(""),
        seed: "non-BMP-🚀",
        thinking_visibility_generation: u64::MAX,
        waiting_for_activity: true,
    };

    assert_eq!(active_work_label_for(input, &TEST_WORDS), "Waiting");
}

#[test]
fn code_point_hash_vectors_match_javascript_iteration_and_imul() {
    let vectors = [
        ("", 2_166_136_261_u32),
        ("ASCII", 3_164_024_976),
        ("café", 856_211_068),
        ("💡", 2_773_280_876),
        ("a💡z", 1_683_909_111),
        ("𐐷", 865_687_542),
        ("😀", 105_948_959),
        ("é", 1_812_687_940),
        ("hello世界", 3_026_333_073),
        ("𝄞", 471_858_369),
    ];

    for (seed, expected) in vectors {
        assert_eq!(code_point_fnv1a(seed), expected, "seed={seed}");
        assert_eq!(
            javascript_code_point_fnv1a(seed),
            expected,
            "alias seed={seed}"
        );
    }
}

#[test]
fn selection_is_deterministic_and_wraps_indices_and_generations() {
    let expected = thinking_word_for("stable-session", 7, &TEST_WORDS);
    for _ in 0..256 {
        assert_eq!(
            thinking_word_for("stable-session", 7, &TEST_WORDS),
            expected
        );
    }

    let index_cases = [
        (0, "Alpha"),
        (TEST_WORDS.len(), "Alpha"),
        (TEST_WORDS.len() + 1, "Beta"),
        (usize::MAX, TEST_WORDS[usize::MAX % TEST_WORDS.len()]),
    ];
    for (index, expected) in index_cases {
        assert_eq!(thinking_word_at(index, &TEST_WORDS), expected);
    }

    let invalid: [&str; 0] = [];
    assert_eq!(thinking_word_at(2, &invalid), FALLBACK_THINKING_WORDS[2]);

    let hash = u64::from(code_point_fnv1a("generation-boundary"));
    let vocabulary_length = u64::try_from(TEST_WORDS.len()).unwrap_or_default();
    let generation_expected = |generation: u64| {
        let index =
            usize::try_from(hash.wrapping_add(generation) % vocabulary_length).unwrap_or_default();
        TEST_WORDS.get(index).copied().unwrap_or_default()
    };
    assert_eq!(
        thinking_word_for("generation-boundary", 0, &TEST_WORDS),
        generation_expected(0)
    );
    assert_eq!(
        thinking_word_for("generation-boundary", u64::MAX, &TEST_WORDS),
        generation_expected(u64::MAX)
    );
}

#[test]
fn visibility_advances_only_on_a_real_reappearance() {
    let mut state = ThinkingVisibilityState::new();
    assert_eq!(state.generation(), 0);
    assert!(!state.was_visible());
    assert!(!state.has_appeared());

    advance_thinking_visibility(&mut state, false);
    assert_eq!(state.generation(), 0);
    advance_thinking_visibility(&mut state, true);
    assert_eq!(state.generation(), 0);
    assert!(state.was_visible());
    assert!(state.has_appeared());
    advance_thinking_visibility(&mut state, true);
    assert_eq!(state.generation(), 0);
    advance_thinking_visibility(&mut state, false);
    assert_eq!(state.generation(), 0);
    advance_thinking_visibility(&mut state, false);
    assert_eq!(state.generation(), 0);
    state.reconcile(true);
    assert_eq!(state.generation(), 1);
    state.reconcile(false);
    state.reconcile(true);
    assert_eq!(state.generation(), 2);
}

#[test]
fn visibility_generation_wraps_at_u64_boundary() {
    let mut state = ThinkingVisibilityState::from_generation(u64::MAX);
    assert_eq!(state.generation(), u64::MAX);
    state.reconcile(false);
    state.reconcile(true);
    assert_eq!(state.generation(), 0);
}
