//! Global prompt reconciliation across every managed engine home.
//!
//! Synchronous filesystem work, run off the async runtime. Each installed
//! engine home's global file is read (Codex's `AGENTS.override.md` wins
//! over `AGENTS.md`, as in Codex itself). Before setup, the distinct texts
//! become candidates. After it, a home holding the canonical text is synced;
//! one Artisan has not written yet, or still holding what Artisan last wrote,
//! is rewritten (the first takeover backs the old file up); anything else was
//! edited outside Artisan and is reported as drifted, never overwritten.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use artisan_database::StoredPromptFile;
use artisan_domain::{GlobalPromptCandidate, GlobalPromptEngineState, GlobalPromptEngineStatus};
use artisan_native_engine::ManagedEngine;
use artisan_native_engine::extensions::{
    EnginePresence, EngineSlot, PromptFileError, back_up_prompt, move_to_backup, normalize_prompt,
    prompt_digest, read_prompt, remove_prompt, write_prompt,
};

/// One engine home's global prompt as found.
#[derive(Clone, Debug)]
pub(crate) struct HomePrompt {
    pub(crate) engine: ManagedEngine,
    /// The home relative to the engine's toolchain folder.
    pub(crate) home_key: String,
    pub(crate) prompt_file: PathBuf,
    /// Present override files that win over `prompt_file`.
    pub(crate) overrides: Vec<PathBuf>,
    /// The text the engine reads; `None` when it has no file.
    pub(crate) text: Result<Option<String>, PromptFileError>,
}

impl HomePrompt {
    /// The digest of what the engine reads; no file counts as empty.
    fn digest(&self) -> Option<String> {
        self.text
            .as_ref()
            .ok()
            .map(|text| prompt_digest(text.as_deref().unwrap_or("")))
    }
}

/// Reads every installed engine home's global prompt.
pub(crate) fn observe(slots: &[EngineSlot], state_root: &Path) -> Vec<HomePrompt> {
    let mut found = Vec::new();
    for slot in slots {
        if slot.presence != EnginePresence::Installed {
            continue;
        }
        let engine_root = state_root.join("toolchain").join(slot.engine.id());
        for home in &slot.homes {
            let overrides: Vec<PathBuf> = home
                .prompt_overrides
                .iter()
                .filter(|path| path.is_file())
                .cloned()
                .collect();
            let text = match overrides.first() {
                Some(path) => read_prompt(path),
                None => read_prompt(&home.prompt_file),
            };
            found.push(HomePrompt {
                engine: slot.engine,
                home_key: home_key(&engine_root, &home.home),
                prompt_file: home.prompt_file.clone(),
                overrides,
                text,
            });
        }
    }
    found
}

fn home_key(engine_root: &Path, home: &Path) -> String {
    home.strip_prefix(engine_root)
        .ok()
        .and_then(|relative| relative.to_str())
        .map_or_else(|| "home".to_owned(), |relative| relative.replace('\\', "/"))
}

/// The distinct non-empty texts the engines hold, each with its engines.
pub(crate) fn candidates(homes: &[HomePrompt]) -> Vec<GlobalPromptCandidate> {
    let mut by_digest: BTreeMap<String, GlobalPromptCandidate> = BTreeMap::new();
    for home in homes {
        let Ok(Some(text)) = &home.text else {
            continue;
        };
        let normalized = normalize_prompt(text);
        if normalized.is_empty() {
            continue;
        }
        let candidate = by_digest
            .entry(prompt_digest(&normalized))
            .or_insert_with_key(|digest| GlobalPromptCandidate {
                candidate_id: digest.clone(),
                engine_ids: Vec::new(),
                text: normalized,
            });
        let engine_id = home.engine.id().to_owned();
        if !candidate.engine_ids.contains(&engine_id) {
            candidate.engine_ids.push(engine_id);
        }
    }
    by_digest.into_values().collect()
}

/// What happened in one home.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HomeOutcome {
    pub(crate) engine: ManagedEngine,
    pub(crate) home_key: String,
    pub(crate) status: GlobalPromptEngineStatus,
    pub(crate) external_text: Option<String>,
    pub(crate) reason: Option<String>,
    /// The digest now on disk when Artisan wrote it or found it canonical.
    pub(crate) recorded_digest: Option<String>,
}

/// Brings each home in line with `canonical`. `force` names engines whose
/// drifted files the user chose to overwrite.
pub(crate) fn apply(
    homes: &[HomePrompt],
    canonical: &str,
    records: &[StoredPromptFile],
    force: Option<ManagedEngine>,
    backups_dir: &Path,
) -> Vec<HomeOutcome> {
    let canonical_digest = prompt_digest(canonical);
    homes
        .iter()
        .map(|home| {
            let outcome = |status, external_text, reason, recorded_digest| HomeOutcome {
                engine: home.engine,
                home_key: home.home_key.clone(),
                status,
                external_text,
                reason,
                recorded_digest,
            };
            let Some(on_disk) = home.digest() else {
                let reason = match &home.text {
                    Err(error) => error.to_string(),
                    Ok(_) => String::new(),
                };
                return outcome(
                    GlobalPromptEngineStatus::Failed,
                    None,
                    Some(capitalized(&reason)),
                    None,
                );
            };
            if on_disk == canonical_digest && home.overrides.is_empty() {
                return outcome(
                    GlobalPromptEngineStatus::Synced,
                    None,
                    None,
                    Some(canonical_digest.clone()),
                );
            }
            let record = records.iter().find(|record| {
                record.engine_id == home.engine.id() && record.home_key == home.home_key
            });
            let first_takeover = record.is_none();
            let unchanged_since_written =
                record.is_some_and(|record| record.written_digest == on_disk);
            if !(first_takeover || unchanged_since_written || force == Some(home.engine)) {
                let external = home.text.clone().ok().flatten().unwrap_or_default();
                return outcome(
                    GlobalPromptEngineStatus::Drifted,
                    Some(external),
                    None,
                    None,
                );
            }
            match write_home(
                home,
                canonical,
                first_takeover || force == Some(home.engine),
                backups_dir,
            ) {
                Ok(()) => outcome(
                    GlobalPromptEngineStatus::Synced,
                    None,
                    None,
                    Some(canonical_digest.clone()),
                ),
                Err(error) => outcome(
                    GlobalPromptEngineStatus::Failed,
                    None,
                    Some(capitalized(&error.to_string())),
                    None,
                ),
            }
        })
        .collect()
}

/// Writes the canonical prompt to one home, backing the replaced file up
/// first when asked, and retiring override files the engine would read
/// instead.
fn write_home(
    home: &HomePrompt,
    canonical: &str,
    back_up: bool,
    backups_dir: &Path,
) -> Result<(), PromptFileError> {
    let label = format!(
        "{}-{}-prompt",
        home.engine.id(),
        home.home_key.replace('/', "_")
    );
    if back_up && home.prompt_file.is_file() {
        back_up_prompt(&home.prompt_file, backups_dir, &label)?;
    }
    for path in &home.overrides {
        move_to_backup(path, backups_dir, &format!("{label}-override"))
            .map_err(|_| PromptFileError::Unwritable)?;
    }
    if normalize_prompt(canonical).is_empty() {
        remove_prompt(&home.prompt_file)
    } else {
        write_prompt(&home.prompt_file, canonical)
    }
}

/// Folds home outcomes into one state per engine, in slot order.
pub(crate) fn engine_states(
    slots: &[EngineSlot],
    outcomes: &[HomeOutcome],
    awaiting_choice: bool,
) -> Vec<GlobalPromptEngineState> {
    slots
        .iter()
        .map(|slot| {
            let mut state = GlobalPromptEngineState {
                engine_id: slot.engine.id().to_owned(),
                status: GlobalPromptEngineStatus::Synced,
                file: Some(slot.prompt_display.to_owned()),
                external_text: None,
                reason: None,
            };
            match slot.presence {
                EnginePresence::Unavailable(reason) => {
                    state.status = GlobalPromptEngineStatus::Unavailable;
                    state.reason = Some(reason.to_owned());
                    state.file = None;
                    return state;
                }
                EnginePresence::Absent => {
                    state.status = GlobalPromptEngineStatus::EngineAbsent;
                    return state;
                }
                EnginePresence::Installed if awaiting_choice => {
                    state.status = GlobalPromptEngineStatus::AwaitingChoice;
                    return state;
                }
                EnginePresence::Installed => {}
            }
            let rank = |status: GlobalPromptEngineStatus| match status {
                GlobalPromptEngineStatus::Failed => 2,
                GlobalPromptEngineStatus::Drifted => 1,
                _ => 0,
            };
            for outcome in outcomes
                .iter()
                .filter(|outcome| outcome.engine == slot.engine)
            {
                if rank(outcome.status) > rank(state.status) {
                    state.status = outcome.status;
                    state.external_text.clone_from(&outcome.external_text);
                    state.reason.clone_from(&outcome.reason);
                }
            }
            state
        })
        .collect()
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => {
            let mut sentence: String = first.to_uppercase().chain(characters).collect();
            if !sentence.ends_with('.') {
                sentence.push('.');
            }
            sentence
        }
        None => "Artisan could not read the engine's global prompt file.".to_owned(),
    }
}
