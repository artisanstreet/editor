//! Native reasoning-display presentation policy.
//!
//! Port of `modules/frontend/src/lib/engine/reasoning-display.ts`.
//! The TypeScript source resolves how a thread's own model presents its
//! thinking, which decides whether the live thinking line may say a summary
//! or must keep a content-free verb.
//!
//! `summary` models publish provider-authored summaries of their own thinking —
//! prose written to be read — so the live thinking line can say the latest one
//! verbatim. `trace` models stream raw chain-of-thought instead: it is neither
//! written for a reader nor a summary of anything, so those turns keep a verb.
//!
//! Contract carried over from the catalog and the TS leaf:
//!
//! - Recognized values are exactly `summary` and `trace`. Unknown, absent, or
//!   unresolvable models read as `summary`; every engine Artisan runs today
//!   publishes summaries, and the failure the other default would cause is
//!   the worse one: a model whose thinking is perfectly presentable would be
//!   silenced behind a verb for the whole run, with nothing on screen to
//!   explain why. A model that genuinely streams raw thought says so in the
//!   catalog.
//! - `model_reasoning_display` defaults absent `reasoning_display` to `summary`
//!   — the same `?? "summary"` in `modules/catalog/src/schema.ts`.
//! - `policy_reasoning_display` returns `summary` when the policy is absent,
//!   when its model is absent, or when the catalog has no entry for it;
//!   otherwise it returns the catalog entry's display.
//! - This is pure presentation policy: it never starts an engine, inspects a
//!   provider, or performs I/O. Callers supply the already-resolved catalog
//!   value.
//!
//! Deliberately out of boundary: engine start, provider inspection, harness
//! routing, catalog fetching, and any `speed`/`effort` presentation — those
//! live in adjacent leaves (`speed-presentation`, `model-selection`).

#![allow(clippy::module_name_repetitions)]

use std::fmt;
use std::str::FromStr;

/// How a model's reasoning stream is safe to show.
///
/// Mirrors `ReasoningDisplay` in `modules/catalog/src/schema.ts`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Default)]
pub enum ReasoningDisplay {
    /// Provider-authored summary prose. The live line may show it verbatim.
    #[default]
    Summary,
    /// Raw chain-of-thought. The live line must keep a verb.
    Trace,
}

impl ReasoningDisplay {
    /// Every recognized display value, in canonical order.
    pub const ALL: [Self; 2] = [Self::Summary, Self::Trace];

    /// Returns the canonical wire/display string.
    ///
    /// Exact values mirror the catalog literals: `"summary"` and `"trace"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Trace => "trace",
        }
    }

    /// Returns the exact audited label for this display.
    ///
    /// For this leaf the label is the canonical string itself; the live-line
    /// decision is the presentation fact, not a separate humanized name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        self.as_str()
    }

    /// Returns a one-line description of what the value means for the UI.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Summary => {
                "provider-authored summary prose written to be read; the live thinking line may show it verbatim"
            }
            Self::Trace => {
                "raw chain-of-thought not written for a reader; the live thinking line keeps a content-free verb"
            }
        }
    }

    /// Whether this display allows showing a live summary.
    #[must_use]
    pub const fn can_show_live_summary(self) -> bool {
        matches!(self, Self::Summary)
    }

    /// Whether this display is the summarizing kind.
    #[must_use]
    pub const fn is_summary(self) -> bool {
        matches!(self, Self::Summary)
    }

    /// Whether this display is the raw-trace kind.
    #[must_use]
    pub const fn is_trace(self) -> bool {
        matches!(self, Self::Trace)
    }

    /// Parses an exact canonical literal.
    ///
    /// Only `"summary"` and `"trace"` are recognized; casing, surrounding
    /// whitespace, and any other alias must be normalized by the caller if
    /// desired (see [`Self::parse_normalized`]).
    #[must_use]
    pub fn from_canonical_str(input: &str) -> Option<Self> {
        match input {
            "summary" => Some(Self::Summary),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    /// Parses a trimmed, ASCII-case-insensitive value.
    ///
    /// Accepts the two canonical literals in any ASCII casing with surrounding
    /// whitespace trimmed. This is a convenience for callers that already
    /// normalize user or config input; the canonical policy itself is
    /// case-sensitive, and the exact parser remains [`Self::from_canonical_str`].
    #[must_use]
    pub fn parse_normalized(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "summary" => Some(Self::Summary),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    /// Ordering rank for deterministic presentation order, if needed.
    ///
    /// `summary` sorts before `trace`, mirroring the catalog's fallback
    /// (`absent -> summary`) and the TS default.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Summary => 0,
            Self::Trace => 1,
        }
    }
}

impl fmt::Display for ReasoningDisplay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ReasoningDisplay {
    type Err = UnknownReasoningDisplay;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::from_canonical_str(input).ok_or_else(|| UnknownReasoningDisplay {
            input: input.to_owned(),
        })
    }
}

/// Error for an unrecognized canonical literal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownReasoningDisplay {
    input: String,
}

impl fmt::Display for UnknownReasoningDisplay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown reasoning display '{}'", self.input)
    }
}

impl std::error::Error for UnknownReasoningDisplay {}

/// The minimal thread-policy projection the display policy needs.
///
/// The TypeScript source checks `policy?.model === undefined` and then looks
/// up `model_manifest.models.find(harness === engine_id && (native_model_id ===
/// model || id === model))`. This pure leaf does not fetch or search a
/// manifest; callers supply the already-resolved catalog value via
/// `resolved_model_display`. The view exists so the policy's own
/// availability semantics remain explicit and testable here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PolicyView<'a> {
    /// Harness/engine that owns the model, if known.
    pub engine_id: Option<&'a str>,
    /// The policy's selected model id or `native_model_id`, if any.
    pub model: Option<&'a str>,
}

impl<'a> PolicyView<'a> {
    /// Builds a view with an engine and a model.
    #[must_use]
    pub const fn new(engine_id: Option<&'a str>, model: Option<&'a str>) -> Self {
        Self { engine_id, model }
    }
}

/// Resolves a model's own display, defaulting an absent catalog value to
/// `summary`.
///
/// Mirrors `model_reasoning_display` in `modules/catalog/src/schema.ts`:
/// `model.capabilities.reasoning_display ?? "summary"`.
#[must_use]
pub const fn model_reasoning_display(display: Option<ReasoningDisplay>) -> ReasoningDisplay {
    match display {
        Some(value) => value,
        None => ReasoningDisplay::Summary,
    }
}

/// Resolves how the thread's own model presents its thinking, which decides
/// whether the live thinking line may say a summary or must keep a verb.
///
/// Mirrors `policy_reasoning_display` in
/// `modules/frontend/src/lib/engine/reasoning-display.ts`:
///
/// - `None` policy reads as `summary`.
/// - A policy with no `model` reads as `summary`.
/// - An unresolvable model (caller supplies `None` for `resolved_model_display`)
///   reads as `summary`.
/// - Otherwise the resolved catalog entry's display is returned verbatim.
///
/// The resolver is pure: it never starts engines, inspects providers, or
/// reaches into a manifest. Callers perform the `harness + id/native_model_id`
/// lookup and pass the result as `resolved_model_display`.
#[must_use]
pub fn policy_reasoning_display(
    policy: Option<PolicyView<'_>>,
    resolved_model_display: Option<ReasoningDisplay>,
) -> ReasoningDisplay {
    let Some(view) = policy else {
        return ReasoningDisplay::Summary;
    };
    if view.model.is_none() {
        return ReasoningDisplay::Summary;
    }
    match resolved_model_display {
        Some(display) => display,
        None => ReasoningDisplay::Summary,
    }
}

/// Whether the live reasoning summary should be shown for a display.
///
/// `trace` models publish no summary to say, so those turns keep the verb;
/// `summary` models allow the verbatim summary. This is the `=== "trace"
/// ? undefined : conversation_live_reasoning_summary(...)` branch in
/// `thread-workspace.svelte`.
#[must_use]
pub const fn should_show_live_summary(display: ReasoningDisplay) -> bool {
    display.can_show_live_summary()
}
