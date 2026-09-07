//! Dependency-free data policy for deterministic thread metadata refinement.
//!
//! This is the native counterpart of
//! `modules/backend/src/threads/thread-metadata-refiner.ts`. It bounds each
//! evidence category independently, then derives only the current goal,
//! rename suggestion, and unlocked title that the source implementation can
//! derive. It does not provide a provider, run asynchronous work, read or
//! write storage, inspect a clock, or cross a protocol transport boundary.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{fmt, str::FromStr};

/// Maximum number of retained entries in each independent evidence category.
pub const MAX_CONTEXT_ITEMS: usize = 8;

/// Maximum length of one evidence item in JavaScript-compatible UTF-16 code
/// units.
pub const MAX_CONTEXT_TEXT_LENGTH: usize = 500;

/// The lifecycle event that caused one metadata refinement request.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ThreadMetadataRefinementTrigger {
    /// An assistant message completed.
    AssistantMessage,
    /// A user message was queued or used to steer a run.
    UserMessage,
    /// A run or orchestration group started.
    RunStarted,
    /// A run or orchestration group completed or stopped.
    RunCompleted,
    /// A run or orchestration group failed.
    RunFailed,
}

impl ThreadMetadataRefinementTrigger {
    /// The five trigger values in their TypeScript source order.
    pub const ALL: [Self; 5] = [
        Self::AssistantMessage,
        Self::UserMessage,
        Self::RunStarted,
        Self::RunCompleted,
        Self::RunFailed,
    ];

    /// Returns the exact TypeScript trigger spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AssistantMessage => "assistant_message",
            Self::UserMessage => "user_message",
            Self::RunStarted => "run_started",
            Self::RunCompleted => "run_completed",
            Self::RunFailed => "run_failed",
        }
    }

    /// Parses one exact TypeScript trigger spelling.
    ///
    /// Matching is case-sensitive and does not trim or otherwise rewrite the
    /// supplied value.
    ///
    /// # Errors
    ///
    /// Returns [`ThreadMetadataRefinementTriggerParseError`] when `value` is
    /// not one of the five trigger spellings.
    pub fn parse(value: &str) -> Result<Self, ThreadMetadataRefinementTriggerParseError> {
        match value {
            "assistant_message" => Ok(Self::AssistantMessage),
            "user_message" => Ok(Self::UserMessage),
            "run_started" => Ok(Self::RunStarted),
            "run_completed" => Ok(Self::RunCompleted),
            "run_failed" => Ok(Self::RunFailed),
            _ => Err(ThreadMetadataRefinementTriggerParseError),
        }
    }
}

impl fmt::Display for ThreadMetadataRefinementTrigger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ThreadMetadataRefinementTrigger {
    type Err = ThreadMetadataRefinementTriggerParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Error returned when a trigger is not an exact contract spelling.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadMetadataRefinementTriggerParseError;

impl fmt::Display for ThreadMetadataRefinementTriggerParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid thread metadata refinement trigger")
    }
}

impl std::error::Error for ThreadMetadataRefinementTriggerParseError {}

/// The projection fields consulted by the deterministic refiner.
///
/// This is intentionally a small policy view of the protocol's
/// `ThreadListItem`: activity, affinity, timestamps, and project collections
/// do not participate in the source refiner's result and therefore do not
/// cross this dependency-free seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadMetadataProjection {
    /// Existing current goal, if the projection has one.
    pub current_goal: Option<String>,
    /// Current thread title.
    pub title: String,
    /// Whether automatic title replacement is suppressed.
    pub title_locked: bool,
}

impl ThreadMetadataProjection {
    /// Creates a policy projection from the fields used by the refiner.
    #[must_use]
    pub fn new(title: impl Into<String>, current_goal: Option<String>, title_locked: bool) -> Self {
        Self {
            current_goal,
            title: title.into(),
            title_locked,
        }
    }
}

/// A protocol-shaped reference to one project.
///
/// The refiner never invents or derives this value. It is present only so a
/// later adapter can carry an explicitly supplied project list through the
/// same typed refinement shape.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProjectRef {
    /// Human-readable project label.
    pub display_name: String,
    /// Stable project identity.
    pub project_id: String,
    /// Opaque project-root description.
    pub root_path: String,
}

impl ProjectRef {
    /// Creates a project reference without normalizing any supplied field.
    #[must_use]
    pub fn new(
        display_name: impl Into<String>,
        project_id: impl Into<String>,
        root_path: impl Into<String>,
    ) -> Self {
        Self {
            display_name: display_name.into(),
            project_id: project_id.into(),
            root_path: root_path.into(),
        }
    }
}

/// Bounded, provider-neutral input to one metadata refinement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadMetadataRefinerInput {
    /// The current thread projection view.
    pub projection: ThreadMetadataProjection,
    /// The lifecycle trigger; it is preserved but does not alter this
    /// deterministic source algorithm.
    pub trigger: ThreadMetadataRefinementTrigger,
    /// Recent assistant-message evidence.
    pub recent_assistant_text: Vec<String>,
    /// Recent user-message evidence.
    pub recent_user_text: Vec<String>,
    /// Recent activity labels.
    pub recent_activity: Vec<String>,
    /// Recent file references.
    pub recent_files: Vec<String>,
    /// Recent artifact labels.
    pub recent_artifacts: Vec<String>,
}

impl ThreadMetadataRefinerInput {
    /// Returns a bounded copy of this input.
    #[must_use]
    pub fn bounded(&self) -> Self {
        Self {
            projection: self.projection.clone(),
            trigger: self.trigger,
            recent_assistant_text: bound_context(&self.recent_assistant_text),
            recent_user_text: bound_context(&self.recent_user_text),
            recent_activity: bound_context(&self.recent_activity),
            recent_files: bound_context(&self.recent_files),
            recent_artifacts: bound_context(&self.recent_artifacts),
        }
    }
}

/// The optional metadata fields an automatic refinement may change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadMetadataRefinement {
    /// The proposed current goal, when user evidence or an existing goal is
    /// available.
    pub current_goal: Option<String>,
    /// Explicitly supplied project references, never inferred by this policy.
    pub mentioned_projects: Option<Vec<ProjectRef>>,
    /// The deterministic title candidate, always present in the source live
    /// implementation.
    pub rename_suggestion: Option<String>,
    /// The title candidate when the current title is not locked.
    pub title: Option<String>,
}

/// Stateless evaluator for the thread metadata refiner policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ThreadMetadataRefinerPolicy;

impl ThreadMetadataRefinerPolicy {
    /// Creates the stateless policy value.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Bounds the five evidence categories without changing the projection or
    /// trigger.
    #[must_use]
    pub fn bound_input(input: &ThreadMetadataRefinerInput) -> ThreadMetadataRefinerInput {
        input.bounded()
    }

    /// Produces the deterministic metadata proposal for one input.
    #[must_use]
    pub fn refine(input: &ThreadMetadataRefinerInput) -> ThreadMetadataRefinement {
        refine_thread_metadata(input)
    }
}

/// Bounds provider evidence before it reaches a refiner implementation.
///
/// Each category takes its last eight raw entries first. The retained entries
/// are then trimmed with ECMAScript `String.prototype.trim` semantics,
/// truncated to at most 500 UTF-16 code units without splitting a Rust
/// Unicode scalar, and finally filtered when empty. Categories never share a
/// retention budget or reorder one another.
#[must_use]
pub fn bound_thread_metadata_refiner_input(
    input: &ThreadMetadataRefinerInput,
) -> ThreadMetadataRefinerInput {
    input.bounded()
}

/// Produces the deterministic metadata proposal from one raw input.
///
/// The input is bounded before selection. The latest retained user text wins
/// over the latest retained file, and the current title is the final fallback.
/// Latest user text replaces the current goal; when there is no user text, a
/// non-empty existing current goal is retained. The title is omitted only
/// while locked, while the rename suggestion remains present. Mentioned
/// projects are always absent because this policy has no project inference
/// input.
#[must_use]
pub fn refine_thread_metadata(input: &ThreadMetadataRefinerInput) -> ThreadMetadataRefinement {
    let input = bound_thread_metadata_refiner_input(input);
    let latest_user_text = input.recent_user_text.last();
    let latest_file = input.recent_files.last();
    let selected_title = latest_user_text
        .or(latest_file)
        .cloned()
        .unwrap_or_else(|| input.projection.title.clone());
    let current_goal = latest_user_text.cloned().or_else(|| {
        input
            .projection
            .current_goal
            .as_ref()
            .filter(|value| !value.is_empty())
            .cloned()
    });

    ThreadMetadataRefinement {
        current_goal,
        mentioned_projects: None,
        rename_suggestion: Some(selected_title.clone()),
        title: (!input.projection.title_locked).then_some(selected_title),
    }
}

/// Counts a string's JavaScript-compatible UTF-16 code units.
#[must_use]
pub fn javascript_utf16_code_units(value: &str) -> usize {
    value.encode_utf16().count()
}

fn bound_context(values: &[String]) -> Vec<String> {
    let start = values.len().saturating_sub(MAX_CONTEXT_ITEMS);
    values[start..]
        .iter()
        .filter_map(|value| {
            let trimmed = value.trim_matches(is_ecmascript_whitespace);
            let bounded = utf16_prefix(trimmed, MAX_CONTEXT_TEXT_LENGTH);
            (!bounded.is_empty()).then_some(bounded)
        })
        .collect()
}

fn utf16_prefix(value: &str, maximum: usize) -> String {
    let mut bounded = String::new();
    let mut used = 0_usize;

    for character in value.chars() {
        let character_length = character.len_utf16();
        if character_length > maximum.saturating_sub(used) {
            break;
        }
        bounded.push(character);
        used += character_length;
    }

    bounded
}

/// Matches the whitespace removed by JavaScript `String.prototype.trim`.
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
