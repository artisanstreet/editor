//! Pure transcript scroll-position policy.
//!
//! This is the dependency-free value and arithmetic slice of
//! `modules/frontend/src/lib/conversation/scroll-position.ts`. It deliberately
//! does not know about GPUI, protocol entities, DOM measurements, transport, or
//! runtime state. Callers adapt their projected conversation items to the
//! small [`ConversationScrollItem`] shape here and apply the returned values to
//! their renderer.
//!
//! Protocol geometry is expected to be finite and non-negative. To keep this
//! leaf total over synthetic or damaged measurements, every geometry input
//! treats a negative value, `NaN`, or either infinity as zero. Arithmetic that
//! would overflow back to infinity uses the operation's safe lower-bound
//! fallback (`0` for a position and the sanitized base for end space). Normal
//! finite inputs therefore retain the TypeScript arithmetic exactly while
//! malformed measurements never leak `NaN` or infinity into a caller.

use std::collections::BTreeSet;

/// Minimum transcript end spacer in pixels.
pub const CONVERSATION_BASE_END_SPACE_PIXELS: f64 = 192.0;

/// Default top inset for an aligned conversation turn in pixels.
pub const CONVERSATION_TURN_TOP_INSET_PIXELS: f64 = 16.0;

/// The only item-kind distinction needed by the scroll policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConversationScrollItemKind {
    /// A durable user-authored message.
    UserMessage,
    /// Any projected item that is not a user-authored message.
    Other,
}

/// A source identity attached to a projected conversation item.
///
/// `reference` is the canonical source reference. `event_id`, when present,
/// is an alternate accepted lookup key; provider, journal, and transport
/// details are intentionally outside this pure policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationSourceRef {
    /// Canonical source reference.
    pub reference: String,
    /// Optional provider event identity accepted as an alias for `reference`.
    pub event_id: Option<String>,
}

impl ConversationSourceRef {
    /// Creates a source reference without an event-id alias.
    #[must_use]
    pub fn new(reference: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            event_id: None,
        }
    }

    /// Creates a source reference with an event-id alias.
    #[must_use]
    pub fn with_event_id(reference: impl Into<String>, event_id: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            event_id: Some(event_id.into()),
        }
    }
}

/// Minimal projected item consumed by the transcript scroll policy.
///
/// `ordinal` remains an `f64` because the source policy receives JavaScript
/// numbers. Valid protocol ordinals are finite and non-negative; selection
/// nevertheless uses [`f64::total_cmp`] so even malformed values have a stable
/// ordering rather than making a comparator panic or return inconsistent
/// results.
#[derive(Clone, Debug, PartialEq)]
pub struct ConversationScrollItem {
    /// Stable projected item identity.
    pub id: String,
    /// Conversation ordering value.
    pub ordinal: f64,
    /// Accepted durable/provider source identities.
    pub source_refs: Vec<ConversationSourceRef>,
    /// Whether this item is a user message.
    pub item_type: ConversationScrollItemKind,
}

impl ConversationScrollItem {
    /// Creates an item with its complete minimal policy shape.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        ordinal: f64,
        item_type: ConversationScrollItemKind,
        source_refs: Vec<ConversationSourceRef>,
    ) -> Self {
        Self {
            id: id.into(),
            ordinal,
            source_refs,
            item_type,
        }
    }

    /// Creates a projected user-message item.
    #[must_use]
    pub fn user_message(
        id: impl Into<String>,
        ordinal: f64,
        source_refs: Vec<ConversationSourceRef>,
    ) -> Self {
        Self::new(
            id,
            ordinal,
            ConversationScrollItemKind::UserMessage,
            source_refs,
        )
    }

    /// Creates a projected non-user item with no source references.
    #[must_use]
    pub fn other(id: impl Into<String>, ordinal: f64) -> Self {
        Self::new(id, ordinal, ConversationScrollItemKind::Other, Vec::new())
    }

    /// Whether this item participates in user-message scroll selection.
    #[must_use]
    pub const fn is_user_message(&self) -> bool {
        matches!(self.item_type, ConversationScrollItemKind::UserMessage)
    }
}

/// Captures the durable user-message IDs visible before a submission begins.
#[must_use]
pub fn conversation_user_message_ids(items: &[ConversationScrollItem]) -> BTreeSet<String> {
    items
        .iter()
        .filter(|item| item.is_user_message())
        .map(|item| item.id.clone())
        .collect()
}

/// Selects the newest projected user message that was absent at submission time.
///
/// Candidates are ordered by descending ordinal and then descending ID. Rust's
/// bytewise string ordering is intentional: unlike locale-sensitive
/// `localeCompare`, it is a stable total ordering for every UTF-8 ID. The
/// protocol supplies ordinary finite ordinals; [`f64::total_cmp`] also makes
/// the behavior deterministic for malformed `f64` values.
#[must_use]
pub fn newest_conversation_user_message(
    items: &[ConversationScrollItem],
    previous_ids: &BTreeSet<String>,
) -> Option<String> {
    items
        .iter()
        .filter(|item| item.is_user_message() && !previous_ids.contains(&item.id))
        .max_by(|left, right| {
            left.ordinal
                .total_cmp(&right.ordinal)
                .then_with(|| left.id.cmp(&right.id))
        })
        .map(|item| item.id.clone())
}

/// Resolves an accepted source reference to the exact canonical user-message ID.
///
/// Both the source's canonical `reference` and its optional `event_id` are
/// accepted. The first matching item in projected order wins, matching the
/// source policy's `find` semantics; non-user items are never eligible.
#[must_use]
pub fn conversation_user_message_with_source_reference(
    items: &[ConversationScrollItem],
    source_reference: &str,
) -> Option<String> {
    items
        .iter()
        .find(|item| {
            item.is_user_message()
                && item.source_refs.iter().any(|source| {
                    source.reference == source_reference
                        || source.event_id.as_deref() == Some(source_reference)
                })
        })
        .map(|item| item.id.clone())
}

/// Computes an immediate bottom position without smooth-scroll behavior.
///
/// Negative or non-finite dimensions are sanitized to zero before applying
/// `max(0, scroll_height - viewport_height)`.
#[must_use]
pub fn conversation_bottom_scroll_top(scroll_height: f64, viewport_height: f64) -> f64 {
    let scroll_height = sanitize_geometry(scroll_height);
    let viewport_height = sanitize_geometry(viewport_height);
    (scroll_height - viewport_height).max(0.0)
}

/// Returns the tolerance in which the transcript still counts as following.
///
/// This is `max(64, viewport_height * 0.06)`. Invalid viewport geometry is
/// treated as zero, so the minimum tolerance remains the deterministic 64px
/// floor.
#[must_use]
pub fn conversation_follow_tolerance(viewport_height: f64) -> f64 {
    (sanitize_geometry(viewport_height) * 0.06).max(64.0)
}

/// Whether the reader is close enough to the bottom to follow new content.
///
/// The comparison is intentionally strict: a distance exactly equal to the
/// tolerance is not following. Scroll position and dimensions use the same
/// finite non-negative sanitization as the other geometry helpers.
#[must_use]
pub fn conversation_is_following(
    scroll_top: f64,
    scroll_height: f64,
    viewport_height: f64,
) -> bool {
    let distance = sanitize_geometry(scroll_height)
        - sanitize_geometry(viewport_height)
        - sanitize_geometry(scroll_top);
    // With finite non-negative operands this can only overflow toward negative
    // infinity. Treating that invalid distance as zero is finite and keeps the
    // conservative "following" result of the original negative comparison.
    let distance = if distance.is_finite() { distance } else { 0.0 };
    distance < conversation_follow_tolerance(viewport_height)
}

/// Aligns an item to the viewport's top inset using the default 16px inset.
#[must_use]
pub fn conversation_aligned_scroll_top(
    current_scroll_top: f64,
    viewport_top: f64,
    item_top: f64,
) -> f64 {
    conversation_aligned_scroll_top_with_inset(
        current_scroll_top,
        viewport_top,
        item_top,
        CONVERSATION_TURN_TOP_INSET_PIXELS,
    )
}

/// Aligns an item to a caller-selected top inset in scroll-content coordinates.
///
/// The result is clamped at zero. Invalid geometry and an overflowing
/// intermediate position fall back to zero.
#[must_use]
pub fn conversation_aligned_scroll_top_with_inset(
    current_scroll_top: f64,
    viewport_top: f64,
    item_top: f64,
    inset: f64,
) -> f64 {
    let raw = sanitize_geometry(current_scroll_top) + sanitize_geometry(item_top)
        - sanitize_geometry(viewport_top)
        - sanitize_geometry(inset);
    if raw.is_finite() { raw.max(0.0) } else { 0.0 }
}

/// Computes the end spacer using the 192px base and 16px inset defaults.
#[must_use]
pub fn conversation_end_space_height(
    viewport_height: f64,
    item_top: f64,
    end_space_top: f64,
) -> f64 {
    conversation_end_space_height_with_options(
        viewport_height,
        item_top,
        end_space_top,
        CONVERSATION_BASE_END_SPACE_PIXELS,
        CONVERSATION_TURN_TOP_INSET_PIXELS,
    )
}

/// Computes the end spacer with explicit base-height and inset values.
///
/// This is `max(base_height, item_top + viewport_height - inset -
/// end_space_top)`. Invalid geometry is sanitized before the calculation; an
/// overflowing expression falls back to the sanitized base height.
#[must_use]
pub fn conversation_end_space_height_with_options(
    viewport_height: f64,
    item_top: f64,
    end_space_top: f64,
    base_height: f64,
    inset: f64,
) -> f64 {
    let base_height = sanitize_geometry(base_height);
    let raw = sanitize_geometry(item_top) + sanitize_geometry(viewport_height)
        - sanitize_geometry(inset)
        - sanitize_geometry(end_space_top);
    if raw.is_finite() {
        base_height.max(raw).max(0.0)
    } else {
        base_height
    }
}

/// Maps malformed geometry to the only safe pixel coordinate: finite zero.
#[inline]
fn sanitize_geometry(value: f64) -> f64 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}
