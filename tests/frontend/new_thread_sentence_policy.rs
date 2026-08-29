//! Exhaustive, dependency-free coverage for the new-thread sentence policy.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_precision_loss)]

#[path = "../../modules/frontend/src/new_thread_sentence_policy.rs"]
mod new_thread_sentence_policy;

use std::fmt::Write as _;

use new_thread_sentence_policy::{
    FALLBACK_SENTENCE, MAX_RANDOM_UNIT, NEW_THREAD_SENTENCES, NewThreadSentenceWord,
    PROJECT_MARKER, STAGGER_STEP_MS, STAGGERED_WORDS, new_thread_sentence_words,
    pick_default_new_thread_sentence, pick_new_thread_sentence,
    pick_production_new_thread_sentence, sentence_word_delay,
};

fn expected_word<'a>(
    delay_ms: u64,
    leading_space: bool,
    prefix: &'a str,
    project: bool,
    suffix: &'a str,
    text: &'a str,
) -> NewThreadSentenceWord<'a> {
    NewThreadSentenceWord {
        delay_ms,
        leading_space,
        prefix,
        project,
        suffix,
        text,
    }
}

#[test]
fn marker_and_six_templates_are_exact_and_name_the_project_once() {
    assert_eq!(PROJECT_MARKER, "{project}");
    assert_eq!(
        NEW_THREAD_SENTENCES,
        [
            "What are we building in {project} today?",
            "What should happen in {project} next?",
            "A new thread in {project}.",
            "Pick up {project} where you left it.",
            "Where would you like to start in {project}?",
            "{project} is open. What first?",
        ]
    );
    assert_eq!(NEW_THREAD_SENTENCES.len(), 6);
    assert_eq!(FALLBACK_SENTENCE, "A new thread in {project}.");
    assert_eq!(STAGGER_STEP_MS, 45);
    assert_eq!(STAGGERED_WORDS, 10);
    assert_eq!(MAX_RANDOM_UNIT, 0.999_999);

    for sentence in NEW_THREAD_SENTENCES {
        assert_eq!(sentence.matches(PROJECT_MARKER).count(), 1, "{sentence}");
    }
}

#[test]
fn every_vocabulary_slot_is_reachable_at_its_lower_boundary() {
    let vocabulary = ["zero", "one", "two", "three", "four", "five"];
    let boundaries = [0.0, 1.0 / 6.0, 2.0 / 6.0, 3.0 / 6.0, 4.0 / 6.0, 5.0 / 6.0];

    for (index, (random_unit, expected)) in boundaries.into_iter().zip(vocabulary).enumerate() {
        assert_eq!(
            pick_new_thread_sentence(None, random_unit, &vocabulary),
            expected,
            "slot {index} at random unit {random_unit}"
        );
    }
}

#[test]
fn random_units_are_clamped_before_indexing() {
    let vocabulary = ["zero", "one", "two", "three"];
    let cases = [
        (-f64::INFINITY, "zero"),
        (-1.0, "zero"),
        (-f64::MIN_POSITIVE, "zero"),
        (0.0, "zero"),
        (0.249_999, "zero"),
        (0.25, "one"),
        (0.5, "two"),
        (0.75, "three"),
        (0.999_998, "three"),
        (MAX_RANDOM_UNIT, "three"),
        (1.0, "three"),
        (1.5, "three"),
        (f64::INFINITY, "three"),
    ];

    for (random_unit, expected) in cases {
        assert_eq!(
            pick_new_thread_sentence(None, random_unit, &vocabulary),
            expected,
            "random unit {random_unit}"
        );
    }
}

#[test]
fn default_wrapper_uses_the_same_deterministic_selection() {
    for (index, expected) in NEW_THREAD_SENTENCES.iter().enumerate() {
        let random_unit = index as f64 / NEW_THREAD_SENTENCES.len() as f64;
        assert_eq!(
            pick_default_new_thread_sentence(None, random_unit),
            *expected
        );
        assert_eq!(
            pick_default_new_thread_sentence(None, random_unit),
            pick_new_thread_sentence(None, random_unit, &NEW_THREAD_SENTENCES)
        );
        assert_eq!(
            pick_production_new_thread_sentence(None, random_unit),
            *expected
        );
    }
}

#[test]
fn previous_selection_rotates_to_the_next_slot_for_every_index() {
    let vocabulary = ["first", "second", "third", "fourth"];

    for (index, current) in vocabulary.iter().enumerate() {
        let random_unit = index as f64 / vocabulary.len() as f64;
        let next = vocabulary[(index + 1) % vocabulary.len()];
        assert_eq!(
            pick_new_thread_sentence(Some(current), random_unit, &vocabulary),
            next,
            "previous value {current} at slot {index}"
        );
    }
}

#[test]
fn one_choice_and_empty_vocabularies_follow_the_fallback_rules() {
    let one_choice = ["only"];
    assert_eq!(
        pick_new_thread_sentence(Some("only"), 0.0, &one_choice),
        "only"
    );
    assert_eq!(pick_new_thread_sentence(None, 1.0, &one_choice), "only");

    let empty: [&str; 0] = [];
    assert_eq!(
        pick_new_thread_sentence(None, 0.5, &empty),
        FALLBACK_SENTENCE
    );
    assert_eq!(
        pick_new_thread_sentence(Some("A new thread in {project}."), 0.5, &empty),
        FALLBACK_SENTENCE
    );
}

#[test]
fn custom_vocabulary_preserves_order_duplicates_and_exact_text() {
    let vocabulary = ["🦀 first", "", "second  ", "🧭 first"];

    assert_eq!(pick_new_thread_sentence(None, 0.0, &vocabulary), "🦀 first");
    assert_eq!(pick_new_thread_sentence(None, 0.25, &vocabulary), "");
    assert_eq!(pick_new_thread_sentence(None, 0.5, &vocabulary), "second  ");
    assert_eq!(
        pick_new_thread_sentence(None, 0.75, &vocabulary),
        "🧭 first"
    );

    let duplicates = ["same", "same", "other"];
    assert_eq!(
        pick_new_thread_sentence(Some("same"), 0.0, &duplicates),
        "same",
        "rotation advances by index, not by searching for a different value"
    );
}

#[test]
fn nan_selection_uses_the_source_array_lookup_fallback() {
    let vocabulary = ["first", "second"];
    assert_eq!(
        pick_new_thread_sentence(None, f64::NAN, &vocabulary),
        FALLBACK_SENTENCE
    );
}

#[test]
fn delay_table_clamps_negative_and_caps_after_index_ten() {
    let cases = [
        (i64::MIN, 0),
        (-3, 0),
        (-1, 0),
        (0, 0),
        (1, 45),
        (9, 405),
        (10, 450),
        (11, 450),
        (40, 450),
        (i64::MAX, 450),
    ];

    for (index, expected) in cases {
        assert_eq!(sentence_word_delay(index), expected, "word index {index}");
    }

    for index in 0..=STAGGERED_WORDS {
        let expected = u64::try_from(index).unwrap_or(0) * STAGGER_STEP_MS;
        assert_eq!(sentence_word_delay(index), expected, "word index {index}");
    }
    assert_eq!(sentence_word_delay(usize::MAX), 450);
}

#[test]
fn empty_and_ecmascript_whitespace_only_templates_have_no_words() {
    let all_ecmascript_whitespace = concat!(
        "\u{0009}", "\u{000A}", "\u{000B}", "\u{000C}", "\u{000D}", "\u{0020}", "\u{00A0}",
        "\u{1680}", "\u{2000}", "\u{2001}", "\u{2002}", "\u{2003}", "\u{2004}", "\u{2005}",
        "\u{2006}", "\u{2007}", "\u{2008}", "\u{2009}", "\u{200A}", "\u{2028}", "\u{2029}",
        "\u{202F}", "\u{205F}", "\u{3000}", "\u{FEFF}",
    );

    assert!(new_thread_sentence_words("").is_empty());
    assert!(new_thread_sentence_words(all_ecmascript_whitespace).is_empty());
    assert!(new_thread_sentence_words(" \t\n\u{FEFF}\u{3000} ").is_empty());
}

#[test]
fn non_ecmascript_separators_remain_literal_token_text() {
    let words = new_thread_sentence_words("left\u{0085}right zero\u{200B}width");

    assert_eq!(
        words,
        vec![
            expected_word(0, false, "", false, "", "left\u{0085}right"),
            expected_word(45, true, "", false, "", "zero\u{200B}width"),
        ]
    );
}

#[test]
fn multiple_ecmascript_whitespace_kinds_collapse_to_semantic_spaces() {
    let words = new_thread_sentence_words(
        "  one\u{0009}\u{00A0}\u{2003}\u{2028}\u{FEFF}two\u{3000}three  ",
    );

    assert_eq!(
        words,
        vec![
            expected_word(0, false, "", false, "", "one"),
            expected_word(45, true, "", false, "", "two"),
            expected_word(90, true, "", false, "", "three"),
        ]
    );
}

#[test]
fn marker_only_prefixed_suffixed_and_mid_token_records_keep_attachment_exact() {
    assert_eq!(
        new_thread_sentence_words(PROJECT_MARKER),
        vec![expected_word(0, false, "", true, "", "")]
    );
    assert_eq!(
        new_thread_sentence_words("open \"{project}\" now"),
        vec![
            expected_word(0, false, "", false, "", "open"),
            expected_word(45, true, "\"", true, "\"", ""),
            expected_word(90, true, "", false, "", "now"),
        ]
    );
    assert_eq!(
        new_thread_sentence_words("pre{project}post"),
        vec![expected_word(0, false, "pre", true, "post", "")]
    );
    assert_eq!(
        new_thread_sentence_words("mid-{project},end"),
        vec![expected_word(0, false, "mid-", true, ",end", "")]
    );
}

#[test]
fn absent_and_multiple_markers_follow_first_occurrence_detection() {
    let words = new_thread_sentence_words("Unicode 🦀 naïve {project}x{project}! tail");

    assert_eq!(
        words,
        vec![
            expected_word(0, false, "", false, "", "Unicode"),
            expected_word(45, true, "", false, "", "🦀"),
            expected_word(90, true, "", false, "", "naïve"),
            expected_word(135, true, "", true, "x{project}!", ""),
            expected_word(180, true, "", false, "", "tail"),
        ]
    );
}

#[test]
fn records_reconstruct_exact_visible_sentence_with_project_replacement() {
    let template = "  \"{project},\"\u{2003}is\u{00A0}ready!  ";
    let words = new_thread_sentence_words(template);
    let mut rendered = String::new();
    for word in &words {
        let text = if word.project {
            "artisan-editor"
        } else {
            word.text
        };
        write!(
            &mut rendered,
            "{}{}{}{}",
            if word.leading_space { " " } else { "" },
            word.prefix,
            text,
            word.suffix
        )
        .expect("writing to a String cannot fail");
    }

    assert_eq!(rendered, "\"artisan-editor,\" is ready!");
    assert_eq!(words[0].delay_ms, sentence_word_delay(0));
    assert_eq!(words[1].delay_ms, sentence_word_delay(1));
    assert_eq!(words[2].delay_ms, sentence_word_delay(2));
}
