//! Private semantic/budget helpers for the conversation projection owner.
//!
//! Nothing here is publicly reachable: these are pure scans and bounded
//! counters that keep [`super::ConversationProjection`] readable. They carry
//! no policy beyond what the owner delegates: retention ceilings, borrowed
//! entity scans, and signed-instant ordering checks.

use artisan_domain::{ConversationItem, ConversationTurn, ItemId, TurnId, UnixMillis};

/// VP-approved client retention ceiling for retained items.
pub(super) const MAX_RETAINED_ITEMS: usize = 2_048;

/// VP-approved client retention ceiling for summed retained body bytes.
pub(super) const MAX_RETAINED_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// Running retention accounting for one staged candidate.
///
/// Counts are maintained incrementally so every prospective mutation can be
/// rejected **before** its replacement text is allocated. Arithmetic is
/// checked; saturation or wraparound can never produce a false pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Retention {
    items: usize,
    body_bytes: u64,
}

/// Why a retention counter refused a prospective mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RetentionFailure {
    /// The item count crossed [`MAX_RETAINED_ITEMS`].
    Items,
    /// The summed body bytes crossed [`MAX_RETAINED_BODY_BYTES`], or the
    /// length addition itself overflowed.
    Bytes,
}

impl Retention {
    /// Seeds the tracker from an existing retained item set.
    ///
    /// # Errors
    ///
    /// Returns the crossed ceiling when the borrowed rows already exceed the
    /// approved client budgets.
    pub(super) fn seed(items: &[ConversationItem]) -> Result<Self, RetentionFailure> {
        let mut retention = Self {
            items: items.len(),
            body_bytes: 0,
        };
        if retention.items > MAX_RETAINED_ITEMS {
            return Err(RetentionFailure::Items);
        }
        for item in items {
            let length = u64::try_from(item_body_len(item)).map_err(|_| RetentionFailure::Bytes)?;
            retention.body_bytes = retention
                .body_bytes
                .checked_add(length)
                .ok_or(RetentionFailure::Bytes)?;
        }
        if retention.body_bytes > MAX_RETAINED_BODY_BYTES {
            return Err(RetentionFailure::Bytes);
        }
        Ok(retention)
    }

    /// Accounts for replacing one item body with a different length.
    ///
    /// # Errors
    ///
    /// Returns [`RetentionFailure::Bytes`] when the prospective total would
    /// cross the byte ceiling or the arithmetic could not stay checked.
    pub(super) fn replace_body(
        &mut self,
        previous_len: usize,
        next_len: usize,
    ) -> Result<(), RetentionFailure> {
        let previous = u64::try_from(previous_len).map_err(|_| RetentionFailure::Bytes)?;
        let next = u64::try_from(next_len).map_err(|_| RetentionFailure::Bytes)?;
        let total = self
            .body_bytes
            .checked_sub(previous)
            .ok_or(RetentionFailure::Bytes)?
            .checked_add(next)
            .ok_or(RetentionFailure::Bytes)?;
        if total > MAX_RETAINED_BODY_BYTES {
            return Err(RetentionFailure::Bytes);
        }
        self.body_bytes = total;
        Ok(())
    }

    /// Accounts for one newly inserted item.
    ///
    /// # Errors
    ///
    /// Returns the crossed ceiling when the insertion would exceed either
    /// approved client budget.
    pub(super) fn insert_item(&mut self, body_len: usize) -> Result<(), RetentionFailure> {
        let next_items = self.items.checked_add(1).ok_or(RetentionFailure::Items)?;
        if next_items > MAX_RETAINED_ITEMS {
            return Err(RetentionFailure::Items);
        }
        self.replace_body(0, body_len)?;
        self.items = next_items;
        Ok(())
    }
}

/// Total UTF-8 byte length of one item's complete body.
pub(super) fn item_body_len(item: &ConversationItem) -> usize {
    match item {
        ConversationItem::UserMessage(message) => message.body.as_str().len(),
        ConversationItem::AssistantMessage(message) => message.body.as_str().len(),
    }
}

/// Position of one turn by identity, if present.
pub(super) fn turn_index(turns: &[ConversationTurn], turn_id: &TurnId) -> Option<usize> {
    turns.iter().position(|turn| &turn.turn_id == turn_id)
}

/// Position of one item by identity, if present.
pub(super) fn item_index(items: &[ConversationItem], item_id: &ItemId) -> Option<usize> {
    items.iter().position(|item| item.item_id() == item_id)
}

/// Whether any staged entity already occupies this shared ordinal.
pub(super) fn ordinal_taken(
    turns: &[ConversationTurn],
    items: &[ConversationItem],
    ordinal: u64,
) -> bool {
    turns.iter().any(|turn| turn.ordinal.get() == ordinal)
        || items.iter().any(|item| item.ordinal().get() == ordinal)
}

/// Whether one entity kind already uses this raw identifier text.
///
/// The durable ledger mints turn/item identities from one globally unique
/// `entity_id` column, so identical raw text across a `TurnId` and an
/// `ItemId` can never name two real entities.
pub(super) fn raw_identity_taken_by_other_kind(
    turns: &[ConversationTurn],
    items: &[ConversationItem],
    identity: RawKind<'_>,
) -> bool {
    match identity {
        RawKind::Turn(text) => items.iter().any(|item| item.item_id().as_str() == text),
        RawKind::Item(text) => turns.iter().any(|turn| turn.turn_id.as_str() == text),
    }
}

/// Which entity kind a raw identifier text belongs to.
#[derive(Clone, Copy, Debug)]
pub(super) enum RawKind<'a> {
    /// A turn identity's raw text.
    Turn(&'a str),
    /// An item identity's raw text.
    Item(&'a str),
}

/// Signed-instant ordering check: creation never follows the last update.
///
/// Equal instants are legal; every signed value remains legal; no unrelated
/// cross-entity or wall-clock comparison belongs here.
pub(super) fn created_not_after_updated(created_at: UnixMillis, updated_at: UnixMillis) -> bool {
    created_at.as_millis() <= updated_at.as_millis()
}
