use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    error::{InstallerError, Result, io},
    platform::Platform,
};

const PROTOCOL_REGISTRY_PATH: &str = r"HKCU\Software\Classes\artisan";
#[cfg(windows)]
const PROTOCOL_REGISTRY_SUBKEY: &str = r"Software\Classes\artisan";
const PROTOCOL_LABEL: &str = "URL:Artisan Protocol";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OwnedIntegration {
    pub path: String,
    pub fingerprint: String,
}

pub fn protocol_command(stable_ae: &Path) -> Result<String> {
    let executable = stable_ae.to_str().ok_or_else(|| {
        InstallerError::InvalidInstallation(
            "permanent ae path cannot be represented in the protocol command".to_owned(),
        )
    })?;
    Ok(format!("\"{executable}\" protocol \"%1\""))
}

fn fingerprint(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

pub fn expected_protocol(stable_ae: &Path) -> Result<OwnedIntegration> {
    Ok(OwnedIntegration {
        path: stable_ae
            .to_str()
            .ok_or_else(|| {
                InstallerError::InvalidInstallation(
                    "permanent ae path cannot be recorded for protocol ownership".to_owned(),
                )
            })?
            .to_owned(),
        fingerprint: fingerprint(&protocol_command(stable_ae)?),
    })
}

pub fn prepare_protocol(
    platform: &Platform,
    stable_ae: &Path,
    recorded: Option<&OwnedIntegration>,
) -> Result<Option<OwnedIntegration>> {
    if platform.os != "windows" {
        return Ok(None);
    }

    #[cfg(windows)]
    {
        plan_windows_protocol_at(
            PROTOCOL_REGISTRY_SUBKEY,
            PROTOCOL_REGISTRY_PATH,
            stable_ae,
            recorded,
        )
        .map(|plan| Some(plan.owned().clone()))
    }

    #[cfg(not(windows))]
    unreachable!("Windows protocol integration is only compiled on Windows")
}

pub fn apply_protocol(
    platform: &Platform,
    stable_ae: &Path,
    recorded: Option<&OwnedIntegration>,
) -> Result<()> {
    if platform.os != "windows" {
        return Ok(());
    }

    #[cfg(windows)]
    {
        let plan = plan_windows_protocol_at(
            PROTOCOL_REGISTRY_SUBKEY,
            PROTOCOL_REGISTRY_PATH,
            stable_ae,
            recorded,
        )?;
        if matches!(plan, ProtocolPlan::Write(_)) {
            write_windows_protocol_at(
                PROTOCOL_REGISTRY_SUBKEY,
                PROTOCOL_REGISTRY_PATH,
                &protocol_command(stable_ae)?,
            )?;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    unreachable!("Windows protocol integration is only compiled on Windows")
}

pub fn verify_protocol(
    platform: &Platform,
    stable_ae: &Path,
    recorded: Option<&OwnedIntegration>,
) -> Result<()> {
    if platform.os != "windows" {
        return Ok(());
    }

    #[cfg(windows)]
    {
        let expected_command = protocol_command(stable_ae)?;
        let expected = expected_protocol(stable_ae)?;
        if recorded != Some(&expected)
            || read_windows_protocol_at(PROTOCOL_REGISTRY_SUBKEY, PROTOCOL_REGISTRY_PATH)?
                != Some(ProtocolSnapshot::expected(&expected_command))
        {
            return Err(InstallerError::InvalidInstallation(
                "artisan:// protocol integration is missing or has drifted; run `ae doctor --fix`"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    #[cfg(not(windows))]
    unreachable!("Windows protocol integration is only compiled on Windows")
}

pub fn remove_protocol(
    platform: &Platform,
    stable_ae: &Path,
    recorded: Option<&OwnedIntegration>,
) -> Result<()> {
    if platform.os != "windows" {
        return Ok(());
    }

    #[cfg(windows)]
    {
        let expected_command = protocol_command(stable_ae)?;
        let expected = expected_protocol(stable_ae)?;
        if let Some(snapshot) =
            read_windows_protocol_at(PROTOCOL_REGISTRY_SUBKEY, PROTOCOL_REGISTRY_PATH)?
            && (recorded == Some(&expected)
                && snapshot == ProtocolSnapshot::expected(&expected_command)
                || recorded.is_some_and(|owned| {
                    snapshot.command().is_some_and(|command| {
                        fingerprint(command) == owned.fingerprint
                            && snapshot == ProtocolSnapshot::expected(command)
                    })
                }))
        {
            delete_windows_protocol_at(PROTOCOL_REGISTRY_SUBKEY, PROTOCOL_REGISTRY_PATH)?;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    unreachable!("Windows protocol integration is only compiled on Windows")
}

#[cfg(windows)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistryNode {
    subkeys: std::collections::BTreeMap<String, Self>,
    values: std::collections::BTreeMap<String, String>,
}

#[cfg(windows)]
impl RegistryNode {
    fn is_subset_of(&self, expected: &Self) -> bool {
        self.values
            .iter()
            .all(|(name, value)| expected.values.get(name) == Some(value))
            && self.subkeys.iter().all(|(name, child)| {
                expected
                    .subkeys
                    .get(name)
                    .is_some_and(|expected_child| child.is_subset_of(expected_child))
            })
    }
}

#[cfg(windows)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct ProtocolSnapshot(RegistryNode);

#[cfg(windows)]
enum ProtocolPlan {
    Ready(OwnedIntegration),
    Write(OwnedIntegration),
}

#[cfg(windows)]
impl ProtocolPlan {
    const fn owned(&self) -> &OwnedIntegration {
        match self {
            Self::Ready(owned) | Self::Write(owned) => owned,
        }
    }
}

#[cfg(windows)]
impl ProtocolSnapshot {
    fn expected(command: &str) -> Self {
        use std::collections::BTreeMap;

        let command_node = RegistryNode {
            subkeys: BTreeMap::new(),
            values: BTreeMap::from([(String::new(), command.to_owned())]),
        };
        let open_node = RegistryNode {
            subkeys: BTreeMap::from([("command".to_owned(), command_node)]),
            values: BTreeMap::new(),
        };
        let shell_node = RegistryNode {
            subkeys: BTreeMap::from([("open".to_owned(), open_node)]),
            values: BTreeMap::new(),
        };
        Self(RegistryNode {
            subkeys: BTreeMap::from([("shell".to_owned(), shell_node)]),
            values: BTreeMap::from([
                (String::new(), PROTOCOL_LABEL.to_owned()),
                ("URL Protocol".to_owned(), String::new()),
            ]),
        })
    }

    fn command(&self) -> Option<&str> {
        self.0
            .subkeys
            .get("shell")?
            .subkeys
            .get("open")?
            .subkeys
            .get("command")?
            .values
            .get("")
            .map(String::as_str)
    }

    fn is_subset_of_expected(&self, command: &str) -> bool {
        self.0.is_subset_of(&Self::expected(command).0)
    }
}

#[cfg(windows)]
fn plan_windows_protocol_at(
    registry_subkey: &str,
    registry_path: &str,
    stable_ae: &Path,
    recorded: Option<&OwnedIntegration>,
) -> Result<ProtocolPlan> {
    let expected_command = protocol_command(stable_ae)?;
    let expected = expected_protocol(stable_ae)?;
    let current = read_windows_protocol_at(registry_subkey, registry_path)?;
    match current {
        None => Ok(ProtocolPlan::Write(expected)),
        Some(snapshot) if snapshot == ProtocolSnapshot::expected(&expected_command) => {
            Ok(ProtocolPlan::Ready(expected))
        }
        Some(snapshot)
            if recorded.is_some() && snapshot.is_subset_of_expected(&expected_command) =>
        {
            Ok(ProtocolPlan::Write(expected))
        }
        Some(snapshot)
            if recorded.is_some_and(|owned| {
                snapshot.command().is_some_and(|command| {
                    fingerprint(command) == owned.fingerprint
                        && snapshot == ProtocolSnapshot::expected(command)
                })
            }) =>
        {
            Ok(ProtocolPlan::Write(expected))
        }
        Some(_) => Err(InstallerError::InvalidInstallation(
            "artisan:// is registered to another application; preserving the existing handler"
                .to_owned(),
        )),
    }
}

#[cfg(windows)]
fn read_registry_node(key: &winreg::RegKey, registry_path: &str) -> Result<RegistryNode> {
    use std::collections::BTreeMap;
    use winreg::types::FromRegValue;

    let mut values = BTreeMap::new();
    for value in key.enum_values() {
        let (name, raw) = value.map_err(io(registry_path))?;
        let decoded = String::from_reg_value(&raw).map_err(io(registry_path))?;
        values.insert(name, decoded);
    }

    let mut subkeys = BTreeMap::new();
    for child in key.enum_keys() {
        let name = child.map_err(io(registry_path))?;
        let key = key.open_subkey(&name).map_err(io(registry_path))?;
        subkeys.insert(name, read_registry_node(&key, registry_path)?);
    }
    Ok(RegistryNode { subkeys, values })
}

#[cfg(windows)]
fn read_windows_protocol_at(
    registry_subkey: &str,
    registry_path: &str,
) -> Result<Option<ProtocolSnapshot>> {
    use std::io::ErrorKind;
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    match current_user.open_subkey(registry_subkey) {
        Ok(key) => read_registry_node(&key, registry_path)
            .map(ProtocolSnapshot)
            .map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io(registry_path)(error)),
    }
}

#[cfg(windows)]
fn write_windows_protocol_at(
    registry_subkey: &str,
    registry_path: &str,
    command: &str,
) -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    if cfg!(debug_assertions) && registry_subkey == PROTOCOL_REGISTRY_SUBKEY {
        eprintln!(
            "development build guard: leaving the artisan:// registration at {registry_path} untouched"
        );
        return Ok(());
    }
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (protocol, _) = current_user
        .create_subkey(registry_subkey)
        .map_err(io(registry_path))?;
    protocol
        .set_value("", &PROTOCOL_LABEL)
        .map_err(io(registry_path))?;
    protocol
        .set_value("URL Protocol", &"")
        .map_err(io(registry_path))?;
    let (open_command, _) = protocol
        .create_subkey(r"shell\open\command")
        .map_err(io(registry_path))?;
    open_command
        .set_value("", &command)
        .map_err(io(registry_path))
}

#[cfg(windows)]
fn delete_windows_protocol_at(registry_subkey: &str, registry_path: &str) -> Result<()> {
    use std::io::ErrorKind;
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    if cfg!(debug_assertions) && registry_subkey == PROTOCOL_REGISTRY_SUBKEY {
        eprintln!(
            "development build guard: leaving the artisan:// registration at {registry_path} untouched"
        );
        return Ok(());
    }
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    match current_user.delete_subkey_all(registry_subkey) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io(registry_path)(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_command_is_a_fixed_single_url_argument() {
        let command =
            protocol_command(Path::new(r"C:\Users\test\AppData\Local\Artisan\bin\ae.exe"))
                .expect("command");
        assert_eq!(
            command,
            r#""C:\Users\test\AppData\Local\Artisan\bin\ae.exe" protocol "%1""#
        );
    }

    #[test]
    fn protocol_fingerprint_changes_with_the_owned_cli() {
        let first = expected_protocol(Path::new(r"C:\Artisan\bin\ae.exe")).expect("first");
        let second = expected_protocol(Path::new(r"D:\Artisan\bin\ae.exe")).expect("second");
        assert_ne!(first.fingerprint, second.fingerprint);
    }

    #[cfg(windows)]
    #[test]
    fn partial_protocol_tree_is_structurally_safe_but_foreign_values_are_not() {
        use std::collections::BTreeMap;

        let command = r#""C:\Artisan\bin\ae.exe" protocol "%1""#;
        let expected = ProtocolSnapshot::expected(command);
        let partial = ProtocolSnapshot(RegistryNode {
            subkeys: BTreeMap::new(),
            values: BTreeMap::from([(String::new(), PROTOCOL_LABEL.to_owned())]),
        });
        assert!(partial.is_subset_of_expected(command));

        let foreign = ProtocolSnapshot(RegistryNode {
            subkeys: expected.0.subkeys,
            values: BTreeMap::from([(String::new(), "URL:Another Product".to_owned())]),
        });
        assert!(!foreign.is_subset_of_expected(command));
    }

    #[cfg(windows)]
    #[test]
    fn windows_registry_lifecycle_preserves_unowned_partial_and_foreign_handlers() {
        use std::time::{SystemTime, UNIX_EPOCH};
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};

        struct RegistryCleanup(String);
        impl Drop for RegistryCleanup {
            fn drop(&mut self) {
                let current_user = RegKey::predef(HKEY_CURRENT_USER);
                let _ = current_user.delete_subkey_all(&self.0);
            }
        }

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let subkey = format!(
            r"Software\ArtisanAcceptance\protocol-{}-{unique}",
            std::process::id()
        );
        let registry_path = format!(r"HKCU\{subkey}");
        let _cleanup = RegistryCleanup(subkey.clone());
        let stable_ae = Path::new(r"C:\Artisan\bin\ae.exe");
        let expected = expected_protocol(stable_ae).expect("expected ownership");
        let expected_command = protocol_command(stable_ae).expect("expected command");

        assert!(
            matches!(
                plan_windows_protocol_at(&subkey, &registry_path, stable_ae, None)
                    .expect("absent plan"),
                ProtocolPlan::Write(_)
            ),
            "an absent key is available for first-time installation"
        );

        let current_user = RegKey::predef(HKEY_CURRENT_USER);
        let (partial, _) = current_user.create_subkey(&subkey).expect("partial key");
        partial
            .set_value("", &PROTOCOL_LABEL)
            .expect("partial label");
        assert!(
            plan_windows_protocol_at(&subkey, &registry_path, stable_ae, None).is_err(),
            "a partial unowned key must never be claimed"
        );
        assert!(matches!(
            plan_windows_protocol_at(&subkey, &registry_path, stable_ae, Some(&expected))
                .expect("owned partial repair"),
            ProtocolPlan::Write(_)
        ));

        write_windows_protocol_at(&subkey, &registry_path, &expected_command)
            .expect("write protocol");
        assert_eq!(
            read_windows_protocol_at(&subkey, &registry_path).expect("read protocol"),
            Some(ProtocolSnapshot::expected(&expected_command))
        );

        let foreign = r#""C:\Foreign\Other.exe" "%1""#;
        write_windows_protocol_at(&subkey, &registry_path, foreign).expect("write foreign handler");
        assert!(
            plan_windows_protocol_at(&subkey, &registry_path, stable_ae, Some(&expected)).is_err(),
            "a foreign handler must be preserved even when Artisan has an ownership record"
        );
        assert_eq!(
            read_windows_protocol_at(&subkey, &registry_path)
                .expect("read preserved foreign handler"),
            Some(ProtocolSnapshot::expected(foreign))
        );

        delete_windows_protocol_at(&subkey, &registry_path).expect("remove isolated protocol");
        assert_eq!(
            read_windows_protocol_at(&subkey, &registry_path).expect("confirm removal"),
            None
        );
    }
}
