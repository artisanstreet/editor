//! Dependency-free language residency and registration policy for Markdown.
//!
//! This is the native counterpart of the language-loading portion of
//! `components/markdown/settled-highlighting.ts`. It describes the loader
//! registry, accepts already-observed load outcomes, and applies the shared
//! highlighter's two-pass registration rule. It deliberately does not load a
//! grammar, construct a highlighter, parse Markdown, or tokenize a fence.

#![allow(clippy::module_name_repetitions)]

use std::collections::BTreeSet;

/// The exact language-loader keys from the legacy settled highlighter.
pub const SUPPORTED_CONVERSATION_LANGUAGE_KEYS: [&str; 24] = [
    "astro",
    "bash",
    "c",
    "cpp",
    "csharp",
    "css",
    "go",
    "html",
    "java",
    "javascript",
    "json",
    "jsx",
    "markdown",
    "powershell",
    "python",
    "rust",
    "sql",
    "svelte",
    "toml",
    "tsx",
    "typescript",
    "vue",
    "xml",
    "yaml",
];

/// Borrows the exact keys supported by the legacy language-loader registry.
#[must_use]
pub const fn supported_conversation_language_keys() -> &'static [&'static str] {
    &SUPPORTED_CONVERSATION_LANGUAGE_KEYS
}

/// Returns whether `language` has a loader in the legacy registry.
#[must_use]
pub fn is_supported_conversation_language(language: &str) -> bool {
    SUPPORTED_CONVERSATION_LANGUAGE_KEYS.contains(&language)
}

/// Outcome of loading one requested language before registration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationLanguageLoadOutcome {
    /// The loader produced a registration that can be given to the
    /// highlighter.
    Loaded,
    /// No loader exists for the requested key.
    Unknown,
    /// The known language's loader failed.
    LoadFailed,
}

impl ConversationLanguageLoadOutcome {
    /// Returns whether this observation produced a registration candidate.
    #[must_use]
    pub const fn is_loadable(self) -> bool {
        matches!(self, Self::Loaded)
    }
}

/// One requested language and the already-observed result of loading it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationLanguageRequest {
    /// The exact key requested by the caller.
    pub language: String,
    /// The result observed for this request.
    pub outcome: ConversationLanguageLoadOutcome,
}

impl ConversationLanguageRequest {
    /// Creates a request from an exact language key and load observation.
    #[must_use]
    pub fn new(language: impl Into<String>, outcome: ConversationLanguageLoadOutcome) -> Self {
        Self {
            language: language.into(),
            outcome,
        }
    }

    /// Creates a request whose loader produced a registration.
    #[must_use]
    pub fn loaded(language: impl Into<String>) -> Self {
        Self::new(language, ConversationLanguageLoadOutcome::Loaded)
    }

    /// Creates a request for a key without a loader.
    #[must_use]
    pub fn unknown(language: impl Into<String>) -> Self {
        Self::new(language, ConversationLanguageLoadOutcome::Unknown)
    }

    /// Creates a request whose known loader failed.
    #[must_use]
    pub fn load_failed(language: impl Into<String>) -> Self {
        Self::new(language, ConversationLanguageLoadOutcome::LoadFailed)
    }
}

/// Result of one shared-highlighter registration pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationLanguageRegistrationPassOutcome {
    /// The pass completed successfully.
    Succeeded,
    /// The pass failed before registration could be considered complete.
    Failed,
}

/// The two shared-highlighter passes required by the legacy loader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConversationLanguageRegistrationPasses {
    /// Result of the first shared-highlighter call.
    pub first: ConversationLanguageRegistrationPassOutcome,
    /// Result of the second shared-highlighter call.
    pub second: ConversationLanguageRegistrationPassOutcome,
}

impl ConversationLanguageRegistrationPasses {
    /// Creates an explicit pair of pass outcomes.
    #[must_use]
    pub const fn new(
        first: ConversationLanguageRegistrationPassOutcome,
        second: ConversationLanguageRegistrationPassOutcome,
    ) -> Self {
        Self { first, second }
    }

    /// Creates the successful two-pass outcome.
    #[must_use]
    pub const fn succeeded() -> Self {
        Self::new(
            ConversationLanguageRegistrationPassOutcome::Succeeded,
            ConversationLanguageRegistrationPassOutcome::Succeeded,
        )
    }

    /// Creates an outcome in which the first pass fails.
    #[must_use]
    pub const fn first_failed() -> Self {
        Self::new(
            ConversationLanguageRegistrationPassOutcome::Failed,
            ConversationLanguageRegistrationPassOutcome::Succeeded,
        )
    }

    /// Creates an outcome in which the second pass fails.
    #[must_use]
    pub const fn second_failed() -> Self {
        Self::new(
            ConversationLanguageRegistrationPassOutcome::Succeeded,
            ConversationLanguageRegistrationPassOutcome::Failed,
        )
    }
}

/// Whether both required registration passes completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationLanguageRegistrationOutcome {
    /// Both shared-highlighter passes completed successfully.
    Succeeded,
    /// At least one required pass failed.
    Failed,
}

impl ConversationLanguageRegistrationOutcome {
    /// Returns whether all loadable languages were registered.
    #[must_use]
    pub const fn is_succeeded(self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

/// Pure output from one requested-language registration attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationLanguageRegistrationReport {
    /// Every requested key in its original order, including duplicates.
    pub requested_languages: Vec<String>,
    /// Unique loaded, supported keys in first-seen order.
    pub loadable_languages: Vec<String>,
    /// Unique keys that could not be loaded because no loader exists.
    pub unknown_languages: Vec<String>,
    /// Unique supported keys whose loader failed.
    pub load_failed_languages: Vec<String>,
    /// Loadable keys considered registered after both passes succeeded.
    /// This is empty when registration fails.
    pub registered_languages: Vec<String>,
    /// Newly resident keys, in the loadable keys' first-seen order.
    pub newly_resident_languages: Vec<String>,
    /// Number of shared-highlighter passes actually attempted. A first-pass
    /// failure prevents the second pass from running, matching the legacy
    /// short-circuiting effect pipeline.
    pub passes_attempted: u8,
    /// Whether the two-pass registration committed its loadable keys.
    pub registration_outcome: ConversationLanguageRegistrationOutcome,
    /// All resident keys after this attempt, sorted for deterministic queries.
    pub resident_languages: Vec<String>,
}

/// Resident-language state for settled Markdown highlighting.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConversationLanguageRegistryPolicy {
    resident_languages: BTreeSet<String>,
}

impl ConversationLanguageRegistryPolicy {
    /// Creates an empty resident-language registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            resident_languages: BTreeSet::new(),
        }
    }

    /// Applies one already-observed request batch.
    ///
    /// Only supported requests observed as [`Loaded`] become registration
    /// candidates. The candidates are committed atomically after the first
    /// and second shared-highlighter passes both succeed. Duplicate loaded
    /// keys are registered once, so repeated or concurrent-facing request
    /// batches have deterministic, idempotent state transitions.
    #[must_use]
    pub fn register_requested_languages(
        &mut self,
        requests: &[ConversationLanguageRequest],
        passes: ConversationLanguageRegistrationPasses,
    ) -> ConversationLanguageRegistrationReport {
        let mut requested_languages = Vec::with_capacity(requests.len());
        let mut loadable_languages = Vec::new();
        let mut unknown_languages = Vec::new();
        let mut load_failed_languages = Vec::new();

        for request in requests {
            requested_languages.push(request.language.clone());

            // A missing registry key is unknown regardless of an inconsistent
            // externally supplied observation: the legacy loader looks up the
            // key before it can attempt a promise and therefore cannot load it.
            if !is_supported_conversation_language(&request.language) {
                push_unique(&mut unknown_languages, &request.language);
                continue;
            }

            match request.outcome {
                ConversationLanguageLoadOutcome::Loaded => {
                    push_unique(&mut loadable_languages, &request.language);
                }
                ConversationLanguageLoadOutcome::Unknown => {
                    push_unique(&mut unknown_languages, &request.language);
                }
                ConversationLanguageLoadOutcome::LoadFailed => {
                    push_unique(&mut load_failed_languages, &request.language);
                }
            }
        }

        let first_pass_succeeded = matches!(
            passes.first,
            ConversationLanguageRegistrationPassOutcome::Succeeded
        );
        let passes_attempted = if first_pass_succeeded { 2 } else { 1 };
        let registration_succeeded = first_pass_succeeded
            && matches!(
                passes.second,
                ConversationLanguageRegistrationPassOutcome::Succeeded
            );

        let (registered_languages, newly_resident_languages) = if registration_succeeded {
            let newly_resident_languages = loadable_languages
                .iter()
                .filter(|language| !self.resident_languages.contains(*language))
                .cloned()
                .collect();
            self.resident_languages
                .extend(loadable_languages.iter().cloned());
            (loadable_languages.clone(), newly_resident_languages)
        } else {
            (Vec::new(), Vec::new())
        };

        ConversationLanguageRegistrationReport {
            requested_languages,
            loadable_languages,
            unknown_languages,
            load_failed_languages,
            registered_languages,
            newly_resident_languages,
            passes_attempted,
            registration_outcome: if registration_succeeded {
                ConversationLanguageRegistrationOutcome::Succeeded
            } else {
                ConversationLanguageRegistrationOutcome::Failed
            },
            resident_languages: self.resident_languages.iter().cloned().collect(),
        }
    }

    /// Returns whether one exact key is resident.
    #[must_use]
    pub fn is_language_resident(&self, language: &str) -> bool {
        self.resident_languages.contains(language)
    }

    /// Returns whether every supplied exact key is resident.
    ///
    /// As in the legacy `Array.every` call, an empty input returns `true`.
    #[must_use]
    pub fn are_languages_resident<I, S>(&self, languages: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        languages
            .into_iter()
            .all(|language| self.is_language_resident(language.as_ref()))
    }

    /// Returns all resident keys in stable lexical order.
    #[must_use]
    pub fn resident_languages(&self) -> Vec<String> {
        self.resident_languages.iter().cloned().collect()
    }
}

/// Appends `language` only when it has not appeared in `values` yet.
fn push_unique(values: &mut Vec<String>, language: &str) {
    if !values.iter().any(|known| known == language) {
        values.push(language.to_owned());
    }
}
