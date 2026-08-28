//! Focused, dependency-free coverage for conversation quiet-status labels.

#[path = "../../modules/frontend/src/conversation_status_labels.rs"]
mod conversation_status_labels;

use conversation_status_labels::{
    ActiveWorkLabelInput, FALLBACK_THINKING_WORDS, ThinkingVisibilityState, active_work_label_for,
    advance_thinking_visibility, background_work_label_for, compacting_label_for,
    javascript_utf16_fnv1a, resolve_thinking_vocabulary, thinking_vocabulary_is_valid,
    thinking_word_at, thinking_word_for, utf16_fnv1a, waiting_label_for,
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
fn invalid_vocabulary_always_resolves_to_the_exact_five_word_fallback() {
    let empty: [&str; 0] = [];
    let duplicate = ["Alpha", "Alpha", "Beta"];
    let empty_word = ["Alpha", ""];

    assert!(!thinking_vocabulary_is_valid(&empty));
    assert!(!thinking_vocabulary_is_valid(&duplicate));
    assert!(!thinking_vocabulary_is_valid(&empty_word));
    assert_eq!(resolve_thinking_vocabulary(&empty), fallback_words_owned());
    assert_eq!(
        resolve_thinking_vocabulary(&duplicate),
        fallback_words_owned()
    );
    assert_eq!(
        resolve_thinking_vocabulary(&empty_word),
        fallback_words_owned()
    );

    let valid = ["Alpha", "Beta", "Gamma"];
    assert!(thinking_vocabulary_is_valid(&valid));
    assert_eq!(
        resolve_thinking_vocabulary(&valid),
        vec!["Alpha", "Beta", "Gamma"]
    );
}

#[test]
fn quiet_status_messages_preserve_exact_copy_and_ellipsis() {
    assert_eq!(waiting_label_for(None), None);
    assert_eq!(
        waiting_label_for(Some("Claude")),
        Some("Waiting for Claude to respond…".to_owned())
    );
    assert_eq!(compacting_label_for(false), None);
    assert_eq!(
        compacting_label_for(true),
        Some("Compacting the conversation…")
    );

    let none: [&str; 0] = [];
    let one = ["worker-a"];
    let two = ["worker-a", "worker-b"];
    let many = ["worker-a", "worker-b", "worker-c"];
    assert_eq!(background_work_label_for(&none), None);
    assert_eq!(
        background_work_label_for(&one),
        Some("Waiting for worker-a to finish…".to_owned())
    );
    assert_eq!(
        background_work_label_for(&two),
        Some("Waiting for worker-a and worker-b to finish…".to_owned())
    );
    assert_eq!(
        background_work_label_for(&many),
        Some("Waiting for 3 background agents…".to_owned())
    );
}

#[test]
fn active_label_precedence_covers_activity_wait_and_both_provider_phases() {
    let no_workers: [&str; 0] = [];
    let one_worker = ["worker-a"];
    let many_workers = ["worker-a", "worker-b", "worker-c"];

    let mut activity_wait = label_input(&many_workers, true);
    activity_wait.waiting_for_activity = true;
    activity_wait.awaiting_compaction = true;
    activity_wait.engine_name = Some("Codex");
    activity_wait.reasoning_summary = Some("summarizing");
    assert_eq!(active_work_label_for(activity_wait, &TEST_WORDS), "Waiting");

    let mut compacting = label_input(&many_workers, false);
    compacting.awaiting_compaction = true;
    compacting.engine_name = Some("Codex");
    assert_eq!(
        active_work_label_for(compacting, &TEST_WORDS),
        "Compacting the conversation…"
    );

    let mut engine_wait = label_input(&no_workers, false);
    engine_wait.engine_name = Some("Claude");
    assert_eq!(
        active_work_label_for(engine_wait, &TEST_WORDS),
        "Waiting for Claude to respond…"
    );

    let before_response_thinking = label_input(&no_workers, false);
    assert_eq!(
        active_work_label_for(before_response_thinking, &TEST_WORDS),
        thinking_word_for("session-1", 0, &TEST_WORDS)
    );

    let mut background_wait = label_input(&many_workers, true);
    background_wait.reasoning_summary = Some("summarizing");
    assert_eq!(
        active_work_label_for(background_wait, &TEST_WORDS),
        "Waiting for 3 background agents…"
    );

    let mut summary = label_input(&one_worker, true);
    summary.background_agent_names = &no_workers;
    summary.reasoning_summary = Some("summarizing");
    assert_eq!(active_work_label_for(summary, &TEST_WORDS), "summarizing");

    let after_response_thinking = label_input(&no_workers, true);
    assert_eq!(
        active_work_label_for(after_response_thinking, &TEST_WORDS),
        thinking_word_for("session-1", 0, &TEST_WORDS)
    );
}

fn utf16_hash_oracle(seed: &str) -> u32 {
    let mut hash = 2_166_136_261_u32;
    for code_unit in seed.encode_utf16() {
        hash ^= u32::from(code_unit);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

#[test]
fn unicode_seed_selection_matches_the_utf16_javascript_oracle() {
    for seed in ["ASCII", "café", "💡", "a💡z", "𐐷"] {
        assert_eq!(utf16_fnv1a(seed), utf16_hash_oracle(seed), "seed={seed}");
        assert_eq!(
            javascript_utf16_fnv1a(seed),
            utf16_hash_oracle(seed),
            "alias seed={seed}"
        );
        let expected = TEST_WORDS[(u64::from(utf16_hash_oracle(seed)) % 5) as usize];
        assert_eq!(thinking_word_for(seed, 0, &TEST_WORDS), expected);
    }
}

#[test]
fn selection_is_deterministic_for_repeated_same_epoch_requests() {
    let expected = thinking_word_for("stable-session", 7, &TEST_WORDS);
    for _ in 0..256 {
        assert_eq!(
            thinking_word_for("stable-session", 7, &TEST_WORDS),
            expected
        );
    }

    assert_eq!(thinking_word_at(0, &TEST_WORDS), "Alpha");
    assert_eq!(thinking_word_at(TEST_WORDS.len() + 1, &TEST_WORDS), "Beta");
    let invalid: [&str; 0] = [];
    assert_eq!(thinking_word_at(2, &invalid), FALLBACK_THINKING_WORDS[2]);
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
