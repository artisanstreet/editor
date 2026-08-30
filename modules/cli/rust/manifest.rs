use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{CliError, Result, instance::read_json};

#[derive(Clone, Debug, Deserialize)]
pub struct InstallationManifest {
    pub activation_state: String,
    pub active_version: Option<String>,
    pub install_root: PathBuf,
    pub permanent_ae_path: Option<PathBuf>,
}

impl InstallationManifest {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Err(CliError::Installation(format!(
                "no installation manifest at {}; this Artisan home has no installation",
                path.display()
            )));
        }
        let value: Self = read_json(path)?;
        if value.activation_state != "active" {
            return Err(CliError::Installation(
                "installation is not fully activated; run `ae doctor --fix`".into(),
            ));
        }
        if value.active_version.as_deref().is_none_or(str::is_empty) {
            return Err(CliError::Installation("active version is missing".into()));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn all_binaries_resolve_to_the_versioned_bin_directory() {
        let manifest = InstallationManifest {
            activation_state: "active".into(),
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
