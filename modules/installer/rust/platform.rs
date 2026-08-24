use std::path::PathBuf;

use crate::error::{InstallerError, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Platform {
    pub os: &'static str,
    pub arch: &'static str,
}

impl Platform {
    pub fn detect() -> Result<Self> {
        let os = match std::env::consts::OS {
            "windows" => "windows",
            "macos" => "macos",
            "linux" => "linux",
            other => return Err(InstallerError::UnsupportedPlatform(other.to_owned())),
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => return Err(InstallerError::UnsupportedPlatform(other.to_owned())),
        };
        Ok(Self { os, arch })
    }

    pub fn target(&self) -> String {
        format!("{}-{}", self.os, self.arch)
    }

    pub fn default_install_root() -> PathBuf {
        #[cfg(windows)]
        {
            std::env::var_os("LOCALAPPDATA")
                .map_or_else(std::env::temp_dir, PathBuf::from)
                .join("Artisan")
        }
        #[cfg(not(windows))]
        {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(".local")
                .join("share")
                .join("artisan")
        }
    }
}

/// Debug bootstrap builds are development tools. They must never operate on
/// the real per-user installation root, so every operation aborts here unless
/// a sandboxed root was supplied. Release builds carry no guard.
#[cfg(debug_assertions)]
pub fn forbid_default_install_root(root: &std::path::Path) -> Result<()> {
    let installed = Platform::default_install_root();
    if !is_same_or_inside(root, &installed) {
        return Ok(());
    }
    Err(InstallerError::DebugBuildGuard(format!(
        "this debug installer build refuses to operate on the installed Artisan root at {}; \
         pass --install-root (or set ARTISAN_INSTALL_ROOT) to a sandbox such as \
         <repo>/.dist/dev/install-root, for example via `pnpm run dev:ae-installer`",
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
    use super::Platform;

    #[test]
    fn signed_linux_product_target_uses_glibc_name() {
        let platform = Platform {
            os: "linux",
            arch: "x64",
        };
        assert_eq!(platform.target(), "linux-x64");
        assert_eq!(
            crate::install::platform_libc(),
            if cfg!(target_env = "musl") {
                "musl"
            } else {
                "glibc"
            }
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_builds_require_a_sandboxed_install_root() {
        use super::forbid_default_install_root;
        use crate::error::InstallerError;

        let installed = Platform::default_install_root();
        assert!(matches!(
            forbid_default_install_root(&installed),
            Err(InstallerError::DebugBuildGuard(message))
                if message.contains("ARTISAN_INSTALL_ROOT")
        ));
        assert!(forbid_default_install_root(&installed.join("versions")).is_err());
        let elsewhere = installed
            .parent()
            .map(|base| base.join("artisan-guard-test-outside-root"))
            .expect("install root has a parent");
        assert!(
            forbid_default_install_root(&elsewhere).is_ok(),
            "a sibling of the install root must remain writable"
        );
    }

    #[cfg(all(debug_assertions, windows))]
    #[test]
    fn sandbox_detection_ignores_case_and_trailing_separators() {
        use std::path::Path;

        use super::is_same_or_inside;

        let base = Path::new(r"C:\Users\Test\AppData\Local\Artisan");
        assert!(is_same_or_inside(
            Path::new(r"c:\users\test\appdata\local\ARTISAN\"),
            base
        ));
        assert!(is_same_or_inside(
            Path::new(r"C:\Users\Test\AppData\Local\Artisan\bin"),
            base
        ));
        assert!(!is_same_or_inside(
            Path::new(r"C:\Users\Test\AppData\Local\Artisan-dev"),
            base
        ));
    }
}
