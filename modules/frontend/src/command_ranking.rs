//! Deterministic, UI-agnostic command-palette filtering and ranking.
//!
//! This is the first-party Rust equivalent of the pinned Bits command scorer
//! used by the legacy command palette. Matching is case-insensitive, while
//! exact original-case matches receive a small preference. A match is an
//! in-order subsequence; a zero score means that no matching subsequence was
//! found. Filtering and group ordering are stable, so equal scores retain the
//! caller's order.
//!
//! Normalization operates on Unicode scalar values. Each scalar is lowered
//! with Rust's [`char::to_lowercase`], which may expand one scalar to several
//! scalars (for example, `İ` becomes `i` followed by a combining dot). This
//! is deliberately documented rather than described as JavaScript UTF-16
//! parity. Rust's [`char::is_whitespace`] supplies the Unicode whitespace set;
//! ASCII `-` is also normalized to a plain space, matching the legacy
//! `/[\s-]/` expression. Original scalars are retained alongside their
//! lowered forms for the case and separator penalties.
//!
//! To keep untrusted command text from creating unbounded memo tables or
//! recursion depth, at most 512 candidate scalars and 64 query scalars are
//! retained for one score. The complete input is still read while normalizing
//! a query, so a long all-whitespace query is recognized as blank. Normal
//! command-palette values are well below these defensive limits; longer
//! values are conservatively scored from their retained prefix.

const SCORE_CONTINUE_MATCH: f64 = 1.0;
const SCORE_SPACE_WORD_JUMP: f64 = 0.9;
const SCORE_NON_SPACE_WORD_JUMP: f64 = 0.8;
const SCORE_CHARACTER_JUMP: f64 = 0.17;
const SCORE_TRANSPOSITION: f64 = 0.1;
const PENALTY_SKIPPED: f64 = 0.999;
const PENALTY_CASE_MISMATCH: f64 = 0.9999;
const PENALTY_NOT_COMPLETE: f64 = 0.99;

const MAX_CANDIDATE_SCALARS: usize = 512;
const MAX_QUERY_SCALARS: usize = 64;

/// Borrowed searchable text for one command value.
///
/// Keywords are appended to `value`, separated by one plain space, only for
/// matching. Neither the value nor the keyword slice is modified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandText<'a> {
    /// Primary text displayed by the command palette.
    pub value: &'a str,
    /// Optional additional text used only for matching.
    pub keywords: &'a [&'a str],
}

impl<'a> CommandText<'a> {
    /// Creates searchable text with the supplied optional keywords.
    #[must_use]
    pub const fn new(value: &'a str, keywords: &'a [&'a str]) -> Self {
        Self { value, keywords }
    }

    /// Creates searchable text without keywords.
    #[must_use]
    pub const fn value(value: &'a str) -> Self {
        Self {
            value,
            keywords: &[],
        }
    }
}

/// An owned command item suitable for filtering while retaining an opaque
/// caller-owned item or identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandItem<T> {
    /// Caller-owned item or opaque identifier returned with the rank.
    pub item: T,
    /// Primary command text.
    pub value: String,
    /// Additional match-only keywords.
    pub keywords: Vec<String>,
}

impl<T> CommandItem<T> {
    /// Creates an item without additional keywords.
    #[must_use]
    pub fn new(item: T, value: impl Into<String>) -> Self {
        Self {
            item,
            value: value.into(),
            keywords: Vec::new(),
        }
    }

    /// Creates an item with additional match-only keywords.
    #[must_use]
    pub fn with_keywords<I, K>(item: T, value: impl Into<String>, keywords: I) -> Self
    where
        I: IntoIterator<Item = K>,
        K: Into<String>,
    {
        Self {
            item,
            value: value.into(),
            keywords: keywords.into_iter().map(Into::into).collect(),
        }
    }
}

/// A borrowed command item for callers that do not need to allocate command
/// values or keywords before filtering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BorrowedCommandItem<'a, T> {
    /// Caller-owned item or opaque identifier returned with the rank.
    pub item: T,
    /// Borrowed searchable value and keywords.
    pub text: CommandText<'a>,
}

impl<'a, T> BorrowedCommandItem<'a, T> {
    /// Creates a borrowed command item.
    #[must_use]
    pub const fn new(item: T, text: CommandText<'a>) -> Self {
        Self { item, text }
    }
}

/// One item retained by a filter, together with its finite rank.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedItem<T> {
    /// Caller-owned item or opaque identifier.
    pub item: T,
    /// Rank in the inclusive range `0.0..=1.0`.
    pub score: f64,
}

/// An owned group of searchable commands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandGroup<G, T> {
    /// Caller-owned group identity.
    pub id: G,
    /// Ordered commands in this group.
    pub items: Vec<CommandItem<T>>,
}

impl<G, T> CommandGroup<G, T> {
    /// Creates a group from its identity and ordered items.
    #[must_use]
    pub const fn new(id: G, items: Vec<CommandItem<T>>) -> Self {
        Self { id, items }
    }
}

/// A non-empty group retained by group filtering, ranked by its best item.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedGroup<G, T> {
    /// Caller-owned group identity.
    pub id: G,
    /// Kept items in descending rank, stably ordered for ties.
    pub items: Vec<RankedItem<T>>,
    /// Highest rank among [`Self::items`].
    pub score: f64,
}

/// Computes the command score without keywords.
///
/// An empty query is not treated specially here: the raw scorer follows the
/// legacy completion rules. Use [`filter_and_rank`] or
/// [`filter_and_rank_groups`] when an empty or normalized-blank query must
/// bypass filtering.
#[must_use]
pub fn score(value: &str, query: &str) -> f64 {
    score_with_keyword_iter(value, query, std::iter::empty::<&str>())
}

/// Computes the command score with borrowed optional keywords.
#[must_use]
pub fn score_with_keywords(value: &str, query: &str, keywords: &[&str]) -> f64 {
    score_with_keyword_iter(value, query, keywords.iter().copied())
}

/// Computes the command score for a borrowed searchable text descriptor.
#[must_use]
pub fn score_text(text: CommandText<'_>, query: &str) -> f64 {
    score_with_keyword_iter(text.value, query, text.keywords.iter().copied())
}

/// Alias named after the legacy scorer's public concept.
#[must_use]
pub fn command_score(value: &str, query: &str) -> f64 {
    score(value, query)
}

/// Alias named after the legacy scorer's public concept, with keywords.
#[must_use]
pub fn command_score_with_keywords(value: &str, query: &str, keywords: &[&str]) -> f64 {
    score_with_keywords(value, query, keywords)
}

/// Filters and stably ranks owned command items.
///
/// A blank or normalized-blank query retains every input item in input order,
/// including items that would otherwise score zero. Non-blank queries retain
/// only positive scores and sort them by descending score. Equal scores keep
/// their original order.
#[must_use]
pub fn filter_and_rank<T, I>(items: I, query: &str) -> Vec<RankedItem<T>>
where
    I: IntoIterator<Item = CommandItem<T>>,
{
    let normalized_query = normalize_text(query, MAX_QUERY_SCALARS);
    let bypass = normalized_query.is_blank();
    let mut ranked = Vec::new();

    for item in items {
        let candidate = normalize_candidate(&item.value, item.keywords.iter().map(String::as_str));
        let item_score = score_normalized(&candidate, &normalized_query);
        if bypass || item_score > 0.0 {
            ranked.push(RankedItem {
                item: item.item,
                score: item_score,
            });
        }
    }

    if !bypass {
        sort_ranked_items(&mut ranked);
    }

    ranked
}

/// Filters and stably ranks borrowed command items while returning their
/// caller-owned items.
#[must_use]
pub fn filter_and_rank_borrowed<'a, T, I>(items: I, query: &str) -> Vec<RankedItem<T>>
where
    I: IntoIterator<Item = BorrowedCommandItem<'a, T>>,
{
    let normalized_query = normalize_text(query, MAX_QUERY_SCALARS);
    let bypass = normalized_query.is_blank();
    let mut ranked = Vec::new();

    for item in items {
        let candidate = normalize_candidate(item.text.value, item.text.keywords.iter().copied());
        let item_score = score_normalized(&candidate, &normalized_query);
        if bypass || item_score > 0.0 {
            ranked.push(RankedItem {
                item: item.item,
                score: item_score,
            });
        }
    }

    if !bypass {
        sort_ranked_items(&mut ranked);
    }

    ranked
}

/// Filters and ranks arbitrary borrowed values by a selector returning their
/// primary command text.
#[must_use]
pub fn filter_by<T, I, F>(items: I, query: &str, mut value: F) -> Vec<RankedItem<T>>
where
    I: IntoIterator<Item = T>,
    F: for<'a> FnMut(&'a T) -> &'a str,
{
    filter_by_text(items, query, |item| CommandText::value(value(item)))
}

/// Filters and ranks arbitrary borrowed values by a selector returning
/// borrowed primary text and optional keywords.
#[must_use]
pub fn filter_by_text<T, I, F>(items: I, query: &str, mut text: F) -> Vec<RankedItem<T>>
where
    I: IntoIterator<Item = T>,
    F: for<'a> FnMut(&'a T) -> CommandText<'a>,
{
    let normalized_query = normalize_text(query, MAX_QUERY_SCALARS);
    let bypass = normalized_query.is_blank();
    let mut ranked = Vec::new();

    for item in items {
        let searchable = text(&item);
        let candidate = normalize_candidate(searchable.value, searchable.keywords.iter().copied());
        let item_score = score_normalized(&candidate, &normalized_query);
        if bypass || item_score > 0.0 {
            ranked.push(RankedItem {
                item,
                score: item_score,
            });
        }
    }

    if !bypass {
        sort_ranked_items(&mut ranked);
    }

    ranked
}

/// Filters, ranks, and stably orders groups of owned command items.
///
/// Groups with no kept items disappear. Otherwise each group's score is the
/// best kept item score, and groups are ordered by descending group score.
/// Group and item ties retain their original input order. Blank queries keep
/// both group and item input order while retaining all items in non-empty
/// groups.
#[must_use]
pub fn filter_and_rank_groups<G, T, I>(groups: I, query: &str) -> Vec<RankedGroup<G, T>>
where
    I: IntoIterator<Item = CommandGroup<G, T>>,
{
    let normalized_query = normalize_text(query, MAX_QUERY_SCALARS);
    let bypass = normalized_query.is_blank();
    let mut ranked_groups = Vec::new();

    for group in groups {
        let mut ranked_items = Vec::new();
        for item in group.items {
            let candidate =
                normalize_candidate(&item.value, item.keywords.iter().map(String::as_str));
            let item_score = score_normalized(&candidate, &normalized_query);
            if bypass || item_score > 0.0 {
                ranked_items.push(RankedItem {
                    item: item.item,
                    score: item_score,
                });
            }
        }

        let Some(best) = ranked_items
            .iter()
            .map(|item| item.score)
            .max_by(f64::total_cmp)
        else {
            continue;
        };

        if !bypass {
            sort_ranked_items(&mut ranked_items);
        }

        ranked_groups.push(RankedGroup {
            id: group.id,
            items: ranked_items,
            score: best,
        });
    }

    if !bypass {
        ranked_groups.sort_by(|left, right| right.score.total_cmp(&left.score));
    }

    ranked_groups
}

fn sort_ranked_items<T>(items: &mut [RankedItem<T>]) {
    items.sort_by(|left, right| right.score.total_cmp(&left.score));
}

#[derive(Clone, Copy)]
struct NormalizedScalar {
    original: char,
    lowered: char,
}

struct NormalizedText {
    scalars: Vec<NormalizedScalar>,
    has_non_space: bool,
}

impl NormalizedText {
    fn new(limit: usize) -> Self {
        Self {
            scalars: Vec::with_capacity(limit),
            has_non_space: false,
        }
    }

    fn push_lowered(&mut self, original: char, lowered: char, limit: usize) -> bool {
        if lowered != ' ' {
            self.has_non_space = true;
        }
        if self.scalars.len() >= limit {
            return false;
        }
        self.scalars.push(NormalizedScalar { original, lowered });
        true
    }

    fn push_str(&mut self, text: &str, limit: usize) {
        if self.scalars.len() >= limit {
            return;
        }

        for original in text.chars() {
            for lowered in original.to_lowercase() {
                let lowered = normalize_separator(lowered);
                if !self.push_lowered(original, lowered, limit) {
                    return;
                }
            }
        }
    }

    fn push_separator(&mut self, limit: usize) {
        let _ = self.push_lowered(' ', ' ', limit);
    }

    fn is_blank(&self) -> bool {
        !self.has_non_space
    }
}

fn normalize_text(text: &str, limit: usize) -> NormalizedText {
    let mut normalized = NormalizedText::new(limit);

    for original in text.chars() {
        for lowered in original.to_lowercase() {
            let lowered = normalize_separator(lowered);
            let _ = normalized.push_lowered(original, lowered, limit);
        }
    }

    normalized
}

fn normalize_candidate<I, K>(value: &str, keywords: I) -> NormalizedText
where
    I: IntoIterator<Item = K>,
    K: AsRef<str>,
{
    let mut normalized = NormalizedText::new(MAX_CANDIDATE_SCALARS);
    normalized.push_str(value, MAX_CANDIDATE_SCALARS);

    let mut keywords = keywords.into_iter();
    if let Some(keyword) = keywords.next() {
        normalized.push_separator(MAX_CANDIDATE_SCALARS);
        normalized.push_str(keyword.as_ref(), MAX_CANDIDATE_SCALARS);

        for keyword in keywords {
            if normalized.scalars.len() >= MAX_CANDIDATE_SCALARS {
                break;
            }
            normalized.push_separator(MAX_CANDIDATE_SCALARS);
            normalized.push_str(keyword.as_ref(), MAX_CANDIDATE_SCALARS);
        }
    }

    normalized
}

fn normalize_separator(character: char) -> char {
    if character.is_whitespace() || character == '-' {
        ' '
    } else {
        character
    }
}

fn score_with_keyword_iter<I, K>(value: &str, query: &str, keywords: I) -> f64
where
    I: IntoIterator<Item = K>,
    K: AsRef<str>,
{
    let candidate = normalize_candidate(value, keywords);
    let normalized_query = normalize_text(query, MAX_QUERY_SCALARS);
    score_normalized(&candidate, &normalized_query)
}

fn score_normalized(candidate: &NormalizedText, query: &NormalizedText) -> f64 {
    let mut scorer = Scorer::new(&candidate.scalars, &query.scalars);
    scorer.score_at(0, 0)
}

struct Scorer<'a> {
    candidate: &'a [NormalizedScalar],
    query: &'a [NormalizedScalar],
    memo: Vec<f64>,
    query_width: usize,
}

impl<'a> Scorer<'a> {
    fn new(candidate: &'a [NormalizedScalar], query: &'a [NormalizedScalar]) -> Self {
        let query_width = query.len() + 1;
        let cell_count = (candidate.len() + 1) * query_width;
        Self {
            candidate,
            query,
            memo: vec![f64::NAN; cell_count],
            query_width,
        }
    }

    fn score_at(&mut self, string_index: usize, abbreviation_index: usize) -> f64 {
        if abbreviation_index >= self.query.len() {
            return if string_index >= self.candidate.len() {
                SCORE_CONTINUE_MATCH
            } else {
                PENALTY_NOT_COMPLETE
            };
        }

        let memo_index = string_index * self.query_width + abbreviation_index;
        let memoized = self.memo[memo_index];
        if !memoized.is_nan() {
            return memoized;
        }

        let abbreviation_char = self.query[abbreviation_index].lowered;
        let mut index = string_index;
        let mut high_score = 0.0;

        while index < self.candidate.len() {
            if self.candidate[index].lowered == abbreviation_char {
                let mut path_score = self.score_at(index + 1, abbreviation_index + 1);
                if path_score > high_score {
                    path_score = self.adjust_match_score(
                        path_score,
                        string_index,
                        abbreviation_index,
                        index,
                    );
                }

                let previous = index
                    .checked_sub(1)
                    .and_then(|previous| self.candidate.get(previous))
                    .map(|character| character.lowered);
                let next_query = self
                    .query
                    .get(abbreviation_index + 1)
                    .map(|character| character.lowered);
                let is_transposition = path_score < SCORE_TRANSPOSITION
                    && previous.is_some()
                    && previous == next_query;
                let is_repeated_query =
                    next_query == Some(abbreviation_char) && previous != Some(abbreviation_char);

                if is_transposition || is_repeated_query {
                    let transposed_score =
                        self.score_at(index + 1, abbreviation_index.saturating_add(2));
                    let weighted_score = transposed_score * SCORE_TRANSPOSITION;
                    if weighted_score > path_score {
                        path_score = weighted_score;
                    }
                }

                if path_score > high_score {
                    high_score = path_score;
                }
            }
            index += 1;
        }

        let high_score = clamp_score(high_score);
        self.memo[memo_index] = high_score;
        high_score
    }

    fn adjust_match_score(
        &self,
        mut score: f64,
        string_index: usize,
        abbreviation_index: usize,
        index: usize,
    ) -> f64 {
        if index == string_index {
            score *= SCORE_CONTINUE_MATCH;
        } else if is_gap(self.candidate[index - 1].original) {
            score *= SCORE_NON_SPACE_WORD_JUMP;
            if string_index > 0 {
                let gap_count = self.candidate[string_index..index - 1]
                    .iter()
                    .filter(|character| is_gap(character.original))
                    .count();
                score = apply_skipped_penalty(score, gap_count);
            }
        } else if is_space_or_hyphen(self.candidate[index - 1].original) {
            score *= SCORE_SPACE_WORD_JUMP;
            if string_index > 0 {
                let space_count = self.candidate[string_index..index - 1]
                    .iter()
                    .filter(|character| is_space_or_hyphen(character.original))
                    .count();
                score = apply_skipped_penalty(score, space_count);
            }
        } else {
            score *= SCORE_CHARACTER_JUMP;
            if string_index > 0 {
                score = apply_skipped_penalty(score, index - string_index);
            }
        }

        if self.candidate[index].original != self.query[abbreviation_index].original {
            score *= PENALTY_CASE_MISMATCH;
        }

        score
    }
}

fn is_gap(character: char) -> bool {
    matches!(
        character,
        '\\' | '/' | '_' | '+' | '.' | '#' | '"' | '@' | '[' | '(' | '{' | '&'
    )
}

fn is_space_or_hyphen(character: char) -> bool {
    character.is_whitespace() || character == '-'
}

fn apply_skipped_penalty(mut score: f64, count: usize) -> f64 {
    for _ in 0..count {
        score *= PENALTY_SKIPPED;
    }
    score
}

fn clamp_score(score: f64) -> f64 {
    if score.is_finite() {
        score.clamp(0.0, 1.0)
    } else {
        0.0
    }
}
