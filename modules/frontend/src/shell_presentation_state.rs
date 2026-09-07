//! Pure persistence policy for the shell presentation preferences.
//!
//! The legacy implementation stores one versioned JSON record under a fixed
//! key. This module keeps that record's shape, deterministic encoding, strict
//! decoding, and storage-repair decisions without acquiring a store or doing
//! any other I/O. A storage adapter classifies its read, executes the returned
//! [`StorageAction`], and feeds the write result back to [`repair`].
//!
//! A malformed value and a read failure are intentionally indistinguishable to
//! the caller of `load`: both return the default and first try to write that
//! default back. Only a failed repair write requests removal of the key. A
//! normal save has no failure path in this policy; its storage error is
//! deliberately absorbed, matching the legacy `Effect.result` boundary.
//!
//! The native caller-side completion of that contract lives here as well:
//! [`ShellPresentationStore`] (with [`InMemoryShellPresentationStore`]) is the
//! synchronous host-owned store seam mirroring the composer-draft session seam,
//! and [`ShellPresentationSession`] drives startup reload and write-on-change
//! through the [`load`]/[`repair`]/[`save`] policy above. Payloads cross the
//! seam through the single canonical codec
//! ([`ShellPresentationState::to_json`] and
//! [`classify_serialized_shell_presentation`]), so there is exactly one
//! encoder and one decoder.

#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;

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
    /// The schema version carried by every state created by [`Self::new`].
    pub const VERSION: u8 = SHELL_PRESENTATION_SCHEMA_VERSION;

    /// Creates a version-1 state from the two durable presentation settings.
    #[must_use]
    pub const fn new(left_collapsed: bool, right_collapsed: bool) -> Self {
        Self {
            version: Self::VERSION,
            left_collapsed,
            right_collapsed,
        }
    }

    /// Creates a state from decoded fields before version validation.
    ///
    /// Normal callers should use [`Self::new`]. This constructor is useful at
    /// the adapter boundary and lets the load policy remain defensive when a
    /// caller supplies a manually constructed unsupported version.
    #[must_use]
    pub const fn with_version(version: u8, left_collapsed: bool, right_collapsed: bool) -> Self {
        Self {
            version,
            left_collapsed,
            right_collapsed,
        }
    }

    /// Returns whether this state belongs to the supported version-1 schema.
    #[must_use]
    pub const fn has_supported_version(self) -> bool {
        self.version == Self::VERSION
    }

    /// Encodes this state as the canonical compact JSON object.
    ///
    /// Field order and boolean spelling are fixed to keep writes byte-for-byte
    /// deterministic. The state type normally contains version `1`; this
    /// method preserves the public `version` field so an adapter can observe a
    /// manually constructed unsupported value before rejecting it.
    #[must_use]
    pub fn to_json(self) -> String {
        let mut encoded = String::from("{\"version\":");
        encoded.push_str(&self.version.to_string());
        encoded.push_str(",\"left_collapsed\":");
        encoded.push_str(boolean_literal(self.left_collapsed));
        encoded.push_str(",\"right_collapsed\":");
        encoded.push_str(boolean_literal(self.right_collapsed));
        encoded.push('}');
        encoded
    }

    /// Alias for [`Self::to_json`] at a serializer boundary.
    #[must_use]
    pub fn serialize(self) -> String {
        self.to_json()
    }

    /// Decodes one canonical or whitespace-formatted JSON state object.
    ///
    /// Decoding accepts the three required fields in any object order, but it
    /// does not coerce values: versions must be the integer `1`, and both
    /// collapsed fields must be JSON booleans.
    ///
    /// # Errors
    ///
    /// Returns [`ShellPresentationDecodeError`] when the input is not a
    /// version-1 shell presentation JSON object.
    pub fn from_json(input: &str) -> Result<Self, ShellPresentationDecodeError> {
        decode_shell_presentation_state(input)
    }

    /// Alias for [`Self::from_json`] at a deserializer boundary.
    ///
    /// # Errors
    ///
    /// Returns [`ShellPresentationDecodeError`] when the input is not a
    /// version-1 shell presentation JSON object.
    pub fn deserialize(input: &str) -> Result<Self, ShellPresentationDecodeError> {
        Self::from_json(input)
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

/// Why a serialized shell presentation value cannot be used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellPresentationDecodeError {
    /// The input is not a JSON object with valid JSON punctuation.
    InvalidJson,
    /// A required version field is absent.
    MissingVersion,
    /// A required left-collapsed field is absent.
    MissingLeftCollapsed,
    /// A required right-collapsed field is absent.
    MissingRightCollapsed,
    /// The version value is not a non-negative JSON integer.
    InvalidVersion,
    /// The version is an integer, but is not the supported version `1`.
    UnsupportedVersion(u64),
    /// The left-collapsed value is not a JSON boolean.
    InvalidLeftCollapsed,
    /// The right-collapsed value is not a JSON boolean.
    InvalidRightCollapsed,
    /// The object contains a field outside the exact persisted schema.
    UnknownField,
    /// A required schema field occurs more than once.
    DuplicateField,
}

/// Decodes a serialized shell presentation state without performing I/O.
///
/// # Errors
///
/// Returns [`ShellPresentationDecodeError`] when the input is not a
/// version-1 shell presentation JSON object.
pub fn decode_shell_presentation_state(
    input: &str,
) -> Result<ShellPresentationState, ShellPresentationDecodeError> {
    let mut parser = JsonObjectParser::new(input);
    parser.expect(b'{')?;

    let mut version = None;
    let mut left_collapsed = None;
    let mut right_collapsed = None;

    if !parser.consume(b'}') {
        loop {
            let field = parser.field()?;
            parser.expect(b':')?;
            match field {
                JsonField::Version => {
                    if version.is_some() {
                        return Err(ShellPresentationDecodeError::DuplicateField);
                    }
                    version = Some(parser.version()?);
                }
                JsonField::LeftCollapsed => {
                    if left_collapsed.is_some() {
                        return Err(ShellPresentationDecodeError::DuplicateField);
                    }
                    left_collapsed =
                        Some(parser.boolean(ShellPresentationDecodeError::InvalidLeftCollapsed)?);
                }
                JsonField::RightCollapsed => {
                    if right_collapsed.is_some() {
                        return Err(ShellPresentationDecodeError::DuplicateField);
                    }
                    right_collapsed =
                        Some(parser.boolean(ShellPresentationDecodeError::InvalidRightCollapsed)?);
                }
            }

            if parser.consume(b'}') {
                break;
            }
            parser.expect(b',')?;
        }
    }

    parser.finish()?;

    let version = version.ok_or(ShellPresentationDecodeError::MissingVersion)?;
    let left_collapsed =
        left_collapsed.ok_or(ShellPresentationDecodeError::MissingLeftCollapsed)?;
    let right_collapsed =
        right_collapsed.ok_or(ShellPresentationDecodeError::MissingRightCollapsed)?;

    if version != u64::from(SHELL_PRESENTATION_SCHEMA_VERSION) {
        return Err(ShellPresentationDecodeError::UnsupportedVersion(version));
    }

    Ok(ShellPresentationState::new(left_collapsed, right_collapsed))
}

/// Classifies a serialized storage value without performing storage I/O.
#[must_use]
pub fn classify_serialized_shell_presentation(value: &str) -> StorageReadObservation {
    match decode_shell_presentation_state(value) {
        Ok(state) => StorageReadObservation::Valid(state),
        Err(_) => StorageReadObservation::Malformed,
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

impl StorageReadObservation {
    /// Classifies one serialized value, mapping every decode failure to
    /// [`Self::Malformed`].
    #[must_use]
    pub fn from_serialized(value: &str) -> Self {
        classify_serialized_shell_presentation(value)
    }
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
        StorageReadObservation::Valid(state) if state.has_supported_version() => LoadTransition {
            state,
            action: StorageAction::None,
        },
        StorageReadObservation::Valid(_)
        | StorageReadObservation::Malformed
        | StorageReadObservation::ReadFailure => LoadTransition {
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

/// A synchronous settings-store seam owned by the embedding host.
///
/// This mirrors the composer-draft session's store seam: the session never
/// assumes a capacity, serialization format, or backing system. The key is
/// always [`SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY`]; it stays a
/// parameter so the session can execute the [`StorageAction`]s returned by
/// [`load`], [`repair`], and [`save`] verbatim.
pub trait ShellPresentationStore {
    /// Reads the raw payload stored under `key`, if one exists.
    fn read(&mut self, key: &str) -> Option<String>;

    /// Writes the raw payload under `key`, replacing any prior value.
    fn write(&mut self, key: &str, value: String);

    /// Removes any payload stored under `key`.
    fn remove(&mut self, key: &str);
}

/// A small deterministic in-memory implementation of [`ShellPresentationStore`].
///
/// This is the settings analogue of the draft-session in-memory seam: a
/// testable synchronous store that performs no I/O. Reads never mutate the
/// map, so startup reloads are idempotent.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InMemoryShellPresentationStore {
    entries: HashMap<String, String>,
}

impl InMemoryShellPresentationStore {
    /// Creates an empty store holding no settings payload.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Returns the number of stored payloads.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no payload is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns whether a payload exists under `key` without changing the store.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Returns the payload under `key` without changing the store.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&String> {
        self.entries.get(key)
    }
}

impl ShellPresentationStore for InMemoryShellPresentationStore {
    fn read(&mut self, key: &str) -> Option<String> {
        self.entries.get(key).cloned()
    }

    fn write(&mut self, key: &str, value: String) {
        self.entries.insert(key.to_owned(), value);
    }

    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

/// Synchronous driver that reloads shell presentation state on startup and
/// persists it on change through a [`ShellPresentationStore`].
///
/// Startup classifies the stored payload with
/// [`classify_serialized_shell_presentation`] and applies the [`load`]
/// policy: a missing key yields the default with no repair, a valid payload
/// is adopted verbatim, and a malformed payload falls back to the default
/// while the default is written back as repair. In-memory writes are
/// infallible, so the [`repair`] removal path never triggers here; hosts with
/// a fallible store keep using [`load`], [`repair`], and [`save`] directly.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ShellPresentationSession {
    state: ShellPresentationState,
}

impl ShellPresentationSession {
    /// Creates a session holding the default presentation state.
    ///
    /// Call [`Self::startup`] before exposing [`Self::state`]; a fresh
    /// session has not yet consulted the store.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: DEFAULT_SHELL_PRESENTATION_STATE,
        }
    }

    /// Returns the session's current presentation state.
    #[must_use]
    pub const fn state(self) -> ShellPresentationState {
        self.state
    }

    /// Reloads the session from the store, repairing a corrupt payload.
    ///
    /// Returns the adopted state: the stored value when it decodes, otherwise
    /// the default.
    pub fn startup<S: ShellPresentationStore>(&mut self, store: &mut S) -> ShellPresentationState {
        let observation = match store.read(SHELL_PRESENTATION_PREFERENCES_STORAGE_KEY) {
            None => StorageReadObservation::Missing,
            Some(payload) => classify_serialized_shell_presentation(&payload),
        };
        let transition = load(observation);
        self.state = transition.state;
        if let StorageAction::WriteDefault { key, state } = transition.action {
            store.write(key, state.to_json());
        }
        self.state
    }

    /// Persists one collapsed-pair change, writing only when the value differs.
    ///
    /// Returns whether the session state changed and a store write was issued.
    pub fn update<S: ShellPresentationStore>(
        &mut self,
        store: &mut S,
        left_collapsed: bool,
        right_collapsed: bool,
    ) -> bool {
        let next = ShellPresentationState::new(left_collapsed, right_collapsed);
        if next == self.state {
            return false;
        }
        self.state = next;
        let transition = save(next, StorageWriteObservation::Succeeded);
        if let StorageAction::Save { key, state } = transition.action {
            store.write(key, state.to_json());
        }
        true
    }
}

const fn boolean_literal(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[derive(Clone, Copy)]
enum JsonField {
    Version,
    LeftCollapsed,
    RightCollapsed,
}

struct JsonObjectParser<'input> {
    input: &'input [u8],
    position: usize,
}

impl<'input> JsonObjectParser<'input> {
    const fn new(input: &'input str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
        }
    }

    fn field(&mut self) -> Result<JsonField, ShellPresentationDecodeError> {
        let field = self.string()?;
        match field.as_str() {
            "version" => Ok(JsonField::Version),
            "left_collapsed" => Ok(JsonField::LeftCollapsed),
            "right_collapsed" => Ok(JsonField::RightCollapsed),
            _ => Err(ShellPresentationDecodeError::UnknownField),
        }
    }

    fn version(&mut self) -> Result<u64, ShellPresentationDecodeError> {
        self.skip_whitespace();
        let start = self.position;
        if !self.next_is_ascii_digit() {
            return Err(ShellPresentationDecodeError::InvalidVersion);
        }
        if self.input[self.position] == b'0'
            && self
                .input
                .get(self.position + 1)
                .is_some_and(u8::is_ascii_digit)
        {
            return Err(ShellPresentationDecodeError::InvalidJson);
        }
        while self.next_is_ascii_digit() {
            self.position += 1;
        }
        std::str::from_utf8(&self.input[start..self.position])
            .ok()
            .and_then(|value| value.parse().ok())
            .ok_or(ShellPresentationDecodeError::InvalidVersion)
    }

    fn boolean(
        &mut self,
        invalid: ShellPresentationDecodeError,
    ) -> Result<bool, ShellPresentationDecodeError> {
        self.skip_whitespace();
        if self.literal_is_value(b"true") {
            self.position += 4;
            return Ok(true);
        }
        if self.literal_is_value(b"false") {
            self.position += 5;
            return Ok(false);
        }
        Err(invalid)
    }

    fn string(&mut self) -> Result<String, ShellPresentationDecodeError> {
        self.skip_whitespace();
        if !self.consume_raw(b'"') {
            return Err(ShellPresentationDecodeError::InvalidJson);
        }

        let start = self.position;
        while let Some(&byte) = self.input.get(self.position) {
            match byte {
                b'"' => {
                    let value = std::str::from_utf8(&self.input[start..self.position])
                        .map_err(|_| ShellPresentationDecodeError::InvalidJson)?
                        .to_owned();
                    self.position += 1;
                    if value.as_bytes().contains(&b'\\')
                        || value.bytes().any(|character| character < 0x20)
                    {
                        return Err(ShellPresentationDecodeError::InvalidJson);
                    }
                    return Ok(value);
                }
                b'\\' | 0x00..=0x1f => {
                    return Err(ShellPresentationDecodeError::InvalidJson);
                }
                _ => self.position += 1,
            }
        }

        Err(ShellPresentationDecodeError::InvalidJson)
    }

    fn expect(&mut self, expected: u8) -> Result<(), ShellPresentationDecodeError> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(ShellPresentationDecodeError::InvalidJson)
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        self.skip_whitespace();
        self.consume_raw(expected)
    }

    fn consume_raw(&mut self, expected: u8) -> bool {
        if self.input.get(self.position) == Some(&expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn finish(&mut self) -> Result<(), ShellPresentationDecodeError> {
        self.skip_whitespace();
        if self.position == self.input.len() {
            Ok(())
        } else {
            Err(ShellPresentationDecodeError::InvalidJson)
        }
    }

    fn skip_whitespace(&mut self) {
        while matches!(
            self.input.get(self.position),
            Some(b' ' | b'\t' | b'\n' | b'\r')
        ) {
            self.position += 1;
        }
    }

    fn next_is_ascii_digit(&self) -> bool {
        self.input
            .get(self.position)
            .is_some_and(u8::is_ascii_digit)
    }

    fn literal_is_value(&self, literal: &[u8]) -> bool {
        self.input.get(self.position..self.position + literal.len()) == Some(literal)
            && matches!(
                self.input.get(self.position + literal.len()),
                None | Some(b' ' | b'\t' | b'\n' | b'\r' | b',' | b'}')
            )
    }
}
