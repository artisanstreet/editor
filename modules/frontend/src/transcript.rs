//! Native transcript/message presentation model for later `GPUI` composition.
//!
//! The UI inventory (`docs/ui/INVENTORY.md` §6.5 "Messages and transcript")
//! audits the selected workflow's transcript semantics: user, assistant, and
//! reasoning messages each render as a distinct article with a fixed label
//! ("Your message", "Assistant message", "Reasoning summary"), and a pending
//! steer announces itself as a status live region labelled "Steering". This
//! module owns exactly those presentation facts as typed values so a renderer
//! cannot drift from the audit and callers cannot supply arbitrary labels.
//!
//! Message content is opaque owned text: it is preserved verbatim — empty,
//! multiline, Unicode, or Markdown-looking alike — with no parsing,
//! sanitizing, truncating, or Markdown interpretation here or reachable
//! through this API. Shared Markdown internals remain behind their own seam
//! in `modules/ui`; this model never couples to them.
//!
//! The model is pure and deterministic and publishes nothing by itself: it
//! supplies the audited semantic values a later renderer will present.

/// Role of one transcript conversation message.
///
/// The inventory records exactly these message kinds for the selected
/// workflow, routed by the transcript dispatcher to distinct articles.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MessageRole {
    /// A message authored by the signed-in user.
    User,
    /// A message authored by the assistant.
    Assistant,
    /// A reasoning summary accompanying assistant work.
    Reasoning,
}

impl MessageRole {
    /// Returns the exact audited article label for this role.
    ///
    /// The label is a fixed presentation fact, never caller-provided text and
    /// never derived from message content.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "Your message",
            Self::Assistant => "Assistant message",
            Self::Reasoning => "Reasoning summary",
        }
    }
}

/// One transcript message: an audited [`MessageRole`] plus opaque owned text.
///
/// Construction is the only way to bind role and content, and neither field
/// is publicly assignable, so content can never influence the audited
/// [`TranscriptMessage::label`]. The text round-trips byte-for-byte.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TranscriptMessage {
    role: MessageRole,
    content: Box<str>,
}

impl TranscriptMessage {
    /// Creates a message carrying `content` verbatim under `role`.
    #[must_use]
    pub fn new(role: MessageRole, content: impl Into<Box<str>>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    /// The audited role of this message.
    #[must_use]
    pub const fn role(&self) -> MessageRole {
        self.role
    }

    /// The exact audited article label for this message's role.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        self.role.label()
    }

    /// The owned message text, exactly as supplied.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// How a non-message transcript element announces itself to a later renderer.
///
/// The inventory records one such announcement kind for the selected
/// workflow: a pending steer presented as a status live region. Keeping the
/// fact a typed value (not a boolean or attribute string) lets the renderer
/// map it onto whatever accessibility surface its platform exposes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AnnouncementRole {
    /// Announces politely as a status region (`role="status"`).
    Status,
}

impl AnnouncementRole {
    /// Returns the exact audited attribute value for this role.
    #[must_use]
    pub const fn attribute_value(self) -> &'static str {
        match self {
            Self::Status => "status",
        }
    }
}

/// Pending-steer presentation fact from the same inventory section.
///
/// While a steer awaits dispatch the transcript announces it as
/// [`AnnouncementRole::Status`] named [`PendingSteer::LABEL`]; it is not a
/// message and carries none of the message roles' article labels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PendingSteer;

impl PendingSteer {
    /// Live-region kind announced while a steer pends.
    pub const ANNOUNCEMENT_ROLE: AnnouncementRole = AnnouncementRole::Status;

    /// Exact audited announcement label.
    pub const LABEL: &str = "Steering";
}
