//! Per-user PATH registration for the stable `ae` launcher.
//!
//! Development builds leave the user's environment untouched and say so;
//! release builds prepend the stable `bin` directory and remove only the exact
//! entry this installation owns. Every mutation is scoped to `HKCU\Environment`
//! on Windows and `~/.local/bin` on Unix.

use std::path::Path;

#[cfg(unix)]
use crate::error::InstallerError;
use crate::error::{Result, io};

#[cfg(windows)]
pub(crate) fn integrate_path(bin: &Path) -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving the user PATH untouched instead of registering {}",
            bin.display()
        );
        return Ok(());
    }
    let environment = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            "Environment",
            winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
        )
        .map_err(io("HKCU\\Environment"))?;
    let current: String = environment.get_value("Path").unwrap_or_default();
    let candidate = bin.display().to_string();
    let next = prepend_windows_path_entry(&current, &candidate);
    if next != current {
        environment
            .set_value("Path", &next)
            .map_err(io("HKCU\\Environment\\Path"))?;
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn prepend_windows_path_entry(current: &str, candidate: &str) -> String {
    std::iter::once(candidate)
        .chain(
            current
                .split(';')
                .filter(|entry| !entry.is_empty() && !entry.eq_ignore_ascii_case(candidate)),
        )
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(unix)]
pub(crate) fn integrate_path(bin: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving ~/.local/bin untouched instead of linking {}",
            bin.display()
        );
        return Ok(());
    }
    let home = std::env::var_os("HOME").ok_or(InstallerError::MissingHome)?;
    let command_bin = std::path::PathBuf::from(home).join(".local").join("bin");
    std::fs::create_dir_all(&command_bin).map_err(io(&command_bin))?;
    let link = command_bin.join("ae");
    let target = bin.join("ae");
    if link.symlink_metadata().is_ok() {
        if std::fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
            return Ok(());
        }
        return Err(InstallerError::InvalidInstallation(format!(
            "refusing to replace existing command at {}",
            link.display()
        )));
    }
    symlink(target, &link).map_err(io(&link))
}

#[cfg(windows)]
pub(crate) fn remove_path_integration(bin: &Path) -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving the user PATH untouched instead of removing {}",
            bin.display()
        );
        return Ok(());
    }
    let environment = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            "Environment",
            winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
        )
        .map_err(io("HKCU\\Environment"))?;
    let current: String = environment.get_value("Path").unwrap_or_default();
    let candidate = bin.display().to_string();
    let next = current
        .split(';')
        .filter(|entry| !entry.eq_ignore_ascii_case(&candidate))
        .collect::<Vec<_>>()
        .join(";");
    if next != current {
        environment
            .set_value("Path", &next)
            .map_err(io("HKCU\\Environment\\Path"))?;
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn remove_path_integration(bin: &Path) -> Result<()> {
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving ~/.local/bin untouched instead of unlinking {}",
            bin.display()
        );
        return Ok(());
    }
    let home = std::env::var_os("HOME").ok_or(InstallerError::MissingHome)?;
    let link = std::path::PathBuf::from(home)
        .join(".local")
        .join("bin")
        .join("ae");
    if link.symlink_metadata().is_ok()
        && std::fs::read_link(&link).ok().as_deref() == Some(bin.join("ae").as_path())
    {
        std::fs::remove_file(&link).map_err(io(&link))?;
    }
    Ok(())
}
