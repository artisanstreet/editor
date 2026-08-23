use std::{env, path::PathBuf};

use crate::{CliError, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    pub root: PathBuf,
    pub manifest: PathBuf,
}

impl Layout {
    pub fn discover() -> Result<Self> {
        let root = match env::var_os("ARTISAN_HOME") {
            Some(value) => PathBuf::from(value),
            None => platform_root()?,
        };
        #[cfg(debug_assertions)]
        forbid_installed_home(&root)?;
        Ok(Self {
            manifest: root.join("installation.json"),
            root,
        })
    }
}

fn platform_root() -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let value = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("Artisan"));
    #[cfg(target_os = "macos")]
    let value = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join("Library/Application Support/Artisan"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let value = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|path| path.join("artisan"));
    value.ok_or_else(|| CliError::Installation("user data directory is unavailable".into()))
}

/// Debug builds are development tools. They must never read or mutate the real
/// installation, so every command aborts here when the effective home resolves
/// to the installed location. Release builds carry no guard.
#[cfg(debug_assertions)]
fn forbid_installed_home(root: &std::path::Path) -> Result<()> {
    let Ok(installed) = platform_root() else {
        return Ok(());
    };
    if !is_same_or_inside(root, &installed) {
        return Ok(());
    }
    Err(CliError::DebugBuildGuard(format!(
        "this debug build of `ae` refuses to touch the installed Artisan home at {}; \
         manage the installation with the installed `ae`, and use \
         `pnpm run dev:ae -- <command>` (ARTISAN_HOME=<repo>/.dist/dev/forge-home) \
         for development",
        installed.display()
    )))
}

#[cfg(debug_assertions)]
fn is_same_or_inside(candidate: &std::path::Path, base: &std::path::Path) -> bool {
    let candidate = comparable_components(candidate);
    let base = comparable_components(base);
    candidate.len() >= base.len() && candidate[..base.len()] == base[..]
}

#[cfg(debug_assertions)]
fn comparable_components(path: &std::path::Path) -> Vec<String> {
    path.components()
        .map(|component| {
            let text = component.as_os_str().to_string_lossy();
            if cfg!(windows) {
                text.to_lowercase()
            } else {
                text.into_owned()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keeps_the_manifest_under_root() {
        let root = PathBuf::from("example");
        let layout = Layout {
            manifest: root.join("installation.json"),
            root: root.clone(),
        };
        assert!(layout.manifest.starts_with(&root));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_builds_refuse_the_installed_artisan_home() {
        let installed = platform_root().expect("installed home");
        assert!(matches!(
            forbid_installed_home(&installed),
            Err(CliError::DebugBuildGuard(message)) if message.contains("pnpm run dev:ae")
        ));
        assert!(forbid_installed_home(&installed.join("data")).is_err());
        let elsewhere = installed
            .parent()
            .map(|base| base.join("artisan-guard-test-outside-home"))
            .expect("installed home has a parent");
        assert!(forbid_installed_home(&elsewhere).is_ok());
    }

    #[cfg(all(debug_assertions, windows))]
    #[test]
    fn installed_home_detection_ignores_case_and_trailing_separators() {
        use std::path::Path;

        let base = Path::new(r"C:\Users\Test\AppData\Local\Artisan");
        assert!(is_same_or_inside(
            Path::new(r"c:\users\test\appdata\local\ARTISAN\"),
            base
        ));
        assert!(is_same_or_inside(
            Path::new(r"C:\Users\Test\AppData\Local\Artisan\data"),
            base
        ));
        assert!(!is_same_or_inside(
            Path::new(r"C:\Users\Test\AppData\Local\Artisan-dev"),
            base
        ));
    }
}
