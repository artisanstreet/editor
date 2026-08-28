//! Pure turn-navigation policy for a conversation transcript.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/conversation/turn-navigator.ts`. The input
//! structs intentionally contain only the fields needed by that policy; they
//! are not protocol or transcript-store types. Building markers has no UI,
//! hydration, or navigation side effects.

#![allow(clippy::module_name_repetitions)]

use std::collections::HashSet;

/// Maximum number of Unicode scalar values retained in a navigator label.
pub const NAVIGATOR_LABEL_MAX_SCALARS: usize = 120;

/// The default distance below the viewport top at which a turn is reached.
pub const DEFAULT_ACTIVE_TURN_THRESHOLD: f64 = 96.0;

/// The identity and stable position of one loaded conversation turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationTurnInput {
    /// Opaque turn identity.
    pub id: String,
    /// Stable position in the conversation.
    pub ordinal: u64,
}

impl ConversationTurnInput {
    /// Creates a turn input from its identity and ordinal.
    #[must_use]
    pub fn new(id: impl Into<String>, ordinal: u64) -> Self {
        Self {
            id: id.into(),
            ordinal,
        }
    }
}

/// The item kinds relevant to turn navigation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationItemKind {
    /// A user-authored message and therefore a reachable navigator anchor.
    UserMessage,
    /// Assistant output, which is intentionally not an anchor.
    AssistantMessage,
    /// Any other loaded item kind, which is also not an anchor.
    Other,
}

/// The minimal loaded-item snapshot needed to build turn markers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedConversationItemInput {
    /// Opaque item identity.
    pub id: String,
    /// Identity of the owning turn.
    pub turn_id: String,
    /// Stable position in the conversation item sequence.
    pub ordinal: u64,
    /// Loaded item discriminator.
    pub kind: ConversationItemKind,
    /// Message text used for the loaded marker label.
    pub text: String,
}

impl LoadedConversationItemInput {
    /// Creates a loaded item input with an explicit item kind.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        turn_id: impl Into<String>,
        ordinal: u64,
        kind: ConversationItemKind,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            turn_id: turn_id.into(),
            ordinal,
            kind,
            text: text.into(),
        }
    }

    /// Creates a user-message item input.
    #[must_use]
    pub fn user_message(
        id: impl Into<String>,
        turn_id: impl Into<String>,
        ordinal: u64,
        text: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            turn_id,
            ordinal,
            ConversationItemKind::UserMessage,
            text,
        )
    }

    /// Creates an assistant-message item input.
    #[must_use]
    pub fn assistant_message(
        id: impl Into<String>,
        turn_id: impl Into<String>,
        ordinal: u64,
        text: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            turn_id,
            ordinal,
            ConversationItemKind::AssistantMessage,
            text,
        )
    }

    /// Creates a non-message item input.
    #[must_use]
    pub fn other(
        id: impl Into<String>,
        turn_id: impl Into<String>,
        ordinal: u64,
        text: impl Into<String>,
    ) -> Self {
        Self::new(id, turn_id, ordinal, ConversationItemKind::Other, text)
    }
}

/// A user-message marker supplied for an unloaded portion of the transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationWindowMarkerInput {
    /// Opaque user-message item identity.
    pub id: String,
    /// Marker label supplied by the window projection.
    pub label: String,
    /// Stable position in the conversation item sequence.
    pub ordinal: u64,
    /// Stable position of the owning turn.
    pub turn_ordinal: u64,
}

impl ConversationWindowMarkerInput {
    /// Creates an unloaded-window marker input.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        ordinal: u64,
        turn_ordinal: u64,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            ordinal,
            turn_ordinal,
        }
    }
}

/// The minimal conversation snapshot consumed by the marker policy.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConversationSnapshotInput {
    /// Loaded turns used both for ownership lookup and the loaded floor.
    pub turns: Vec<ConversationTurnInput>,
    /// Loaded conversation items.
    pub items: Vec<LoadedConversationItemInput>,
    /// Optional markers for user messages outside the loaded window.
    pub window_markers: Option<Vec<ConversationWindowMarkerInput>>,
}

impl ConversationSnapshotInput {
    /// Creates a snapshot input with an optional older-history marker window.
    #[must_use]
    pub fn new(
        turns: Vec<ConversationTurnInput>,
        items: Vec<LoadedConversationItemInput>,
        window_markers: Option<Vec<ConversationWindowMarkerInput>>,
    ) -> Self {
        Self {
            turns,
            items,
            window_markers,
        }
    }

    /// Returns the lowest loaded turn ordinal, if any turn is loaded.
    #[must_use]
    pub fn loaded_turn_floor(&self) -> Option<u64> {
        self.turns.iter().map(|turn| turn.ordinal).min()
    }
}

/// The marker exposed to a transcript navigator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationTurnMarker {
    /// The conversation item identity.
    pub id: String,
    /// One-line, scalar-bounded preview text.
    pub label: String,
    /// Stable item position used for oldest-first ordering.
    pub ordinal: u64,
    /// The owning turn position, when the loaded item resolved its turn.
    pub turn_ordinal: Option<u64>,
}

/// A loaded or remote marker's offset from the top of the viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConversationTurnOffset<'a> {
    /// Marker identity returned when this offset is active.
    pub id: &'a str,
    /// Offset from the viewport top in pixels.
    pub top: f64,
}

impl<'a> ConversationTurnOffset<'a> {
    /// Creates a borrowed turn offset.
    #[must_use]
    pub const fn new(id: &'a str, top: f64) -> Self {
        Self { id, top }
    }
}

/// Builds the reachable user-message markers, oldest first.
///
/// Loaded user messages are the authoritative source for ids present in the
/// loaded window. Remote markers may supplement them only below the lowest
/// loaded turn when a floor exists. Labels are normalized before the empty
/// check and final scalar-value truncation, matching the TypeScript policy.
/// Fewer than two reachable markers returns an empty vector because a
/// navigator with no possible movement is not useful.
#[must_use]
pub fn conversation_turn_markers(
    snapshot: &ConversationSnapshotInput,
) -> Vec<ConversationTurnMarker> {
    let loaded_ids: HashSet<&str> = snapshot
        .items
        .iter()
        .filter(|item| item.kind == ConversationItemKind::UserMessage)
        .map(|item| item.id.as_str())
        .collect();

    let mut markers = snapshot
        .items
        .iter()
        .filter(|item| item.kind == ConversationItemKind::UserMessage)
        .filter_map(|item| {
            let label = navigator_label(&item.text);
            (!label.is_empty()).then(|| ConversationTurnMarker {
                id: item.id.clone(),
                label,
                ordinal: item.ordinal,
                turn_ordinal: owning_turn_ordinal(&snapshot.turns, &item.turn_id),
            })
        })
        .collect::<Vec<_>>();

    let floor = snapshot.loaded_turn_floor();
    if let Some(window_markers) = snapshot.window_markers.as_deref() {
        markers.extend(window_markers.iter().filter_map(|marker| {
            if loaded_ids.contains(marker.id.as_str())
                || floor.is_some_and(|floor| marker.turn_ordinal >= floor)
            {
                return None;
            }

            let label = navigator_label(&marker.label);
            (!label.is_empty()).then(|| ConversationTurnMarker {
                id: marker.id.clone(),
                label,
                ordinal: marker.ordinal,
                turn_ordinal: Some(marker.turn_ordinal),
            })
        }));
    }

    markers.sort_by(|left, right| {
        left.ordinal
            .cmp(&right.ordinal)
            .then_with(|| left.id.cmp(&right.id))
    });

    if markers.len() > 1 {
        markers
    } else {
        Vec::new()
    }
}

/// Returns the lowest loaded turn ordinal, if the snapshot has loaded turns.
#[must_use]
pub fn conversation_turn_floor(snapshot: &ConversationSnapshotInput) -> Option<u64> {
    snapshot.loaded_turn_floor()
}

/// Selects the active turn using the default reached threshold.
///
/// Offsets must already be in transcript order. The last offset whose top is
/// at or above the threshold wins; if none has passed, the first offset is the
/// best description of the reading position. Empty input has no active turn.
#[must_use]
pub fn active_conversation_turn<'a>(offsets: &[ConversationTurnOffset<'a>]) -> Option<&'a str> {
    active_conversation_turn_with_threshold(offsets, DEFAULT_ACTIVE_TURN_THRESHOLD)
}

/// Selects the active turn using an explicit reached threshold.
#[must_use]
pub fn active_conversation_turn_with_threshold<'a>(
    offsets: &[ConversationTurnOffset<'a>],
    threshold: f64,
) -> Option<&'a str> {
    if offsets.is_empty() {
        return None;
    }

    offsets
        .iter()
        .rfind(|offset| offset.top <= threshold)
        .map(|offset| offset.id)
        .or_else(|| offsets.first().map(|offset| offset.id))
}

/// Normalizes one label to a trimmed single line and a scalar-value limit.
fn navigator_label(text: &str) -> String {
    let mut label = String::new();
    let mut pending_space = false;
    let mut scalar_count = 0;

    for character in text.chars() {
        if is_ecmascript_whitespace(character) {
            if !label.is_empty() {
                pending_space = true;
            }
            continue;
        }

        if pending_space {
            label.push(' ');
            scalar_count += 1;
            pending_space = false;
            if scalar_count == NAVIGATOR_LABEL_MAX_SCALARS {
                return label;
            }
        }

        label.push(character);
        scalar_count += 1;
        if scalar_count == NAVIGATOR_LABEL_MAX_SCALARS {
            return label;
        }
    }

    label
}

/// Finds the owning turn's ordinal with the same last-entry-wins behavior as
/// a JavaScript `Map` built from the turn list.
fn owning_turn_ordinal(turns: &[ConversationTurnInput], turn_id: &str) -> Option<u64> {
    turns
        .iter()
        .rev()
        .find(|turn| turn.id == turn_id)
        .map(|turn| turn.ordinal)
}

/// Matches the whitespace consumed by the source `/\s+/u` expression,
/// including the ECMAScript BOM whitespace code point.
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
