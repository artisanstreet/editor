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
//! The title rules intentionally stay with this small intent because the
//! repair marker shares one title string with the development and attention
//! markers. This keeps their ordering and ownership rules convergent without
//! accessing a browser title, DOM, shell, or protocol service.

#![allow(clippy::module_name_repetitions)]

/// The exact visible development marker used at the front of a title.
pub const DEV_TITLE_MARKER: &str = "[Dev]";

/// The invisible doubled U+2060 WORD JOINER used to request Forge repair.
pub const FORGE_REPAIR_TITLE_MARKER: &str = "\u{2060}\u{2060}";

const DEV_TITLE_PREFIX: &str = "[Dev] ";
const ATTENTION_WORD_JOINER: &str = "\u{2060}";

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
        attention_count: Option<usize>,
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
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ForgeRepairTitleIntent {
    attention_count: Option<usize>,
    awaiting_answer: bool,
    requests_forge_repair: bool,
}

impl ForgeRepairTitleIntent {
    /// Creates a title intent from the attention writer's state and repair
    /// request flag.
    #[must_use]
    pub const fn new(
        attention_count: Option<usize>,
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
    pub const fn attention_count(self) -> Option<usize> {
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

/// Formats the protocol attention marker for an already-derived count.
///
/// A zero count with an open question uses the standalone `(?)` form. Every
/// other count uses decimal digits, optionally followed by `?`, and exactly
/// one U+2060 WORD JOINER. The count is a Rust collection count, so truncation
/// and numeric coercion have already happened at the caller's boundary.
#[must_use]
pub fn attention_title_marker_for(count: usize, awaiting_answer: bool) -> String {
    if count == 0 && awaiting_answer {
        return format!("(?){ATTENTION_WORD_JOINER}");
    }

    let question = if awaiting_answer { "?" } else { "" };
    format!("({count}{question}){ATTENTION_WORD_JOINER}")
}

/// Rewrites a title using the shared development, attention, and repair rules.
///
/// The exact `[Dev] ` prefix remains first. Only a leading attention marker
/// with one to four ASCII digits (optionally followed by `?`), or the
/// standalone `(?)` form, is considered owned and removed. Plain
/// parenthesized route titles and malformed marker-like text stay untouched.
/// Every existing doubled U+2060 repair marker is removed before at most one
/// requested marker is appended.
///
/// `None` omits the attention marker and ignores `awaiting_answer`, matching
/// the source helper's `undefined` count branch. The function does not access
/// or write a document title.
#[must_use]
pub fn attention_marked_title(
    title: &str,
    attention_count: Option<usize>,
    requests_forge_repair: bool,
    awaiting_answer: bool,
) -> String {
    let (development_prefix, bare_title) = match title.strip_prefix(DEV_TITLE_PREFIX) {
        Some(bare_title) => (DEV_TITLE_PREFIX, bare_title),
        None => ("", title),
    };

    let bare_title =
        strip_attention_marker_prefix(bare_title).replace(FORGE_REPAIR_TITLE_MARKER, "");
    let repair_suffix = if requests_forge_repair {
        FORGE_REPAIR_TITLE_MARKER
    } else {
        ""
    };

    match attention_count {
        Some(count) => format!(
            "{development_prefix}{} {bare_title}{repair_suffix}",
            attention_title_marker_for(count, awaiting_answer)
        ),
        None => format!("{development_prefix}{bare_title}{repair_suffix}"),
    }
}

/// Removes one owned attention marker from the start of a title.
fn strip_attention_marker_prefix(title: &str) -> &str {
    let Some(body) = title.strip_prefix('(') else {
        return title;
    };

    let bytes = body.as_bytes();
    let mut index = 0;

    if bytes.first() == Some(&b'?') {
        index = 1;
    } else {
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }

        if index == 0 || index > 4 {
            return title;
        }

        if bytes.get(index) == Some(&b'?') {
            index += 1;
        }
    }

    let Some(rest) = body.get(index..) else {
        return title;
    };
    rest.strip_prefix(")\u{2060} ").unwrap_or(title)
}
