//! Inline ANSI input is consumed directly. Stored sources use the bounded source viewer.
use artisan_api::{Output, TextContent};
pub fn shell_output(output: Option<&Output>) -> Option<&str> {
    match output {
        Some(Output {
            text: TextContent::Inline(text),
            ..
        }) => Some(text),
        _ => None,
    }
}
