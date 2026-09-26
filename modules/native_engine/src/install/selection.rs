//! The persisted per-engine version selection.
//!
//! `selection.json` is Forge state: `latest` (the default, automatic updates
//! apply) or one exact held version (automatic updates never apply). A
//! missing file means `latest`.

use std::{fmt, path::Path};

use serde::{Deserialize, Serialize};

use crate::io as native_files;
use crate::io::{AtomicReplaceOutcome, NativeFileError};

use super::{
    catalog::ManagedEngine,
    state::{ManagedStateError, engine_file, map_state_file_error, map_state_replace_error},
    version::EngineVersion,
};

const MAX_SELECTION_BYTES: usize = 1024;
const LATEST: &str = "latest";

/// Which version of an engine the Forge keeps active.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineSelection {
    /// Follow the vendor's current release; automatic updates apply.
    Latest,
    /// Hold one exact version; automatic updates never apply.
    Held(EngineVersion),
}

impl EngineSelection {
    /// Parses `latest` or an exact version.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value == LATEST {
            Some(Self::Latest)
        } else {
            EngineVersion::parse(value).map(Self::Held)
        }
    }

    /// Returns whether automatic updates apply.
    #[must_use]
    pub const fn follows_latest(&self) -> bool {
        matches!(self, Self::Latest)
    }
}

impl fmt::Display for EngineSelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Latest => formatter.write_str(LATEST),
            Self::Held(version) => version.fmt(formatter),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SelectionDocument {
    format_version: u32,
    selection: String,
}

/// Reads the selection; a missing file is [`EngineSelection::Latest`].
///
/// # Errors
///
/// Returns [`ManagedStateError`] when the file is unsafe, oversized, or not
/// a valid selection document.
pub fn read_selection(
    engine_root: &Path,
    engine: ManagedEngine,
) -> Result<EngineSelection, ManagedStateError> {
    let path = engine_file(engine_root, engine, "selection.json")?;
    let bytes = match native_files::read_bounded(&path, MAX_SELECTION_BYTES) {
        Ok(bytes) => bytes,
        Err(NativeFileError::NotFound) => return Ok(EngineSelection::Latest),
        Err(error) => return Err(map_state_file_error(error)),
    };
    let document: SelectionDocument =
        serde_json::from_slice(&bytes).map_err(|_| ManagedStateError::Malformed)?;
    if document.format_version != 1 {
        return Err(ManagedStateError::UnsupportedVersion);
    }
    EngineSelection::parse(&document.selection).ok_or(ManagedStateError::Malformed)
}

/// Atomically publishes the selection.
///
/// # Errors
///
/// Returns [`ManagedStateError`] when the destination is unsafe or the
/// atomic publication fails.
pub fn write_selection(
    engine_root: &Path,
    engine: ManagedEngine,
    selection: &EngineSelection,
) -> Result<AtomicReplaceOutcome, ManagedStateError> {
    let path = engine_file(engine_root, engine, "selection.json")?;
    let bytes = serde_json::to_vec(&SelectionDocument {
        format_version: 1,
        selection: selection.to_string(),
    })
    .map_err(|_| ManagedStateError::Encode)?;
    native_files::replace_file(&path, &bytes).map_err(map_state_replace_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn selection_defaults_to_latest_and_round_trips_a_held_version() {
        let root = tempfile::tempdir().unwrap();
        let engine_root = root.path().join("toolchain").join("claude");
        fs::create_dir_all(&engine_root).unwrap();
        assert_eq!(
            read_selection(&engine_root, ManagedEngine::Claude),
            Ok(EngineSelection::Latest)
        );
        let held = EngineSelection::parse("2.1.282").unwrap();
        let _committed = write_selection(&engine_root, ManagedEngine::Claude, &held).unwrap();
        assert_eq!(
            read_selection(&engine_root, ManagedEngine::Claude),
            Ok(held)
        );
        let _committed = write_selection(
            &engine_root,
            ManagedEngine::Claude,
            &EngineSelection::Latest,
        )
        .unwrap();
        assert_eq!(
            read_selection(&engine_root, ManagedEngine::Claude),
            Ok(EngineSelection::Latest)
        );
    }

    #[test]
    fn selection_rejects_other_engines_roots_and_malformed_documents() {
        let root = tempfile::tempdir().unwrap();
        let engine_root = root.path().join("toolchain").join("claude");
        fs::create_dir_all(&engine_root).unwrap();
        assert_eq!(
            read_selection(&engine_root, ManagedEngine::Codex),
            Err(ManagedStateError::InvalidRoot)
        );
        for malformed in [
            r#"{"format_version":1,"selection":"newest"}"#,
            r#"{"format_version":1,"selection":"latest","extra":1}"#,
            r"not json",
        ] {
            fs::write(engine_root.join("selection.json"), malformed).unwrap();
            assert_eq!(
                read_selection(&engine_root, ManagedEngine::Claude),
                Err(ManagedStateError::Malformed)
            );
        }
        fs::write(
            engine_root.join("selection.json"),
            r#"{"format_version":2,"selection":"latest"}"#,
        )
        .unwrap();
        assert_eq!(
            read_selection(&engine_root, ManagedEngine::Claude),
            Err(ManagedStateError::UnsupportedVersion)
        );
    }
}
