//! Reader-visible failure values for composer actions.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/composer/action-failure.ts`. The legacy boundary
//! carries only a title and a description: the title names the refused action,
//! while the description carries the caller's cause. The composer producer
//! passes those values to `ReportActionFailure(title, description)` and keeps
//! the action-specific title selection at that call site.
//!
//! This leaf owns no notification, transport, error-code, retry, or rendering
//! behavior. It only stores the two reader-visible strings exactly as supplied.

#![allow(clippy::module_name_repetitions)]

/// The reader-visible result of a refused composer action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerActionFailure {
    /// The caller-supplied cause, shown below the action title.
    pub description: String,
    /// The caller-supplied name of the action that was refused.
    pub title: String,
}

impl ComposerActionFailure {
    /// Creates a failure with the same title/description order used by the
    /// composer's `ReportActionFailure` producer.
    ///
    /// Neither value is trimmed, normalized, inferred, or otherwise changed.
    /// In particular, empty and Unicode values remain valid and are copied
    /// into the owned fields byte-for-byte.
    #[must_use]
    pub fn new(title: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            title: title.into(),
        }
    }
}
