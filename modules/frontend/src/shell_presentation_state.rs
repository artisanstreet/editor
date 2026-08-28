//! Pure durable-state policy for the native shell presentation preferences.
//!
//! The legacy implementation stores one versioned record under a fixed key.
//! This module keeps the record shape and the decisions around storage, while
//! leaving encoding, persistence, and the browser/desktop storage adapter to a
//! caller. A caller first classifies a read as a [`StorageReadObservation`],
//! executes the returned [`StorageAction`], and feeds the write observation
//! back to [`repair`] when the action was a repair write.
//!
//! A malformed value and a read failure are intentionally indistinguishable to
//! the caller of `Load`: both return the default and first try to write that
//! default back. Only a failed repair write requests removal of the key. A
//! normal save has no failure path in this policy; its storage error is
//! deliberately absorbed, matching the legacy `Effect.result` boundary.

/// The only schema version understood by the shell presentation state.
pub const SHELL_PRESENTATION_SCHEMA_VERSION: u8 = 1;

/// The exact durable-storage key used by the legacy preferences service.
pub const SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY: &str = "artisan.shell-presentation";

/// The version-1 shell presentation record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellPresentationState {
    /// The persisted schema version. Valid stored records use version `1`.
    pub version: u8,
    /// Whether the left shell rail is collapsed.
    pub left_collapsed: bool,
    /// Whether the right shell rail is collapsed.
    pub right_collapsed: bool,
}

impl ShellPresentationState {
    /// Creates a version-1 state from the two durable presentation settings.
    #[must_use]
    pub const fn new(left_collapsed: bool, right_collapsed: bool) -> Self {
        Self {
            version: SHELL_PRESENTATION_SCHEMA_VERSION,
            left_collapsed,
            right_collapsed,
        }
    }
}

/// The default used when no valid stored state is available.
pub const DEFAULT_SHELL_PRESENTATION_STATE: ShellPresentationState =
    ShellPresentationState::new(false, false);

impl Default for ShellPresentationState {
    fn default() -> Self {
        DEFAULT_SHELL_PRESENTATION_STATE
    }
}

/// The result of classifying a storage read before applying load policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageReadObservation {
    /// The storage key was absent.
    Missing,
    /// The storage value decoded as the exact version-1 state schema.
    Valid(ShellPresentationState),
    /// The key existed, but its value did not decode as the state schema.
    Malformed,
    /// The storage read itself failed.
    ReadFailure,
}

/// The result observed after requesting a storage write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageWriteObservation {
    /// The requested write completed.
    Succeeded,
    /// The requested write failed.
    Failed,
}

/// A pure storage request emitted by a state transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageAction {
    /// No storage request is needed for this transition.
    None,
    /// Write the version-1 default as malformed-value repair.
    WriteDefault {
        /// The key to write.
        key: &'static str,
        /// The repaired value.
        state: ShellPresentationState,
    },
    /// Persist a caller-provided version-1 state.
    Save {
        /// The key to write.
        key: &'static str,
        /// The value to persist.
        state: ShellPresentationState,
    },
    /// Remove a malformed value after its repair write failed.
    Remove {
        /// The key to remove.
        key: &'static str,
    },
}

/// The pure result of applying load policy to one storage read observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadTransition {
    /// The state returned to the caller.
    pub state: ShellPresentationState,
    /// The first storage request, if this read requires repair.
    pub action: StorageAction,
}

/// The pure result of applying a repair-write observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RepairTransition {
    /// The follow-up request, if repair removal is required.
    pub action: StorageAction,
}

/// The outcome of a normal save from the policy's point of view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveOutcome {
    /// The save write completed.
    Succeeded,
    /// The save write failed, but the failure is intentionally absorbed.
    FailureAbsorbed,
}

/// The pure result of a normal save request and its observed write outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaveTransition {
    /// The save request that was issued.
    pub action: StorageAction,
    /// Whether the write succeeded or was absorbed as a deliberate failure.
    pub outcome: SaveOutcome,
}

/// Applies the legacy load policy to a classified storage read.
#[must_use]
pub const fn load(observation: StorageReadObservation) -> LoadTransition {
    match observation {
        StorageReadObservation::Missing => LoadTransition {
            state: DEFAULT_SHELL_PRESENTATION_STATE,
            action: StorageAction::None,
        },
        StorageReadObservation::Valid(state) => LoadTransition {
            state,
            action: StorageAction::None,
        },
        StorageReadObservation::Malformed | StorageReadObservation::ReadFailure => LoadTransition {
            state: DEFAULT_SHELL_PRESENTATION_STATE,
            action: StorageAction::WriteDefault {
                key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
                state: DEFAULT_SHELL_PRESENTATION_STATE,
            },
        },
    }
}

/// Applies the legacy malformed-repair policy after a load transition's write.
///
/// Removal is requested only when the preceding transition actually requested
/// a default repair write and that write failed. Successful repair, ordinary
/// missing/valid loads, and unrelated write observations produce no follow-up
/// action.
#[must_use]
pub const fn repair(
    load_transition: LoadTransition,
    observation: StorageWriteObservation,
) -> RepairTransition {
    let action = match (load_transition.action, observation) {
        (StorageAction::WriteDefault { key, .. }, StorageWriteObservation::Failed) => {
            StorageAction::Remove { key }
        }
        _ => StorageAction::None,
    };

    RepairTransition { action }
}

/// Applies the normal save policy to an observed write.
///
/// The storage write is always the only requested action. A failed ordinary
/// save is reported as [`SaveOutcome::FailureAbsorbed`], with no repair or
/// removal request, matching the legacy service's deliberately absorbed save
/// failures.
#[must_use]
pub const fn save(
    state: ShellPresentationState,
    observation: StorageWriteObservation,
) -> SaveTransition {
    let outcome = match observation {
        StorageWriteObservation::Succeeded => SaveOutcome::Succeeded,
        StorageWriteObservation::Failed => SaveOutcome::FailureAbsorbed,
    };

    SaveTransition {
        action: StorageAction::Save {
            key: SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY,
            state,
        },
        outcome,
    }
}
