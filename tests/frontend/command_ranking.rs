//! Focused behavioral coverage for the first-party command ranking seam.

use artisan_frontend::command_ranking::{
    BorrowedCommandItem, CommandGroup, CommandItem, CommandText, filter_and_rank,
    filter_and_rank_borrowed, filter_and_rank_groups, filter_by_text, score, score_text,
    score_with_keywords,
};

fn item(id: usize, value: &str) -> CommandItem<usize> {
    CommandItem::new(id, value)
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "expected {expected:.15}, got {actual:.15}"
    );
}

#[test]
fn exact_contiguous_match_is_perfect_but_a_prefix_is_flatly_incomplete() {
    assert_close(score("calc", "calc"), 1.0);
    assert_close(score("calculator", "calc"), 0.99);
    assert!(score("calculator", "cal") < 1.0);
}

#[test]
fn word_gap_beats_punctuation_gap_and_mid_word_jumps() {
    let word_start = score("open file", "file");
    let punctuation_start = score("open/file", "file");
    let mid_word_start = score("openfile", "file");

    assert_close(word_start, 0.9);
    assert_close(punctuation_start, 0.8);
    assert!(mid_word_start < punctuation_start);
    assert!(punctuation_start < word_start);
}

#[test]
fn skipped_separators_and_mid_word_characters_are_penalized() {
    let immediate_gap = score("a_b", "ab");
    let skipped_gap = score("a__b", "ab");
    let immediate_space = score("a b", "ab");
    let skipped_space = score("a  b", "ab");
    let immediate_mid_word = score("acb", "ab");
    let skipped_mid_word = score("acccb", "ab");

    assert!(skipped_gap < immediate_gap);
    assert!(skipped_space < immediate_space);
    assert!(skipped_mid_word < immediate_mid_word);
    assert_close(immediate_gap, 0.8);
    assert_close(immediate_space, 0.9);
    assert!(immediate_mid_word < immediate_gap);
}

#[test]
fn case_mismatch_is_small_but_measurable_and_matching_is_insensitive() {
    let exact = score("abc", "abc");
    let different_case = score("ABC", "abc");

    assert_close(exact, 1.0);
    assert!(different_case > 0.99);
    assert!(different_case < exact);
    assert_close(different_case, 0.9999_f64.powi(3));
}

#[test]
fn transposition_is_kept_as_a_heavily_penalized_alternative() {
    let in_order = score("ouch", "uc");
    let transposed = score("curtain", "uc");

    assert!(in_order > transposed);
    assert!(transposed > 0.0);
    assert!(transposed <= 0.1);
}

#[test]
fn hyphens_and_unicode_whitespace_normalize_to_the_same_match_space() {
    let hyphenated = score("foo-bar", "foo bar");
    let unicode_space = score("foo\u{2003}bar", "foo bar");
    let query_unicode_space = score("foo-bar", "foo\u{2003}bar");

    assert!(hyphenated > 0.99);
    assert!(unicode_space > 0.99);
    assert!(query_unicode_space > 0.99);
}

#[test]
fn rust_lowercase_expansion_is_scalar_based_and_explicitly_tested() {
    let expanded = score("\u{0130}", "i");

    assert_close(expanded, 0.99 * 0.9999);
    assert!(expanded < 1.0);
}

#[test]
fn keywords_extend_match_text_without_mutating_callers() {
    let value = String::from("Calculator");
    let keywords = ["math", "arithmetic"];
    let value_before = value.clone();
    let keywords_before = keywords;

    assert_close(score(value.as_str(), "math"), 0.0);
    assert!(score_with_keywords(value.as_str(), "math", &keywords) > 0.0);
    assert_eq!(value, value_before);
    assert_eq!(keywords, keywords_before);
    assert_close(
        score_text(CommandText::new(&value, &keywords), "math"),
        0.891,
    );
}

#[test]
fn zero_scores_are_excluded_from_non_blank_results_and_items_sort_stably() {
    let results = filter_and_rank([item(1, "ab"), item(2, "ab"), item(3, "unrelated")], "ab");

    assert_eq!(
        results.iter().map(|result| result.item).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_close(results[0].score, results[1].score);
}

#[test]
fn blank_and_normalized_blank_queries_bypass_filtering_and_preserve_order() {
    let values = [item(1, "first"), item(2, "second"), item(3, "third")];

    let blank = filter_and_rank(values.clone(), "");
    let normalized_blank = filter_and_rank(values, "-\u{2003}\t");

    assert_eq!(
        blank.iter().map(|result| result.item).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        normalized_blank
            .iter()
            .map(|result| result.item)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

#[test]
fn borrowed_and_selector_apis_keep_opaque_items() {
    let keywords = ["alias"];
    let borrowed = filter_and_rank_borrowed(
        [
            BorrowedCommandItem::new(10, CommandText::new("Alpha", &keywords)),
            BorrowedCommandItem::new(20, CommandText::value("Beta")),
        ],
        "alias",
    );
    assert_eq!(borrowed.len(), 1);
    assert_eq!(borrowed[0].item, 10);

    let source = [(1_u8, "Alpha"), (2_u8, "Beta")];
    let selected = filter_by_text(source, "be", |item| CommandText::value(item.1));
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].item.0, 2);
}

#[test]
fn groups_drop_empty_groups_and_sort_by_best_item_with_stable_ties() {
    let groups = [
        CommandGroup::new(10, vec![item(101, "ab")]),
        CommandGroup::new(20, vec![item(201, "ab")]),
        CommandGroup::new(30, vec![item(301, "unrelated")]),
        CommandGroup::new(40, Vec::new()),
    ];
    let results = filter_and_rank_groups(groups, "ab");

    assert_eq!(
        results.iter().map(|group| group.id).collect::<Vec<_>>(),
        vec![10, 20]
    );
    assert_eq!(results[0].items[0].item, 101);
    assert_eq!(results[1].items[0].item, 201);
    assert_close(results[0].score, results[1].score);
}

#[test]
fn groups_reorder_by_best_item_when_scores_differ() {
    let groups = [
        CommandGroup::new(1, vec![item(11, "a-longer-command")]),
        CommandGroup::new(2, vec![item(21, "a")]),
    ];
    let results = filter_and_rank_groups(groups, "a");

    assert_eq!(
        results.iter().map(|group| group.id).collect::<Vec<_>>(),
        vec![2, 1]
    );
    assert!(results[0].score > results[1].score);
}

#[test]
fn blank_group_queries_keep_non_empty_groups_and_their_input_order() {
    let groups = [
        CommandGroup::new(1, vec![item(11, "first"), item(12, "second")]),
        CommandGroup::new(2, Vec::new()),
        CommandGroup::new(3, vec![item(31, "third")]),
    ];
    let results = filter_and_rank_groups(groups, "\u{2003}-");

    assert_eq!(
        results.iter().map(|group| group.id).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert_eq!(
        results[0]
            .items
            .iter()
            .map(|result| result.item)
            .collect::<Vec<_>>(),
        vec![11, 12]
    );
    assert_eq!(results[1].items[0].item, 31);
}

#[test]
fn long_unicode_input_is_bounded_and_returns_finite_scores() {
    let long_value = format!("{}needle", "İ😀".repeat(20_000));
    let long_query = "İ😀".repeat(20_000);

    let result = std::panic::catch_unwind(|| score(&long_value, &long_query));
    assert!(result.is_ok(), "long Unicode scoring must not panic");
    let value = result.expect("the panic assertion above must hold");
    assert!(value.is_finite());
    assert!((0.0..=1.0).contains(&value));
}
