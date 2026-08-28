//! Dependency-free policy for the sentence shown when a new thread opens.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/root/new-thread-sentence.ts`. The reached Svelte
//! route currently owns a different opening panel, so this module deliberately
//! contains only the reusable sentence vocabulary and word-level projection.
//! It does not choose a project, render a view, read a clock, or obtain random
//! input. Callers supply the random unit and apply the returned records to
//! their renderer.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// The exact token in a sentence that stands for the selected project name.
pub const PROJECT_MARKER: &str = "{project}";

/// The six sentence templates in their exact legacy order.
pub const NEW_THREAD_SENTENCES: [&str; 6] = [
    "What are we building in {project} today?",
    "What should happen in {project} next?",
    "A new thread in {project}.",
    "Pick up {project} where you left it.",
    "Where would you like to start in {project}?",
    "{project} is open. What first?",
];

/// The exact sentence used when a vocabulary or random index cannot produce a
/// candidate.
pub const FALLBACK_SENTENCE: &str = "A new thread in {project}.";

/// Milliseconds between successive word reveals.
pub const STAGGER_STEP_MS: u64 = 45;

/// The largest word index that receives an increasing stagger.
pub const STAGGERED_WORDS: usize = 10;

/// The greatest random unit accepted before indexing the vocabulary.
pub const MAX_RANDOM_UNIT: f64 = 0.999_999;

const STAGGERED_WORD_INDEX_LIMIT: i128 = 10;

/// Selects a sentence from a caller-supplied vocabulary.
///
/// `random_unit` follows the legacy `Math.random()` convention, but it is an
/// explicit input here so this policy remains deterministic and testable. It
/// is clamped to `0..=0.999_999` before the floor-and-scale index calculation.
/// An empty vocabulary uses the exact legacy fallback. When there is more than
/// one choice, a selection equal to `previous` advances one slot and wraps;
/// a one-choice vocabulary, including the fallback, is allowed to repeat.
///
/// `NaN` has the same safe fallback as an out-of-range array lookup in the
/// source JavaScript. Positive and negative infinity follow the source clamp.
/// Vocabulary entries are returned without copying or normalization.
#[must_use = "use the selected new-thread sentence"]
pub fn pick_new_thread_sentence<'sentence>(
    previous: Option<&str>,
    random_unit: f64,
    vocabulary: &[&'sentence str],
) -> &'sentence str {
    if vocabulary.is_empty() {
        return FALLBACK_SENTENCE;
    }

    let Some(index) = bounded_random_index(random_unit, vocabulary.len()) else {
        return FALLBACK_SENTENCE;
    };

    let selected = vocabulary[index];
    if vocabulary.len() > 1 && previous == Some(selected) {
        return vocabulary[(index + 1) % vocabulary.len()];
    }

    selected
}

/// Selects a sentence from the canonical six-entry vocabulary.
///
/// This wrapper supplies [`NEW_THREAD_SENTENCES`] for callers that do not need
/// a custom vocabulary while retaining the explicit deterministic random unit.
#[must_use = "use the selected new-thread sentence"]
pub fn pick_default_new_thread_sentence(previous: Option<&str>, random_unit: f64) -> &'static str {
    pick_new_thread_sentence(previous, random_unit, &NEW_THREAD_SENTENCES)
}

/// Selects from the six production sentences using the source-oriented name.
///
/// This is an alias for [`pick_default_new_thread_sentence`] for callers that
/// describe the canonical vocabulary as the production sentence set.
#[must_use = "use the selected new-thread sentence"]
pub fn pick_production_new_thread_sentence(previous: Option<&str>, unit: f64) -> &'static str {
    pick_default_new_thread_sentence(previous, unit)
}

/// Computes the reveal delay for a zero-based word index, in milliseconds.
///
/// The source applies `Math.min(Math.max(0, index), 10) * 45`. Negative
/// indices clamp to zero. Delays stop increasing at index ten, where the
/// result is `450` ms. The integral input accepts both ordinary `usize`
/// enumerate indices and signed values used to exercise the source's negative
/// boundary.
#[must_use = "apply the computed word reveal delay"]
pub fn sentence_word_delay<Index>(index: Index) -> u64
where
    Index: TryInto<i128>,
{
    let index = index.try_into().ok().unwrap_or(i128::MAX);
    let capped_index = index.clamp(0, STAGGERED_WORD_INDEX_LIMIT);
    let capped_index = u64::try_from(capped_index).unwrap_or(0);
    capped_index * STAGGER_STEP_MS
}

/// One word-level record used to reveal an opening sentence.
///
/// Non-project words carry `text`; the first project marker in a token instead
/// sets `project` and splits that token into `prefix` and `suffix`. The
/// separator is represented by `leading_space`, because the source collapses
/// every run of ECMAScript whitespace to one semantic ASCII space at render
/// time.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NewThreadSentenceWord<'sentence> {
    /// Delay before this word becomes sharp, in milliseconds.
    pub delay_ms: u64,
    /// Whether one semantic space precedes this word.
    pub leading_space: bool,
    /// Text attached before the first project marker in this token.
    pub prefix: &'sentence str,
    /// Whether this record is the token containing the project marker.
    pub project: bool,
    /// Text attached after the first project marker in this token.
    pub suffix: &'sentence str,
    /// The complete token text for a non-project record; empty for a project
    /// record.
    pub text: &'sentence str,
}

/// Splits a sentence into the exact word records consumed by the renderer.
///
/// Splitting uses the ECMAScript `\s+` set rather than Rust's broader
/// [`char::is_whitespace`] set. Empty tokens from leading, trailing, or
/// repeated whitespace are discarded, and the remaining token order supplies
/// both `leading_space` and the capped reveal index. The first occurrence of
/// [`PROJECT_MARKER`] in a token owns all text before and after it, including
/// punctuation or any later marker.
#[must_use = "render or otherwise consume the projected sentence words"]
pub fn new_thread_sentence_words(template: &str) -> Vec<NewThreadSentenceWord<'_>> {
    template
        .split(is_ecmascript_whitespace)
        .filter(|token| !token.is_empty())
        .enumerate()
        .map(|(index, token)| {
            let delay_ms = sentence_word_delay(index);
            let leading_space = index > 0;

            let Some((prefix, suffix)) = token.split_once(PROJECT_MARKER) else {
                return NewThreadSentenceWord {
                    delay_ms,
                    leading_space,
                    prefix: "",
                    project: false,
                    suffix: "",
                    text: token,
                };
            };

            NewThreadSentenceWord {
                delay_ms,
                leading_space,
                prefix,
                project: true,
                suffix,
                text: "",
            }
        })
        .collect()
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn bounded_random_index(random_unit: f64, vocabulary_len: usize) -> Option<usize> {
    if random_unit.is_nan() {
        return None;
    }

    let unit = random_unit.clamp(0.0, MAX_RANDOM_UNIT);
    let scaled = unit * vocabulary_len as f64;
    if !scaled.is_finite() {
        return None;
    }

    let index = scaled.floor() as usize;
    Some(index.min(vocabulary_len - 1))
}

/// Matches the whitespace consumed by the source `/\s+/u` expression.
fn is_ecmascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}
