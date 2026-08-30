use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::{CliError, Result};

const ROOT_CONFLICT: &str = "ARTISAN_HOME and ARTISAN_INSTALL_ROOT select different roots";
const ROOT_OWNERSHIP: &str = "installation manifest does not own the requested root";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativePlatform {
    Windows,
    Macos,
    Unix,
}

impl NativePlatform {
    const fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "macos")]
        {
            Self::Macos
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            Self::Unix
        }
        #[cfg(not(any(unix, target_os = "windows")))]
        {
            Self::Unix
        }
    }

    const fn ae_name(self) -> &'static str {
        match self {
            Self::Windows => "ae.exe",
            Self::Macos | Self::Unix => "ae",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RootSource {
    Explicit,
    InstalledExecutable,
    PlatformDefault,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    pub root: PathBuf,
    pub manifest: PathBuf,
}

impl Layout {
    pub fn discover() -> Result<Self> {
        let install_root = env::var_os("ARTISAN_INSTALL_ROOT").map(PathBuf::from);
        let artisan_home = env::var_os("ARTISAN_HOME").map(PathBuf::from);
        let current_executable = if install_root.is_none() && artisan_home.is_none() {
            Some(env::current_exe().map_err(|source| CliError::Io {
                context: "resolve current executable",
                source,
            })?)
        } else {
            None
        };
        let local_app_data = env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let home = env::var_os("HOME").map(PathBuf::from);
        let xdg_data_home = env::var_os("XDG_DATA_HOME").map(PathBuf::from);
        let root = discover_root_from(
            install_root.as_deref(),
            artisan_home.as_deref(),
            current_executable.as_deref(),
            NativePlatform::current(),
            local_app_data.as_deref(),
            home.as_deref(),
            xdg_data_home.as_deref(),
        )?;
        #[cfg(debug_assertions)]
        forbid_installed_home(&root)?;
        Ok(Self {
            manifest: root.join("installation.json"),
            root,
        })
    }

    pub fn native_instance_path(&self) -> PathBuf {
        crate::instance::NativeInstanceConfig::native_path(&self.root)
    }
}

fn discover_root_from(
    install_root: Option<&Path>,
    artisan_home: Option<&Path>,
    current_executable: Option<&Path>,
    platform: NativePlatform,
    local_app_data: Option<&Path>,
    home: Option<&Path>,
    xdg_data_home: Option<&Path>,
) -> Result<PathBuf> {
    let explicit = [install_root, artisan_home];
    for root in explicit.into_iter().flatten() {
        require_absolute_root(root)?;
    }
    if let (Some(install_root), Some(artisan_home)) = (install_root, artisan_home)
        && !same_path(install_root, artisan_home)
    {
        return Err(CliError::Installation(ROOT_CONFLICT.into()));
    }
    if let Some(root) = install_root.or(artisan_home) {
        return select_root(root.to_path_buf(), RootSource::Explicit);
    }

    if let Some(current_executable) = current_executable
        && let Some(root) = installed_executable_root(current_executable, platform)
    {
        return select_root(root, RootSource::InstalledExecutable);
    }

    let (default_root, legacy_root) =
        platform_roots(platform, local_app_data, home, xdg_data_home)?;
    if !default_root.exists() && legacy_root_is_nonempty(&legacy_root)? {
        return Err(CliError::Installation(format!(
            "legacy Artisan root detected at {}; no migration was performed; select an explicit native root",
            legacy_root.display()
        )));
    }
    select_root_with_source(default_root, RootSource::PlatformDefault)
}

fn select_root(root: PathBuf, source: RootSource) -> Result<PathBuf> {
    require_absolute_root(&root)?;
    select_root_with_source(root, source)
}

fn select_root_with_source(root: PathBuf, source: RootSource) -> Result<PathBuf> {
    if matches!(
        source,
        RootSource::Explicit | RootSource::InstalledExecutable
    ) && is_legacy_root(&root)
    {
        require_native_manifest(&root)?;
    }
    Ok(root)
}

fn require_absolute_root(root: &Path) -> Result<()> {
    if root.is_absolute() {
        return Ok(());
    }
    Err(CliError::Installation(
        "installation root must be an absolute path".into(),
    ))
}

fn platform_roots(
    platform: NativePlatform,
    local_app_data: Option<&Path>,
    home: Option<&Path>,
    xdg_data_home: Option<&Path>,
) -> Result<(PathBuf, PathBuf)> {
    let (base, legacy_name) = match platform {
        NativePlatform::Windows => (local_app_data.map(Path::to_path_buf), "Artisan"),
        NativePlatform::Macos => (
            home.map(|path| path.join("Library").join("Application Support")),
            "Artisan",
        ),
        NativePlatform::Unix => {
            let base = match xdg_data_home {
                Some(path) if !path.as_os_str().is_empty() => path.to_path_buf(),
                _ => home
                    .filter(|path| !path.as_os_str().is_empty())
                    .map(|path| path.join(".local").join("share"))
                    .ok_or_else(|| {
                        CliError::Installation("user data directory is unavailable".into())
                    })?,
            };
            (Some(base), "artisan")
        }
    };
    let base = base
        .filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
        .ok_or_else(|| CliError::Installation("user data directory is unavailable".into()))?;
    Ok((base.join("Artisan Street"), base.join(legacy_name)))
}

fn platform_root() -> Result<PathBuf> {
    let local_app_data = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let home = env::var_os("HOME").map(PathBuf::from);
    let xdg_data_home = env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    platform_roots(
        NativePlatform::current(),
        local_app_data.as_deref(),
        home.as_deref(),
        xdg_data_home.as_deref(),
    )
    .map(|(root, _)| root)
}

fn legacy_root_is_nonempty(legacy_root: &Path) -> Result<bool> {
    match fs::read_dir(legacy_root) {
        Ok(mut entries) => entries
            .next()
            .transpose()
            .map(|entry| entry.is_some())
            .map_err(|source| CliError::Io {
                context: "inspect legacy Artisan root",
                source,
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => Ok(true),
        Err(source) => Err(CliError::Io {
            context: "inspect legacy Artisan root",
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
    crate::manifest::InstallationManifest::load_for_root(&path, root)
        .map(|_| ())
        .map_err(|_| CliError::Installation(ROOT_OWNERSHIP.into()))
}

fn installed_executable_root(
    current_executable: &Path,
    platform: NativePlatform,
) -> Option<PathBuf> {
    if !current_executable.is_absolute()
        || !component_matches(current_executable.file_name(), platform.ae_name())
    {
        return None;
    }
    let bin = current_executable.parent()?;
    if !component_matches(bin.file_name(), "bin") {
        return None;
    }
    let candidate = bin.parent()?;
    if let Some(version) = candidate.file_name()
        && let Some(versions) = candidate.parent()
        && component_matches(versions.file_name(), "versions")
    {
        if !safe_version_component(version) {
            return None;
        }
        return versions
            .parent()
            .filter(|root| root.is_absolute())
            .map(Path::to_path_buf);
    }
    candidate.is_absolute().then(|| candidate.to_path_buf())
}

fn component_matches(value: Option<&OsStr>, expected: &str) -> bool {
    value.is_some_and(|value| {
        if cfg!(windows) {
            value.to_string_lossy().eq_ignore_ascii_case(expected)
        } else {
            value == OsStr::new(expected)
        }
    })
}

fn safe_version_component(value: &OsStr) -> bool {
    let value = value.to_string_lossy();
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('\0')
        && !value
            .chars()
            .any(|character| matches!(character, '/' | '\\'))
}

fn same_path(left: &Path, right: &Path) -> bool {
    comparable_components(left) == comparable_components(right)
}

/// Debug builds are development tools. They must never read or mutate the real
/// installation, so every command aborts here when the effective home resolves
/// to the installed location. Release builds carry no guard.
#[cfg(debug_assertions)]
fn forbid_installed_home(root: &Path) -> Result<()> {
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

    #[test]
    fn layout_resolves_the_native_instance_through_the_v2_contract() {
        let root = PathBuf::from("Artisan Street");
        let layout = Layout {
            manifest: root.join("installation.json"),
            root: root.clone(),
        };
        assert_eq!(
            layout.native_instance_path(),
            crate::instance::NativeInstanceConfig::native_path(&root)
        );
    }

    #[test]
    fn platform_defaults_are_exact_and_have_no_temporary_fallback() {
        let windows_base = Path::new(if cfg!(windows) {
            r"C:\Users\Ada\AppData\Local"
        } else {
            "/Users/Ada/Library"
        });
        let (windows, _) =
            platform_roots(NativePlatform::Windows, Some(windows_base), None, None).unwrap();
        assert_eq!(windows, windows_base.join("Artisan Street"));

        let mac_home = Path::new(if cfg!(windows) {
            r"C:\Users\Ada"
        } else {
            "/Users/Ada"
        });
        let (macos, _) = platform_roots(NativePlatform::Macos, None, Some(mac_home), None).unwrap();
        assert_eq!(
            macos,
            mac_home
                .join("Library")
                .join("Application Support")
                .join("Artisan Street")
        );

        let xdg = Path::new(if cfg!(windows) {
            r"C:\Users\Ada\AppData\Local"
        } else {
            "/home/ada/.local/share"
        });
        let (linux_xdg, _) = platform_roots(NativePlatform::Unix, None, None, Some(xdg)).unwrap();
        assert_eq!(linux_xdg, xdg.join("Artisan Street"));
        let (linux_home, _) =
            platform_roots(NativePlatform::Unix, None, Some(mac_home), None).unwrap();
        assert_eq!(
            linux_home,
            mac_home.join(".local").join("share").join("Artisan Street")
        );
        for root in [windows, macos, linux_xdg, linux_home] {
            assert!(!root.ends_with("Artisan"));
        }
        assert!(matches!(
            platform_roots(NativePlatform::Unix, None, None, None),
            Err(CliError::Installation(message)) if message == "user data directory is unavailable"
        ));
    }

    #[test]
    fn explicit_roots_must_be_absolute_and_conflicts_fail_closed() {
        let root = PathBuf::from(if cfg!(windows) {
            r"C:\Users\Ada\Artisan Street"
        } else {
            "/Users/Ada/Artisan Street"
        });
        assert_eq!(
            discover_root_from(
                Some(&root),
                Some(&root.join(".")),
                None,
                NativePlatform::current(),
                None,
                None,
                None,
            )
            .unwrap(),
            root
        );
        let other = root.with_file_name("Other");
        assert!(matches!(
            discover_root_from(
                Some(&root),
                Some(&other),
                None,
                NativePlatform::current(),
                None,
                None,
                None,
            ),
            Err(CliError::Installation(message)) if message == ROOT_CONFLICT
        ));
        assert!(matches!(
            discover_root_from(
                Some(Path::new("relative")),
                None,
                None,
                NativePlatform::current(),
                None,
                None,
                None,
            ),
            Err(CliError::Installation(message)) if message == "installation root must be an absolute path"
        ));
    }

    #[test]
    fn installed_executable_shapes_derive_stable_and_versioned_roots_with_spaces() {
        let root = PathBuf::from(if cfg!(windows) {
            r"C:\Users\Ada\Artisan Street"
        } else {
            "/Users/Ada/Artisan Street"
        });
        let stable = root.join("bin").join(NativePlatform::current().ae_name());
        assert_eq!(
            installed_executable_root(&stable, NativePlatform::current()),
            Some(root.clone())
        );
        let versioned = root
            .join("versions")
            .join("1.2.3 with spaces")
            .join("bin")
            .join(NativePlatform::current().ae_name());
        assert_eq!(
            installed_executable_root(&versioned, NativePlatform::current()),
            Some(root)
        );
        let unrelated = versioned.with_file_name(if cfg!(windows) {
            "editor.exe"
        } else {
            "editor"
        });
        assert_eq!(
            installed_executable_root(&unrelated, NativePlatform::current()),
            None
        );
    }

    #[test]
    fn default_discovery_refuses_a_nonempty_legacy_root_without_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let legacy = home.join(".local").join("share").join("artisan");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("old-state.json"), b"legacy").unwrap();
        let error = discover_root_from(
            None,
            None,
            Some(&directory.path().join("development").join("ae")),
            NativePlatform::Unix,
            None,
            Some(&home),
            None,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "Artisan is not installed correctly: legacy Artisan root detected at {}; no migration was performed; select an explicit native root",
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
    fn explicit_legacy_root_gate_normalizes_child_parent_spellings() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Artisan");
        fs::create_dir_all(root.join("child")).unwrap();
        let spellings = [root.clone(), root.join("child").join("..")];

        for spelling in &spellings {
            let error = discover_root_from(
                Some(spelling),
                None,
                None,
                NativePlatform::current(),
                None,
                None,
                None,
            )
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                "Artisan is not installed correctly: installation manifest does not own the requested root"
            );
        }

        let manifest = json!({
            "activation_state": "active",
            "finalization_state": "complete",
            "active_version": "1.2.3",
            "install_root": root,
            "permanent_ae_path": directory
                .path()
                .join("Artisan")
                .join("bin")
                .join(if cfg!(windows) { "ae.exe" } else { "ae" }),
        });
        fs::write(
            directory.path().join("Artisan").join("installation.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        for spelling in &spellings {
            let selected = discover_root_from(
                Some(spelling),
                None,
                None,
                NativePlatform::current(),
                None,
                None,
                None,
            )
            .unwrap();
            assert!(same_path(&selected, &root));
        }
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
        assert!(forbid_installed_home(&std::env::temp_dir().join("artisan-dev-home")).is_ok());
    }

    #[cfg(all(debug_assertions, windows))]
    #[test]
    fn installed_home_detection_ignores_case_and_trailing_separators() {
        let base = Path::new(r"C:\Users\Test\AppData\Local\Artisan Street");
        assert!(is_same_or_inside(
            Path::new(r"c:\users\test\appdata\local\ARTISAN STREET\"),
            base
        ));
        assert!(is_same_or_inside(
            Path::new(r"C:\Users\Test\AppData\Local\Artisan Street\data"),
            base
        ));
        assert!(!is_same_or_inside(
            Path::new(r"C:\Users\Test\AppData\Local\Artisan Street-old"),
            base
        ));
    }
}
