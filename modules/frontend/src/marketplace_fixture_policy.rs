//! Deterministic outcomes for the frontend marketplace fixture commands.
//!
//! This is the dependency-free native counterpart of
//! `modules/frontend/src/lib/runtime/fixtures/marketplace-commands.ts`.
//! Selecting an outcome does not call a client, transport, marketplace,
//! network, OAuth provider, persistence layer, or asynchronous runtime. A
//! later host boundary can interpret the typed outcome when it is ready to
//! execute a fixture command.

/// Exact failure returned by unavailable routine fixture operations.
pub const ROUTINE_FIXTURES_UNAVAILABLE_MESSAGE: &str =
    "Marketplace routine fixtures are unavailable.";

/// Exact failure returned by unavailable capability fixture operations.
pub const CAPABILITY_FIXTURES_UNAVAILABLE_MESSAGE: &str =
    "Marketplace capability fixtures are unavailable.";

/// Exact failure returned by unavailable npx-skills fixture operations.
pub const NPX_SKILLS_FIXTURES_UNAVAILABLE_MESSAGE: &str =
    "Marketplace npx-skills fixtures are unavailable.";

/// Exact authorization URL returned by the fixture OAuth-begin operation.
pub const FIXTURE_OAUTH_AUTHORIZATION_URL: &str = "https://fixture.invalid/oauth/authorize";

/// Exact continuation reference returned by the fixture OAuth-begin operation.
pub const FIXTURE_OAUTH_CONTINUATION_REFERENCE: &str = "fixture-oauth-continuation";

/// One command exposed by the legacy marketplace fixture client.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MarketplaceFixtureCommand {
    /// Preview a routine installation.
    PreviewRoutineInstall,
    /// Request a routine installation.
    RequestRoutineInstall,
    /// Decide a routine installation approval.
    DecideRoutineInstall,
    /// Enable an installed routine.
    EnableRoutine,
    /// Disable an installed routine.
    DisableRoutine,
    /// Remove an installed routine.
    RemoveRoutine,
    /// Roll back a routine installation.
    RollbackRoutine,
    /// Synchronize a routine.
    SyncRoutine,
    /// Resolve routine drift.
    ResolveRoutineDrift,
    /// Request a routine drift overwrite.
    RequestRoutineDriftOverwrite,
    /// Decide a routine drift overwrite approval.
    DecideRoutineDriftOverwrite,
    /// Invoke a routine.
    InvokeRoutine,
    /// Discover npx skills.
    DiscoverNpxSkills,
    /// Import npx skills.
    ImportNpxSkills,
    /// Preview a capability connection.
    PreviewCapabilityConnect,
    /// Request a capability connection.
    RequestCapabilityConnect,
    /// Decide a capability connection approval.
    DecideCapabilityConnect,
    /// Start a capability.
    StartCapability,
    /// Reconnect a capability.
    ReconnectCapability,
    /// Check capability health.
    CheckCapabilityHealth,
    /// Disconnect a capability.
    DisconnectCapability,
    /// Restart a capability.
    RestartCapability,
    /// Uninstall a capability.
    UninstallCapability,
    /// Enable a capability.
    EnableCapability,
    /// Disable a capability.
    DisableCapability,
    /// Remove a capability.
    RemoveCapability,
    /// Synchronize a capability.
    SyncCapability,
    /// Resolve capability drift.
    ResolveCapabilityDrift,
    /// Request a capability drift overwrite.
    RequestCapabilityDriftOverwrite,
    /// Decide a capability drift overwrite approval.
    DecideCapabilityDriftOverwrite,
    /// Request a capability invocation.
    RequestCapabilityInvocation,
    /// Decide a capability invocation approval.
    DecideCapabilityInvocation,
    /// Invoke a capability.
    InvokeCapability,
    /// Begin capability OAuth.
    BeginCapabilityOAuth,
    /// Complete capability OAuth.
    CompleteCapabilityOAuth,
    /// Refresh capability OAuth.
    RefreshCapabilityOAuth,
    /// Revoke capability OAuth.
    RevokeCapabilityOAuth,
}

impl MarketplaceFixtureCommand {
    /// Returns the exact fallback command ID for receipt-producing commands.
    ///
    /// `None` identifies an operation whose legacy fixture result is not a
    /// command receipt, such as an unavailable operation or OAuth begin.
    #[must_use]
    pub const fn fallback_command_id(self) -> Option<&'static str> {
        match self {
            Self::RequestRoutineInstall => Some("fixture-routine-install"),
            Self::DecideRoutineInstall => Some("fixture-routine-decision"),
            Self::EnableRoutine => Some("fixture-routine-enable"),
            Self::DisableRoutine => Some("fixture-routine-disable"),
            Self::RemoveRoutine => Some("fixture-routine-remove"),
            Self::RollbackRoutine => Some("fixture-routine-rollback"),
            Self::SyncRoutine => Some("fixture-routine-sync"),
            Self::ResolveRoutineDrift => Some("fixture-routine-drift"),
            Self::RequestRoutineDriftOverwrite => Some("fixture-routine-drift-overwrite-request"),
            Self::DecideRoutineDriftOverwrite => Some("fixture-routine-drift-overwrite-decision"),
            Self::ImportNpxSkills => Some("fixture-npx-skills-import"),
            Self::RequestCapabilityConnect => Some("fixture-capability-connect"),
            Self::DecideCapabilityConnect => Some("fixture-capability-decision"),
            Self::StartCapability => Some("fixture-capability-start"),
            Self::ReconnectCapability => Some("fixture-capability-reconnect"),
            Self::CheckCapabilityHealth => Some("fixture-capability-health"),
            Self::DisconnectCapability => Some("fixture-capability-disconnect"),
            Self::RestartCapability => Some("fixture-capability-restart"),
            Self::UninstallCapability => Some("fixture-capability-uninstall"),
            Self::EnableCapability => Some("fixture-capability-enable"),
            Self::DisableCapability => Some("fixture-capability-disable"),
            Self::RemoveCapability => Some("fixture-capability-remove"),
            Self::SyncCapability => Some("fixture-capability-sync"),
            Self::ResolveCapabilityDrift => Some("fixture-capability-drift"),
            Self::RequestCapabilityDriftOverwrite => {
                Some("fixture-capability-drift-overwrite-request")
            }
            Self::DecideCapabilityDriftOverwrite => {
                Some("fixture-capability-drift-overwrite-decision")
            }
            Self::CompleteCapabilityOAuth => Some("fixture-capability-oauth-complete"),
            Self::RefreshCapabilityOAuth => Some("fixture-capability-oauth-refresh"),
            Self::RevokeCapabilityOAuth => Some("fixture-capability-oauth-revoke"),
            Self::PreviewRoutineInstall
            | Self::InvokeRoutine
            | Self::DiscoverNpxSkills
            | Self::PreviewCapabilityConnect
            | Self::RequestCapabilityInvocation
            | Self::DecideCapabilityInvocation
            | Self::InvokeCapability
            | Self::BeginCapabilityOAuth => None,
        }
    }

    /// Returns the exact fixture-unavailable message for unsupported commands.
    #[must_use]
    pub const fn unsupported_message(self) -> Option<&'static str> {
        match self {
            Self::PreviewRoutineInstall | Self::InvokeRoutine => {
                Some(ROUTINE_FIXTURES_UNAVAILABLE_MESSAGE)
            }
            Self::DiscoverNpxSkills => Some(NPX_SKILLS_FIXTURES_UNAVAILABLE_MESSAGE),
            Self::PreviewCapabilityConnect
            | Self::RequestCapabilityInvocation
            | Self::DecideCapabilityInvocation
            | Self::InvokeCapability => Some(CAPABILITY_FIXTURES_UNAVAILABLE_MESSAGE),
            _ => None,
        }
    }

    /// Returns whether the command produces an accepted fixture receipt.
    #[must_use]
    pub const fn produces_receipt(self) -> bool {
        self.fallback_command_id().is_some()
    }

    /// Returns whether the command is an unsupported fixture operation.
    #[must_use]
    pub const fn is_unsupported(self) -> bool {
        self.unsupported_message().is_some()
    }

    /// Selects the deterministic pure outcome for this command.
    ///
    /// `None` represents an absent JavaScript `command_id` property. For a
    /// receipt-producing command, `Some`, including `Some("")`, is retained
    /// exactly; only `None` selects the operation-specific fallback. IDs do
    /// not affect unsupported or OAuth-begin outcomes because those legacy
    /// handlers do not consume a command ID.
    #[must_use]
    pub fn outcome(self, command_id: Option<&str>) -> MarketplaceFixtureOutcome {
        if let Some(message) = self.unsupported_message() {
            return MarketplaceFixtureOutcome::Unsupported {
                operation: self,
                message,
            };
        }

        if self == Self::BeginCapabilityOAuth {
            return MarketplaceFixtureOutcome::OAuthBegin {
                operation: self,
                authorization_url: FIXTURE_OAUTH_AUTHORIZATION_URL,
                continuation_reference: FIXTURE_OAUTH_CONTINUATION_REFERENCE,
            };
        }

        // Unsupported and OAuth-begin commands returned above are the only
        // commands without a receipt fallback in the exhaustive enum.
        let fallback = self.fallback_command_id().unwrap_or_default();
        MarketplaceFixtureOutcome::Receipt {
            operation: self,
            command_id: command_id.unwrap_or(fallback).to_owned(),
        }
    }
}

/// Deterministic, side-effect-free result of selecting one marketplace
/// fixture command.
///
/// Every variant carries its originating operation. This makes two commands
/// with the same caller-supplied ID distinguishable and keeps a receipt,
/// unsupported result, or OAuth result from becoming an ambiguous shared
/// fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MarketplaceFixtureOutcome {
    /// An accepted fixture receipt identity.
    Receipt {
        /// Command that produced this receipt outcome.
        operation: MarketplaceFixtureCommand,
        /// Caller-supplied command ID, or the exact operation fallback.
        command_id: String,
    },
    /// An operation unavailable in the deterministic fixture.
    Unsupported {
        /// Unsupported command that produced this failure outcome.
        operation: MarketplaceFixtureCommand,
        /// Exact client-facing fixture-unavailable message.
        message: &'static str,
    },
    /// The deterministic capability OAuth authorization values.
    OAuthBegin {
        /// OAuth operation that produced this outcome.
        operation: MarketplaceFixtureCommand,
        /// Exact authorization URL exposed by the fixture.
        authorization_url: &'static str,
        /// Exact continuation reference exposed by the fixture.
        continuation_reference: &'static str,
    },
}

impl MarketplaceFixtureOutcome {
    /// Returns the command whose fixture behavior produced this outcome.
    #[must_use]
    pub const fn operation(&self) -> MarketplaceFixtureCommand {
        match self {
            Self::Receipt { operation, .. }
            | Self::Unsupported { operation, .. }
            | Self::OAuthBegin { operation, .. } => *operation,
        }
    }

    /// Returns the command ID when this is a receipt outcome.
    #[must_use]
    pub fn command_id(&self) -> Option<&str> {
        match self {
            Self::Receipt { command_id, .. } => Some(command_id),
            Self::Unsupported { .. } | Self::OAuthBegin { .. } => None,
        }
    }
}

/// Selects a deterministic outcome for one marketplace fixture command.
#[must_use]
pub fn marketplace_fixture_outcome(
    command: MarketplaceFixtureCommand,
    command_id: Option<&str>,
) -> MarketplaceFixtureOutcome {
    command.outcome(command_id)
}
