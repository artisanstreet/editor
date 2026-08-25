//! Composer draft/submission state for the first native workflow.
//!
//! This leaf models the audited text-draft slice of the legacy composer
//! (`routes/components/thread-composer.svelte`): editable draft text, derived
//! send readiness, typed blocked outcomes, and the single-flight submission
//! lifecycle. It is deliberately UI-toolkit-neutral — plain owned Rust state
//! with no GPUI rendering, attachment, IME, caret, transport, receipt, engine,
//! or persistence machinery — so a later GPUI entity/action can own and drive
//! it synchronously without blocking the UI thread.
//!
//! Ground rules carried over from the cited source: readiness follows the
//! legacy `send_ready` derivation; a send attempt that cannot start is a
//! typed outcome that preserves the draft ("your message is kept in the
//! composer"); submissions are single-flight; and the draft resets only when
//! the receiver accepts the body that was actually begun — never a newer
//! replacement drafted while that flight ran. Body validity delegates to the
//! native domain's bounded [`MessageBody`] (trim-non-blank plus the 65,536
//! UTF-8-byte ceiling recorded in `docs/decisions/NATIVE_PRODUCT_SCOPE.md`);
//! blank or oversized drafts surface the domain's own error and never start a
//! flight.

use std::error::Error;
use std::fmt;

use artisan_domain::text::{MessageBody, MessageBodyError};

/// Why a begin-submission attempt produced no flight.
///
/// Every variant is a final answer for the attempt that produced it: none of
/// them mutates the draft or starts a submission, so the caller can render the
/// reason and leave recovery to the user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmissionBlocked {
    /// The draft failed native [`MessageBody`] validation — blank after
    /// trimming ([`MessageBodyError::Blank`]) or over the UTF-8 byte ceiling
    /// ([`MessageBodyError::TooLong`]). No flight was started.
    InvalidBody(MessageBodyError),
    /// A submission is already in flight; the lifecycle admits one flight at
    /// a time.
    InFlight,
    /// Sending is disabled on this surface, such as while it prepares or
    /// loses its session.
    Disabled,
}

impl fmt::Display for SubmissionBlocked {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBody(error) => write!(formatter, "{error}"),
            Self::InFlight => formatter.write_str("a submission is already in flight"),
            Self::Disabled => formatter.write_str("sending is disabled on this surface"),
        }
    }
}

impl Error for SubmissionBlocked {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidBody(error) => Some(error),
            Self::InFlight | Self::Disabled => None,
        }
    }
}

/// Disposition applied to the draft when one flight finishes.
///
/// The distinction preserves retry safety: only an accepted send may reset
/// the composer, mirroring the legacy split between its complete-submission
/// cleanup (which cleared the text) and its failure/release path (which left
/// the text alone).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftDisposition {
    /// The receiver accepted the composed body: reset the draft so the next
    /// message starts empty.
    Accepted,
    /// The flight ended without an accepted send: keep every character so the
    /// same message can be corrected and retried.
    Retained,
}

/// Toolkit-neutral state for one composer: its draft text plus the
/// single-flight submission lifecycle around it.
///
/// Instances are plain values with no threading or toolkit requirements; a
/// future GPUI entity owns one and exposes the operations as actions. Nothing
/// here renders, persists, or performs network work.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerState {
    /// Verbatim draft text exactly as supplied, untrimmed.
    draft: String,
    /// Whether the owning surface currently disallows sending.
    disabled: bool,
    /// Body validated when the active flight began, retained so an accepted
    /// finish can clear only text it actually sent.
    flight: Option<MessageBody>,
}

impl ComposerState {
    /// Creates an empty, enabled composer with no flight in progress.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the draft exactly as supplied, including surrounding
    /// whitespace; the domain never silently rewrites client content.
    #[must_use]
    pub fn draft(&self) -> &str {
        &self.draft
    }

    /// Replaces the draft text. Editing stays available at any time — even
    /// mid-flight, where the legacy editor froze input instead; see
    /// [`Self::finish_submission`] for how a newer draft outlives an older
    /// accepted send. Whether to freeze input is a presentation decision for
    /// the owning surface.
    pub fn set_draft(&mut self, draft: impl Into<String>) {
        self.draft = draft.into();
    }

    /// Returns whether sending is currently disallowed on this surface.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Enables or disables sending for this surface. Changing this during an
    /// active flight does not end the flight; it only gates further begins.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }

    /// Returns whether one begun submission is awaiting its finish.
    #[must_use]
    pub fn is_submitting(&self) -> bool {
        self.flight.is_some()
    }

    /// Returns whether the send control should be armed right now.
    ///
    /// Mirrors the legacy derivation: enabled, not mid-flight, and a draft
    /// with visible characters after trimming. Deliberately stricter than
    /// [`MessageBody`] validation — the byte ceiling is enforced when a
    /// submission begins, matching a legacy button that stayed armed for
    /// oversized drafts until the attempt itself was rejected. Callsite
    /// conditions such as external block reasons and receiver arming are the
    /// owner's contribution and stay outside this leaf.
    #[must_use]
    pub fn send_ready(&self) -> bool {
        self.flight.is_none() && !self.disabled && !self.draft.trim().is_empty()
    }

    /// Attempts to begin one submission flight.
    ///
    /// On success the validated body is returned ready for the caller's send
    /// seam, and that snapshot is retained as the active flight — for retry
    /// safety the draft itself is kept until [`Self::finish_submission`]
    /// reports an accepted send of this same body.
    ///
    /// Refusals follow the legacy check order: draft validity first, then an
    /// already-active flight, then a disabled surface. No refusal starts a
    /// flight or alters the draft.
    ///
    /// # Errors
    ///
    /// Returns [`SubmissionBlocked::InvalidBody`] carrying the native domain
    /// error for a blank or over-ceiling draft, [`SubmissionBlocked::InFlight`]
    /// when a submission is already active, or [`SubmissionBlocked::Disabled`]
    /// when the surface disallows sending.
    pub fn begin_submission(&mut self) -> Result<MessageBody, SubmissionBlocked> {
        let body =
            MessageBody::parse(self.draft.clone()).map_err(SubmissionBlocked::InvalidBody)?;
        if self.flight.is_some() {
            return Err(SubmissionBlocked::InFlight);
        }
        if self.disabled {
            return Err(SubmissionBlocked::Disabled);
        }

        self.flight = Some(body.clone());
        Ok(body)
    }

    /// Ends the active flight with the given [`DraftDisposition`].
    ///
    /// An accepted send clears the draft only when it still equals the body
    /// that flight actually began; text entered or set after the begin — the
    /// model permits programmatic edits even though the legacy editor froze
    /// them — survives its completion untouched. A retained outcome never
    /// touches the draft. Finishing while no flight is active does nothing,
    /// so a duplicate completion cannot discard anything either.
    pub fn finish_submission(&mut self, disposition: DraftDisposition) {
        let Some(body) = self.flight.take() else {
            return;
        };

        if disposition == DraftDisposition::Accepted && self.draft == body.as_str() {
            self.draft.clear();
        }
    }
}
