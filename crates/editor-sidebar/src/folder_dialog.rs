//! Choosing a project folder with the system's own dialog, as the old
//! Editor did (`wsl_project_picker`, `PickDirectory`): a Forge on this
//! machine gets the folder dialog directly; a Forge in WSL gets the Windows
//! dialog opened at the distribution's home share, and the choice is mapped
//! back to its Linux path. A remote Forge has no dialog of ours to open and
//! is browsed with [`crate::picker::FolderPicker`] instead.

use editor_core::HostKind;
use gpui::{App, PathPromptOptions, Task};

/// Opens the dialog for a host of `kind`; `None` when it has none. The task
/// gives the chosen folder as the host spells it, or `None` if cancelled.
pub(crate) fn choose(kind: &HostKind, cx: &App) -> Option<Task<anyhow::Result<Option<String>>>> {
    match kind {
        HostKind::Local => {
            let chosen = cx.prompt_for_paths(PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Choose a project folder".into()),
            });
            Some(cx.background_executor().spawn(async move {
                let paths = chosen.await??;
                Ok(paths
                    .and_then(|paths| paths.into_iter().next())
                    .map(|path| path.to_string_lossy().into_owned()))
            }))
        }
        #[cfg(windows)]
        HostKind::Wsl { distro } => {
            let distro = distro.clone();
            Some(
                cx.background_executor()
                    .spawn(async move { wsl::choose(&distro) }),
            )
        }
        #[cfg(not(windows))]
        HostKind::Wsl { .. } => None,
        HostKind::Remote => None,
    }
}

/// WSL shares as Windows sees them (`\\wsl$\Ubuntu\home\sander`).
#[cfg(any(windows, test))]
mod wsl {
    /// `\\?\UNC\server\…` reads as `\\server\…`; `/` as `\`.
    fn share_path(path: &str) -> String {
        let normalized = path.replace('/', "\\");
        if normalized
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
        {
            format!(r"\\{}", &normalized[8..])
        } else {
            normalized
        }
    }

    /// The Linux path of a folder on `distro`'s share; `None` for anything
    /// outside that share or with `.`/`..` parts.
    pub(super) fn linux_path(path: &str, distro: &str) -> Option<String> {
        let normalized = share_path(path);
        let mut parts = normalized.strip_prefix("\\\\")?.split('\\');
        let server = parts.next()?;
        if !server.eq_ignore_ascii_case("wsl$") && !server.eq_ignore_ascii_case("wsl.localhost") {
            return None;
        }
        if !parts.next()?.eq_ignore_ascii_case(distro) {
            return None;
        }
        let parts: Vec<_> = parts.filter(|part| !part.is_empty()).collect();
        if parts
            .iter()
            .any(|part| *part == "." || *part == ".." || part.contains(':'))
        {
            return None;
        }
        Some(format!("/{}", parts.join("/")))
    }

    /// The share of `home` on `distro`, where the dialog opens.
    pub(super) fn home_share(distro: &str, home: &str) -> Option<String> {
        if distro.is_empty()
            || distro.contains(['/', '\\', ':'])
            || distro.chars().any(char::is_control)
            || !home.starts_with('/')
            || home.contains(['\\', ':'])
            || home.chars().any(char::is_control)
        {
            return None;
        }
        let share = format!(r"\\wsl$\{distro}{}", home.replace('/', "\\"));
        linux_path(&share, distro)?;
        Some(share)
    }

    /// Asks `distro` for its user's home, then lets the user choose a
    /// folder from there.
    #[cfg(windows)]
    pub(super) fn choose(distro: &str) -> anyhow::Result<Option<String>> {
        use anyhow::Context as _;
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = std::process::Command::new("wsl.exe")
            .args([
                "--distribution",
                distro,
                "--exec",
                "sh",
                "-c",
                "printf %s \"$HOME\"",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .with_context(|| format!("running wsl.exe for {distro}"))?;
        anyhow::ensure!(
            output.status.success(),
            "wsl.exe couldn't read the home folder of {distro} ({})",
            output.status
        );
        let home = String::from_utf8(output.stdout).context("the WSL home folder isn't UTF-8")?;
        let share = home_share(distro, &home)
            .with_context(|| format!("{distro} reported an unusable home folder {home:?}"))?;
        let Some(chosen) = native_dialog::FileDialogBuilder::default()
            .set_title("Choose a project folder in WSL")
            .set_location(&share)
            .open_single_dir()
            .show()
            .context("opening the folder dialog")?
        else {
            return Ok(None);
        };
        let chosen = chosen.to_string_lossy();
        linux_path(&chosen, distro)
            .map(Some)
            .with_context(|| format!("{chosen} isn't a folder in {distro}"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_dialog_opens_at_the_distro_home_and_maps_back() {
            for home in ["/home/sander", "/root", "/srv/My Home", "/"] {
                let share = home_share("Ubuntu", home).unwrap();
                assert_eq!(linux_path(&share, "Ubuntu").as_deref(), Some(home));
            }
            assert_eq!(
                linux_path(r"\\wsl.localhost\Ubuntu\home\sander\My Project", "ubuntu").as_deref(),
                Some("/home/sander/My Project")
            );
            assert_eq!(
                linux_path(r"\\?\UNC\wsl$\Ubuntu\home\sander", "Ubuntu").as_deref(),
                Some("/home/sander")
            );
            for path in [
                r"C:\projects",
                r"\\server\Ubuntu\project",
                r"\\wsl$\Debian\project",
                r"\\wsl$\Ubuntu\..\project",
            ] {
                assert_eq!(linux_path(path, "Ubuntu"), None, "{path}");
            }
            for home in ["", "home/sander", "/home\\other", "/home\n"] {
                assert!(home_share("Ubuntu", home).is_none(), "{home:?}");
            }
        }
    }
}
