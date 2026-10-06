//! Engine global prompt files.
//!
//! Reads are bounded and refuse links; writes go through the atomic replace
//! the rest of the Forge uses. Texts are compared after normalization (line
//! endings, a byte-order mark, and surrounding whitespace), so an engine or
//! editor that only reflows line endings is not mistaken for an edit.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

use super::skill::hex;
use crate::io::NativeFileError;

/// Reads a global prompt file; `None` when it does not exist.
///
/// # Errors
///
/// Returns [`PromptFileError`] when the file is a link, too large, not
/// UTF-8, or unreadable.
pub fn read_prompt(path: &Path) -> Result<Option<String>, PromptFileError> {
    match crate::io::read_bounded(path, artisan_domain::GLOBAL_PROMPT_MAX_BYTES) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| PromptFileError::NotText),
        Err(NativeFileError::NotFound) => Ok(None),
        Err(NativeFileError::TooLarge) => Err(PromptFileError::TooLarge),
        Err(_) => Err(PromptFileError::Unreadable),
    }
}

/// Writes `text` as the global prompt file, creating its folder. A
/// non-empty text always ends with one line break.
///
/// # Errors
///
/// Returns [`PromptFileError::Unwritable`] when the file cannot be written.
pub fn write_prompt(path: &Path, text: &str) -> Result<(), PromptFileError> {
    let parent = path.parent().ok_or(PromptFileError::Unwritable)?;
    fs::create_dir_all(parent).map_err(|_| PromptFileError::Unwritable)?;
    let mut contents = text.trim_end().to_owned();
    if !contents.is_empty() {
        contents.push('\n');
    }
    crate::io::replace_file(path, contents.as_bytes())
        .map(|_| ())
        .map_err(|_| PromptFileError::Unwritable)
}

/// Removes a global prompt file when it exists.
///
/// # Errors
///
/// Returns [`PromptFileError::Unwritable`] when it cannot be removed.
pub fn remove_prompt(path: &Path) -> Result<(), PromptFileError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PromptFileError::Unwritable),
    }
}

/// Copies a global prompt file into `backups_dir` before Artisan first
/// replaces it, and returns the copy's location.
///
/// # Errors
///
/// Returns [`PromptFileError::Unwritable`] when the copy cannot be made.
pub fn back_up_prompt(
    path: &Path,
    backups_dir: &Path,
    label: &str,
) -> Result<PathBuf, PromptFileError> {
    fs::create_dir_all(backups_dir).map_err(|_| PromptFileError::Unwritable)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let destination = backups_dir.join(format!("{millis}-{label}.md"));
    fs::copy(path, &destination).map_err(|_| PromptFileError::Unwritable)?;
    Ok(destination)
}

/// Normalizes a prompt for comparison: no byte-order mark, `\n` line
/// endings, no surrounding whitespace.
#[must_use]
pub fn normalize_prompt(text: &str) -> String {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n")
        .trim()
        .to_owned()
}

/// Lowercase hexadecimal SHA-256 of the normalized prompt.
#[must_use]
pub fn prompt_digest(text: &str) -> String {
    hex(&Sha256::digest(normalize_prompt(text).as_bytes()))
}

/// Why a global prompt file could not be used.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum PromptFileError {
    #[error("the global prompt file is a link or cannot be read")]
    Unreadable,
    #[error("the global prompt file is larger than Artisan's limit")]
    TooLarge,
    #[error("the global prompt file is not UTF-8 text")]
    NotText,
    #[error("the global prompt file cannot be written")]
    Unwritable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_round_trip_and_normalize() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".codex").join("AGENTS.md");
        assert_eq!(read_prompt(&file), Ok(None));
        write_prompt(&file, "Be brief.\r\n\n").unwrap();
        assert_eq!(read_prompt(&file).unwrap().as_deref(), Some("Be brief.\n"));
        assert_eq!(
            prompt_digest("\u{feff}Be brief.\r\n"),
            prompt_digest("Be brief.")
        );
        assert_ne!(prompt_digest("Be brief."), prompt_digest("Be thorough."));
        let backup = back_up_prompt(&file, &home.path().join("backups"), "codex").unwrap();
        assert_eq!(fs::read_to_string(backup).unwrap(), "Be brief.\n");
        remove_prompt(&file).unwrap();
        remove_prompt(&file).unwrap();
        assert_eq!(read_prompt(&file), Ok(None));
    }

    #[test]
    fn oversized_and_binary_prompts_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let large = home.path().join("large.md");
        fs::write(
            &large,
            "a".repeat(artisan_domain::GLOBAL_PROMPT_MAX_BYTES + 1),
        )
        .unwrap();
        assert_eq!(read_prompt(&large), Err(PromptFileError::TooLarge));
        let binary = home.path().join("binary.md");
        fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(read_prompt(&binary), Err(PromptFileError::NotText));
    }
}
