mod parser;
mod screen;
mod style;

pub use screen::{DEFAULT_MAX_LINES, MAX_LINE_CHARS, TerminalScreen};
pub use style::{TerminalColor, TerminalLine, TerminalRun, TerminalStyle};
