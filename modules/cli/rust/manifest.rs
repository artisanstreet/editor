use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::{CliError, Result, instance::read_json};

const ROOT_OWNERSHIP: &str = "installation manifest does not own the requested root";

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
        let expected_path = selected_root.join("installation.json");
        if !same_path(path, &expected_path) {
            return Err(CliError::Installation(ROOT_OWNERSHIP.to_owned()));
        }
        if !path.is_file() {
            return Err(CliError::Installation(format!(
                "no installation manifest at {}; this Artisan home has no installation",
                path.display()
            )));
        }
        let mut value: Self = read_json(path)?;
        if !same_path(&value.install_root, selected_root) {
            return Err(CliError::Installation(ROOT_OWNERSHIP.to_owned()));
        }
        if !value.active_version.as_deref().is_some_and(is_safe_version) {
            return Err(CliError::Installation("active version is invalid".into()));
        }
        if !value
            .permanent_ae_path
            .as_deref()
            .is_some_and(|path| is_owned_permanent_ae_path(selected_root, path))
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
        value.install_root = selected_root.to_path_buf();
        Ok(value)
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
    let names: &[&str] = if cfg!(windows) {
        &["ae.exe", "ae.cmd", "ae.bat"]
    } else {
        &["ae"]
    };
    names
        .iter()
        .map(|name| root.join("bin").join(name))
        .any(|expected| same_path(path, &expected))
}

fn same_path(left: &Path, right: &Path) -> bool {
    comparable_components(left) == comparable_components(right)
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
        path::{Path, PathBuf},
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
    fn load_binds_the_manifest_and_preserves_pending_finalization() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan Street");
        let path = write_manifest(
            &root,
            &root.join(".").join("nested").join(".."),
            Some("1.2.3"),
            Some(&native_permanent_path(&root)),
        );
        let manifest = InstallationManifest::load_for_root(&path, &root).unwrap();
        assert_eq!(manifest.install_root, root);
        assert_eq!(manifest.finalization_state.as_deref(), Some("pending"));
        assert_eq!(
            manifest.version_root(),
            manifest.install_root.join("versions/1.2.3")
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
