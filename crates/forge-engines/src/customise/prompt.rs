//! The global prompt in each engine's own file (master's
//! `extension_sync/prompt.rs`).
//!
//! Forge remembers what it last wrote to each engine home
//! (`<home>/.artisan/global-prompt.sha256`). A file still holding that, or
//! one Forge never wrote, is brought in line (the first takeover backs the
//! old file up); anything else was edited outside Artisan and is reported,
//! never overwritten until the user decides.

use super::{paths, remove_file_if_present, write_if_changed};
use artisan_api::EngineId;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One engine's global prompt file as found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptFile {
    /// The file relative to the engine's home, as the user would look for it.
    pub display: String,
    /// What the engine reads (an override file wins); `None` without a file.
    pub text: Result<Option<String>, String>,
}

/// Where one engine's file stands after [`apply_prompt`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptApplied {
    Synced,
    /// Edited outside Artisan: what it says now.
    Drifted(String),
    /// Why it couldn't be read or written.
    Failed(String),
}

/// Line endings and surrounding blank space don't make two prompts differ.
#[must_use]
pub fn normalize_prompt(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_owned()
}

/// Lowercase hexadecimal SHA-256.
#[must_use]
pub fn prompt_digest(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// Lowercase hexadecimal.
pub(super) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn record_path(home: &Path) -> PathBuf {
    home.join(".artisan").join("global-prompt.sha256")
}

fn read_text(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The file the engine reads: an override (Codex's `AGENTS.override.md`)
/// when present, else its prompt file.
fn source(engine: EngineId, home: &Path) -> Option<(PathBuf, Vec<PathBuf>, PathBuf)> {
    let paths = paths(engine, home)?;
    let overrides: Vec<PathBuf> = paths
        .shadows
        .iter()
        .filter(|path| path.is_file())
        .cloned()
        .collect();
    let read = overrides
        .first()
        .cloned()
        .unwrap_or_else(|| paths.prompt.clone());
    Some((read, overrides, paths.prompt))
}

/// Reads an engine's global prompt; `None` for an engine without one.
#[must_use]
pub fn read_prompt_file(engine: EngineId, home: &Path) -> Option<PromptFile> {
    let (read, _, prompt) = source(engine, home)?;
    Some(PromptFile {
        display: display(home, &prompt),
        text: read_text(&read)
            .map_err(|error| format!("Couldn't read {}: {error}.", read.display())),
    })
}

fn display(home: &Path, path: &Path) -> String {
    path.strip_prefix(home)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Brings an engine's file in line with `canonical` unless it was edited
/// outside Artisan; `force` overwrites such an edit (backed up first).
/// `None` for an engine without a prompt file.
#[must_use]
pub fn apply_prompt(
    engine: EngineId,
    home: &Path,
    canonical: &str,
    force: bool,
) -> Option<PromptApplied> {
    let (read, overrides, prompt) = source(engine, home)?;
    let on_disk = match read_text(&read) {
        Ok(text) => text.unwrap_or_default(),
        Err(error) => {
            return Some(PromptApplied::Failed(format!(
                "Couldn't read {}: {error}.",
                display(home, &read)
            )));
        }
    };
    let record_file = record_path(home);
    if normalize_prompt(&on_disk) == normalize_prompt(canonical) && overrides.is_empty() {
        if let Err(error) = write_if_changed(&record_file, prompt_digest(&on_disk).as_bytes()) {
            tracing::warn!(%engine, file = %record_file.display(), "couldn't record the global prompt: {error}");
        }
        return Some(PromptApplied::Synced);
    }
    let record = fs::read_to_string(&record_file)
        .ok()
        .map(|digest| digest.trim().to_owned());
    let first_takeover = record.is_none();
    let unchanged = record.as_deref() == Some(prompt_digest(&on_disk).as_str());
    if !(first_takeover || unchanged || force) {
        return Some(PromptApplied::Drifted(on_disk));
    }
    let backups = home.join(".artisan").join("backups");
    let stamp = artisan_api::now();
    let written = (|| -> io::Result<()> {
        if (first_takeover || force) && !normalize_prompt(&on_disk).is_empty() {
            back_up(&read, &backups, stamp)?;
        }
        for path in &overrides {
            if path != &read {
                back_up(path, &backups, stamp)?;
            }
            remove_file_if_present(path)?;
        }
        if normalize_prompt(canonical).is_empty() {
            remove_file_if_present(&prompt)?;
            write_if_changed(&record_file, prompt_digest("").as_bytes())
        } else {
            write_if_changed(&prompt, canonical.as_bytes())?;
            write_if_changed(&record_file, prompt_digest(canonical).as_bytes())
        }
    })();
    Some(match written {
        Ok(()) => PromptApplied::Synced,
        Err(error) => PromptApplied::Failed(format!(
            "Couldn't write {}: {error}.",
            display(home, &prompt)
        )),
    })
}

fn back_up(path: &Path, backups: &Path, stamp: artisan_api::Timestamp) -> io::Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    fs::create_dir_all(backups)?;
    let name = path.file_name().map_or_else(
        || "prompt".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    fs::copy(path, backups.join(format!("{name}.{stamp}"))).map(drop)
}
