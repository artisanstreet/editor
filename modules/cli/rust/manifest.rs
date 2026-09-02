use std::{
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

use crate::{CliError, Result, instance::read_json};

const ROOT_OWNERSHIP: &str = "installation manifest does not own the requested root";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallationFinalization {
    Complete,
    Pending,
    Missing,
    Invalid,
}

impl InstallationFinalization {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Pending => "pending",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct InstallationManifest {
    pub activation_state: String,
    #[serde(default)]
    pub finalization_state: Option<String>,
    pub active_version: Option<String>,
    pub install_root: PathBuf,
    pub permanent_ae_path: Option<PathBuf>,
}

impl InstallationManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let selected_root = path
            .parent()
            .ok_or_else(|| CliError::Installation(ROOT_OWNERSHIP.to_owned()))?;
        Self::load_for_root(path, selected_root)
    }

    pub fn load_for_root(path: &Path, selected_root: &Path) -> Result<Self> {
        let selected_root = lexical_normalize_absolute(selected_root)
            .ok_or_else(|| CliError::Installation(ROOT_OWNERSHIP.to_owned()))?;
        let expected_path = selected_root.join("installation.json");
        let requested_path = lexical_normalize_absolute(path)
            .ok_or_else(|| CliError::Installation(ROOT_OWNERSHIP.to_owned()))?;
        if !same_path(&requested_path, &expected_path) {
            return Err(CliError::Installation(ROOT_OWNERSHIP.to_owned()));
        }
        let metadata = match fs::symlink_metadata(&expected_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(CliError::Installation(format!(
                    "no installation manifest at {}; this Artisan home has no installation",
                    expected_path.display()
                )));
            }
            Err(source) => {
                return Err(CliError::Io {
                    context: "inspect installation manifest",
                    source,
                });
            }
        };
        if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file() {
            return Err(CliError::Installation(ROOT_OWNERSHIP.to_owned()));
        }
        let mut value: Self = read_json(&expected_path)?;
        let manifest_root = lexical_normalize_absolute(&value.install_root)
            .ok_or_else(|| CliError::Installation(ROOT_OWNERSHIP.to_owned()))?;
        if !same_path(&manifest_root, &selected_root) {
            return Err(CliError::Installation(ROOT_OWNERSHIP.to_owned()));
        }
        if !value.active_version.as_deref().is_some_and(is_safe_version) {
            return Err(CliError::Installation("active version is invalid".into()));
        }
        if !value
            .permanent_ae_path
            .as_deref()
            .and_then(lexical_normalize_absolute)
            .is_some_and(|path| is_owned_permanent_ae_path(&selected_root, &path))
        {
            return Err(CliError::Installation(
                "permanent ae path is invalid".into(),
            ));
        }
        if value.activation_state != "active" {
            return Err(CliError::Installation(
                "installation is not fully activated; run `ae doctor --fix`".into(),
            ));
        }
        // The selected root is the authority for all later version and
        // process resolution. Keep an equivalent manifest spelling from
        // becoming a second root authority.
        value.install_root.clone_from(&selected_root);
        let stable_name = if cfg!(windows) { "ae.exe" } else { "ae" };
        value.permanent_ae_path = Some(selected_root.join("bin").join(stable_name));
        Ok(value)
    }

    /// Classifies only the canonical installation pointer. Temporary and
    /// previous transaction files are deliberately outside this authority.
    pub(crate) fn inspect(path: &Path) -> (InstallationFinalization, Option<Self>) {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (InstallationFinalization::Missing, None);
            }
            Err(_) => return (InstallationFinalization::Invalid, None),
        };
        if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file() {
            return (InstallationFinalization::Invalid, None);
        }

        let finalization_field = match fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(value) => match value
                    .as_object()
                    .and_then(|object| object.get("finalization_state"))
                {
                    None => Some(false),
                    Some(value) if value.is_string() => Some(true),
                    Some(_) => None,
                },
                Err(_) => None,
            },
            Err(_) => None,
        };
        let Some(finalization_field) = finalization_field else {
            return (InstallationFinalization::Invalid, None);
        };

        match Self::load(path) {
            Ok(manifest) => {
                let finalization = if finalization_field {
                    manifest.finalization_status()
                } else {
                    InstallationFinalization::Missing
                };
                (finalization, Some(manifest))
            }
            Err(_) => (InstallationFinalization::Invalid, None),
        }
    }

    pub(crate) fn finalization_status(&self) -> InstallationFinalization {
        match (
            self.activation_state.as_str(),
            self.finalization_state.as_deref(),
        ) {
            ("active", Some("complete")) => InstallationFinalization::Complete,
            ("active", Some("pending")) => InstallationFinalization::Pending,
            ("active", None) => InstallationFinalization::Missing,
            _ => InstallationFinalization::Invalid,
        }
    }

    pub fn version_root(&self) -> PathBuf {
        self.install_root
            .join("versions")
            .join(self.active_version.as_deref().unwrap_or_default())
    }

    pub fn forge_executable(&self) -> PathBuf {
        let directory = self.version_root().join("bin");
        #[cfg(target_os = "windows")]
        return directory.join("forge.exe");
        #[cfg(not(target_os = "windows"))]
        directory.join("forge")
    }

    pub fn editor_executable(&self) -> PathBuf {
        let directory = self.version_root().join("bin");
        #[cfg(target_os = "windows")]
        return directory.join("editor.exe");
        #[cfg(not(target_os = "windows"))]
        directory.join("editor")
    }

    pub fn installer_executable(&self) -> PathBuf {
        let directory = self.version_root().join("bin");
        #[cfg(target_os = "windows")]
        return directory.join("installer.exe");
        #[cfg(not(target_os = "windows"))]
        directory.join("installer")
    }

    pub fn ae_executable(&self) -> PathBuf {
        let directory = self.version_root().join("bin");
        #[cfg(target_os = "windows")]
        return directory.join("ae.exe");
        #[cfg(not(target_os = "windows"))]
        directory.join("ae")
    }
}

fn is_safe_version(value: &str) -> bool {
    if value.is_empty()
        || value.contains('\0')
        || value
            .chars()
            .any(|character| matches!(character, '/' | '\\'))
    {
        return false;
    }
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn is_owned_permanent_ae_path(root: &Path, path: &Path) -> bool {
    let name = if cfg!(windows) { "ae.exe" } else { "ae" };
    same_path(path, &root.join("bin").join(name))
}

fn metadata_is_symlink_or_reparse(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn same_path(left: &Path, right: &Path) -> bool {
    comparable_components(left) == comparable_components(right)
}

fn lexical_normalize_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                normalized = PathBuf::from(prefix.as_os_str());
            }
            Component::RootDir => {
                if normalized.as_os_str().is_empty() {
                    normalized.push(component.as_os_str());
                } else {
                    let mut value: OsString = normalized.into_os_string();
                    value.push(component.as_os_str());
                    normalized = PathBuf::from(value);
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                }
            }
            Component::Normal(normal) => normalized.push(normal),
        }
    }
    normalized.is_absolute().then_some(normalized)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ComparableComponent {
    Prefix(String),
    Root,
    Normal(String),
    Parent,
}

fn comparable_components(path: &Path) -> Vec<ComparableComponent> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => components.push(ComparableComponent::Prefix(
                comparable_text(prefix.as_os_str()),
            )),
            Component::RootDir => components.push(ComparableComponent::Root),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(components.last(), Some(ComparableComponent::Normal(_))) {
                    components.pop();
                } else if !matches!(components.last(), Some(ComparableComponent::Root)) {
                    components.push(ComparableComponent::Parent);
                }
            }
            Component::Normal(normal) => {
                components.push(ComparableComponent::Normal(comparable_text(normal)));
            }
        }
    }
    components
}

fn comparable_text(value: &std::ffi::OsStr) -> String {
    let value = value.to_string_lossy();
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Component, Path, PathBuf},
    };

    use serde_json::json;

    use super::*;

    fn native_permanent_path(root: &Path) -> PathBuf {
        root.join("bin")
            .join(if cfg!(windows) { "ae.exe" } else { "ae" })
    }

    fn write_manifest(
        root: &Path,
        install_root: &Path,
        active_version: Option<&str>,
        permanent_ae_path: Option<&Path>,
    ) -> PathBuf {
        fs::create_dir_all(root).unwrap();
        let path = root.join("installation.json");
        let value = json!({
            "activation_state": "active",
            "finalization_state": "pending",
            "active_version": active_version,
            "install_root": install_root,
            "permanent_ae_path": permanent_ae_path,
        });
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }

    #[test]
    fn a_home_without_an_installation_names_the_missing_installation() {
        let missing = std::env::temp_dir()
            .join(format!("artisan-cli-manifest-test-{}", std::process::id()))
            .join("installation.json");
        let error = InstallationManifest::load(&missing).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("no installation manifest"), "{message}");
    }

    #[test]
    fn load_rejects_a_relative_manifest_path_before_filesystem_lookup() {
        let error =
            InstallationManifest::load(Path::new("relative/installation.json")).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Artisan is not installed correctly: installation manifest does not own the requested root"
        );
    }

    #[test]
    fn load_binds_the_manifest_and_preserves_pending_finalization() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan Street");
        fs::create_dir_all(root.join("child")).unwrap();
        write_manifest(
            &root,
            &root.join(".").join("nested").join(".."),
            Some("1.2.3"),
            Some(&native_permanent_path(&root)),
        );
        let selected = root.join("child").join("..");
        let manifest_path = selected.join("installation.json");
        let manifest = InstallationManifest::load_for_root(&manifest_path, &selected).unwrap();
        assert_eq!(manifest.install_root, root);
        assert_eq!(
            manifest.permanent_ae_path,
            Some(native_permanent_path(&root))
        );
        assert_eq!(manifest.finalization_state.as_deref(), Some("pending"));
        assert_eq!(manifest.version_root(), root.join("versions/1.2.3"));
        assert!(
            manifest
                .version_root()
                .components()
                .all(|component| !matches!(component, Component::CurDir | Component::ParentDir))
        );
    }

    #[test]
    fn finalization_classifier_accepts_only_active_complete_or_pending() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan Street");
        let path = write_manifest(
            &root,
            &root,
            Some("1.2.3"),
            Some(&native_permanent_path(&root)),
        );

        let (state, manifest) = InstallationManifest::inspect(&path);
        assert_eq!(state, InstallationFinalization::Pending);
        assert_eq!(
            manifest
                .as_ref()
                .map(InstallationManifest::finalization_status),
            Some(InstallationFinalization::Pending)
        );

        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["finalization_state"] = json!("complete");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let (state, _) = InstallationManifest::inspect(&path);
        assert_eq!(state, InstallationFinalization::Complete);

        value["finalization_state"] = json!("unknown");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let (state, _) = InstallationManifest::inspect(&path);
        assert_eq!(state, InstallationFinalization::Invalid);

        value["finalization_state"] = serde_json::Value::Null;
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let (state, _) = InstallationManifest::inspect(&path);
        assert_eq!(state, InstallationFinalization::Invalid);

        value.as_object_mut().unwrap().remove("finalization_state");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let (state, _) = InstallationManifest::inspect(&path);
        assert_eq!(state, InstallationFinalization::Missing);
    }

    #[test]
    fn temporary_and_previous_pointers_never_become_authority() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan Street");
        fs::create_dir_all(&root).unwrap();
        let value = json!({
            "activation_state": "active",
            "finalization_state": "complete",
            "active_version": "1.2.3",
            "install_root": root,
            "permanent_ae_path": native_permanent_path(&root),
        });
        fs::write(
            root.join(".installation.json.tmp"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join(".installation.json.previous"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();

        let (state, manifest) = InstallationManifest::inspect(&root.join("installation.json"));
        assert_eq!(state, InstallationFinalization::Missing);
        assert!(manifest.is_none());
    }

    #[test]
    fn malformed_root_mismatched_inactive_and_unsafe_pointers_are_invalid() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan Street");
        let path = write_manifest(
            &root,
            &root,
            Some("1.2.3"),
            Some(&native_permanent_path(&root)),
        );
        fs::write(&path, b"not json").unwrap();
        assert_eq!(
            InstallationManifest::inspect(&path).0,
            InstallationFinalization::Invalid
        );

        for (activation_state, install_root) in [
            ("inactive", root.clone()),
            ("active", directory.path().join("Other")),
        ] {
            let value = json!({
                "activation_state": activation_state,
                "finalization_state": "complete",
                "active_version": "1.2.3",
                "install_root": install_root,
                "permanent_ae_path": native_permanent_path(&root),
            });
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(
                InstallationManifest::inspect(&path).0,
                InstallationFinalization::Invalid
            );
        }

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(
            InstallationManifest::inspect(&path).0,
            InstallationFinalization::Invalid
        );

        #[cfg(unix)]
        {
            fs::remove_dir(&path).unwrap();
            std::os::unix::fs::symlink(directory.path().join("outside.json"), &path).unwrap();
            assert_eq!(
                InstallationManifest::inspect(&path).0,
                InstallationFinalization::Invalid
            );
        }
    }

    #[test]
    fn load_rejects_a_relative_manifest_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan Street");
        let path = write_manifest(
            &root,
            Path::new("Artisan Street"),
            Some("1.2.3"),
            Some(&native_permanent_path(&root)),
        );
        let error = InstallationManifest::load_for_root(&path, &root).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Artisan is not installed correctly: installation manifest does not own the requested root"
        );
    }

    #[test]
    fn manifest_root_mismatch_is_rejected_before_resolution() {
        let directory = tempfile::tempdir().unwrap();
        let selected = directory.path().join("Artisan Street");
        let other = directory.path().join("Other");
        let path = write_manifest(
            &selected,
            &other,
            Some("1.2.3"),
            Some(&native_permanent_path(&selected)),
        );
        let error = InstallationManifest::load_for_root(&path, &selected).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Artisan is not installed correctly: installation manifest does not own the requested root"
        );
    }

    #[test]
    fn active_version_must_be_a_safe_single_path_component() {
        for version in [
            None,
            Some(""),
            Some("."),
            Some(".."),
            Some("../escape"),
            Some("a/b"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("Artisan Street");
            let path = write_manifest(&root, &root, version, Some(&native_permanent_path(&root)));
            let error = InstallationManifest::load_for_root(&path, &root).unwrap_err();
            assert_eq!(
                error.to_string(),
                "Artisan is not installed correctly: active version is invalid",
                "version {version:?}"
            );
        }
    }

    #[test]
    fn permanent_ae_must_be_the_direct_native_launcher_under_root_bin() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan Street");
        let invalid_paths: [Option<PathBuf>; 4] = [
            None,
            Some(root.join("bin").join("not-ae")),
            Some(
                root.join("bin")
                    .join("nested")
                    .join(if cfg!(windows) { "ae.exe" } else { "ae" }),
            ),
            Some(
                root.with_file_name("Artisan Street-evil")
                    .join("bin")
                    .join(if cfg!(windows) { "ae.exe" } else { "ae" }),
            ),
        ];
        for permanent_ae_path in invalid_paths {
            let path = write_manifest(&root, &root, Some("1.2.3"), permanent_ae_path.as_deref());
            let error = InstallationManifest::load_for_root(&path, &root).unwrap_err();
            assert!(error.to_string().contains("permanent ae path is invalid"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn command_script_launchers_are_not_native_manifest_launchers() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan Street");
        for launcher in ["ae.cmd", "ae.bat"] {
            let path = write_manifest(
                &root,
                &root,
                Some("1.2.3"),
                Some(&root.join("bin").join(launcher)),
            );
            let error = InstallationManifest::load_for_root(&path, &root).unwrap_err();
            assert!(error.to_string().contains("permanent ae path is invalid"));
        }
    }

    #[test]
    fn all_binaries_resolve_to_the_versioned_bin_directory() {
        let manifest = InstallationManifest {
            activation_state: "active".into(),
            finalization_state: Some("complete".into()),
            active_version: Some("1.2.3".into()),
            install_root: if cfg!(windows) {
                PathBuf::from(r"C:\Users\Ada\Artisan Street")
            } else {
                PathBuf::from("/opt/Artisan Street")
            },
            permanent_ae_path: None,
        };

        let bin = manifest.version_root().join("bin");
        #[cfg(windows)]
        let expected = [
            bin.join("ae.exe"),
            bin.join("installer.exe"),
            bin.join("editor.exe"),
            bin.join("forge.exe"),
        ];
        #[cfg(not(windows))]
        let expected = [
            bin.join("ae"),
            bin.join("installer"),
            bin.join("editor"),
            bin.join("forge"),
        ];

        assert_eq!(
            [
                manifest.ae_executable(),
                manifest.installer_executable(),
                manifest.editor_executable(),
                manifest.forge_executable(),
            ],
            expected
        );
        assert!(manifest.forge_executable().starts_with(&bin));
    }
}
