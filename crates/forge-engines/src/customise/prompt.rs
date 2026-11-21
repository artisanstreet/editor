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

const RECORD: &str = "global-prompt.sha256";
const LEGACY_RECORD: &str = ".artisan/global-prompt.sha256";
const BACKUPS: &str = "backups";

fn record_path(home: &Path) -> PathBuf {
    crate::home::forge_records(home).join(RECORD)
}

fn in_home<'path>(home: &Path, path: &'path Path) -> &'path Path {
    path.strip_prefix(home).unwrap_or_else(|_| {
        unreachable!(
            "invariant: the global prompt {} lies in the engine home {}",
            path.display(),
            home.display()
        )
    })
}

fn read_text(home: &Path, path: &Path) -> io::Result<Option<String>> {
    crate::home::read(home, in_home(home, path))?
        .map(|bytes| {
            String::from_utf8(bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        })
        .transpose()
}

fn present(home: &Path, path: &Path) -> bool {
    !matches!(crate::home::read(home, in_home(home, path)), Ok(None))
}

fn source(engine: EngineId, home: &Path) -> Option<(PathBuf, Vec<PathBuf>, PathBuf)> {
    let paths = paths(engine, home)?;
    let overrides: Vec<PathBuf> = paths
        .shadows
        .iter()
        .filter(|path| present(home, path))
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
        text: read_text(home, &read)
            .map_err(|error| format!("Couldn't read {}: {error}.", read.display())),
    })
}

#[must_use]
pub fn inspect_prompt(engine: EngineId, home: &Path, canonical: &str) -> Option<PromptApplied> {
    let (read, overrides, _) = source(engine, home)?;
    Some(
        match read_text(home, &read).map(Option::unwrap_or_default) {
            Ok(text)
                if overrides.is_empty()
                    && normalize_prompt(&text) == normalize_prompt(canonical) =>
            {
                PromptApplied::Synced
            }
            Ok(text) => PromptApplied::Drifted(text),
            Err(error) => {
                PromptApplied::Failed(format!("Couldn't read {}: {error}.", read.display()))
            }
        },
    )
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
    Config {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: crate::home::ConfigError,
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
            Self::Config { path, source, .. } => {
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

fn read_record(engine: EngineId, home: &Path) -> Result<Option<String>, ApplyPromptError> {
    let path = record_path(home);
    let recorded = match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            crate::home::read(home, Path::new(LEGACY_RECORD)).map_err(|source| {
                ApplyPromptError::ReadRecord {
                    engine,
                    path: home.join(LEGACY_RECORD),
                    source,
                }
            })
        }
        Err(source) => Err(ApplyPromptError::ReadRecord {
            engine,
            path,
            source,
        }),
    }?;
    Ok(recorded.map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned()))
}

pub(super) fn reconcile(
    engine: EngineId,
    home: &Path,
    canonical: &str,
    force: bool,
) -> Option<Result<(Reconciled, String), ApplyPromptError>> {
    let (read, overrides, prompt) = source(engine, home)?;
    Some((|| {
        let on_disk = read_text(home, &read)
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
        let record = read_record(engine, home)?;
        let first_takeover = record.is_none();
        let unchanged = record.as_deref() == Some(prompt_digest(&on_disk).as_str());
        if !(first_takeover || unchanged || force) {
            return Ok((Reconciled::Drifted, on_disk));
        }
        let backups = crate::home::forge_records(home).join(BACKUPS);
        let stamp = artisan_api::now();
        if (first_takeover || force) && !normalize_prompt(&on_disk).is_empty() {
            back_up(&read, on_disk.as_bytes(), &backups, stamp).map_err(|source| {
                ApplyPromptError::Write {
                    engine,
                    path: read.clone(),
                    source,
                }
            })?;
        }
        let configuring = |path: &Path| {
            let path = path.to_path_buf();
            move |source| ApplyPromptError::Config {
                engine,
                path,
                source,
            }
        };
        for path in &overrides {
            if path != &read {
                let shadowed = read_text(home, path)
                    .map_err(|source| ApplyPromptError::Read {
                        engine,
                        path: path.clone(),
                        source,
                    })?
                    .unwrap_or_default();
                back_up(path, shadowed.as_bytes(), &backups, stamp).map_err(|source| {
                    ApplyPromptError::Write {
                        engine,
                        path: path.clone(),
                        source,
                    }
                })?;
            }
            crate::home::remove_config(engine, home, in_home(home, path))
                .map_err(configuring(path))?;
        }
        let recorded = if normalize_prompt(canonical).is_empty() {
            crate::home::remove_config(engine, home, in_home(home, &prompt))
                .map_err(configuring(&prompt))?;
            prompt_digest("")
        } else {
            crate::home::write_config(
                engine,
                home,
                in_home(home, &prompt),
                canonical.as_bytes(),
                PROMPT_MODE,
            )
            .map_err(configuring(&prompt))?;
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

fn back_up(
    path: &Path,
    text: &[u8],
    backups: &Path,
    stamp: artisan_api::Timestamp,
) -> io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(backups)?;
    let name = path.file_name().map_or_else(
        || "prompt".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    fs::write(backups.join(format!("{name}.{stamp}")), text)
}

#[cfg(test)]
#[path = "../../tests/customise/prompt/inspection.rs"]
mod inspection_tests;

#[cfg(test)]
#[path = "../../tests/customise/prompt.rs"]
mod ownership_tests;
