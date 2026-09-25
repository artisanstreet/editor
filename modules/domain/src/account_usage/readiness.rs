//! Whether an engine's account can run, as the Forge decides it.
//!
//! The Forge judges readiness from its own probed usage reads and freshness
//! window and sends the verdict with every usage report; clients render it
//! and never re-derive it from the report or their own clock.

use std::fmt;

use super::{EngineUsageError, validate_usage_text};
use crate::bounds::ENGINE_USAGE_REASON_MAX_BYTES;

/// The Forge's verdict on one engine account.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EngineReadinessVerdict {
    /// The account authenticated inside the freshness window; its models run.
    Ready,
    /// The provider reports no signed-in account.
    NeedsSignIn,
    /// The account is unreachable, failing, stale, or has no surface.
    NotReady,
    /// The Forge has not observed the account yet.
    Checking,
}

impl EngineReadinessVerdict {
    /// Returns the stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NeedsSignIn => "needs_sign_in",
            Self::NotReady => "not_ready",
            Self::Checking => "checking",
        }
    }
}

impl fmt::Display for EngineReadinessVerdict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One verdict with its presentation-ready reason.
///
/// The reason is a complete sentence naming the engine (for example "Codex
/// account sign-in is required."); `Ready` carries none.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineReadiness {
    verdict: EngineReadinessVerdict,
    reason: Option<String>,
}

impl EngineReadiness {
    /// Creates one verdict with an optional bounded reason.
    ///
    /// # Errors
    ///
    /// Returns [`EngineUsageError`] when the reason is blank, carries control
    /// characters, or exceeds the usage reason bound.
    pub fn new(
        verdict: EngineReadinessVerdict,
        reason: Option<String>,
    ) -> Result<Self, EngineUsageError> {
        if let Some(reason) = reason.as_deref() {
            validate_usage_text(reason, "readiness_reason", ENGINE_USAGE_REASON_MAX_BYTES)?;
        }
        Ok(Self { verdict, reason })
    }

    /// A ready verdict.
    #[must_use]
    pub const fn ready() -> Self {
        Self {
            verdict: EngineReadinessVerdict::Ready,
            reason: None,
        }
    }

    /// A not-ready verdict without a reason: the conservative default of a
    /// report the Forge has not judged.
    #[must_use]
    pub const fn not_ready() -> Self {
        Self {
            verdict: EngineReadinessVerdict::NotReady,
            reason: None,
        }
    }

    /// Returns the verdict.
    #[must_use]
    pub const fn verdict(&self) -> EngineReadinessVerdict {
        self.verdict
    }

    /// Returns whether the engine's models may run.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self.verdict, EngineReadinessVerdict::Ready)
    }

    /// Returns the presentation-ready reason, when one applies.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}
