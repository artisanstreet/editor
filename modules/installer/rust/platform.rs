use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::error::{InstallerError, Result};

const ROOT_CONFLICT: &str = "ARTISAN_HOME and ARTISAN_INSTALL_ROOT select different roots";
const ROOT_OWNERSHIP: &str = "installation manifest does not own the requested root";

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

    pub fn default_install_root() -> Result<PathBuf> {
        let local_app_data = env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let home = env::var_os("HOME").map(PathBuf::from);
        let xdg_data_home = env::var_os("XDG_DATA_HOME").map(PathBuf::from);
        default_roots_for(
            std::env::consts::OS,
            local_app_data.as_deref(),
            home.as_deref(),
            xdg_data_home.as_deref(),
        )
        .map(|(root, _)| root)
    }
}

pub fn resolve_install_root(
    command_line: Option<&Path>,
    install_root_env: Option<&Path>,
    artisan_home_env: Option<&Path>,
) -> Result<PathBuf> {
    let local_app_data = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let home = env::var_os("HOME").map(PathBuf::from);
    let xdg_data_home = env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    resolve_install_root_from(
        command_line,
        install_root_env,
        artisan_home_env,
        std::env::consts::OS,
        local_app_data.as_deref(),
        home.as_deref(),
        xdg_data_home.as_deref(),
    )
}

fn resolve_install_root_from(
    command_line: Option<&Path>,
    install_root_env: Option<&Path>,
    artisan_home_env: Option<&Path>,
    os: &str,
    local_app_data: Option<&Path>,
    home: Option<&Path>,
    xdg_data_home: Option<&Path>,
) -> Result<PathBuf> {
    let explicit = [command_line, install_root_env, artisan_home_env];
    let explicit: Vec<&Path> = explicit.into_iter().flatten().collect();
    for root in &explicit {
        require_absolute_root(root)?;
    }
    for (index, left) in explicit.iter().enumerate() {
        for right in explicit.iter().skip(index + 1) {
            if !same_path(left, right) {
                return Err(InstallerError::InvalidInstallation(ROOT_CONFLICT.into()));
            }
        }
    }
    if let Some(root) = command_line.or(install_root_env).or(artisan_home_env) {
        return select_explicit_root(root.to_path_buf());
    }

    let (default_root, legacy_root) = default_roots_for(os, local_app_data, home, xdg_data_home)?;
    if !default_root.exists() && legacy_root_is_nonempty(&legacy_root)? {
        return Err(InstallerError::InvalidInstallation(format!(
            "legacy Artisan root detected at {}; no migration was performed; select an explicit native root",
            legacy_root.display()
        )));
    }
    Ok(default_root)
}

fn select_explicit_root(root: PathBuf) -> Result<PathBuf> {
    require_absolute_root(&root)?;
    if is_legacy_root(&root) {
        require_native_manifest(&root)?;
    }
    Ok(root)
}

fn require_absolute_root(root: &Path) -> Result<()> {
    if root.is_absolute() {
        return Ok(());
    }
    Err(InstallerError::InvalidInstallation(
        "installation root must be an absolute path".into(),
    ))
}

fn default_roots_for(
    os: &str,
    local_app_data: Option<&Path>,
    home: Option<&Path>,
    xdg_data_home: Option<&Path>,
) -> Result<(PathBuf, PathBuf)> {
    let (base, legacy_name) = match os {
        "windows" => (local_app_data.map(Path::to_path_buf), "Artisan"),
        "macos" => (
            home.map(|path| path.join("Library").join("Application Support")),
            "Artisan",
        ),
        "linux" => {
            let base = match xdg_data_home {
                Some(path) if !path.as_os_str().is_empty() => path.to_path_buf(),
                _ => home
                    .filter(|path| !path.as_os_str().is_empty())
                    .map(|path| path.join(".local").join("share"))
                    .ok_or_else(|| user_data_unavailable())?,
            };
            (Some(base), "artisan")
        }
        other => return Err(InstallerError::UnsupportedPlatform(other.to_owned())),
    };
    let base = base
        .filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
        .ok_or_else(user_data_unavailable)?;
    Ok((base.join("Artisan Street"), base.join(legacy_name)))
}

fn user_data_unavailable() -> InstallerError {
    InstallerError::InvalidInstallation("user data directory is unavailable".into())
}

fn legacy_root_is_nonempty(legacy_root: &Path) -> Result<bool> {
    match fs::read_dir(legacy_root) {
        Ok(mut entries) => entries
            .next()
            .transpose()
            .map(|entry| entry.is_some())
            .map_err(|source| InstallerError::FileSystem {
                path: legacy_root.to_path_buf(),
                source,
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => Ok(true),
        Err(source) => Err(InstallerError::FileSystem {
            path: legacy_root.to_path_buf(),
            source,
        }),
    }
}

fn is_legacy_root(root: &Path) -> bool {
    matches!(
        comparable_components(root).last(),
        Some(ComparableComponent::Normal(name))
            if name.eq_ignore_ascii_case("Artisan")
    )
}

fn require_native_manifest(root: &Path) -> Result<()> {
    let path = root.join("installation.json");
    let bytes = fs::read(&path).map_err(|_| invalid_root())?;
    let document: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid_root())?;
    let Some(install_root) = document
        .get("install_root")
        .and_then(serde_json::Value::as_str)
    else {
        return Err(invalid_root());
    };
    if !same_path(Path::new(install_root), root) {
        return Err(invalid_root());
    }
    if document
        .get("activation_state")
        .and_then(serde_json::Value::as_str)
        != Some("active")
    {
        return Err(InstallerError::InvalidInstallation(
            "installation is not fully activated; run `ae doctor --fix`".into(),
        ));
    }
    let Some(active_version) = document
        .get("active_version")
        .and_then(serde_json::Value::as_str)
    else {
        return Err(InstallerError::InvalidInstallation(
            "active version is invalid".into(),
        ));
    };
    if !is_safe_version(active_version) {
        return Err(InstallerError::InvalidInstallation(
            "active version is invalid".into(),
        ));
    }
    let Some(permanent_ae_path) = document
        .get("permanent_ae_path")
        .and_then(serde_json::Value::as_str)
    else {
        return Err(InstallerError::InvalidInstallation(
            "permanent ae path is invalid".into(),
        ));
    };
    let stable_name = if cfg!(windows) { "ae.exe" } else { "ae" };
    if !same_path(
        Path::new(permanent_ae_path),
        &root.join("bin").join(stable_name),
    ) {
        return Err(InstallerError::InvalidInstallation(
            "permanent ae path is invalid".into(),
        ));
    }
    Ok(())
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

fn invalid_root() -> InstallerError {
    InstallerError::InvalidInstallation(ROOT_OWNERSHIP.into())
}

fn same_path(left: &Path, right: &Path) -> bool {
    comparable_components(left) == comparable_components(right)
}

#[cfg(debug_assertions)]
/// Debug bootstrap builds are development tools. They must never operate on
/// the real per-user installation root, so every operation aborts here unless
/// a sandboxed root was supplied. Release builds carry no guard.
pub fn forbid_default_install_root(root: &Path) -> Result<()> {
    let Ok(installed) = Platform::default_install_root() else {
        return Ok(());
    };
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
fn is_same_or_inside(candidate: &Path, base: &Path) -> bool {
    let candidate = comparable_components(candidate);
    let base = comparable_components(base);
    candidate.len() >= base.len() && candidate[..base.len()] == base[..]
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

fn comparable_text(value: &OsStr) -> String {
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

    use super::{Platform, ROOT_CONFLICT, default_roots_for, resolve_install_root_from};
    use crate::error::InstallerError;

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

    #[test]
    fn platform_defaults_are_exact_without_a_temporary_fallback() {
        let windows_base = Path::new(if cfg!(windows) {
            r"C:\Users\Ada\AppData\Local"
        } else {
            "/Users/Ada/AppData/Local"
        });
        let (windows, _) = default_roots_for("windows", Some(windows_base), None, None).unwrap();
        assert_eq!(windows, windows_base.join("Artisan Street"));

        let home = Path::new(if cfg!(windows) {
            r"C:\Users\Ada"
        } else {
            "/Users/Ada"
        });
        let (macos, _) = default_roots_for("macos", None, Some(home), None).unwrap();
        assert_eq!(
            macos,
            home.join("Library")
                .join("Application Support")
                .join("Artisan Street")
        );

        let xdg = Path::new(if cfg!(windows) {
            r"C:\Users\Ada\Data"
        } else {
            "/home/ada/.local/share"
        });
        let (linux_xdg, _) = default_roots_for("linux", None, None, Some(xdg)).unwrap();
        assert_eq!(linux_xdg, xdg.join("Artisan Street"));
        let (linux_home, _) = default_roots_for("linux", None, Some(home), None).unwrap();
        assert_eq!(
            linux_home,
            home.join(".local").join("share").join("Artisan Street")
        );
        for root in [windows, macos, linux_xdg, linux_home] {
            assert!(!root.ends_with("Artisan"));
        }
        assert!(matches!(
            default_roots_for("linux", None, None, None),
            Err(InstallerError::InvalidInstallation(message))
                if message == "user data directory is unavailable"
        ));
    }

    #[test]
    fn explicit_roots_are_absolute_and_conflicting_sources_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan Street");
        assert_eq!(
            resolve_install_root_from(
                Some(&root),
                Some(&root.join(".")),
                Some(&root),
                "linux",
                None,
                None,
                None,
            )
            .unwrap(),
            root
        );
        let other = root.with_file_name("Other");
        assert!(matches!(
            resolve_install_root_from(
                Some(&root),
                Some(&other),
                None,
                "linux",
                None,
                None,
                None,
            ),
            Err(InstallerError::InvalidInstallation(message)) if message == ROOT_CONFLICT
        ));
        assert!(matches!(
            resolve_install_root_from(
                Some(Path::new("relative")),
                None,
                None,
                "linux",
                None,
                None,
                None,
            ),
            Err(InstallerError::InvalidInstallation(message))
                if message == "installation root must be an absolute path"
        ));
    }

    #[test]
    fn default_discovery_refuses_a_nonempty_legacy_root_without_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let legacy = home.join(".local").join("share").join("artisan");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("old-state.json"), b"legacy").unwrap();
        let error = resolve_install_root_from(None, None, None, "linux", None, Some(&home), None)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "installation state is invalid: legacy Artisan root detected at {}; no migration was performed; select an explicit native root",
                legacy.display()
            )
        );
        assert!(
            !home
                .join(".local")
                .join("share")
                .join("Artisan Street")
                .exists()
        );
        assert_eq!(fs::read(legacy.join("old-state.json")).unwrap(), b"legacy");
    }

    #[test]
    fn explicitly_selected_legacy_root_requires_a_complete_owned_native_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan");
        fs::create_dir_all(root.join("child")).unwrap();
        let spellings = [root.clone(), root.join("child").join("..")];
        let stable_name = if cfg!(windows) { "ae.exe" } else { "ae" };

        fs::write(
            root.join("installation.json"),
            serde_json::to_vec(&json!({ "install_root": root })).unwrap(),
        )
        .unwrap();
        for spelling in &spellings {
            assert_eq!(
                resolve_install_root_from(Some(spelling), None, None, "linux", None, None, None,)
                    .unwrap_err()
                    .to_string(),
                "installation state is invalid: installation is not fully activated; run `ae doctor --fix`"
            );
        }

        fs::write(
            root.join("installation.json"),
            serde_json::to_vec(&json!({
                "activation_state": "active",
                "active_version": "../escape",
                "install_root": root,
                "permanent_ae_path": root.join("bin").join(stable_name),
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            resolve_install_root_from(Some(&root), None, None, "linux", None, None, None,)
                .unwrap_err()
                .to_string(),
            "installation state is invalid: active version is invalid"
        );

        for permanent_ae_path in [
            root.join("bin").join("not-ae"),
            root.with_file_name("Artisan-evil")
                .join("bin")
                .join(stable_name),
        ] {
            fs::write(
                root.join("installation.json"),
                serde_json::to_vec(&json!({
                    "activation_state": "active",
                    "active_version": "1.2.3",
                    "install_root": root,
                    "permanent_ae_path": permanent_ae_path,
                }))
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                resolve_install_root_from(Some(&root), None, None, "linux", None, None, None,)
                    .unwrap_err()
                    .to_string(),
                "installation state is invalid: permanent ae path is invalid"
            );
        }

        fs::write(
            root.join("installation.json"),
            serde_json::to_vec(&json!({
                "activation_state": "active",
                "finalization_state": "complete",
                "active_version": "1.2.3",
                "install_root": root,
                "permanent_ae_path": root.join("bin").join(stable_name),
            }))
            .unwrap(),
        )
        .unwrap();
        for spelling in &spellings {
            assert_eq!(
                resolve_install_root_from(Some(spelling), None, None, "linux", None, None, None,)
                    .unwrap(),
                spelling.clone()
            );
        }
        assert_eq!(
            fs::read_to_string(root.join("installation.json")).unwrap(),
            serde_json::to_string(&json!({
                "activation_state": "active",
                "finalization_state": "complete",
                "active_version": "1.2.3",
                "install_root": root,
                "permanent_ae_path": root.join("bin").join(stable_name),
            }))
            .unwrap()
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_builds_require_a_sandboxed_install_root() {
        use super::forbid_default_install_root;

        let installed = Platform::default_install_root().expect("installed root");
        assert!(matches!(
            forbid_default_install_root(&installed),
            Err(InstallerError::DebugBuildGuard(message))
                if message.contains("ARTISAN_INSTALL_ROOT")
        ));
        assert!(forbid_default_install_root(&installed.join("versions")).is_err());
        assert!(
            forbid_default_install_root(&std::env::temp_dir().join("artisan-dev-install-root"))
                .is_ok()
        );
    }

    #[cfg(all(debug_assertions, windows))]
    #[test]
    fn sandbox_detection_ignores_case_and_trailing_separators() {
        use super::is_same_or_inside;

        let base = Path::new(r"C:\Users\Test\AppData\Local\Artisan Street");
        assert!(is_same_or_inside(
            Path::new(r"c:\users\test\appdata\local\ARTISAN STREET\"),
            base
        ));
        assert!(is_same_or_inside(
            Path::new(r"C:\Users\Test\AppData\Local\Artisan Street\bin"),
            base
        ));
        assert!(!is_same_or_inside(
            Path::new(r"C:\Users\Test\AppData\Local\Artisan Street-old"),
            base
        ));
    }
}
