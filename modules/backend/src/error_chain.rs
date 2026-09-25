//! One-line rendering of a complete error source chain for process diagnostics.
//!
//! Forge errors keep their Display messages short and stage-classified and
//! carry the underlying typed cause as `source()`. Printing only the top
//! message loses that cause, so fatal paths render the whole chain as
//! `top: cause: cause`.
//!
//! The chain only formats Display text that each error type already exposes;
//! Forge error types never format capability, credential, or payload bytes.
//! Peer-supplied text that can reach a transport error (for example a QUIC
//! close reason) is rendered with control characters escaped, so it cannot
//! forge additional log lines.

use std::error::Error;
use std::fmt::{self, Write as _};

/// Displays `error` followed by every `source()` beneath it, joined by `": "`.
///
/// A source whose message the previous message already ends with is skipped,
/// so errors that both embed their source (`"...: {0}"` or `#[error(transparent)]`)
/// and return it from `source()` are not printed twice.
#[derive(Clone, Copy)]
pub struct ErrorChain<'a>(pub &'a (dyn Error + 'static));

impl fmt::Display for ErrorChain<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut previous = self.0.to_string();
        write_escaped(formatter, &previous)?;
        let mut current = self.0.source();
        while let Some(error) = current {
            let message = error.to_string();
            if !message.is_empty() && !previous.ends_with(&message) {
                formatter.write_str(": ")?;
                write_escaped(formatter, &message)?;
            }
            previous = message;
            current = error.source();
        }
        Ok(())
    }
}

fn write_escaped(formatter: &mut fmt::Formatter<'_>, message: &str) -> fmt::Result {
    for character in message.chars() {
        if character.is_control() {
            write!(formatter, "{}", character.escape_default())?;
        } else {
            formatter.write_char(character)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ErrorChain;
    use thiserror::Error;

    #[derive(Debug, Error)]
    #[error("leaf failed\nforged line")]
    struct Leaf;

    #[derive(Debug, Error)]
    #[error("middle failed")]
    struct Middle(#[source] Leaf);

    #[derive(Debug, Error)]
    #[error(transparent)]
    struct Transparent(#[from] Middle);

    #[derive(Debug, Error)]
    #[error("top failed")]
    struct Top(#[source] Transparent);

    #[derive(Debug, Error)]
    #[error("embedding failed: {0}")]
    struct Embedding(#[source] Middle);

    #[test]
    fn renders_every_cause_once_and_escapes_control_characters() {
        let error = Top(Transparent(Middle(Leaf)));
        assert_eq!(
            ErrorChain(&error).to_string(),
            "top failed: middle failed: leaf failed\\nforged line"
        );
    }

    #[test]
    fn skips_a_source_already_embedded_in_its_parent() {
        let error = Embedding(Middle(Leaf));
        assert_eq!(
            ErrorChain(&error).to_string(),
            "embedding failed: middle failed: leaf failed\\nforged line"
        );
    }
}
