use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{CliError, Result, profile::read_json};

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
                "no installation manifest at {}; this Artisan home has no installation. \
                 The repo development Forge runs without one (`pnpm run dev:forge`, then \
                 `pnpm run dev:open` or `dev:pair`); to exercise the installed flow in \
                 development, create the sandboxed installation with \
                 `pnpm run dev:ae-bootstrap -- install`",
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
        let directory = self.version_root().join("forge");
        #[cfg(target_os = "windows")]
        return directory.join("Artisan Forge.exe");
        #[cfg(not(target_os = "windows"))]
        directory.join("artisan-forge")
    }

    pub fn editor_executable(&self) -> PathBuf {
        let directory = self.version_root().join("editor");
        #[cfg(target_os = "windows")]
        return directory.join("Artisan Editor.exe");
        #[cfg(not(target_os = "windows"))]
        directory.join("artisan-editor")
    }

    pub fn bootstrap_executable(&self) -> PathBuf {
        let directory = self.version_root().join("bin");
        #[cfg(target_os = "windows")]
        return directory.join("artisan-bootstrap.exe");
        #[cfg(not(target_os = "windows"))]
        directory.join("artisan-bootstrap")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_home_without_an_installation_names_the_gap_and_both_development_paths() {
        let missing = std::env::temp_dir()
            .join(format!("artisan-cli-manifest-test-{}", std::process::id()))
            .join("installation.json");
        let error = InstallationManifest::load(&missing).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("no installation manifest"), "{message}");
        assert!(message.contains("pnpm run dev:forge"), "{message}");
        assert!(message.contains("dev:ae-bootstrap -- install"), "{message}");
    }
}
