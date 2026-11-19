use super::{paths, write_if_changed};
use artisan_api::EngineId;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptFile {
    pub display: String,
    pub text: Result<Option<String>, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptApplied {
    Synced,
    Drifted(String),
    Failed(String),
}

#[must_use]
pub fn normalize_prompt(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_owned()
}

#[must_use]
pub fn prompt_digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

const PROMPT_MODE: u32 = 0o644;

fn relative(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home).map(Path::to_str) {
        Ok(Some(relative)) => relative.to_owned(),
        Ok(None) | Err(_) => unreachable!(
            "invariant: the global prompt {} lies in the engine home {} and is named in UTF-8",
            path.display(),
            home.display()
        ),
    }
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

#[must_use]
pub fn read_prompt_file(engine: EngineId, home: &Path) -> Option<PromptFile> {
    let (read, _, _) = source(engine, home)?;
    Some(PromptFile {
        display: display(home, &read),
        text: read_text(&read)
            .map_err(|error| format!("Couldn't read {}: {error}.", read.display())),
    })
}

#[must_use]
pub fn inspect_prompt(engine: EngineId, home: &Path, canonical: &str) -> Option<PromptApplied> {
    let (read, overrides, _) = source(engine, home)?;
    Some(match read_text(&read).map(Option::unwrap_or_default) {
        Ok(text)
            if overrides.is_empty() && normalize_prompt(&text) == normalize_prompt(canonical) =>
        {
            PromptApplied::Synced
        }
        Ok(text) => PromptApplied::Drifted(text),
        Err(error) => PromptApplied::Failed(format!("Couldn't read {}: {error}.", read.display())),
    })
}

fn display(home: &Path, path: &Path) -> String {
    path.strip_prefix(home)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyPromptError {
    #[error("reading the {engine} global prompt {}", path.display())]
    Read {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("reading the {engine} global prompt record {}", path.display())]
    ReadRecord {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("writing the {engine} global prompt {}", path.display())]
    Write {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("writing the {engine} global prompt {}", path.display())]
    Guard {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: crate::sandbox::GuardError,
    },
}

impl ApplyPromptError {
    fn reason(&self, home: &Path) -> String {
        match self {
            Self::Read { path, source, .. } | Self::ReadRecord { path, source, .. } => {
                format!("Couldn't read {}: {source}.", display(home, path))
            }
            Self::Write { path, source, .. } => {
                format!("Couldn't write {}: {source}.", display(home, path))
            }
            Self::Guard { path, source, .. } => {
                format!(
                    "Couldn't write {}: {}.",
                    display(home, path),
                    diagnostics::chain(source)
                )
            }
        }
    }
}

pub(super) enum Reconciled {
    Synced,
    Drifted,
}

#[must_use]
pub fn apply_prompt(
    engine: EngineId,
    home: &Path,
    canonical: &str,
    force: bool,
) -> Option<PromptApplied> {
    Some(match reconcile(engine, home, canonical, force)? {
        Ok((Reconciled::Synced, _)) => PromptApplied::Synced,
        Ok((Reconciled::Drifted, on_disk)) => PromptApplied::Drifted(on_disk),
        Err(error) => PromptApplied::Failed(error.reason(home)),
    })
}

fn read_record(engine: EngineId, path: &Path) -> Result<Option<String>, ApplyPromptError> {
    read_text(path)
        .map(|record| record.map(|digest| digest.trim().to_owned()))
        .map_err(|source| ApplyPromptError::ReadRecord {
            engine,
            path: path.to_owned(),
            source,
        })
}

pub(super) fn reconcile(
    engine: EngineId,
    home: &Path,
    canonical: &str,
    force: bool,
) -> Option<Result<(Reconciled, String), ApplyPromptError>> {
    let (read, overrides, prompt) = source(engine, home)?;
    Some((|| {
        let on_disk = read_text(&read)
            .map_err(|source| ApplyPromptError::Read {
                engine,
                path: read.clone(),
                source,
            })?
            .unwrap_or_default();
        let record_file = record_path(home);
        if normalize_prompt(&on_disk) == normalize_prompt(canonical) && overrides.is_empty() {
            if let Err(error) = write_if_changed(&record_file, prompt_digest(&on_disk).as_bytes()) {
                tracing::warn!(%engine, file = %record_file.display(), "couldn't record the global prompt: {error}");
            }
            return Ok((Reconciled::Synced, on_disk));
        }
        let record = read_record(engine, &record_file)?;
        let first_takeover = record.is_none();
        let unchanged = record.as_deref() == Some(prompt_digest(&on_disk).as_str());
        if !(first_takeover || unchanged || force) {
            return Ok((Reconciled::Drifted, on_disk));
        }
        let backups = home.join(".artisan").join("backups");
        let stamp = artisan_api::now();
        if (first_takeover || force) && !normalize_prompt(&on_disk).is_empty() {
            back_up(&read, &backups, stamp).map_err(|source| ApplyPromptError::Write {
                engine,
                path: read.clone(),
                source,
            })?;
        }
        let guarding = |path: &Path| {
            let path = path.to_path_buf();
            move |source| ApplyPromptError::Guard {
                engine,
                path,
                source,
            }
        };
        for path in &overrides {
            if path != &read {
                back_up(path, &backups, stamp).map_err(|source| ApplyPromptError::Write {
                    engine,
                    path: path.clone(),
                    source,
                })?;
            }
            crate::sandbox::guard::remove(engine, home, &relative(home, path))
                .map_err(guarding(path))?;
        }
        let recorded = if normalize_prompt(canonical).is_empty() {
            crate::sandbox::guard::remove(engine, home, &relative(home, &prompt))
                .map_err(guarding(&prompt))?;
            prompt_digest("")
        } else {
            crate::sandbox::guard::write(
                engine,
                home,
                &relative(home, &prompt),
                canonical.as_bytes(),
                PROMPT_MODE,
            )
            .map_err(guarding(&prompt))?;
            prompt_digest(canonical)
        };
        write_if_changed(&record_file, recorded.as_bytes()).map_err(|source| {
            ApplyPromptError::Write {
                engine,
                path: record_file.clone(),
                source,
            }
        })?;
        Ok((Reconciled::Synced, on_disk))
    })())
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

#[cfg(test)]
#[path = "../../tests/customise/prompt/inspection.rs"]
mod inspection_tests;

#[cfg(test)]
#[path = "../../tests/customise/prompt.rs"]
mod ownership_tests;
