//! Pure command-receipt selection for the frontend runtime fixture.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/runtime/fixtures/policies.ts`. It retains the
//! fixture's command-specific fallback IDs and TypeScript nullish-coalescing
//! behavior while stopping at a typed intent. Constructing an intent performs
//! no client, transport, engine, journal, clock, or other host operation.

/// One of the fixture client policy commands whose receipt identity is chosen
/// by this module.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FixturePolicyCommand {
    /// Update the global model-behaviour fixture policy.
    UpdateModelBehaviour,
    /// Update the thread-session-policy fixture policy.
    UpdateThreadSessionPolicy,
    /// Update the thread-retention-policy fixture policy.
    UpdateThreadRetentionPolicy,
}

impl FixturePolicyCommand {
    /// Returns the command ID used when the matching TypeScript input omits
    /// `command_id`.
    #[must_use]
    pub const fn default_command_id(self) -> &'static str {
        match self {
            Self::UpdateModelBehaviour => "fixture-model-behaviour-update",
            Self::UpdateThreadSessionPolicy => "fixture-session-policy-update",
            Self::UpdateThreadRetentionPolicy => "fixture-retention-update",
        }
    }
}

/// Typed metadata describing the receipt a fixture command would return.
///
/// This is an intent for later fixture-driven proof, not a transport receipt:
/// creating one does not run a client command or claim that an engine accepted
/// anything. The command ID is owned so a supplied `&str` remains byte-for-
/// byte identical after selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureReceiptIntent {
    /// The fixture policy command associated with this intent.
    pub command: FixturePolicyCommand,
    /// The supplied command ID, or the command's fixture fallback ID.
    pub command_id: String,
}

impl FixtureReceiptIntent {
    /// Selects the receipt identity for one fixture policy command.
    ///
    /// `None` is the Rust equivalent of an absent JavaScript property. In
    /// particular, `Some("")` remains an explicitly supplied empty ID and
    /// does not fall back to [`FixturePolicyCommand::default_command_id`].
    #[must_use]
    pub fn new(command: FixturePolicyCommand, command_id: Option<&str>) -> Self {
        Self {
            command,
            command_id: command_id
                .unwrap_or(command.default_command_id())
                .to_owned(),
        }
    }
}

/// Selects a typed fixture receipt intent for one policy command.
#[must_use]
pub fn fixture_receipt_intent(
    command: FixturePolicyCommand,
    command_id: Option<&str>,
) -> FixtureReceiptIntent {
    FixtureReceiptIntent::new(command, command_id)
}

/// Selects the model-behaviour update fixture receipt intent.
#[must_use]
pub fn update_model_behaviour(command_id: Option<&str>) -> FixtureReceiptIntent {
    fixture_receipt_intent(FixturePolicyCommand::UpdateModelBehaviour, command_id)
}

/// Selects the thread-session-policy update fixture receipt intent.
#[must_use]
pub fn update_thread_session_policy(command_id: Option<&str>) -> FixtureReceiptIntent {
    fixture_receipt_intent(FixturePolicyCommand::UpdateThreadSessionPolicy, command_id)
}

/// Selects the thread-retention-policy update fixture receipt intent.
#[must_use]
pub fn update_thread_retention_policy(command_id: Option<&str>) -> FixtureReceiptIntent {
    fixture_receipt_intent(
        FixturePolicyCommand::UpdateThreadRetentionPolicy,
        command_id,
    )
}
