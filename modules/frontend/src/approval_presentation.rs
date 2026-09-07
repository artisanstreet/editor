//! Pure approval presentation policy.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/conversation/approval-presentation.ts` and the
//! presentation decisions in `conversation-approval.svelte`. It models only
//! the fields that policy reads and returns the renderer-facing values; it
//! performs no engine, host, DOM, or asynchronous work.

#![allow(clippy::module_name_repetitions)]

/// The state values defined by the conversation approval protocol.
///
/// Unknown raw values are retained so an adapter can pass a future protocol
/// value through this pure boundary. The presentation policy treats every
/// unknown value like the TypeScript `StateTitle` fallback: it is settled and
/// receives the `cancelled` title suffix.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ApprovalState {
    /// The approval is waiting for the reader.
    Requested,
    /// The reader approved the request.
    Approved,
    /// The reader denied the request.
    Rejected,
    /// The request was cancelled before completion.
    Cancelled,
    /// A state introduced by a newer or malformed protocol value.
    Unknown(String),
}

impl ApprovalState {
    /// Classifies an exact protocol state without trimming or case folding.
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "requested" => Self::Requested,
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            "cancelled" => Self::Cancelled,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Returns the exact raw state represented by this value.
    #[must_use]
    pub fn as_raw(&self) -> &str {
        match self {
            Self::Requested => "requested",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::Unknown(raw) => raw,
        }
    }

    /// Whether the state is exactly the pending TypeScript state.
    #[must_use]
    pub const fn is_requested(&self) -> bool {
        matches!(self, Self::Requested)
    }

    /// The status icon used for a settled approval row.
    ///
    /// The Svelte call site renders `CircleCheck` only for `approved`; every
    /// other state, including unknown values, renders `CircleX`.
    #[must_use]
    pub const fn status_icon_name(&self) -> &'static str {
        match self {
            Self::Approved => "circle-check",
            Self::Requested | Self::Rejected | Self::Cancelled | Self::Unknown(_) => "circle-x",
        }
    }

    fn title(&self, noun: &str) -> Option<String> {
        match self {
            Self::Requested => None,
            Self::Approved => Some(format!("{noun} approved")),
            Self::Rejected => Some(format!("{noun} denied")),
            Self::Cancelled | Self::Unknown(_) => Some(format!("{noun} cancelled")),
        }
    }
}

impl From<&str> for ApprovalState {
    fn from(raw: &str) -> Self {
        Self::from_raw(raw)
    }
}

impl From<String> for ApprovalState {
    fn from(raw: String) -> Self {
        match raw.as_str() {
            "requested" => Self::Requested,
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            "cancelled" => Self::Cancelled,
            _ => Self::Unknown(raw),
        }
    }
}

/// The request kinds defined by the conversation approval protocol.
///
/// The `Unknown` variant makes the runtime fallback in the TypeScript
/// function representable even though the protocol schema currently limits
/// the union to the three named kinds. Unknown kinds normalize to
/// [`Self::Action`] in the returned presentation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ApprovalKind {
    /// A command that the engine is waiting to run.
    Command,
    /// A file-change operation that the engine is waiting to apply.
    FileChange,
    /// A generic action requiring reader approval.
    Action,
    /// A kind introduced by a newer or malformed protocol value.
    Unknown(String),
}

impl ApprovalKind {
    /// Classifies an exact protocol kind without trimming or case folding.
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "command" => Self::Command,
            "file_change" => Self::FileChange,
            "action" => Self::Action,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Returns the exact raw kind represented by this value.
    #[must_use]
    pub fn as_raw(&self) -> &str {
        match self {
            Self::Command => "command",
            Self::FileChange => "file_change",
            Self::Action => "action",
            Self::Unknown(raw) => raw,
        }
    }

    /// Returns the request icon intent from `conversation-approval.svelte`.
    ///
    /// Commands use `Terminal2`; file changes, generic actions, and unknown
    /// values use `FileDiff`.
    #[must_use]
    pub const fn icon_name(&self) -> &'static str {
        match self {
            Self::Command => "terminal-2",
            Self::FileChange | Self::Action | Self::Unknown(_) => "file-diff",
        }
    }
}

impl From<&str> for ApprovalKind {
    fn from(raw: &str) -> Self {
        Self::from_raw(raw)
    }
}

impl From<String> for ApprovalKind {
    fn from(raw: String) -> Self {
        match raw.as_str() {
            "command" => Self::Command,
            "file_change" => Self::FileChange,
            "action" => Self::Action,
            _ => Self::Unknown(raw),
        }
    }
}

/// The structured request attached to an approval item.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ApprovalRequest {
    /// The request kind.
    pub kind: ApprovalKind,
    /// The command to display for a command request, when supplied.
    pub command: Option<String>,
    /// The working directory to display for a command request, when supplied.
    pub cwd: Option<String>,
    /// The provider-authored reason, when supplied.
    pub reason: Option<String>,
}

impl ApprovalRequest {
    /// Creates a request with the exact structured fields consumed by policy.
    #[must_use]
    pub const fn new(
        kind: ApprovalKind,
        command: Option<String>,
        cwd: Option<String>,
        reason: Option<String>,
    ) -> Self {
        Self {
            kind,
            command,
            cwd,
            reason,
        }
    }

    /// Creates a command request.
    #[must_use]
    pub const fn command(
        command: Option<String>,
        cwd: Option<String>,
        reason: Option<String>,
    ) -> Self {
        Self::new(ApprovalKind::Command, command, cwd, reason)
    }

    /// Creates a file-change request.
    #[must_use]
    pub const fn file_change(reason: Option<String>) -> Self {
        Self::new(ApprovalKind::FileChange, None, None, reason)
    }

    /// Creates a generic action request.
    #[must_use]
    pub const fn action(reason: Option<String>) -> Self {
        Self::new(ApprovalKind::Action, None, None, reason)
    }
}

/// The approval-item fields consumed by the presentation policy.
///
/// The other durable `ConversationItem` fields are intentionally outside this
/// leaf. An adapter can project the approval item's `prompt`, `state`, and
/// optional `request` into this dependency-free value.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ApprovalItem {
    /// The legacy provider prompt.
    pub prompt: String,
    /// The approval state.
    pub state: ApprovalState,
    /// The optional structured approval request.
    pub request: Option<ApprovalRequest>,
}

impl ApprovalItem {
    /// Creates an item from the fields consumed by the pure policy.
    #[must_use]
    pub fn new(
        prompt: impl Into<String>,
        state: ApprovalState,
        request: Option<ApprovalRequest>,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            state,
            request,
        }
    }
}

/// The exact renderer-facing approval presentation.
///
/// `None` represents an omitted optional property in the TypeScript object.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ApprovalPresentation {
    /// Label for the affirmative action.
    pub approve_label: &'static str,
    /// Command text included only for command requests when supplied.
    pub command: Option<String>,
    /// Working directory included only for command requests when supplied.
    pub cwd: Option<String>,
    /// The request description shown only while the item is pending, where
    /// the TypeScript policy supplies one.
    pub description: Option<String>,
    /// The normalized presentation kind.
    pub kind: ApprovalKind,
    /// The exact title string for this state and kind.
    pub title: String,
}

impl ApprovalPresentation {
    /// Returns the request icon intent for this normalized presentation.
    #[must_use]
    pub const fn icon_name(&self) -> &'static str {
        self.kind.icon_name()
    }

    /// Returns the settled-row status icon intent for `state`.
    #[must_use]
    pub const fn status_icon(state: &ApprovalState) -> &'static str {
        state.status_icon_name()
    }
}

/// The ECMAScript whitespace characters used by JavaScript `trim()` and `\s`.
///
/// Rust's Unicode whitespace predicate differs at U+0085 and U+FEFF, so the
/// set is spelled out to keep legacy-description and plumbing parity exact.
const fn is_ecmascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

const fn is_ecmascript_line_terminator(character: char) -> bool {
    matches!(character, '\u{000A}' | '\u{000D}' | '\u{2028}' | '\u{2029}')
}

/// Returns whether `trimmed` matches the TypeScript protocol-plumbing regex:
/// `/^item\/.+\/requestApproval(?:\s+for\s+\S+)?$/u`.
///
/// The TypeScript caller supplies `prompt.trim()` to this check. This helper
/// therefore intentionally does not trim its input; callers that need the
/// complete legacy policy should use [`legacy_description`]. JavaScript's
/// dot does not match line terminators, which is handled explicitly here.
#[must_use]
pub fn is_protocol_plumbing(trimmed: &str) -> bool {
    const NEEDLE: &str = "/requestApproval";
    let Some(rest) = trimmed.strip_prefix("item/") else {
        return false;
    };

    for (needle_start, _) in rest.match_indices(NEEDLE) {
        let prefix = &rest[..needle_start];
        if prefix.is_empty() || prefix.chars().any(is_ecmascript_line_terminator) {
            continue;
        }

        let suffix = &rest[needle_start + NEEDLE.len()..];
        if suffix.is_empty() || is_optional_for_suffix(suffix) {
            return true;
        }
    }

    false
}

fn is_optional_for_suffix(suffix: &str) -> bool {
    let Some(after_leading_whitespace) = consume_ecmascript_whitespace(suffix) else {
        return false;
    };
    let Some(after_for) = after_leading_whitespace.strip_prefix("for") else {
        return false;
    };
    let Some(token) = consume_ecmascript_whitespace(after_for) else {
        return false;
    };

    !token.is_empty()
        && token
            .chars()
            .all(|character| !is_ecmascript_whitespace(character))
}

fn consume_ecmascript_whitespace(input: &str) -> Option<&str> {
    let end = input
        .char_indices()
        .take_while(|&(_, character)| is_ecmascript_whitespace(character))
        .last()
        .map_or(0, |(index, character)| index + character.len_utf8());

    (end > 0).then(|| &input[end..])
}

/// Returns the legacy description after JavaScript-compatible trimming.
///
/// An empty prompt and protocol-plumbing prompt both produce `None`.
#[must_use]
pub fn legacy_description(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim_matches(is_ecmascript_whitespace);
    if trimmed.is_empty() || is_protocol_plumbing(trimmed) {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn state_title(state: &ApprovalState, noun: &str) -> Option<String> {
    state.title(noun)
}

/// Computes the exact presentation returned by `GetApprovalPresentation`.
///
/// Structured `reason` takes precedence over the legacy prompt using
/// TypeScript's nullish behavior: an explicitly supplied empty string remains
/// empty. Command and working-directory fields are retained only for the
/// command branch. A missing or unknown request kind uses the generic action
/// branch, and only the requested state exposes a description.
#[must_use]
pub fn get_approval_presentation(item: &ApprovalItem) -> ApprovalPresentation {
    let reason = item
        .request
        .as_ref()
        .and_then(|request| request.reason.clone())
        .or_else(|| legacy_description(&item.prompt));

    match item.request.as_ref() {
        Some(request) if matches!(&request.kind, ApprovalKind::Command) => {
            let title = state_title(&item.state, "Command")
                .unwrap_or_else(|| String::from("Run this command?"));
            let description = item.state.is_requested().then_some(reason).flatten();
            ApprovalPresentation {
                approve_label: "Run command",
                command: request.command.clone(),
                cwd: request.cwd.clone(),
                description,
                kind: ApprovalKind::Command,
                title,
            }
        }
        Some(request) if matches!(&request.kind, ApprovalKind::FileChange) => {
            let title = state_title(&item.state, "Changes")
                .unwrap_or_else(|| String::from("Apply these changes?"));
            let description = item.state.is_requested().then_some(reason).flatten();
            ApprovalPresentation {
                approve_label: "Apply changes",
                command: None,
                cwd: None,
                description,
                kind: ApprovalKind::FileChange,
                title,
            }
        }
        _ => {
            let title = state_title(&item.state, "Action")
                .unwrap_or_else(|| String::from("Allow this action?"));
            let description = if item.state.is_requested() {
                Some(reason.unwrap_or_else(|| {
                    String::from("Artisan needs your approval before it can continue.")
                }))
            } else {
                None
            };
            ApprovalPresentation {
                approve_label: "Approve",
                command: None,
                cwd: None,
                description,
                kind: ApprovalKind::Action,
                title,
            }
        }
    }
}
