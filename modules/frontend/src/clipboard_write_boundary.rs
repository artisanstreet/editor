//! Pure clipboard-write intent and capability boundary.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/browser/clipboard.ts`. It owns the caller's
//! exact text value and maps one adapter failure into one typed error. The
//! adapter is supplied by a later host integration; this module does not
//! access a clipboard, browser, runtime, or global state.

#![allow(clippy::module_name_repetitions)]

use std::error::Error;
use std::fmt;

/// One request to write exactly one text value to a clipboard capability.
///
/// The value is owned so an adapter can be invoked without borrowing the
/// caller beyond this operation. Construction and access perform no trimming,
/// normalization, encoding conversion, or other transformation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ClipboardWriteIntent {
    text: String,
}

impl ClipboardWriteIntent {
    /// Creates an intent containing the supplied text verbatim.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// Borrows the exact text that the adapter will receive.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the owned text without changing it.
    #[must_use]
    pub fn into_text(self) -> String {
        self.text
    }
}

impl From<&str> for ClipboardWriteIntent {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for ClipboardWriteIntent {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

/// The host capability that performs one clipboard text write.
///
/// Implementations are deliberately outside this pure boundary. They may
/// use the platform clipboard when a later native integration supplies one,
/// while tests can provide a deterministic fake without any host access.
pub trait ClipboardWriteAdapter {
    /// The unmodified failure value produced by this adapter.
    type Error;

    /// Attempts exactly one text write using the supplied text.
    ///
    /// The adapter receives the intent's exact UTF-8 text. This method does
    /// not prescribe retries, fallback behavior, permissions, or read support.
    ///
    /// # Errors
    ///
    /// Returns the adapter's failure value unchanged. The public
    /// [`write_clipboard_text`] operation wraps that value in
    /// [`ClipboardWriteError`].
    fn write_text(&mut self, text: &str) -> Result<(), Self::Error>;
}

/// Typed failure for one clipboard text-write attempt.
///
/// The adapter's error remains available as a typed value through
/// [`Self::cause`] or [`Self::into_cause`]. When that value implements
/// [`std::error::Error`], it is also exposed through [`Error::source`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ClipboardWriteError<Cause> {
    cause: Cause,
}

impl<Cause> ClipboardWriteError<Cause> {
    /// Wraps one adapter cause without changing or erasing it.
    pub const fn new(cause: Cause) -> Self {
        Self { cause }
    }

    /// Borrows the exact adapter cause carried by this failure.
    #[must_use]
    pub const fn cause(&self) -> &Cause {
        &self.cause
    }

    /// Returns the exact adapter cause carried by this failure.
    #[must_use]
    pub fn into_cause(self) -> Cause {
        self.cause
    }
}

impl<Cause> fmt::Display for ClipboardWriteError<Cause> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("clipboard text write failed")
    }
}

impl<Cause> Error for ClipboardWriteError<Cause>
where
    Cause: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

/// Sends one text-write intent through the supplied adapter.
///
/// The adapter is invoked once with the intent's exact text. A successful
/// adapter result stays `Ok(())`; an adapter failure is wrapped without
/// normalization, retry, fallback, or other side effect.
///
/// # Errors
///
/// Returns [`ClipboardWriteError`] containing the adapter's exact failure
/// value.
pub fn write_clipboard_text<A>(
    adapter: &mut A,
    intent: &ClipboardWriteIntent,
) -> Result<(), ClipboardWriteError<A::Error>>
where
    A: ClipboardWriteAdapter,
{
    adapter
        .write_text(intent.text())
        .map_err(ClipboardWriteError::new)
}
