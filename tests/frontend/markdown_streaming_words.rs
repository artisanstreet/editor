//! Exhaustive table coverage for the dependency-free streaming-word policy.

#[path = "../../modules/frontend/src/markdown_streaming_words.rs"]
mod markdown_streaming_words;

use markdown_streaming_words::{
    StreamingNode, StreamingWordAnimationSelection, StreamingWordDelayEvent,
    StreamingWordDelayOutcome, StreamingWordsTarget, count_pending_streaming_words,
    decide_streaming_word_delay_or_target, find_next_reveal_boundary, get_streaming_word_pacing,
    is_append_only_streaming_target, reveal_streaming_words, select_latest_word_animation,
    should_animate_latest_word, should_animate_streaming_target, wrap_streaming_words,
    wrap_streaming_words_with_reused_prefix,
};

fn target(text: &str, streaming: bool) -> StreamingWordsTarget {
    StreamingWordsTarget::new(text, streaming)
}

#[test]
fn target_replacement_and_append_detection_are_table_driven() {
    let cases = [
        ("", "", true, true),
        ("", "new", true, true),
        ("prefix", "prefix", false, true),
        ("prefix", "prefix more", true, true),
        ("prefix", "prefix\u{00A0}more", true, true),
        ("prefix", "replacement", true, false),
        ("longer", "long", false, false),
        ("café", "café au lait", true, true),
    ];

    for (current, next, streaming, expected) in cases {
        assert_eq!(
            is_append_only_streaming_target(current, &target(next, streaming)),
            expected,
            "current={current:?}, next={next:?}"
        );
    }
}

#[test]
fn animation_eligibility_preserves_live_and_unsettled_rules() {
    let cases = [
        (false, true, true),
        (false, false, true),
        (true, true, true),
        (true, false, false),
    ];

    for (presentation_settled, streaming, expected) in cases {
        assert_eq!(
            should_animate_streaming_target(presentation_settled, &target("payload", streaming),),
            expected,
            "settled={presentation_settled}, streaming={streaming}"
        );
    }
}

#[test]
fn reveal_boundaries_preserve_whitespace_and_unicode_word_boundaries() {
    let cases = [
        ("", "one two", true, "one ", 4),
        ("one", "one two", true, "one ", 4),
        ("one ", "one two", true, "one ", 4),
        ("", "one two", false, "one ", 4),
        ("one ", "one two", false, "one two", 7),
        ("", "one", true, "", 0),
        ("", "one", false, "one", 3),
        ("", "one ", true, "one ", 4),
        ("", "café monde", true, "café ", 6),
        ("", "one\u{00A0}two", true, "one\u{00A0}", 5),
        ("", "one\u{2003}two", true, "one\u{2003}", 6),
        ("", "\u{FEFF}word next", true, "\u{FEFF}", 3),
        ("", "😀 beta", true, "😀 ", 5),
        // ECMAScript \s does not classify NEL as whitespace.
        ("", "one\u{0085}two", true, "", 0),
    ];

    for (current, text, streaming, expected_text, expected_boundary) in cases {
        let destination = target(text, streaming);
        assert_eq!(
            find_next_reveal_boundary(current, &destination),
            expected_boundary,
            "boundary current={current:?}, target={text:?}"
        );
        assert_eq!(
            reveal_streaming_words(current, &destination, 1),
            expected_text,
            "reveal current={current:?}, target={text:?}"
        );
    }
}

#[test]
fn replacement_reveals_all_and_reveal_count_stays_bounded_by_ticks() {
    let replacement = target("new response", true);
    assert_eq!(
        reveal_streaming_words("old response", &replacement, 1),
        "new response"
    );
    assert_eq!(find_next_reveal_boundary("old response", &replacement), 12);
    assert_eq!(
        count_pending_streaming_words("old response", &replacement),
        0
    );

    let settled = target("one two three", false);
    assert_eq!(reveal_streaming_words("", &settled, 1), "one ");
    assert_eq!(reveal_streaming_words("", &settled, 2), "one two ");
    assert_eq!(reveal_streaming_words("one ", &settled, 1), "one two ");
    assert_eq!(reveal_streaming_words("one ", &settled, 2), "one two three");

    let open = target("one two", true);
    assert_eq!(reveal_streaming_words("", &open, 2), "one ");
    assert_eq!(reveal_streaming_words("one ", &open, 2), "one ");
}

#[test]
fn pending_word_count_matches_append_only_and_unicode_runs() {
    let cases = [
        ("", "one two", true, 2),
        ("one ", "one two", true, 1),
        ("one", "one two", true, 1),
        ("one ", "one two", false, 1),
        ("", "one\u{00A0}two\u{2003}three", true, 3),
        ("", "one\u{0085}two", true, 1),
        ("old", "new text", true, 0),
        ("one two", "one two", true, 0),
    ];

    for (current, text, streaming, expected) in cases {
        assert_eq!(
            count_pending_streaming_words(current, &target(text, streaming)),
            expected,
            "current={current:?}, target={text:?}"
        );
    }
}

#[test]
fn every_backlog_threshold_selects_the_exact_legacy_pacing() {
    let cases = [
        (0, 40, 1),
        (1, 40, 1),
        (4, 40, 1),
        (5, 28, 1),
        (12, 28, 1),
        (13, 20, 2),
        (32, 20, 2),
        (33, 16, 4),
        (96, 16, 4),
        (97, 16, 8),
        (usize::MAX, 16, 8),
    ];

    for (backlog, delay_ms, words) in cases {
        assert_eq!(
            get_streaming_word_pacing(backlog),
            markdown_streaming_words::StreamingWordPacing { delay_ms, words },
            "backlog={backlog}"
        );
    }
}

#[test]
fn delay_outcomes_are_pure_typed_decisions() {
    let append = target("prefix more", true);
    let replacement = target("new snapshot", false);
    let cases = [
        (
            StreamingWordDelayEvent::Elapsed,
            StreamingWordDelayOutcome::Elapsed,
        ),
        (
            StreamingWordDelayEvent::Target {
                target: append.clone(),
            },
            StreamingWordDelayOutcome::Target { target: append },
        ),
        (
            StreamingWordDelayEvent::Target {
                target: replacement.clone(),
            },
            StreamingWordDelayOutcome::Target {
                target: replacement,
            },
        ),
    ];

    for (event, expected) in cases {
        assert_eq!(decide_streaming_word_delay_or_target(event), expected);
    }
}

#[test]
fn animation_generation_selection_is_single_use_and_pure() {
    let cases = [
        (
            None,
            None,
            StreamingWordAnimationSelection {
                animate_latest_word: false,
                consumed_generation: None,
            },
        ),
        (
            Some(7),
            None,
            StreamingWordAnimationSelection {
                animate_latest_word: true,
                consumed_generation: Some(7),
            },
        ),
        (
            Some(7),
            Some(7),
            StreamingWordAnimationSelection {
                animate_latest_word: false,
                consumed_generation: Some(7),
            },
        ),
        (
            Some(8),
            Some(7),
            StreamingWordAnimationSelection {
                animate_latest_word: true,
                consumed_generation: Some(8),
            },
        ),
        (
            None,
            Some(8),
            StreamingWordAnimationSelection {
                animate_latest_word: false,
                consumed_generation: Some(8),
            },
        ),
    ];

    for (generation, consumed, expected) in cases {
        assert_eq!(select_latest_word_animation(generation, consumed), expected);
        assert_eq!(
            should_animate_latest_word(generation, consumed),
            expected.animate_latest_word
        );
    }
}

#[test]
fn wrapping_splits_text_keeps_whitespace_raw_and_marks_only_the_latest_word() {
    let nodes = vec![StreamingNode::text("one  two\tthree")];
    let expected = vec![
        StreamingNode::word("one"),
        StreamingNode::text("  "),
        StreamingNode::word("two"),
        StreamingNode::text("\t"),
        StreamingNode::word_with_incoming("three", true),
    ];

    assert_eq!(wrap_streaming_words(&nodes, true), expected);
    assert_eq!(
        wrap_streaming_words(&nodes, false),
        vec![
            StreamingNode::word("one"),
            StreamingNode::text("  "),
            StreamingNode::word("two"),
            StreamingNode::text("\t"),
            StreamingNode::word("three"),
        ]
    );
}

#[test]
fn nested_elements_wrap_in_depth_first_order() {
    let nodes = vec![StreamingNode::element(
        "blockquote",
        vec![
            StreamingNode::text("first "),
            StreamingNode::element("em", vec![StreamingNode::text("second third")]),
            StreamingNode::text(" tail"),
        ],
    )];

    assert_eq!(
        wrap_streaming_words(&nodes, true),
        vec![StreamingNode::element(
            "blockquote",
            vec![
                StreamingNode::word("first"),
                StreamingNode::text(" "),
                StreamingNode::element(
                    "em",
                    vec![
                        StreamingNode::word("second"),
                        StreamingNode::text(" "),
                        StreamingNode::word("third"),
                    ],
                ),
                StreamingNode::text(" "),
                StreamingNode::word_with_incoming("tail", true),
            ],
        )]
    );
}

#[test]
fn every_excluded_subtree_tag_is_case_insensitive_and_opaque() {
    for tag in [
        "code", "CODE", "math", "MATH", "mermaid", "MERMAID", "pre", "PRE",
    ] {
        let excluded = StreamingNode::element(tag, vec![StreamingNode::text("raw one two")]);
        let nodes = vec![excluded.clone(), StreamingNode::text("visible word")];
        let result = wrap_streaming_words(&nodes, true);

        assert_eq!(result[0], excluded, "tag={tag}");
        assert_eq!(
            result[1..],
            [
                StreamingNode::word("visible"),
                StreamingNode::text(" "),
                StreamingNode::word_with_incoming("word", true),
            ],
            "tag={tag}"
        );
    }

    let stream_word =
        StreamingNode::element("StReAm-WoRd", vec![StreamingNode::text("already wrapped")]);
    assert_eq!(
        wrap_streaming_words(&[stream_word.clone(), StreamingNode::text("new")], true,),
        vec![stream_word, StreamingNode::word_with_incoming("new", true)]
    );
}

#[test]
fn reused_prefix_is_never_rewrapped_or_reanimated() {
    let reused = StreamingNode::element(
        "p",
        vec![StreamingNode::word_with_incoming("already", true)],
    );
    let tail = StreamingNode::text("new words");
    let nodes = vec![reused.clone(), tail];

    assert_eq!(
        wrap_streaming_words_with_reused_prefix(&nodes, 1, true),
        vec![
            reused,
            StreamingNode::word("new"),
            StreamingNode::text(" "),
            StreamingNode::word_with_incoming("words", true),
        ]
    );

    let raw_prefix = StreamingNode::text("parser-owned prefix");
    assert_eq!(
        wrap_streaming_words_with_reused_prefix(std::slice::from_ref(&raw_prefix), 99, true),
        vec![raw_prefix]
    );
}

#[test]
fn settled_targets_expose_final_words_and_replace_without_animation() {
    let settled = target("settled target", false);

    assert!(!should_animate_streaming_target(true, &settled));
    assert_eq!(find_next_reveal_boundary("", &settled), 8);
    assert_eq!(reveal_streaming_words("", &settled, 2), "settled target");
    assert_eq!(count_pending_streaming_words("", &settled), 2);

    let settled_replacement = target("fresh snapshot", false);
    assert!(!should_animate_streaming_target(true, &settled_replacement));
    assert_eq!(
        reveal_streaming_words("old snapshot", &settled_replacement, 1),
        "fresh snapshot"
    );
}
