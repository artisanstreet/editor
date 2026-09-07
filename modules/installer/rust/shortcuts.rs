use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{
    background_process::background_command,
    error::{InstallerError, Result},
    integrations::OwnedIntegration,
    platform::Platform,
};

/// Both launchers point at the permanent `ae` rather than a versioned editor
/// executable, so an update never leaves a shortcut aimed at a directory it
/// just superseded.
const SHORTCUT_NAME: &str = "Artisan Editor.lnk";
const APP_USER_MODEL_ID: &str = "com.usebarekey.artisan-editor";
const TOAST_ACTIVATOR_CLSID: &str = "{A7D8D3E7-9DE2-4C09-8D4B-4E490C20D3A4}";

/// Where a shortcut is installed and what it should contain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutTarget {
    pub link: PathBuf,
    pub executable: PathBuf,
    pub arguments: String,
    pub icon: PathBuf,
    pub app_user_model_id: String,
    pub toast_activator_clsid: String,
}

impl ShortcutTarget {
    /// The fingerprint covers everything a repair would rewrite, so a shortcut
    /// whose icon source has rotted to a superseded version reads as drifted
    /// rather than current.
    fn fingerprint(&self) -> String {
        hex::encode(Sha256::digest(
            format!(
                "{}|{}|{}|{}|{}",
                self.executable.display(),
                self.arguments,
                self.icon.display(),
                self.app_user_model_id,
                self.toast_activator_clsid,
            )
            .as_bytes(),
        ))
    }

    pub fn owned(&self) -> Result<OwnedIntegration> {
        Ok(OwnedIntegration {
            path: self
                .link
                .to_str()
                .ok_or_else(|| {
                    InstallerError::InvalidInstallation(
                        "shortcut path cannot be recorded for ownership".to_owned(),
                    )
                })?
                .to_owned(),
            fingerprint: self.fingerprint(),
        })
    }
}

/// Resolves the launchers this installation owns: the desktop shortcut and the
/// Start Menu entry, both invoking `ae open`.
///
/// The icon is taken from the active release's native Editor executable at
/// `release/bin/editor.exe`. That path changes with every update, which is
/// exactly why the shortcuts must be rewritten on activation instead of
/// created once at first install — a shortcut left pointing at a removed
/// version keeps a cached icon until Windows re-reads it, then falls back to a
/// blank document.
pub fn targets(platform: &Platform, stable_ae: &Path, release: &Path) -> Vec<ShortcutTarget> {
    if platform.os != "windows" {
        return Vec::new();
    }

    let icon = release.join("bin").join("editor.exe");
    [desktop_directory(), start_menu_directory()]
        .into_iter()
        .flatten()
        .map(|directory| ShortcutTarget {
            link: directory.join(SHORTCUT_NAME),
            executable: stable_ae.to_path_buf(),
            arguments: "open".to_owned(),
            icon: icon.clone(),
            app_user_model_id: APP_USER_MODEL_ID.to_owned(),
            toast_activator_clsid: TOAST_ACTIVATOR_CLSID.to_owned(),
        })
        .collect()
}

/// Writes every owned launcher, replacing one whose contents have drifted.
/// Returns the ownership records to persist alongside the other integrations.
pub fn apply(targets: &[ShortcutTarget]) -> Result<Vec<OwnedIntegration>> {
    let mut owned = Vec::with_capacity(targets.len());
    for target in targets {
        write(target)?;
        owned.push(target.owned()?);
    }
    Ok(owned)
}

/// Removes only launchers this installation still recognises as its own, so a
/// shortcut a user re-pointed at something else survives an uninstall.
pub fn remove(targets: &[ShortcutTarget], recorded: &[OwnedIntegration]) -> Result<()> {
    for target in targets {
        let is_owned = recorded.iter().any(|entry| {
            Path::new(&entry.path) == target.link && entry.fingerprint == target.fingerprint()
        });
        if is_owned && target.link.exists() {
            std::fs::remove_file(&target.link).map_err(crate::error::io(&target.link))?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn desktop_directory() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Desktop"))
}

#[cfg(windows)]
fn start_menu_directory() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|roaming| {
        PathBuf::from(roaming)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
    })
}

#[cfg(not(windows))]
const fn desktop_directory() -> Option<PathBuf> {
    None
}

#[cfg(not(windows))]
const fn start_menu_directory() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn write(target: &ShortcutTarget) -> Result<()> {
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving the shortcut at {} untouched",
            target.link.display()
        );
        return Ok(());
    }
    if let Some(parent) = target.link.parent() {
        std::fs::create_dir_all(parent).map_err(crate::error::io(parent))?;
    }
    let script = shortcut_script(target);
    let status = background_command("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .status()
        .map_err(InstallerError::CleanupHelper)?;
    if !status.success() {
        return Err(InstallerError::InvalidInstallation(format!(
            "could not write the launcher at {}",
            target.link.display()
        )));
    }
    Ok(())
}

/// `WScript` authors ordinary shortcut fields, then the safe managed COM helper
/// persists the two Windows toast property-store values `WScript` cannot expose.
fn shortcut_script(target: &ShortcutTarget) -> String {
    format!(
        "$shell = New-Object -ComObject WScript.Shell; \
         $link = $shell.CreateShortcut('{link}'); \
         $link.TargetPath = '{executable}'; \
         $link.Arguments = '{arguments}'; \
         $link.IconLocation = '{icon}'; \
         $link.WorkingDirectory = '{working}'; \
         $link.WindowStyle = 7; \
         $link.Save(); \
         Add-Type -TypeDefinition @'\n{property_store_helper}\n'@; \
         [ArtisanShortcutProperties]::Apply('{link}', '{app_user_model_id}', [Guid]'{toast_activator_clsid}')",
        link = escape(&target.link.display().to_string()),
        executable = escape(&target.executable.display().to_string()),
        arguments = escape(&target.arguments),
        icon = escape(&target.icon.display().to_string()),
        working = escape(
            &target
                .executable
                .parent()
                .unwrap_or(&target.executable)
                .display()
                .to_string()
        ),
        app_user_model_id = escape(&target.app_user_model_id),
        toast_activator_clsid = escape(&target.toast_activator_clsid),
        property_store_helper = PROPERTY_STORE_HELPER,
    )
}

const PROPERTY_STORE_HELPER: &str = r#"
using System;
using System.Runtime.InteropServices;

[ComImport, Guid("00021401-0000-0000-C000-000000000046")]
class ShellLink {}

[ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IPropertyStore {
    void GetCount(out uint count);
    void GetAt(uint index, out PropertyKey key);
    void GetValue(ref PropertyKey key, out PropVariant value);
    void SetValue(ref PropertyKey key, ref PropVariant value);
    void Commit();
}

[ComImport, Guid("0000010B-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IPersistFile {
    void GetClassID(out Guid classId);
    void IsDirty();
    void Load([MarshalAs(UnmanagedType.LPWStr)] string fileName, uint mode);
    void Save([MarshalAs(UnmanagedType.LPWStr)] string fileName, bool remember);
    void SaveCompleted([MarshalAs(UnmanagedType.LPWStr)] string fileName);
    void GetCurFile([MarshalAs(UnmanagedType.LPWStr)] out string fileName);
}

[StructLayout(LayoutKind.Sequential, Pack = 4)]
struct PropertyKey {
    internal Guid format;
    internal uint id;
    internal PropertyKey(Guid format, uint id) { this.format = format; this.id = id; }
}

[StructLayout(LayoutKind.Explicit)]
struct PropVariant : IDisposable {
    [FieldOffset(0)] internal ushort type;
    [FieldOffset(8)] internal IntPtr pointer;
    internal static PropVariant String(string value) {
        return new PropVariant { type = 31, pointer = Marshal.StringToCoTaskMemUni(value) };
    }
    internal static PropVariant Guid(Guid value) {
        var pointer = Marshal.AllocCoTaskMem(16);
        Marshal.Copy(value.ToByteArray(), 0, pointer, 16);
        return new PropVariant { type = 72, pointer = pointer };
    }
    public void Dispose() {
        if (pointer != IntPtr.Zero) Marshal.FreeCoTaskMem(pointer);
        pointer = IntPtr.Zero;
    }
}

public static class ArtisanShortcutProperties {
    const uint StgmReadWrite = 0x00000002;
    public static void Apply(string path, string appUserModelId, Guid toastActivatorClsid) {
        var link = new ShellLink();
        var store = (IPropertyStore)link;
        var appUserModelIdKey = new PropertyKey(new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), 5);
        var toastActivatorClsidKey = new PropertyKey(new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), 26);
        var appId = PropVariant.String(appUserModelId);
        var clsid = PropVariant.Guid(toastActivatorClsid);
        try {
            ((IPersistFile)link).Load(path, StgmReadWrite);
            store.SetValue(ref appUserModelIdKey, ref appId);
            store.SetValue(ref toastActivatorClsidKey, ref clsid);
            store.Commit();
            ((IPersistFile)link).Save(path, true);
        } finally {
            appId.Dispose();
            clsid.Dispose();
            Marshal.FinalReleaseComObject(link);
        }
    }
}
"#;

#[cfg(not(windows))]
fn write(_target: &ShortcutTarget) -> Result<()> {
    Ok(())
}

/// PowerShell single-quoted strings take a doubled quote as an escape and
/// treat everything else literally, which is what a Windows path needs.
fn escape(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> ShortcutTarget {
        ShortcutTarget {
            link: PathBuf::from(r"C:\Users\test\Desktop\Artisan Editor.lnk"),
            executable: PathBuf::from(r"C:\Artisan\bin\ae.exe"),
            arguments: "open".to_owned(),
            icon: PathBuf::from(r"C:\Artisan\versions\0.2.14\bin\editor.exe"),
            app_user_model_id: APP_USER_MODEL_ID.to_owned(),
            toast_activator_clsid: TOAST_ACTIVATOR_CLSID.to_owned(),
        }
    }

    /// The icon source moves every release, so an otherwise identical shortcut
    /// from a superseded version must not be mistaken for a current one.
    #[test]
    fn the_fingerprint_follows_the_versioned_icon() {
        let current = target();
        let mut stale = target();
        stale.icon = PathBuf::from(r"C:\Artisan\versions\0.2.11\bin\editor.exe");
        assert_ne!(current.fingerprint(), stale.fingerprint());
    }

    #[test]
    fn shortcut_launches_stable_ae_open_with_the_native_editor_icon() {
        let target = target();
        assert_eq!(target.executable, PathBuf::from(r"C:\Artisan\bin\ae.exe"));
        assert_eq!(target.arguments, "open");
        assert_eq!(
            target.icon,
            PathBuf::from(r"C:\Artisan\versions\0.2.14\bin\editor.exe")
        );
    }

    #[test]
    fn the_fingerprint_covers_toast_identity() {
        let current = target();
        let mut drifted = target();
        drifted.toast_activator_clsid = "{00000000-0000-0000-0000-000000000000}".to_owned();
        assert_ne!(current.fingerprint(), drifted.fingerprint());
    }

    #[test]
    fn shortcut_script_persists_windows_toast_properties() {
        let script = shortcut_script(&target());
        assert!(script.contains("ArtisanShortcutProperties"));
        assert!(script.contains("System.Runtime.InteropServices"));
        assert!(script.contains("SetValue(ref appUserModelIdKey"));
        assert!(script.contains("SetValue(ref toastActivatorClsidKey"));
        assert!(script.contains(APP_USER_MODEL_ID));
        assert!(script.contains(TOAST_ACTIVATOR_CLSID));
        let load = script
            .find("Load(path, StgmReadWrite)")
            .expect("loads the authored shortcut");
        let set_value = script
            .find("SetValue(ref appUserModelIdKey")
            .expect("sets the first property");
        let save = script
            .find("Save(path, true)")
            .expect("saves the preserved shortcut");
        assert!(load < set_value && set_value < save);
    }

    /// A launcher a user re-pointed elsewhere is not ours to delete.
    #[test]
    fn removal_skips_a_shortcut_that_is_no_longer_ours() {
        let target = target();
        let foreign = OwnedIntegration {
            path: target.link.display().to_string(),
            fingerprint: "0".repeat(64),
        };
        remove(std::slice::from_ref(&target), &[foreign]).expect("removal");
        assert!(!target.link.exists());
    }

    #[test]
    fn single_quotes_cannot_break_out_of_the_shortcut_script() {
        assert_eq!(escape(r"C:\it's\here"), r"C:\it''s\here");
    }
}
