//! Deterministic state and persistence policy for the usage-recovery setting.
//!
//! This is the dependency-free native counterpart of
//! `routes/components/settings/usage-recovery.svelte`. The surrounding
//! settings controller owns the authoritative stream and performs the actual
//! persistence. This leaf only admits the switch action, tracks the local
//! save/message state, and applies authoritative updates.

#![allow(clippy::module_name_repetitions)]

/// The exact reader-facing message shown when Forge does not confirm a save.
pub const USAGE_RECOVERY_SAVE_FAILURE_MESSAGE: &str =
    "Couldn't verify the new default. Forge did not confirm the change.";

/// The durable setting's scope for newly created turns.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UsageRecoveryDefaultScope {
    /// The setting is captured as the default for a new interrupted turn.
    NewTurn,
}

impl UsageRecoveryDefaultScope {
    /// Returns the stable scope spelling used by policy-facing diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NewTurn => "new_turn",
        }
    }
}

/// Stable facts shared by the settings surface and interruption cards.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct UsageRecoveryScopeFacts {
    /// The scope in which the settings value becomes a default.
    pub default_scope: UsageRecoveryDefaultScope,
    /// Whether an individual interruption may override the default.
    pub per_interruption_override_available: bool,
}

/// The audited scope facts from the legacy settings and interruption views.
pub const USAGE_RECOVERY_SCOPE_FACTS: UsageRecoveryScopeFacts = UsageRecoveryScopeFacts {
    default_scope: UsageRecoveryDefaultScope::NewTurn,
    per_interruption_override_available: true,
};

/// Returns the stable usage-recovery scope facts.
#[must_use]
pub const fn usage_recovery_scope_facts() -> UsageRecoveryScopeFacts {
    USAGE_RECOVERY_SCOPE_FACTS
}

/// Authoritative values supplied by the session-defaults stream.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct UsageRecoveryAuthoritativeState {
    /// Whether Forge-backed session defaults are currently available.
    pub available: bool,
    /// The current authoritative default for usage-limit continuation.
    pub auto_continue_usage_limits: bool,
}

impl UsageRecoveryAuthoritativeState {
    /// Creates one authoritative snapshot from the stream's two relevant
    /// fields.
    #[must_use]
    pub const fn new(available: bool, auto_continue_usage_limits: bool) -> Self {
        Self {
            available,
            auto_continue_usage_limits,
        }
    }
}

/// Typed persistence intent emitted for an admitted switch action.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UsageRecoveryPersistenceCommand {
    /// Persist the requested new session-default value.
    SetAutoContinueUsageLimits {
        /// Desired value passed to the session-defaults controller.
        enabled: bool,
    },
}

impl UsageRecoveryPersistenceCommand {
    /// Returns the desired value carried by this command.
    #[must_use]
    pub const fn enabled(self) -> bool {
        match self {
            Self::SetAutoContinueUsageLimits { enabled } => enabled,
        }
    }
}

/// Why a switch action was not admitted.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UsageRecoverySaveRejection {
    /// The authoritative settings state is unavailable.
    Unavailable,
    /// A previous save is still in flight.
    Saving,
}

/// One result observed after the persistence boundary completes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UsageRecoverySaveOutcome {
    /// Forge accepted and confirmed the requested persistence operation.
    Succeeded,
    /// The operation failed to obtain a Forge confirmation.
    Failed,
}

/// Authoritative usage-recovery values plus local interaction state.
///
/// `available` and `auto_continue_usage_limits` are never optimistically
/// changed by [`Self::start_save`]. They are replaced only by
/// [`Self::apply_authoritative_update`], matching the settings controller's
/// stream. `saving` and `message` are local to this surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageRecoverySettingsState {
    /// Current authoritative settings availability.
    pub available: bool,
    /// Current authoritative new-turn continuation default.
    pub auto_continue_usage_limits: bool,
    /// Whether a persistence command is currently in flight.
    pub saving: bool,
    /// Reader-facing status text; empty means no message is shown.
    pub message: String,
}

impl UsageRecoverySettingsState {
    /// Creates local state from an authoritative stream snapshot.
    #[must_use]
    pub fn new(authoritative: UsageRecoveryAuthoritativeState) -> Self {
        Self {
            available: authoritative.available,
            auto_continue_usage_limits: authoritative.auto_continue_usage_limits,
            saving: false,
            message: String::new(),
        }
    }

    /// Returns the authoritative portion of this state.
    #[must_use]
    pub const fn authoritative(&self) -> UsageRecoveryAuthoritativeState {
        UsageRecoveryAuthoritativeState {
            available: self.available,
            auto_continue_usage_limits: self.auto_continue_usage_limits,
        }
    }

    /// Whether the switch must be disabled in the renderer.
    #[must_use]
    pub const fn switch_disabled(&self) -> bool {
        !self.available || self.saving
    }

    /// Whether an action may be admitted and a save started.
    #[must_use]
    pub const fn can_start_save(&self) -> bool {
        !self.switch_disabled()
    }

    /// Starts a save if the switch is admitted.
    ///
    /// Starting clears the previous status message, marks the local save as
    /// in flight, and emits the inverse of the current authoritative value.
    /// The authoritative value itself is intentionally left untouched until a
    /// later stream update arrives.
    ///
    /// # Errors
    ///
    /// Returns [`UsageRecoverySaveRejection::Saving`] if a save is already in
    /// flight, or [`UsageRecoverySaveRejection::Unavailable`] if usage recovery
    /// is unavailable.
    pub fn start_save(
        &mut self,
    ) -> Result<UsageRecoveryPersistenceCommand, UsageRecoverySaveRejection> {
        if self.saving {
            return Err(UsageRecoverySaveRejection::Saving);
        }
        if !self.available {
            return Err(UsageRecoverySaveRejection::Unavailable);
        }

        self.message.clear();
        self.saving = true;
        Ok(
            UsageRecoveryPersistenceCommand::SetAutoContinueUsageLimits {
                enabled: !self.auto_continue_usage_limits,
            },
        )
    }

    /// Applies an authoritative stream update without disturbing local save
    /// or status-message state.
    pub fn apply_authoritative_update(&mut self, next: UsageRecoveryAuthoritativeState) {
        self.available = next.available;
        self.auto_continue_usage_limits = next.auto_continue_usage_limits;
    }

    /// Settles a persistence attempt.
    ///
    /// Both outcomes always clear `saving`. A failure reports the exact
    /// legacy message and leaves the authoritative default unchanged. A
    /// successful save leaves the message as-is; normal callers have already
    /// cleared it in [`Self::start_save`].
    pub fn finish_save(&mut self, outcome: UsageRecoverySaveOutcome) {
        self.saving = false;
        if matches!(outcome, UsageRecoverySaveOutcome::Failed) {
            USAGE_RECOVERY_SAVE_FAILURE_MESSAGE.clone_into(&mut self.message);
        }
    }

    /// Settles a successful save and clears its local in-flight state.
    pub fn save_succeeded(&mut self) {
        self.finish_save(UsageRecoverySaveOutcome::Succeeded);
    }

    /// Settles a failed save and exposes the exact Forge-confirmation error.
    pub fn save_failed(&mut self) {
        self.finish_save(UsageRecoverySaveOutcome::Failed);
    }
}
