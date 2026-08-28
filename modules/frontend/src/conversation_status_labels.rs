//! Pure quiet-status labels for a live conversation work session.
//!
//! This is the dependency-free portion of
//! `modules/frontend/src/lib/conversation/activity-status.ts`: it owns the
//! caller-supplied thinking vocabulary, exact quiet-status copy, deterministic
//! word selection, and the visibility epoch that advances when a status line
//! is removed and later mounted again. Conversation-item projection,
//! lifecycle interpretation, and rendering remain outside this module.

/// The exact words used when a supplied thinking vocabulary is unusable.
pub const FALLBACK_THINKING_WORDS: [&str; 5] = [
    "Pondering",
    "Percolating",
    "Recombobulating",
    "Puttering",
    "Zesting",
];

const FNV_OFFSET_BASIS: u32 = 2_166_136_261;
const FNV_PRIME: u32 = 16_777_619;

fn fallback_thinking_vocabulary() -> Vec<String> {
    FALLBACK_THINKING_WORDS
        .iter()
        .map(|word| (*word).to_owned())
        .collect()
}

fn fallback_thinking_word() -> String {
    match FALLBACK_THINKING_WORDS.first() {
        Some(word) => (*word).to_owned(),
        None => String::new(),
    }
}

/// Returns whether every supplied word is non-empty and occurs exactly once.
///
/// Validation is deliberately exact: whitespace is part of a word, just as it
/// is for the TypeScript `NonEmptyString` schema. A caller that supplies an
/// empty slice, an empty word, or a duplicate gets the fallback vocabulary
/// from [`resolve_thinking_vocabulary`].
#[must_use]
pub fn thinking_vocabulary_is_valid<T: AsRef<str>>(vocabulary: &[T]) -> bool {
    if vocabulary.is_empty() {
        return false;
    }

    for (index, word) in vocabulary.iter().enumerate() {
        let word = word.as_ref();
        if word.is_empty()
            || vocabulary[..index]
                .iter()
                .any(|previous| previous.as_ref() == word)
        {
            return false;
        }
    }
    true
}

/// Resolves a caller-supplied vocabulary into owned words.
///
/// A non-empty vocabulary with non-empty, unique entries is copied verbatim.
/// Invalid, empty, or duplicate input is replaced as a whole by the exact
/// five-word fallback, matching the TypeScript loader rather than silently
/// dropping only the offending entry.
#[must_use]
pub fn resolve_thinking_vocabulary<T: AsRef<str>>(vocabulary: &[T]) -> Vec<String> {
    if !thinking_vocabulary_is_valid(vocabulary) {
        return fallback_thinking_vocabulary();
    }

    vocabulary
        .iter()
        .map(|word| word.as_ref().to_owned())
        .collect()
}

/// Selects a word by index from a resolved caller vocabulary.
///
/// The index wraps over the selected vocabulary. Resolution happens here too
/// so every public selector has the same safe fallback for invalid input.
#[must_use]
pub fn thinking_word_at<T: AsRef<str>>(index: usize, vocabulary: &[T]) -> String {
    let words = resolve_thinking_vocabulary(vocabulary);
    words[index % words.len()].clone()
}

/// Computes the JavaScript-compatible unsigned UTF-16 FNV-1a hash of a seed.
///
/// JavaScript strings expose UTF-16 code units to `charCodeAt`; iterating
/// [`str::encode_utf16`] preserves that representation in Rust, including
/// surrogate pairs for non-BMP characters. `Math.imul(... ) >>> 0` is the
/// corresponding wrapping `u32` multiplication below.
#[must_use]
pub fn utf16_fnv1a(seed: &str) -> u32 {
    let mut hash = FNV_OFFSET_BASIS;
    for code_unit in seed.encode_utf16() {
        hash ^= u32::from(code_unit);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Alias naming the JavaScript compatibility boundary explicitly.
#[must_use]
pub fn javascript_utf16_fnv1a(seed: &str) -> u32 {
    utf16_fnv1a(seed)
}

/// Selects one thinking word for a seed and visibility generation.
///
/// The seed supplies the stable starting point. A later visibility generation
/// advances through the same vocabulary. Generation addition wraps in `u64`,
/// making the finite Rust state deterministic even at its representation
/// boundary.
#[must_use]
pub fn thinking_word_for<T: AsRef<str>>(
    seed: &str,
    visibility_generation: u64,
    vocabulary: &[T],
) -> String {
    let words = resolve_thinking_vocabulary(vocabulary);
    let vocabulary_length = match u64::try_from(words.len()) {
        Ok(length) if length != 0 => length,
        Ok(_) | Err(_) => return fallback_thinking_word(),
    };
    let selected_index =
        u64::from(utf16_fnv1a(seed)).wrapping_add(visibility_generation) % vocabulary_length;
    let Ok(index) = usize::try_from(selected_index) else {
        return fallback_thinking_word();
    };
    match words.get(index) {
        Some(word) => word.clone(),
        None => fallback_thinking_word(),
    }
}

/// Returns the exact pre-response provider wait label when an engine is known.
#[must_use]
pub fn waiting_label_for(engine_name: Option<&str>) -> Option<String> {
    engine_name.map(|name| format!("Waiting for {name} to respond…"))
}

/// Returns the exact context-compaction label when compaction is pending.
#[must_use]
pub const fn compacting_label_for(awaiting_compaction: bool) -> Option<&'static str> {
    if awaiting_compaction {
        Some("Compacting the conversation…")
    } else {
        None
    }
}

/// Returns the exact post-response background-worker label for any worker
/// count.
#[must_use]
pub fn background_work_label_for<T: AsRef<str>>(names: &[T]) -> Option<String> {
    match names {
        [] => None,
        [name] => Some(format!("Waiting for {} to finish…", name.as_ref())),
        [first, second] => Some(format!(
            "Waiting for {} and {} to finish…",
            first.as_ref(),
            second.as_ref()
        )),
        _ => Some(format!("Waiting for {} background agents…", names.len())),
    }
}

/// Pure inputs needed to choose the active quiet-status label.
///
/// `background_agent_names` is already ordered for presentation by the
/// caller. This module does not inspect conversation items or infer that list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActiveWorkLabelInput<'a> {
    /// Whether a provider-side context compaction is currently required.
    pub awaiting_compaction: bool,
    /// Names of delegated workers that remain after the provider responded.
    pub background_agent_names: &'a [&'a str],
    /// Display name of the engine receiving the request, if known.
    pub engine_name: Option<&'a str>,
    /// Whether the provider has accepted/started responding to the turn.
    pub provider_responded: bool,
    /// Latest provider-authored reasoning summary, if one is available.
    pub reasoning_summary: Option<&'a str>,
    /// Stable session identity used as the thinking-word seed.
    pub seed: &'a str,
    /// Current rendered quiet-status visibility epoch.
    pub thinking_visibility_generation: u64,
    /// Whether a provider-started activity currently owns the visible wait.
    pub waiting_for_activity: bool,
}

/// Chooses the active quiet-status label using the audited precedence.
///
/// Waiting activity wins first. Before a provider response, compaction wins
/// over an engine wait, which wins over a thinking word. After a response,
/// background workers win over a reasoning summary, which wins over the
/// thinking word. `Some("")` for a summary or name is preserved because the
/// source policy uses nullish fallback rather than truthiness.
#[must_use]
pub fn active_work_label_for<T: AsRef<str>>(
    input: ActiveWorkLabelInput<'_>,
    vocabulary: &[T],
) -> String {
    if input.waiting_for_activity {
        return "Waiting".to_owned();
    }

    if !input.provider_responded {
        if let Some(label) = compacting_label_for(input.awaiting_compaction) {
            return label.to_owned();
        }
        if let Some(label) = waiting_label_for(input.engine_name) {
            return label;
        }
        return thinking_word_for(input.seed, input.thinking_visibility_generation, vocabulary);
    }

    if let Some(label) = background_work_label_for(input.background_agent_names) {
        return label;
    }
    if let Some(summary) = input.reasoning_summary {
        return summary.to_owned();
    }
    thinking_word_for(input.seed, input.thinking_visibility_generation, vocabulary)
}

/// Pure state for the status line's visibility epoch.
///
/// The first visible mount keeps generation zero. Each later false-to-true
/// transition increments the generation once; repeated visible observations
/// and hidden observations do not. This mirrors the reactive reconciliation
/// without depending on a UI runtime or scheduling mechanism.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ThinkingVisibilityState {
    status_line_was_visible: bool,
    status_line_has_appeared: bool,
    thinking_visibility_generation: u64,
}

impl ThinkingVisibilityState {
    /// Creates an unseen status line at generation zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            status_line_was_visible: false,
            status_line_has_appeared: false,
            thinking_visibility_generation: 0,
        }
    }

    /// Creates an unseen-but-already-appeared state at a chosen generation.
    ///
    /// This is useful when adopting persisted component-local state and makes
    /// the wrapping behavior testable without billions of transitions.
    #[must_use]
    pub const fn from_generation(generation: u64) -> Self {
        Self {
            status_line_was_visible: false,
            status_line_has_appeared: true,
            thinking_visibility_generation: generation,
        }
    }

    /// Returns the generation used by the next visible quiet-status mount.
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.thinking_visibility_generation
    }

    /// Returns whether the most recently reconciled observation was visible.
    #[must_use]
    pub const fn was_visible(self) -> bool {
        self.status_line_was_visible
    }

    /// Returns whether a visible status line has ever appeared in this state.
    #[must_use]
    pub const fn has_appeared(self) -> bool {
        self.status_line_has_appeared
    }

    /// Reconciles one observed visibility value and advances the epoch when
    /// the line reappears after having been removed.
    pub fn reconcile(&mut self, status_line_visible: bool) {
        if status_line_visible && !self.status_line_was_visible {
            if self.status_line_has_appeared {
                self.thinking_visibility_generation =
                    self.thinking_visibility_generation.wrapping_add(1);
            }
            self.status_line_has_appeared = true;
        }
        self.status_line_was_visible = status_line_visible;
    }
}

/// Advances a caller-owned visibility state using one pure observation.
pub fn advance_thinking_visibility(state: &mut ThinkingVisibilityState, status_line_visible: bool) {
    state.reconcile(status_line_visible);
}
