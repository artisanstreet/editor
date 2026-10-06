//! The user's global prompt, which the Forge keeps in every managed engine.
//!
//! Each engine reads standing instructions from its own global file
//! (Claude Code's `CLAUDE.md`, Codex's and Grok's `AGENTS.md`, …). The Forge
//! stores one canonical prompt with its saved versions and writes it to every
//! engine, so the user edits it once in Artisan. Before the Forge first
//! writes, it imports what the engines already had: one shared text is
//! adopted silently, differing texts are offered to the user as candidates.
//! An engine file edited outside Artisan afterwards is reported as drifted
//! and is never overwritten until the user decides.

use thiserror::Error;

use crate::identifiers::RequestId;

/// Maximum bytes of the global prompt and of each candidate or drifted text.
pub const GLOBAL_PROMPT_MAX_BYTES: usize = 64 * 1024;
/// Maximum distinct candidates offered during setup.
pub const GLOBAL_PROMPT_CANDIDATES_MAX: usize = 8;
/// Maximum engines reported.
pub const GLOBAL_PROMPT_ENGINES_MAX: usize = 16;
/// Maximum bytes of one presentation-ready reason or file location.
pub const GLOBAL_PROMPT_DETAIL_MAX_BYTES: usize = 1024;
/// Bytes of a lowercase hexadecimal SHA-256 digest.
pub const GLOBAL_PROMPT_DIGEST_BYTES: usize = 64;

/// The saved version of the global prompt; every save moves it forward.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GlobalPromptRevision(u64);

impl GlobalPromptRevision {
    /// Wraps a revision number.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the revision number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Where the global prompt stands in one engine.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GlobalPromptEngineStatus {
    /// The engine's global file holds the prompt.
    Synced,
    /// The engine's global file was edited outside Artisan; `external_text`
    /// holds what it says now.
    Drifted,
    /// Waiting for the user to choose the prompt during setup.
    AwaitingChoice,
    /// The engine is not installed yet; the prompt follows once it is.
    EngineAbsent,
    /// The engine cannot run on this Forge yet.
    Unavailable,
    /// Writing the prompt failed; `reason` says why.
    Failed,
}

impl GlobalPromptEngineStatus {
    /// Returns the stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Synced => "synced",
            Self::Drifted => "drifted",
            Self::AwaitingChoice => "awaiting_choice",
            Self::EngineAbsent => "engine_absent",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
        }
    }
}

/// The global prompt's state in one engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GlobalPromptEngineState {
    /// Engine identifier (`claude`, `codex`, …).
    pub engine_id: String,
    /// Where the prompt stands there.
    pub status: GlobalPromptEngineStatus,
    /// The engine's global file, relative to its Artisan-managed home.
    pub file: Option<String>,
    /// What the engine's file says now, for `Drifted`.
    pub external_text: Option<String>,
    /// Presentation-ready reason for `Unavailable` and `Failed`.
    pub reason: Option<String>,
}

/// One distinct prompt found in the engines during setup.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GlobalPromptCandidate {
    /// Lowercase hexadecimal SHA-256 of the normalized text.
    pub candidate_id: String,
    /// The engines whose global file holds this text.
    pub engine_ids: Vec<String>,
    /// The text.
    pub text: String,
}

/// Whether the canonical prompt is established.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub enum GlobalPromptSetup {
    /// The canonical prompt is established and written to the engines.
    #[default]
    Ready,
    /// The engines hold different prompts: the user chooses which one
    /// becomes canonical before the Forge writes anything.
    ChoiceRequired(Vec<GlobalPromptCandidate>),
}

/// The global prompt and its state in every engine.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct GlobalPrompt {
    revision: GlobalPromptRevision,
    text: String,
    setup: GlobalPromptSetup,
    engines: Vec<GlobalPromptEngineState>,
}

impl GlobalPrompt {
    /// Creates a validated global prompt.
    ///
    /// # Errors
    ///
    /// Returns [`GlobalPromptError`] for an oversized text, too many or
    /// repeated engines or candidates, or an invalid detail.
    pub fn new(
        revision: GlobalPromptRevision,
        text: String,
        setup: GlobalPromptSetup,
        engines: Vec<GlobalPromptEngineState>,
    ) -> Result<Self, GlobalPromptError> {
        validate_prompt_text(&text)?;
        if let GlobalPromptSetup::ChoiceRequired(candidates) = &setup {
            if candidates.len() < 2 || candidates.len() > GLOBAL_PROMPT_CANDIDATES_MAX {
                return Err(GlobalPromptError::Candidates);
            }
            let mut seen = std::collections::HashSet::with_capacity(candidates.len());
            for candidate in candidates {
                validate_digest(&candidate.candidate_id)?;
                validate_prompt_text(&candidate.text)?;
                if !seen.insert(candidate.candidate_id.as_str())
                    || candidate.engine_ids.is_empty()
                    || candidate.engine_ids.len() > GLOBAL_PROMPT_ENGINES_MAX
                {
                    return Err(GlobalPromptError::Candidates);
                }
                for engine_id in &candidate.engine_ids {
                    crate::engine_install::validate_engine_id(engine_id)
                        .map_err(|_| GlobalPromptError::EngineId)?;
                }
            }
        }
        if engines.len() > GLOBAL_PROMPT_ENGINES_MAX {
            return Err(GlobalPromptError::TooMany);
        }
        let mut seen = std::collections::HashSet::with_capacity(engines.len());
        for state in &engines {
            crate::engine_install::validate_engine_id(&state.engine_id)
                .map_err(|_| GlobalPromptError::EngineId)?;
            if !seen.insert(state.engine_id.as_str()) {
                return Err(GlobalPromptError::Duplicate);
            }
            for detail in [&state.file, &state.reason].into_iter().flatten() {
                validate_detail(detail)?;
            }
            if let Some(external) = &state.external_text {
                validate_prompt_text(external)?;
            }
            if (state.status == GlobalPromptEngineStatus::Drifted) != state.external_text.is_some()
            {
                return Err(GlobalPromptError::Inconsistent);
            }
        }
        Ok(Self {
            revision,
            text,
            setup,
            engines,
        })
    }

    /// Returns the saved version.
    #[must_use]
    pub const fn revision(&self) -> GlobalPromptRevision {
        self.revision
    }

    /// Returns the canonical text; empty until the user writes one.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns whether setup still needs the user's choice.
    #[must_use]
    pub const fn setup(&self) -> &GlobalPromptSetup {
        &self.setup
    }

    /// Returns the state per engine, in Forge engine order.
    #[must_use]
    pub fn engines(&self) -> &[GlobalPromptEngineState] {
        &self.engines
    }

    /// Returns one engine's state.
    #[must_use]
    pub fn engine(&self, engine_id: &str) -> Option<&GlobalPromptEngineState> {
        self.engines
            .iter()
            .find(|state| state.engine_id == engine_id)
    }
}

/// Reads the global prompt. The connection that read it receives later
/// changes pushed as [`Event::GlobalPrompt`](crate::Event::GlobalPrompt).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ReadGlobalPrompt;

/// Saves a new version of the global prompt and writes it to every engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SaveGlobalPrompt {
    /// Client-minted request identity.
    pub request_id: RequestId,
    /// The revision the user edited; a save over a newer revision is
    /// refused so concurrent edits are never silently lost.
    pub base_revision: GlobalPromptRevision,
    /// The new text; empty clears the prompt.
    pub text: String,
}

/// How the user settles setup or a drifted engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum GlobalPromptResolution {
    /// Setup: make this candidate the canonical prompt.
    ChooseCandidate {
        /// The candidate.
        candidate_id: String,
    },
    /// Drift: make this engine's edited file the canonical prompt, written
    /// to every engine.
    ImportEngine {
        /// The drifted engine.
        engine_id: String,
    },
    /// Drift: replace this engine's edited file with the canonical prompt.
    OverwriteEngine {
        /// The drifted engine.
        engine_id: String,
    },
}

/// Settles setup or a drifted engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResolveGlobalPrompt {
    /// Client-minted request identity.
    pub request_id: RequestId,
    /// The decision.
    pub resolution: GlobalPromptResolution,
}

/// Validates a prompt text: bounded, and free of control characters other
/// than line breaks and tabs.
///
/// # Errors
///
/// Returns [`GlobalPromptError::Text`] for an invalid text.
pub fn validate_prompt_text(text: &str) -> Result<(), GlobalPromptError> {
    if text.len() > GLOBAL_PROMPT_MAX_BYTES
        || text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(GlobalPromptError::Text);
    }
    Ok(())
}

fn validate_detail(detail: &str) -> Result<(), GlobalPromptError> {
    if detail.trim().is_empty()
        || detail.len() > GLOBAL_PROMPT_DETAIL_MAX_BYTES
        || detail.chars().any(char::is_control)
    {
        return Err(GlobalPromptError::Detail);
    }
    Ok(())
}

fn validate_digest(digest: &str) -> Result<(), GlobalPromptError> {
    (digest.len() == GLOBAL_PROMPT_DIGEST_BYTES
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then_some(())
    .ok_or(GlobalPromptError::Candidates)
}

/// Invalid global prompt data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum GlobalPromptError {
    #[error("global prompt text is too long or holds control characters")]
    Text,
    #[error("global prompt candidates are invalid")]
    Candidates,
    #[error("engine identifier is invalid")]
    EngineId,
    #[error("global prompt detail is invalid")]
    Detail,
    #[error("a drifted engine must carry its external text, and only then")]
    Inconsistent,
    #[error("too many global prompt engines")]
    TooMany,
    #[error("an engine appears twice")]
    Duplicate,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(engine_id: &str, status: GlobalPromptEngineStatus) -> GlobalPromptEngineState {
        GlobalPromptEngineState {
            engine_id: engine_id.to_owned(),
            status,
            file: Some(".claude/CLAUDE.md".to_owned()),
            external_text: (status == GlobalPromptEngineStatus::Drifted)
                .then(|| "edited".to_owned()),
            reason: None,
        }
    }

    fn candidate(id: char, engine: &str) -> GlobalPromptCandidate {
        GlobalPromptCandidate {
            candidate_id: id.to_string().repeat(GLOBAL_PROMPT_DIGEST_BYTES),
            engine_ids: vec![engine.to_owned()],
            text: format!("prompt from {engine}"),
        }
    }

    #[test]
    fn prompts_validate_text_engines_and_drift() {
        let prompt = GlobalPrompt::new(
            GlobalPromptRevision::new(3),
            "Use British spelling.\n".to_owned(),
            GlobalPromptSetup::Ready,
            vec![
                state("claude", GlobalPromptEngineStatus::Synced),
                state("codex", GlobalPromptEngineStatus::Drifted),
            ],
        )
        .unwrap();
        assert_eq!(prompt.revision().get(), 3);
        assert_eq!(
            prompt.engine("codex").map(|state| state.status),
            Some(GlobalPromptEngineStatus::Drifted)
        );
        let mut inconsistent = state("codex", GlobalPromptEngineStatus::Synced);
        inconsistent.external_text = Some("edited".to_owned());
        assert_eq!(
            GlobalPrompt::new(
                GlobalPromptRevision::new(0),
                String::new(),
                GlobalPromptSetup::Ready,
                vec![inconsistent]
            ),
            Err(GlobalPromptError::Inconsistent)
        );
        assert_eq!(
            validate_prompt_text("bell\u{7}"),
            Err(GlobalPromptError::Text)
        );
        assert_eq!(
            validate_prompt_text(&"a".repeat(GLOBAL_PROMPT_MAX_BYTES + 1)),
            Err(GlobalPromptError::Text)
        );
    }

    #[test]
    fn setup_offers_at_least_two_distinct_candidates() {
        let choice = |candidates| {
            GlobalPrompt::new(
                GlobalPromptRevision::new(0),
                String::new(),
                GlobalPromptSetup::ChoiceRequired(candidates),
                Vec::new(),
            )
        };
        assert!(choice(vec![candidate('a', "claude"), candidate('b', "codex")]).is_ok());
        assert_eq!(
            choice(vec![candidate('a', "claude")]),
            Err(GlobalPromptError::Candidates)
        );
        assert_eq!(
            choice(vec![candidate('a', "claude"), candidate('a', "codex")]),
            Err(GlobalPromptError::Candidates)
        );
    }
}
