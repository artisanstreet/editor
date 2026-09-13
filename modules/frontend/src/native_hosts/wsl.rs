//! WSL share names come from the trusted invitation source, never a host label.
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

pub(crate) fn distribution(path: &str) -> Option<String> {
    let normalized = share_path(path);
    let mut parts = normalized.strip_prefix("\\\\")?.split('\\');
    let server = parts.next()?;
    if !server.eq_ignore_ascii_case("wsl$") && !server.eq_ignore_ascii_case("wsl.localhost") {
        return None;
    }
    let distro = parts.next()?;
    if distro.is_empty() || distro == "." || distro == ".." || distro.chars().any(char::is_control)
    {
        return None;
    }
    Some(distro.to_owned())
}

#[cfg(any(windows, test))]
pub(crate) fn linux_path(path: &str, expected_distribution: &str) -> Option<String> {
    let actual = distribution(path)?;
    if !actual.eq_ignore_ascii_case(expected_distribution) {
        return None;
    }
    let normalized = share_path(path);
    let parts: Vec<_> = normalized
        .strip_prefix("\\\\")?
        .split('\\')
        .skip(2)
        .filter(|part| !part.is_empty())
        .collect();
    if parts
        .iter()
        .any(|part| *part == "." || *part == ".." || part.contains(':'))
    {
        return None;
    }
    Some(format!("/{}", parts.join("/")))
}

#[cfg(any(windows, test))]
fn home_share(distro: &str, home: &str) -> Option<String> {
    if distro.is_empty()
        || distro.contains(['/', '\\', ':'])
        || distro.chars().any(char::is_control)
    {
        return None;
    }
    if !home.starts_with('/') || home.contains(['\\', ':']) || home.chars().any(char::is_control) {
        return None;
    }
    let share = format!(r"\\wsl$\{distro}{}", home.replace('/', "\\"));
    linux_path(&share, distro)?;
    Some(share)
}

#[cfg(windows)]
pub(crate) fn default_home_share(distro: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let executable = std::path::PathBuf::from(std::env::var_os("SystemRoot")?)
        .join("System32")
        .join("wsl.exe");
    let output = Command::new(executable)
        .args([
            "--distribution",
            distro,
            "--exec",
            "sh",
            "-c",
            "printf %s \"$HOME\"",
        ])
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    home_share(distro, std::str::from_utf8(&output.stdout).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn home_paths_use_the_distro_user_and_preserve_spaces() {
        for home in ["/home/sander", "/root", "/srv/My Home", "/"] {
            let share = home_share("Ubuntu", home).unwrap();
            assert_eq!(linux_path(&share, "Ubuntu").as_deref(), Some(home));
        }
        assert_eq!(
            home_share("Ubuntu", "/home/sander").as_deref(),
            Some(r"\\wsl$\Ubuntu\home\sander")
        );
        for home in [
            "",
            "home/sander",
            "/home/../root",
            "/home\\other",
            "/home\n",
        ] {
            assert!(home_share("Ubuntu", home).is_none());
        }
        assert!(home_share("Ubuntu\\other", "/root").is_none());
    }
    #[test]
    fn only_selected_wsl_share_maps_to_host_paths() {
        assert_eq!(
            linux_path(r"\\wsl$\Ubuntu\home\sander\My Project", "Ubuntu").as_deref(),
            Some("/home/sander/My Project")
        );
        assert_eq!(
            linux_path(r"\\wsl.localhost\Ubuntu\", "Ubuntu").as_deref(),
            Some("/")
        );
        assert_eq!(
            linux_path(r"\\?\UNC\wsl.localhost\Ubuntu\home\sander", "Ubuntu").as_deref(),
            Some("/home/sander")
        );
        for path in [
            r"C:\projects",
            r"\\server\Ubuntu\project",
            r"\\wsl$\Debian\project",
            r"\\wsl$\Ubuntu\..\project",
        ] {
            assert_eq!(linux_path(path, "Ubuntu"), None);
        }
    }
}
