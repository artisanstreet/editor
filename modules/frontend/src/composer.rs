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
//! replacement drafted while that flight ran.
//!
//! Two further contracts are explicit native decisions, not legacy behavior:
//! the legacy editor froze input mid-flight, so nothing in the audited
//! surface can establish what should happen when edits are allowed to
//! continue. First, identity fencing — each begun flight carries an opaque
//! token required for its completion, so a late or duplicated completion of
//! an older flight can neither consume nor clear a newer one. Second,
//! authored-edit preservation — every actual post-begin draft change,
//! including one later reverted to the original text, blocks an accepted
//! clear, while rewriting the already-held text is not an edit.
//!
//! Body validity delegates to the native domain's bounded [`MessageBody`]
//! (trim-non-blank plus the 65,536 UTF-8-byte ceiling recorded in
//! `docs/decisions/NATIVE_PRODUCT_SCOPE.md`); blank or oversized drafts
//! surface the domain's own error and never start a flight.

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// Every issuable single-use flight identity has been minted exactly
    /// once: the checked counter issues `2^64 - 1` values (0 through
    /// `u64::MAX - 1`) and reserves `u64::MAX` as its exhaustion state, so
    /// beginning another submission would require reusing retired identity.
    /// Unreachable in practice within one process lifetime; no flight was
    /// started and the draft is untouched.
    IdentityExhausted,
}

impl fmt::Display for SubmissionBlocked {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBody(error) => write!(formatter, "{error}"),
            Self::InFlight => formatter.write_str("a submission is already in flight"),
            Self::Disabled => formatter.write_str("sending is disabled on this surface"),
            Self::IdentityExhausted => {
                formatter.write_str("flight identity is exhausted; no further submission can begin")
            }
        }
    }
}

impl Error for SubmissionBlocked {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidBody(error) => Some(error),
            Self::InFlight | Self::Disabled | Self::IdentityExhausted => None,
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

/// Source of per-flight identity: every begun submission mints the next
/// value exactly once, and a value already handed out is never issued again —
/// not even after the composing state resets or a different composer begins
/// its own flights. The increment is overflow-checked and issues exactly
/// `2^64 - 1` values, 0 through `u64::MAX - 1`; `u64::MAX` is reserved as the
/// exhaustion state, where minting fails and begins are refused (see
/// [`SubmissionBlocked::IdentityExhausted`]) rather than wrapping around to
/// reissue retired values. This mirrors the platform decision that
/// request/response correlation is single-use and retired identifiers are
/// never reissued (`docs/decisions/NATIVE_PRODUCT_SCOPE.md`).
static NEXT_FLIGHT_ORDINAL: AtomicU64 = AtomicU64::new(0);

/// Opaque, single-use identity of one begun submission flight.
///
/// Only [`ComposerState::begin_submission`] mints a token, and only passing
/// a flight's still-current token to [`ComposerState::finish_submission`]
/// ends it. The ordinal inside is private with no public constructor or
/// conversion, so flight identity cannot be forged from outside this module.
///
/// Deriving [`Clone`] and [`Copy`] is deliberate and harmless: possession
/// proves nothing once a flight retires, because completion is honored only
/// while the minting state still holds that exact active flight, and retired
/// identities are never reissued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubmissionToken {
    /// Monotonic, never-reissued ordinal minted at begin time.
    ordinal: u64,
}

impl SubmissionToken {
    /// Mints the next unused flight identity, or [`None`] once all
    /// `2^64 - 1` issuable values (0 through `u64::MAX - 1`, with `u64::MAX`
    /// reserved as the exhaustion state) have been issued exactly once —
    /// retirement stays permanent even at exhaustion.
    fn mint() -> Option<Self> {
        let ordinal = NEXT_FLIGHT_ORDINAL
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |issued| {
                issued.checked_add(1)
            })
            .ok()?;
        Some(Self { ordinal })
    }
}

/// The one permitted active flight: its opaque identity plus the validated
/// body retained so an accepted finish can clear only text it actually sent.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveFlight {
    token: SubmissionToken,
    body: MessageBody,
    /// Sticky marker of any actual draft change after this flight began:
    /// set by the first differing [`ComposerState::set_draft`] write and
    /// never cleared by reverting, so an accepted send cannot discard text
    /// the user truly touched. Starts `false` with each newly begun flight
    /// and is never inherited by a later one.
    changed_since_begin: bool,
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
    /// The active flight, if any: its opaque identity plus the body
    /// validated when it began, retained so an accepted finish can clear
    /// only text it actually sent.
    flight: Option<ActiveFlight>,
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
    ///
    /// Only an actual value change counts as an edit: writing the text the
    /// draft already holds is a no-op, while any differing write during an
    /// active flight marks that flight changed — permanently for that
    /// flight, even if later writes restore the exact submitted text.
    pub fn set_draft(&mut self, draft: impl Into<String>) {
        let draft = draft.into();
        if self.draft == draft {
            return;
        }
        self.draft = draft;
        if let Some(flight) = self.flight.as_mut() {
            flight.changed_since_begin = true;
        }
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
    /// On success the validated body and the flight's newly minted opaque
    /// [`SubmissionToken`] are returned together, ready for the caller's
    /// send seam, and that snapshot is retained as the active flight — for
    /// retry safety the draft itself is kept until
    /// [`Self::finish_submission`] reports an accepted send of this same
    /// body with the draft unchanged since begin. The token is the only key
    /// that can later end this flight, and the flight starts out unmarked.
    ///
    /// Refusals follow the legacy check order: draft validity first, then an
    /// already-active flight, then a disabled surface. No refusal starts a
    /// flight, mints an identity, or alters the draft.
    ///
    /// # Errors
    ///
    /// Returns [`SubmissionBlocked::InvalidBody`] carrying the native domain
    /// error for a blank or over-ceiling draft, [`SubmissionBlocked::InFlight`]
    /// when a submission is already active, [`SubmissionBlocked::Disabled`]
    /// when the surface disallows sending, or
    /// [`SubmissionBlocked::IdentityExhausted`] when every single-use flight
    /// identity has already been issued and none may be reused.
    pub fn begin_submission(
        &mut self,
    ) -> Result<(MessageBody, SubmissionToken), SubmissionBlocked> {
        let body =
            MessageBody::parse(self.draft.clone()).map_err(SubmissionBlocked::InvalidBody)?;
        if self.flight.is_some() {
            return Err(SubmissionBlocked::InFlight);
        }
        if self.disabled {
            return Err(SubmissionBlocked::Disabled);
        }

        let token = SubmissionToken::mint().ok_or(SubmissionBlocked::IdentityExhausted)?;
        self.flight = Some(ActiveFlight {
            token,
            body: body.clone(),
            changed_since_begin: false,
        });
        Ok((body, token))
    }

    /// Ends exactly the flight identified by `token`, applying the given
    /// [`DraftDisposition`] to it.
    ///
    /// Completion requires the currently active flight's own token. Any other
    /// completion — no flight at all, a token already retired by an earlier
    /// finish, or an older flight's token replayed after a newer flight began
    /// — is inert: it cannot consume the newer flight, clear or rewrite its
    /// draft, end it early, or rearm submission. Identity is per attempt, not
    /// per body, so even a retry of byte-identical text carries a fresh token
    /// that the retired one cannot complete.
    ///
    /// An accepted send of the matching flight clears the draft only when no
    /// actual change has happened since its begin: text entered or set after
    /// the begin — the model permits programmatic edits even though the
    /// legacy editor froze them — survives completion untouched, and so does
    /// text edited away and then restored to the exact submitted bytes.
    /// Writing the already-held text is not an edit and does not block the
    /// clear. A retained outcome never touches the draft.
    pub fn finish_submission(&mut self, token: SubmissionToken, disposition: DraftDisposition) {
        let Some(flight) = self.flight.take_if(|flight| flight.token == token) else {
            return;
        };

        if disposition == DraftDisposition::Accepted
            && !flight.changed_since_begin
            && self.draft == flight.body.as_str()
        {
            self.draft.clear();
        }
    }
}
