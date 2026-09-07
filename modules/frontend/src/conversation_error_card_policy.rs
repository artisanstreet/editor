//! Dependency-free presentation policy for a conversation error card.
//!
//! This is the native counterpart of
//! `routes/components/conversation-error-card.svelte`. The caller supplies
//! the result of the catalog lookup, the error code and detail facts, and the
//! result of any locale-aware reset-time formatting. This leaf does not own a
//! catalog, parse or format dates, access a browser, or write a clipboard.
//!
//! The supplied definition is intentionally separate from the supplied code:
//! a newer backend may send a code that this renderer does not know yet. The
//! catalog adapter can provide its unknown definition while this policy still
//! renders and offers the original code as the durable diagnostic artifact.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// The catalog fields read by the error card after its lookup has completed.
///
/// This is a borrowed view of an external catalog result, not a Rust copy of
/// the catalog. The catalog adapter remains responsible for selecting the
/// known or unknown definition.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CatalogErrorDefinition<'a> {
    /// The catalog-provided heading for the failure.
    pub title: &'a str,
    /// The catalog-provided explanation for the failure.
    pub summary: &'a str,
}

impl<'a> CatalogErrorDefinition<'a> {
    /// Creates a view over one already-resolved catalog definition.
    #[must_use]
    pub const fn new(title: &'a str, summary: &'a str) -> Self {
        Self { title, summary }
    }
}

/// Inputs needed to project one conversation error card.
///
/// `captured_detail` is the detail carried by the error reference itself;
/// `projected_detail` is the component's separate summary/detail input. The
/// former wins whenever it is present, including an explicitly empty string,
/// matching JavaScript's nullish (`??`) precedence.
///
/// `formatted_reset_label` is the already-formatted result from a date/time
/// adapter. It is deliberately not a timestamp: this policy must not invent
/// locale, timezone, parsing, or clock-format behavior.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ConversationErrorCardInput<'a> {
    /// The already-resolved catalog definition, including an unknown fallback
    /// when the renderer does not recognize the supplied code.
    pub definition: CatalogErrorDefinition<'a>,
    /// The exact error code retained from the conversation error reference.
    pub code: &'a str,
    /// Detail captured on the error reference itself.
    pub captured_detail: Option<&'a str>,
    /// Detail supplied by the surrounding projected conversation item.
    pub projected_detail: Option<&'a str>,
    /// The optional label returned by the locale-aware reset-time adapter.
    pub formatted_reset_label: Option<&'a str>,
}

impl<'a> ConversationErrorCardInput<'a> {
    /// Creates card inputs from one catalog result and the renderer facts.
    #[must_use]
    pub const fn new(
        definition: CatalogErrorDefinition<'a>,
        code: &'a str,
        captured_detail: Option<&'a str>,
        projected_detail: Option<&'a str>,
        formatted_reset_label: Option<&'a str>,
    ) -> Self {
        Self {
            definition,
            code,
            captured_detail,
            projected_detail,
            formatted_reset_label,
        }
    }
}

/// The host action requested by the card's copy-code control.
///
/// Constructing this value only describes an intent. A later clipboard
/// adapter owns the actual write; this module never touches a clipboard.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[must_use = "dispatch or retain the conversation error card action"]
pub enum ConversationErrorCardCommand<'a> {
    /// Ask a host adapter to write the exact error code.
    CopyCode {
        /// The exact code shown by the card and sent to the adapter.
        code: &'a str,
    },
}

impl<'a> ConversationErrorCardCommand<'a> {
    /// Creates the copy command for one exact error code.
    #[must_use = "retain the copy command for a host adapter"]
    pub const fn copy_code(code: &'a str) -> Self {
        Self::CopyCode { code }
    }

    /// Returns the exact code carried by this command.
    #[must_use]
    pub const fn code(self) -> &'a str {
        match self {
            Self::CopyCode { code } => code,
        }
    }
}

/// The result reported by a host adapter after a copy-code command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CopyCodeResult {
    /// The host accepted the exact code for copying.
    Succeeded,
    /// The host rejected or failed the copy operation.
    Failed,
}

/// Typed copy feedback state for the error-card adapter boundary.
///
/// The card itself has no clipboard implementation or feedback side effect;
/// the state lets a later host/UI adapter retain the result without exposing
/// an untyped boolean or an opaque browser error.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CopyCodeState {
    /// No copy result has been supplied yet.
    #[default]
    Idle,
    /// The latest supplied copy result succeeded.
    Success,
    /// The latest supplied copy result failed.
    Failure,
}

impl CopyCodeState {
    /// Applies one typed host result to the visible copy state.
    #[must_use]
    pub const fn from_result(result: CopyCodeResult) -> Self {
        match result {
            CopyCodeResult::Succeeded => Self::Success,
            CopyCodeResult::Failed => Self::Failure,
        }
    }

    /// Returns whether the state represents a successful copy.
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    /// Returns whether the state represents a failed copy.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Failure)
    }
}

impl From<CopyCodeResult> for CopyCodeState {
    fn from(result: CopyCodeResult) -> Self {
        Self::from_result(result)
    }
}

/// Card facts ready for a native renderer and its host adapters.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationErrorCardPresentation<'a> {
    /// Catalog-provided failure title.
    pub title: &'a str,
    /// Catalog summary, optionally followed by the reset sentence.
    pub summary: String,
    /// The diagnostic line, with captured detail taking precedence.
    pub diagnostic: Option<&'a str>,
    /// The exact code that remains visible even for an unknown definition.
    pub code: &'a str,
    /// Whether the code/copy control is rendered.
    pub code_visible: bool,
    /// Typed host command for copying [`Self::code`].
    pub copy_command: ConversationErrorCardCommand<'a>,
    /// Exact accessible label for the copy control.
    pub copy_accessible_label: String,
}

impl<'a> ConversationErrorCardPresentation<'a> {
    /// Returns the diagnostic line under its source-oriented name.
    #[must_use]
    pub const fn detail(&self) -> Option<&'a str> {
        self.diagnostic
    }

    /// Returns the diagnostic line under its card-oriented name.
    #[must_use]
    pub const fn diagnostic(&self) -> Option<&'a str> {
        self.diagnostic
    }

    /// Returns whether the code/copy control is visible.
    #[must_use]
    pub const fn is_code_visible(&self) -> bool {
        self.code_visible
    }

    /// Borrows the exact accessible label for the copy control.
    #[must_use]
    pub fn accessible_label(&self) -> &str {
        &self.copy_accessible_label
    }
}

/// Applies the Svelte `error.detail ?? detail` precedence rule.
///
/// `Some("")` is a supplied value, not an absent value, and therefore wins
/// over the projected detail.
#[must_use]
pub const fn diagnostic_detail<'a>(
    captured_detail: Option<&'a str>,
    projected_detail: Option<&'a str>,
) -> Option<&'a str> {
    match captured_detail {
        Some(detail) => Some(detail),
        None => projected_detail,
    }
}

/// Returns the exact accessible label used by the copy control.
#[must_use]
pub fn copy_code_accessible_label(code: &str) -> String {
    format!("Copy error code {code}")
}

/// Projects supplied error-card facts without looking up or duplicating a
/// catalog definition.
///
/// A reset sentence is appended only when the date/time adapter supplies a
/// label. The label's contents, including an explicitly supplied empty value,
/// are retained verbatim; no date parsing or locale formatting occurs here.
#[must_use]
pub fn present_conversation_error_card(
    input: ConversationErrorCardInput<'_>,
) -> ConversationErrorCardPresentation<'_> {
    let diagnostic = diagnostic_detail(input.captured_detail, input.projected_detail);
    let summary = match input.formatted_reset_label {
        Some(label) => format!("{} Resets {label}.", input.definition.summary),
        None => input.definition.summary.to_owned(),
    };

    ConversationErrorCardPresentation {
        title: input.definition.title,
        summary,
        diagnostic,
        code: input.code,
        code_visible: true,
        copy_command: ConversationErrorCardCommand::copy_code(input.code),
        copy_accessible_label: copy_code_accessible_label(input.code),
    }
}
