//! Last-used composer model, Forge host, and project order preferences.
//!
//! Per-thread engine configuration stays authoritative: a thread with a saved
//! run configuration always shows and sends with exactly that. These
//! preferences only seed the choice where nothing durable exists yet — a new
//! thread with no saved model starts from the last-used model instead of the
//! catalog default, and a fresh launch selects the last-used host instead of
//! always opening the local machine.
//!
//! Storage mirrors the frame-rate preference: one small file per preference
//! under `<artisan home>/ui/`, written atomically through a pending sibling.
//! Every read is bounded and every failure falls back to current behavior
//! (no preference), so a missing, corrupt, or stale file can never break
//! startup or sending.

use crate::native_model_catalog::{
    NativeContextSelection, NativeModelCatalog, NativeModelPolicy, NativeModelSelection,
    NativeOptionValue,
};
use artisan_domain::ProjectId;
use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};

const MODEL_FILE_NAME: &str = "last-used-model";
const HOST_FILE_NAME: &str = "last-used-host";
const MODEL_FILE_VERSION: i64 = 1;
const PROJECT_ORDER_FILE_VERSION: i64 = 1;
/// Upper bound for one preference file; ids are short strings.
const MAX_FILE_BYTES: u64 = 8 * 1024;
/// Upper bound for one persisted identifier.
const MAX_FIELD_CHARS: usize = 512;

fn ui_path(file_name: &str) -> Option<PathBuf> {
    artisan_editor_cli::paths::Layout::discover()
        .ok()
        .map(|layout| layout.root.join("ui").join(file_name))
}

fn save_bytes(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let pending = path.with_extension("pending");
    if std::fs::write(&pending, bytes).is_err() {
        return;
    }
    let _ = std::fs::rename(pending, path);
}

fn load_bytes(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    std::io::Read::take(file, MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    Some(bytes)
}

/// The persisted identity of one committed model policy.
///
/// Only catalog-stable identifiers are stored: the catalog revision and the
/// host profile scope are deliberately excluded so [`restore_policy`] always
/// revalidates against the live snapshot and the current scope instead of
/// pinning a stale revision or another host's profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StoredModelPolicy {
    engine_id: String,
    model_id: String,
    native_model_id: String,
    route_id: Option<String>,
    variant_id: Option<String>,
    reasoning_effort: Option<(String, String)>,
    speed: Option<(String, String)>,
    context_window: Option<(String, String)>,
    permission: Option<(String, String)>,
}

impl StoredModelPolicy {
    fn from_policy(policy: &NativeModelPolicy) -> Option<Self> {
        Some(Self {
            engine_id: bounded_id(&policy.engine_id)?,
            model_id: bounded_id(&policy.model_id)?,
            native_model_id: bounded_id(&policy.native_model_id)?,
            route_id: match policy.native_selection.as_ref() {
                None => None,
                Some(selection) => Some(bounded_id(&selection.provider_route_id)?),
            },
            variant_id: match policy
                .native_selection
                .as_ref()
                .and_then(|selection| selection.variant_id.as_ref())
            {
                None => None,
                Some(variant) => Some(bounded_id(variant)?),
            },
            reasoning_effort: match policy.reasoning_effort.as_ref() {
                None => None,
                Some(value) => Some((bounded_id(&value.id)?, bounded_value(&value.native_value)?)),
            },
            speed: match policy.speed.as_ref() {
                None => None,
                Some(value) => Some((bounded_id(&value.id)?, bounded_value(&value.native_value)?)),
            },
            context_window: match policy.context_window.as_ref() {
                None => None,
                Some(value) => Some((bounded_id(&value.id)?, bounded_value(&value.native_suffix)?)),
            },
            permission: match policy.permission.as_ref() {
                None => None,
                Some(value) => Some((bounded_id(&value.id)?, bounded_value(&value.native_value)?)),
            },
        })
    }
}

fn bounded_id(value: &str) -> Option<String> {
    bounded_text(value, false)
}

fn bounded_value(value: &str) -> Option<String> {
    bounded_text(value, true)
}

/// Copies one persisted string when it is non-empty and bounded.
///
/// Identifiers reject surrounding whitespace (catalog ids are exact); native
/// values keep theirs verbatim because providers compare them exactly.
fn bounded_text(value: &str, keep_whitespace: bool) -> Option<String> {
    let checked = if keep_whitespace { value } else { value.trim() };
    if checked.is_empty() || checked.chars().count() > MAX_FIELD_CHARS {
        return None;
    }
    Some(checked.to_owned())
}

/// Records one committed model policy as the last-used preference.
///
/// Best-effort: storage failures are ignored because a preference must never
/// surface as send-blocking UI.
pub(crate) fn save_model_policy(policy: &NativeModelPolicy) -> Option<StoredModelPolicy> {
    let stored = StoredModelPolicy::from_policy(policy)?;
    let path = ui_path(MODEL_FILE_NAME)?;
    save_bytes(&path, stored.to_json().as_bytes());
    Some(stored)
}

/// Reads the last-used model preference, if one is stored and well-formed.
pub(crate) fn load_stored_model() -> Option<StoredModelPolicy> {
    let path = ui_path(MODEL_FILE_NAME)?;
    let bytes = load_bytes(&path)?;
    StoredModelPolicy::from_json(&bytes)
}

/// Revalidates one stored preference against the live catalog snapshot.
///
/// Returns a current-revision policy when the stored model still exists with
/// the same engine and native identity; stale options are dropped by the
/// shared rebase instead of failing the whole preference.
pub(crate) fn restore_policy(
    snapshot: &NativeModelCatalog,
    stored: &StoredModelPolicy,
) -> Option<NativeModelPolicy> {
    snapshot.rebase_policy(&stored.candidate_for_restore())
}

impl StoredModelPolicy {
    pub(crate) fn candidate_for_restore(&self) -> NativeModelPolicy {
        NativeModelPolicy {
            catalog_revision: String::new(),
            profile_id: None,
            engine_id: self.engine_id.clone(),
            model_id: self.model_id.clone(),
            native_model_id: self.native_model_id.clone(),
            native_selection: self.route_id.as_ref().map(|route_id| NativeModelSelection {
                model_id: self.native_model_id.clone(),
                provider_route_id: route_id.clone(),
                variant_id: self.variant_id.clone(),
            }),
            reasoning_effort: self
                .reasoning_effort
                .as_ref()
                .map(|(id, value)| NativeOptionValue {
                    id: id.clone(),
                    native_value: value.clone(),
                }),
            speed: self.speed.as_ref().map(|(id, value)| NativeOptionValue {
                id: id.clone(),
                native_value: value.clone(),
            }),
            context_window: self.context_window.as_ref().map(|(id, suffix)| {
                NativeContextSelection {
                    id: id.clone(),
                    native_suffix: suffix.clone(),
                    native_config: None,
                }
            }),
            permission: self
                .permission
                .as_ref()
                .map(|(id, value)| NativeOptionValue {
                    id: id.clone(),
                    native_value: value.clone(),
                }),
        }
    }

    fn to_json(&self) -> String {
        let mut value = serde_json::json!({
            "version": MODEL_FILE_VERSION,
            "engine_id": self.engine_id,
            "model_id": self.model_id,
            "native_model_id": self.native_model_id,
        });
        if let Some(route_id) = &self.route_id {
            value["route_id"] = serde_json::Value::String(route_id.clone());
        }
        if let Some(variant_id) = &self.variant_id {
            value["variant_id"] = serde_json::Value::String(variant_id.clone());
        }
        if let Some((id, native_value)) = &self.reasoning_effort {
            value["reasoning_effort"] = option_json(id, native_value);
        }
        if let Some((id, native_value)) = &self.speed {
            value["speed"] = option_json(id, native_value);
        }
        if let Some((id, suffix)) = &self.context_window {
            value["context_window"] = serde_json::json!({"id": id, "native_suffix": suffix});
        }
        if let Some((id, native_value)) = &self.permission {
            value["permission"] = option_json(id, native_value);
        }
        value.to_string()
    }

    fn from_json(bytes: &[u8]) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        if value.get("version")?.as_i64()? != MODEL_FILE_VERSION {
            return None;
        }
        let stored = Self {
            engine_id: bounded_id(value.get("engine_id")?.as_str()?)?,
            model_id: bounded_id(value.get("model_id")?.as_str()?)?,
            native_model_id: bounded_id(value.get("native_model_id")?.as_str()?)?,
            route_id: optional_id(&value, "route_id").ok()?,
            variant_id: optional_id(&value, "variant_id").ok()?,
            reasoning_effort: optional_option(&value, "reasoning_effort", "native_value").ok()?,
            speed: optional_option(&value, "speed", "native_value").ok()?,
            context_window: optional_option(&value, "context_window", "native_suffix").ok()?,
            permission: optional_option(&value, "permission", "native_value").ok()?,
        };
        Some(stored)
    }
}

fn option_json(id: &str, native_value: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "native_value": native_value})
}

fn optional_id(value: &serde_json::Value, key: &str) -> Result<Option<String>, ()> {
    let Some(field) = value.get(key) else {
        return Ok(None);
    };
    if field.is_null() {
        return Ok(None);
    }
    let Some(text) = field.as_str() else {
        return Err(());
    };
    Ok(Some(bounded_id(text).ok_or(())?))
}

fn optional_option(
    value: &serde_json::Value,
    key: &str,
    native_key: &str,
) -> Result<Option<(String, String)>, ()> {
    let Some(object) = value.get(key) else {
        return Ok(None);
    };
    if object.is_null() {
        return Ok(None);
    }
    if !object.is_object() {
        return Err(());
    }
    let Some(id) = object.get("id").and_then(serde_json::Value::as_str) else {
        return Err(());
    };
    let Some(native) = object.get(native_key).and_then(serde_json::Value::as_str) else {
        return Err(());
    };
    Ok(Some((
        bounded_id(id).ok_or(())?,
        bounded_value(native).ok_or(())?,
    )))
}

/// Records the selected Forge host: `None` is the local machine, `Some` is a
/// registered remote host home. Best-effort like the model preference.
pub(crate) fn save_host(home: Option<&Path>) {
    let Some(path) = ui_path(HOST_FILE_NAME) else {
        return;
    };
    match home {
        None => save_bytes(&path, &[]),
        Some(home) => save_bytes(&path, home.as_os_str().as_encoded_bytes()),
    }
}

/// Reads the last-used host: `None` means the local machine (or no
/// preference is stored), `Some` is a registered remote host home.
pub(crate) fn load_host() -> Option<PathBuf> {
    let path = ui_path(HOST_FILE_NAME)?;
    let bytes = load_bytes(&path)?;
    if bytes.is_empty() || bytes.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    Some(PathBuf::from(text))
}

fn project_order_path_in(ui_directory: &Path, home: Option<&Path>) -> PathBuf {
    let Some(home) = home else {
        return ui_directory.join("local");
    };
    use artisan_editor_cli::credentials::hosts;
    let identity = hosts::read_private(home, "host.json")
        .and_then(|bytes| hosts::HostInvitation::decode(&bytes))
        .and_then(|invitation| invitation.id());
    match identity {
        Ok(identity) => ui_directory.join(format!("host-{identity}")),
        Err(_) => home.join("ui").join("project-order"),
    }
}

fn project_order_path(home: Option<&Path>) -> Option<PathBuf> {
    Some(project_order_path_in(&ui_path("project-orders")?, home))
}

/// Records project cycling order for this Forge, retaining the leading entries
/// that fit the bounded preference file. Remote identities survive reconnects.
pub(crate) fn save_project_order(home: Option<&Path>, projects: &[ProjectId]) {
    let Some(path) = project_order_path(home) else {
        return;
    };
    save_bytes(&path, project_order_json(projects).as_bytes());
}

/// Reads the saved order for this Forge. Unavailable or malformed preferences
/// leave the caller free to use the current project listing order.
pub(crate) fn load_project_order(home: Option<&Path>) -> Vec<ProjectId> {
    project_order_path(home)
        .and_then(|path| load_bytes(&path))
        .and_then(|bytes| parse_project_order(&bytes))
        .unwrap_or_default()
}

fn project_order_json(projects: &[ProjectId]) -> String {
    let mut identifiers = Vec::new();
    let mut seen = HashSet::new();
    let mut encoded_length = serde_json::json!({
        "version": PROJECT_ORDER_FILE_VERSION,
        "projects": [],
    })
    .to_string()
    .len();
    for project in projects {
        if !seen.insert(project) {
            continue;
        }
        let identifier = serde_json::Value::String(project.as_str().to_owned());
        let additional_length = identifier.to_string().len() + usize::from(!identifiers.is_empty());
        if encoded_length + additional_length > MAX_FILE_BYTES as usize {
            break;
        }
        encoded_length += additional_length;
        identifiers.push(identifier);
    }
    serde_json::json!({
        "version": PROJECT_ORDER_FILE_VERSION,
        "projects": identifiers,
    })
    .to_string()
}

fn parse_project_order(bytes: &[u8]) -> Option<Vec<ProjectId>> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if value.get("version")?.as_i64()? != PROJECT_ORDER_FILE_VERSION {
        return None;
    }
    let mut projects = Vec::new();
    let mut seen = HashSet::new();
    for identifier in value.get("projects")?.as_array()? {
        let project = ProjectId::parse(identifier.as_str()?).ok()?;
        if seen.insert(project.clone()) {
            projects.push(project);
        }
    }
    Some(projects)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored_policy() -> StoredModelPolicy {
        StoredModelPolicy {
            engine_id: "codex".into(),
            model_id: "codex-luna".into(),
            native_model_id: "gpt-5.6-luna".into(),
            route_id: Some("route-a".into()),
            variant_id: Some("medium".into()),
            reasoning_effort: Some(("effort-id".into(), "medium".into())),
            speed: None,
            context_window: Some(("ctx-id".into(), "272k".into())),
            permission: Some(("perm-id".into(), "read-write".into())),
        }
    }

    #[test]
    fn model_preference_round_trips_through_json() {
        let stored = stored_policy();
        let parsed = StoredModelPolicy::from_json(stored.to_json().as_bytes()).expect("round trip");
        assert_eq!(parsed, stored);
    }

    #[test]
    fn model_preference_rejects_foreign_versions_and_garbage() {
        let stored = stored_policy();
        let mut foreign: serde_json::Value =
            serde_json::from_str(&stored.to_json()).expect("valid json");
        foreign["version"] = serde_json::json!(2);
        assert!(StoredModelPolicy::from_json(foreign.to_string().as_bytes()).is_none());
        for garbage in [
            "",
            "{",
            "{}",
            "[]",
            "null",
            r#"{"version":1}"#,
            r#"{"version":1,"engine_id":"","model_id":"x","native_model_id":"y"}"#,
            r#"{"version":1,"engine_id":"e","model_id":"m","native_model_id":"n","route_id":42}"#,
            r#"{"version":1,"engine_id":"e","model_id":"m","native_model_id":"n","reasoning_effort":{"id":"e"}}"#,
        ] {
            assert!(
                StoredModelPolicy::from_json(garbage.as_bytes()).is_none(),
                "must reject {garbage:?}"
            );
        }
    }

    #[test]
    fn model_preference_rejects_overlong_identifiers() {
        let stored = stored_policy();
        let mut oversized = stored.clone();
        oversized.model_id = "m".repeat(MAX_FIELD_CHARS + 1);
        assert!(StoredModelPolicy::from_json(oversized.to_json().as_bytes()).is_none());
    }

    #[test]
    fn candidate_policy_carries_stored_identity_without_scope() {
        let candidate = stored_policy().candidate_for_restore();
        assert_eq!(candidate.engine_id, "codex");
        assert_eq!(candidate.model_id, "codex-luna");
        assert_eq!(candidate.native_model_id, "gpt-5.6-luna");
        assert!(candidate.profile_id.is_none());
        let selection = candidate.native_selection.expect("route");
        assert_eq!(selection.provider_route_id, "route-a");
        assert_eq!(selection.variant_id.as_deref(), Some("medium"));
        assert_eq!(
            candidate
                .reasoning_effort
                .as_ref()
                .map(|value| value.id.as_str()),
            Some("effort-id")
        );
        assert_eq!(
            candidate
                .context_window
                .as_ref()
                .map(|value| value.native_suffix.as_str()),
            Some("272k")
        );
    }

    #[test]
    fn host_preference_distinguishes_local_from_missing() {
        let dir =
            std::env::temp_dir().join(format!("artisan-last-used-test-{}", std::process::id()));
        let path = dir.join("last-used-host");
        assert!(load_bytes(&path).is_none());
        save_bytes(&path, &[]);
        assert_eq!(load_bytes(&path).expect("local marker"), Vec::<u8>::new());
        save_bytes(&path, "/tmp/remote-home".as_bytes());
        assert_eq!(load_bytes(&path).expect("host"), b"/tmp/remote-home");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn project_order_preserves_order_and_removes_duplicates() {
        let project = |value| ProjectId::parse(value).expect("project id");
        let projects = vec![
            project("project-b"),
            project("project-a"),
            project("project-b"),
        ];
        let encoded = project_order_json(&projects);
        assert_eq!(
            parse_project_order(encoded.as_bytes()).expect("round trip"),
            projects[..2],
        );
        assert_eq!(
            parse_project_order(br#"{"version":1,"projects":["b","a","b"]}"#)
                .expect("duplicate entries"),
            vec![project("b"), project("a")],
        );
        assert_eq!(
            parse_project_order(project_order_json(&[]).as_bytes()).expect("empty order"),
            Vec::<ProjectId>::new(),
        );
    }

    #[test]
    fn project_order_rejects_malformed_preferences() {
        for malformed in [
            "",
            "{",
            "{}",
            "[]",
            "null",
            r#"{"version":2,"projects":["a"]}"#,
            r#"{"version":1,"projects":"a"}"#,
            r#"{"version":1,"projects":[42]}"#,
            r#"{"version":1,"projects":[""]}"#,
            r#"{"version":1,"projects":["a","invalid id"]}"#,
        ] {
            assert!(
                parse_project_order(malformed.as_bytes()).is_none(),
                "must reject {malformed:?}",
            );
        }
        let overlong = serde_json::json!({"version": 1, "projects": ["a".repeat(129)]});
        assert!(parse_project_order(overlong.to_string().as_bytes()).is_none());
    }

    #[test]
    fn project_order_retains_a_loadable_prefix_within_the_file_bound() {
        let projects: Vec<_> = (0..100)
            .map(|index| {
                ProjectId::parse(format!("{}{index:02}", "\\\"".repeat(63)))
                    .expect("escaped project id")
            })
            .collect();
        let encoded = project_order_json(&projects);
        assert!(encoded.len() <= MAX_FILE_BYTES as usize);
        let restored = parse_project_order(encoded.as_bytes()).expect("bounded order");
        assert!(!restored.is_empty());
        assert!(restored.len() < projects.len());
        assert_eq!(restored, projects[..restored.len()]);
    }

    #[test]
    fn project_order_isolates_hosts_and_survives_invitation_replacement() {
        use artisan_editor_cli::credentials::hosts::{HostInvitation, install_private};
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "artisan-project-order-test-{}-{unique}",
            std::process::id(),
        ));
        let ui_directory = root.join("ui/project-orders");
        let first = root.join("first-registration");
        let replacement = root.join("replacement-registration");
        let other = root.join("other-host");
        let mut invitation = HostInvitation {
            version: 1,
            name: "Ubuntu".into(),
            endpoint: "127.0.0.1:4433".parse().expect("endpoint"),
            incarnation: [1; 16],
            pid: 1,
            certificate: "AQID".into(),
            bootstrap: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
        };
        install_private(
            &first,
            "host.json",
            &invitation.encode().expect("invitation"),
        )
        .expect("register first host");
        invitation.incarnation = [2; 16];
        install_private(
            &replacement,
            "host.json",
            &invitation.encode().expect("invitation"),
        )
        .expect("register replacement");
        invitation.certificate = "BAUG".into();
        install_private(
            &other,
            "host.json",
            &invitation.encode().expect("invitation"),
        )
        .expect("register other host");

        let local_path = project_order_path_in(&ui_directory, None);
        let first_path = project_order_path_in(&ui_directory, Some(&first));
        assert_eq!(
            first_path,
            project_order_path_in(&ui_directory, Some(&replacement)),
        );
        let other_path = project_order_path_in(&ui_directory, Some(&other));
        assert_ne!(first_path, other_path);
        assert_ne!(first_path, local_path);
        assert_ne!(other_path, local_path);

        for (path, id) in [
            (&local_path, "local-project"),
            (&first_path, "first-project"),
            (&other_path, "other-project"),
        ] {
            let projects = vec![ProjectId::parse(id).expect("project")];
            save_bytes(path, project_order_json(&projects).as_bytes());
            assert_eq!(
                parse_project_order(&load_bytes(path).expect("saved preference"))
                    .expect("valid order"),
                projects,
            );
        }
        let custom_home = root.join("custom-home");
        let fallback = project_order_path_in(&ui_directory, Some(&custom_home));
        assert_eq!(fallback, custom_home.join("ui/project-order"));
        assert_ne!(fallback, local_path);
        assert_ne!(fallback, first_path);
        save_bytes(&fallback, &vec![b' '; MAX_FILE_BYTES as usize + 1]);
        assert!(load_bytes(&fallback).is_none());
        std::fs::remove_dir_all(root).expect("remove test preferences");
    }
}
