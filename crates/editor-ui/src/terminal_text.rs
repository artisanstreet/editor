//! ANSI terminal output as styled text lines.
//!
//! A line-model terminal for dev servers, compilers and test runners, not a
//! full screen grid: full-screen TUIs (`htop`, `vim`) are out of scope. Raw
//! output bytes go through [`TerminalScreen::feed`] in whatever chunks they
//! arrive; escape sequences and UTF-8 code points split across chunks parse
//! exactly as if they had arrived whole. The model is pure data with no
//! `GPUI` dependency; [`highlights`] maps a materialized [`TerminalLine`] onto
//! `StyledText` highlight runs through a themed [`TerminalPalette`].
//!
//! Supported, deliberately small:
//!
//! - SGR: reset, bold, dim, italic, underline, inverse, strikethrough, the 16
//!   base colours plus bright, 256-colour and truecolor (semicolon and colon
//!   forms).
//! - `\r` overwrite, `\t`, backspace, LF as CRLF (cooked-mode output), erase in
//!   line, relative and absolute cursor moves, save and restore cursor.
//! - Clear screen (`CSI 2J`/`CSI 3J`, `ESC c`) drops everything before it so
//!   the view starts fresh; [`TerminalScreen::cleared_earlier`] tells the UI
//!   to say "earlier output cleared".
//! - OSC 8 hyperlinks become a link on the style. Every other OSC, DCS, SOS,
//!   PM and APC string, and every unknown or private CSI, is consumed and
//!   dropped.
//!
//! The parser is hand-rolled instead of depending on `vte`: the subset is
//! small, and the state machine is bounded so no input can panic or grow
//! memory without limit.

mod paint;
mod parser;
mod screen;
mod style;

#[cfg(test)]
mod tests;

pub use paint::{TerminalPalette, highlights};
pub use screen::{DEFAULT_MAX_LINES, MAX_LINE_CHARS, TerminalScreen};
pub use style::{TerminalColor, TerminalLine, TerminalRun, TerminalStyle};
