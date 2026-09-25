//! Forge-pool preferences an older Editor kept in loose files under
//! `<install root>/ui/`: the last-used model (`last-used-model`) and each
//! host's project order (`project-orders/host-<identity>`,
//! `project-orders/local`, or, for a host whose invitation could not be
//! read, `<host home>/ui/project-order`).
//!
//! They belong to the Forge (see `docs/plans/stateless-editor.md`): the
//! Editor reads them once, hands them to the connected Forge, and removes
//! them through [`super::storage`]. Nothing here writes a file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use artisan_domain::{CatalogOptionId, CatalogSelection, ModelFavoriteId, ProjectId};

const LEGACY_DIRECTORY_NAME: &str = "ui";
const MODEL_FILE_NAME: &str = "last-used-model";
const PROJECT_ORDERS_DIRECTORY_NAME: &str = "project-orders";
const FILE_VERSION: i64 = 1;
/// Upper bound for one legacy file; identifiers are short strings.
pub(super) const MAX_FILE_BYTES: u64 = 8 * 1024;

/// The legacy Forge-pool preferences found for one host.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct LegacyForgePreferences {
    /// The last-used model, as catalog identities.
    pub(crate) selection: Option<CatalogSelection>,
    /// The host's project order, most recent first.
    pub(crate) project_order: Vec<ProjectId>,
    /// Every file found, removed once the Forge answered.
    pub(super) files: Vec<PathBuf>,
}

impl LegacyForgePreferences {
    /// Whether any legacy file exists.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Removes the files once the Forge has answered for them. A file that
    /// cannot be removed is reported and left for the next connection.
    pub(crate) fn retire(self) {
        for problem in super::storage::remove_legacy_files(&self.files) {
            eprintln!("editor settings: {problem}");
        }
    }

    /// Legacy preferences found in `files`, for application tests.
    #[cfg(test)]
    pub(crate) fn for_test(
        selection: Option<CatalogSelection>,
        project_order: Vec<ProjectId>,
        files: Vec<PathBuf>,
    ) -> Self {
        Self {
            selection,
            project_order,
            files,
        }
    }
}

/// Finds the legacy preferences under `root` for the host whose credential
/// home is `home` (`None` for this computer).
pub(super) fn find(
    root: &Path,
    home: Option<&Path>,
    read: impl Fn(&Path) -> Option<Vec<u8>>,
) -> LegacyForgePreferences {
    let legacy = root.join(LEGACY_DIRECTORY_NAME);
    let mut found = LegacyForgePreferences::default();
    let model = legacy.join(MODEL_FILE_NAME);
    if let Some(bytes) = read(&model) {
        found.selection = parse_model(&bytes);
        found.files.push(model);
    }
    let order = project_order_path(&legacy.join(PROJECT_ORDERS_DIRECTORY_NAME), home);
    if let Some(bytes) = read(&order) {
        found.project_order = parse_project_order(&bytes).unwrap_or_default();
        found.files.push(order);
    }
    found
}

fn project_order_path(directory: &Path, home: Option<&Path>) -> PathBuf {
    let Some(home) = home else {
        return directory.join("local");
    };
    use artisan_editor_cli::credentials::hosts;
    let identity = hosts::read_private(home, "host.json")
        .and_then(|bytes| hosts::HostInvitation::decode(&bytes))
        .and_then(|invitation| invitation.id());
    match identity {
        Ok(identity) => directory.join(format!("host-{identity}")),
        // The older Editor fell back to a file inside the host's credential
        // home; it is read and removed from there.
        Err(_) => home.join(LEGACY_DIRECTORY_NAME).join("project-order"),
    }
}

fn versioned(bytes: &[u8]) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    (value.get("version")?.as_i64()? == FILE_VERSION).then_some(value)
}

/// The last-used model as the catalog identities the Forge resolves.
fn parse_model(bytes: &[u8]) -> Option<CatalogSelection> {
    let value = versioned(bytes)?;
    let option = |key: &str| {
        value
            .get(key)
            .and_then(|option| option.get("id"))
            .and_then(serde_json::Value::as_str)
            .and_then(|id| CatalogOptionId::parse(id).ok())
    };
    Some(CatalogSelection {
        model_id: ModelFavoriteId::parse(value.get("model_id")?.as_str()?.to_owned()).ok()?,
        profile_id: None,
        reasoning_effort: option("reasoning_effort"),
        speed: option("speed"),
        context_window: option("context_window"),
        permission: option("permission"),
    })
}

/// The project order, most recent first, without repeats.
fn parse_project_order(bytes: &[u8]) -> Option<Vec<ProjectId>> {
    let value = versioned(bytes)?;
    let mut seen = HashSet::new();
    Some(
        value
            .get("projects")?
            .as_array()?
            .iter()
            .filter_map(|identifier| ProjectId::parse(identifier.as_str()?).ok())
            .filter(|project| seen.insert(project.clone()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_used_model_becomes_catalog_identities() {
        let selection = parse_model(
            br#"{"version":1,"engine_id":"codex","model_id":"codex-luna","native_model_id":"gpt-5.6-luna","reasoning_effort":{"id":"effort-id","native_value":"medium"},"context_window":{"id":"ctx-id","native_suffix":"272k"}}"#,
        )
        .expect("model file parses");
        assert_eq!(selection.model_id.as_str(), "codex-luna");
        assert_eq!(selection.profile_id, None);
        assert_eq!(
            selection
                .reasoning_effort
                .as_ref()
                .map(CatalogOptionId::as_str),
            Some("effort-id")
        );
        assert_eq!(
            selection
                .context_window
                .as_ref()
                .map(CatalogOptionId::as_str),
            Some("ctx-id")
        );
        assert_eq!(selection.speed, None);
        for rejected in [
            "",
            "{}",
            r#"{"version":2,"model_id":"m"}"#,
            r#"{"version":1}"#,
        ] {
            assert_eq!(parse_model(rejected.as_bytes()), None, "{rejected}");
        }
    }

    #[test]
    fn the_project_order_keeps_its_first_occurrences() {
        let order = parse_project_order(br#"{"version":1,"projects":["b","a","b"]}"#)
            .expect("order parses");
        assert_eq!(
            order,
            vec![
                ProjectId::parse("b").unwrap(),
                ProjectId::parse("a").unwrap()
            ]
        );
        assert_eq!(parse_project_order(br#"{"version":2,"projects":[]}"#), None);
    }

    #[test]
    fn this_computer_reads_its_local_order_and_the_shared_model() {
        let root = Path::new("/install");
        let files = [
            root.join("ui/last-used-model"),
            root.join("ui/project-orders/local"),
        ];
        let found = find(root, None, |path| {
            files.contains(&path.to_path_buf()).then(|| {
                if path.ends_with("local") {
                    br#"{"version":1,"projects":["p1"]}"#.to_vec()
                } else {
                    br#"{"version":1,"model_id":"codex-luna"}"#.to_vec()
                }
            })
        });
        assert_eq!(found.files, files);
        assert_eq!(found.project_order, vec![ProjectId::parse("p1").unwrap()]);
        assert!(found.selection.is_some());
        assert!(find(root, None, |_| None).is_empty());
    }
}
