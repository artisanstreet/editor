//! Pure renderer state and title intent for requesting Forge repair.
//!
//! This is the native boundary for
//! `modules/frontend/src/lib/root/forge-repair-request.svelte.ts`. The
//! renderer-owned request is a monotonic flag: it starts unset and a request
//! makes it set, without an acknowledgement, retry, or clearing transition.
//! The title writer receives the request as an intent alongside its existing
//! attention state. Applying that intent is pure string construction; a host
//! owns any eventual title write and its failure cannot roll back this flag.
//!
//! The title intent delegates composition to the shared attention-title
//! authority because the repair marker shares one title string with the
//! development and attention markers. This keeps their ordering, parsing,
//! and JavaScript-number rules convergent without accessing a browser title,
//! DOM, shell, or protocol service.

#![allow(clippy::module_name_repetitions)]

pub use crate::attention_title_policy::{
    DEV_TITLE_MARKER, FORGE_REPAIR_TITLE_MARKER, attention_marked_title,
};

/// Renderer state for the outstanding Forge-repair request.
///
/// The state is deliberately monotonic. [`Self::request`] is idempotent, and
/// this boundary exposes no acknowledgement or clear operation. A caller can
/// copy or inspect the state without coupling it to a host side effect.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ForgeRepairRequestState {
    requested: bool,
}

impl ForgeRepairRequestState {
    /// Creates the initial state with no repair request.
    #[must_use]
    pub const fn new() -> Self {
        Self { requested: false }
    }

    /// Returns whether a repair request has been made.
    #[must_use]
    pub const fn requested(self) -> bool {
        self.requested
    }

    /// Returns whether a repair request has been made.
    #[must_use]
    pub const fn is_requested(self) -> bool {
        self.requested()
    }

    /// Records a repair request.
    ///
    /// Repeated calls leave the state set. This mutation is complete before a
    /// caller performs any host title write, so a host-side write failure does
    /// not and cannot roll the request back.
    pub const fn request(&mut self) {
        self.requested = true;
    }

    /// Builds the one title-writer intent for the current request state.
    ///
    /// `attention_count` and `awaiting_answer` belong to the single attention
    /// title writer. Passing them through here preserves both values while
    /// the repair suffix is added or removed; `None` means that the writer
    /// should omit the attention marker, just as the browser helper's
    /// `undefined` count branch does.
    #[must_use]
    pub const fn title_rewrite_intent(
        self,
        attention_count: Option<f64>,
        awaiting_answer: bool,
    ) -> ForgeRepairTitleIntent {
        ForgeRepairTitleIntent::new(attention_count, awaiting_answer, self.requested)
    }
}

/// The pure title rewrite requested by [`ForgeRepairRequestState`].
///
/// This carries the existing attention state rather than replacing it with a
/// repair-only title. The caller can pass the intent to [`Self::apply_to`]
/// whenever the title writer observes a route title or a request change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForgeRepairTitleIntent {
    attention_count: Option<f64>,
    awaiting_answer: bool,
    requests_forge_repair: bool,
}

impl ForgeRepairTitleIntent {
    /// Creates a title intent from the attention writer's state and repair
    /// request flag.
    #[must_use]
    pub const fn new(
        attention_count: Option<f64>,
        awaiting_answer: bool,
        requests_forge_repair: bool,
    ) -> Self {
        Self {
            attention_count,
            awaiting_answer,
            requests_forge_repair,
        }
    }

    /// Returns the attention count that will be written, if any.
    #[must_use]
    pub const fn attention_count(self) -> Option<f64> {
        self.attention_count
    }

    /// Returns whether the title will retain the awaiting-answer marker.
    #[must_use]
    pub const fn awaiting_answer(self) -> bool {
        self.awaiting_answer
    }

    /// Returns whether the title will carry one Forge-repair suffix.
    #[must_use]
    pub const fn requests_forge_repair(self) -> bool {
        self.requests_forge_repair
    }

    /// Applies the intent to a route-owned title without performing I/O.
    #[must_use]
    pub fn apply_to(self, title: &str) -> String {
        attention_marked_title(
            title,
            self.attention_count,
            self.requests_forge_repair,
            self.awaiting_answer,
        )
    }
}
