//! Shared text-input state/validation seam for GPUI surfaces holding typed
//! plaintext across frames.
//!
//! Pinned GPUI 0.2.2 ships no editable text widget
//! (`docs/ui/GPUI_CAPABILITIES.md` §2.11), so the native composer and the
//! search/command editors will own their buffers on top of this state. The
//! audited call sites already agree on the shared shape kept here:
//!
//! - **Whole-value replacement.** Every update swaps the entire value,
//!   exactly like `UpdateDraft` (`thread-composer.svelte:180`), draft restore
//!   (`composer/dom.ts:149`), and the Command input's two-way binding.
//! - **Intake canonicalization.** [`normalize_input`] removes zero-width
//!   spaces (attachment-marker chrome excluded from content,
//!   `composer/dom.ts:23`) and folds CR/CRLF to LF (document breaks are `\n`,
//!   `composer/dom.ts:33`) at the single point text enters state.
//! - **Two emptiness predicates.** Placeholder visibility keys off strictly
//!   empty (`visible = value.length === 0`, `lib/composer-placeholder.ts`)
//!   while sendability additionally requires visible characters after
//!   trimming (`draft.trim().length > 0`, `thread-composer.svelte:176`) — a
//!   whitespace-only draft hides the placeholder yet stays unsubmittable.
//! - **Typed non-blank handoff.** [`NonBlankText`] applies the repository's
//!   trim-then-reject rule and retains the text untrimmed, because the
//!   composer submits the raw document and payload shaping stays caller
//!   policy.
//! - **Search-key derivation.** Match keys are `query.trim().toLowerCase()`
//!   with blank queries bypassing filtering entirely
//!   (`settings/font-picker.svelte:51`, launcher `variants/launcher.svelte:30`,
//!   INVENTORY §6.3).
//!
//! Trim and blank checks in this seam use Rust `str::trim` (Unicode
//! `White_Space`) and therefore diverge from the legacy JavaScript
//! `String.prototype.trim` on a handful of code points. The differences are
//! intentional and align this seam with the current native
//! `MessageBody`/`ComposerState` stack; do not broaden [`normalize_input`] or
//! invent ECMAScript trimming here:
//!
//! - `U+200B` (zero-width space) is whitespace in neither Rust nor JavaScript
//!   trim and is removed only by [`normalize_input`] intake.
//! - `U+FEFF` (BOM/zero-width no-break space) is blank to JavaScript trim
//!   but non-blank to Rust trim; the native seam accepts it and
//!   [`TextInputState::search_key`] returns `Some`.
//! - `U+0085` (NEL/next line) is non-blank to JavaScript trim but blank to
//!   Rust trim; the native seam rejects it and `search_key` returns `None`.
//!
//! Deliberate limits: no caret, selection, IME/composition, platform
//! text-service, rendering, or widget behavior; no byte ceilings (bounded
//! text stays in `artisan_domain`); no product submit/persistence policy.
//! The model is plain `std`, so behavioral tests need no window.

use std::borrow::Cow;

use thiserror::Error;

/// Folds external plaintext into the canonical input form.
///
/// Removes zero-width spaces (U+200B) and folds CR/CRLF to LF; every other
/// character passes through untouched. Trimming and case folding happen only
/// in derived views such as [`TextInputState::search_key`], never in the
/// stored value. Clean input is returned borrowed, without allocation.
#[must_use]
pub fn normalize_input(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') && !text.contains('\u{200B}') {
        return Cow::Borrowed(text);
    }

    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                normalized.push('\n');
            }
            '\u{200B}' => {}
            other => normalized.push(other),
        }
    }
    Cow::Owned(normalized)
}

/// Canonical plaintext held by one text-input surface across frames.
///
/// Constructed empty and advanced by whole-value replacement; the stored
/// value is always canonical ([`normalize_input`] ran on entry).
#[derive(Clone, Debug, Default)]
pub struct TextInputState {
    value: String,
}

impl TextInputState {
    /// Replaces the whole value with externally supplied text.
    ///
    /// Returns whether the canonical value changed: a replacement equal to
    /// the current value after normalization mutates nothing and reports
    /// `false`, so callers skip placeholder regeneration and repaints when
    /// nothing changed. Replacement with the empty string is the audited
    /// post-send clear.
    pub fn set_value(&mut self, text: &str) -> bool {
        let next = normalize_input(text);
        if next.as_ref() == self.value.as_str() {
            return false;
        }
        self.value = next.into_owned();
        true
    }

    /// Returns the canonical stored value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Whether the value is strictly empty: the placeholder-visibility rule
    /// (`visible = value.length === 0`). A whitespace-only draft is not
    /// empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Whether the value lacks visible characters after trimming: the negated
    /// sendability gate (`draft.trim().length > 0`). Blank input cannot start
    /// a message and yields no search key.
    ///
    /// Uses Rust `str::trim` semantics; see module docs for the deliberate
    /// divergence from JavaScript trim on `U+FEFF`/`U+0085`.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.value.trim().is_empty()
    }

    /// Validates the current value as submittable text.
    ///
    /// # Errors
    ///
    /// Returns [`BlankTextError`] when [`Self::is_blank`] holds. The accepted
    /// text is retained untrimmed.
    pub fn non_blank(&self) -> Result<NonBlankText, BlankTextError> {
        NonBlankText::new(&self.value)
    }

    /// Derives the trimmed, lowercased match key used by search surfaces.
    ///
    /// Returns `None` exactly when the value is blank — where the audited
    /// command menu bypasses filtering — and otherwise trims edges only,
    /// preserving interior spacing. Trimming follows Rust `str::trim`; see
    /// module docs for `U+FEFF`/`U+0085` parity notes.
    #[must_use]
    pub fn search_key(&self) -> Option<String> {
        if self.is_blank() {
            return None;
        }
        Some(self.value.trim().to_lowercase())
    }
}

/// Text validated to keep visible characters after trimming.
///
/// Applies the repository's trim-then-reject rule at the interaction
/// boundary and stores the original text untrimmed, so accepted payloads
/// reach submission exactly as authored. Validation uses Rust `str::trim`;
/// see module docs for the intentional `U+FEFF`/`U+0085` difference from
/// JavaScript trim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NonBlankText(String);

impl NonBlankText {
    /// Validates and retains external text.
    ///
    /// # Errors
    ///
    /// Returns [`BlankTextError`] when `text` is empty or contains only
    /// whitespace per Rust `str::trim`. Zero-width spaces (`U+200B`) are not
    /// whitespace in either Rust or the legacy JavaScript trim;
    /// [`normalize_input`] removes them before text reaches a gate. `U+FEFF`
    /// and `U+0085` intentionally follow Rust, not JavaScript, trim (see
    /// module docs).
    pub fn new(text: impl Into<String>) -> Result<Self, BlankTextError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(BlankTextError);
        }
        Ok(Self(text))
    }

    /// Returns the retained text exactly as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Input text carried no visible characters after trimming.
#[derive(Clone, Copy, Debug, Eq, Error, Hash, PartialEq)]
#[error("input text cannot be empty or whitespace-only")]
pub struct BlankTextError;
