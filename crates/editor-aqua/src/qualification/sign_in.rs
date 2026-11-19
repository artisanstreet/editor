use crate::core::config_file::WriteConfigError;
use crate::core::hosts::{self, PairError};
use crate::core::{EditorFiles, HostId, IdentityError};
use crate::qualification::fixture::PairingInput;
use std::path::PathBuf;

const CORE_URL: &str = "http://127.0.0.1:1";
pub(crate) const ACCOUNT_ID: &str = "acct_capture0000000000000000";
pub const MACHINE_ID: &str = "mch_capture0000000000000000";

#[derive(Debug, thiserror::Error)]
pub enum JoinForgeError {
    #[error("load the Editor's identity from {}", editor.display())]
    Identity {
        editor: PathBuf,
        #[source]
        source: IdentityError,
    },
    #[error("pair the Editor with the Forge at {address}")]
    Pair {
        address: String,
        #[source]
        source: PairError,
    },
    #[error("save the Editor's hosts {hosts:?}")]
    SaveHosts {
        hosts: Vec<HostId>,
        #[source]
        source: WriteConfigError,
    },
    #[error("encode the signed-in account")]
    EncodeAccount(#[source] serde_json::Error),
    #[error("write the signed-in account to {}", path.display())]
    WriteAccount {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write the finished onboarding into {}", editor.display())]
    Onboarding {
        editor: PathBuf,
        #[source]
        source: WriteConfigError,
    },
}

#[derive(Clone, Debug)]
pub struct ForgeInvite {
    pub host: HostId,
    pub name: String,
    pub pairing: PairingInput,
}

#[cfg(feature = "capture")]
pub(crate) fn join_forge(
    files: &EditorFiles,
    pairing: &PairingInput,
) -> Result<HostId, JoinForgeError> {
    let host = HostId(MACHINE_ID.to_owned());
    join_forges(
        files,
        &[ForgeInvite {
            host: host.clone(),
            name: "Forge".to_owned(),
            pairing: pairing.clone(),
        }],
    )?;
    Ok(host)
}

pub fn join_forges(files: &EditorFiles, invites: &[ForgeInvite]) -> Result<(), JoinForgeError> {
    let identity = files
        .identity()
        .map_err(|source| JoinForgeError::Identity {
            editor: files.dir.clone(),
            source,
        })?;
    let mut configs = Vec::with_capacity(invites.len());
    for invite in invites {
        let mut config = crate::core::runtime::runtime()
            .block_on(hosts::pair(
                &invite.pairing.address,
                &invite.pairing.code,
                &invite.pairing.fingerprint,
                &identity,
                "capture",
                invite.name.clone(),
                hosts::HostKind::Local,
            ))
            .map_err(|source| JoinForgeError::Pair {
                address: invite.pairing.address.clone(),
                source,
            })?;
        config.id = invite.host.clone();
        configs.push(config);
    }
    let last = invites.first().map(|invite| invite.host.clone());
    files
        .save_hosts(&hosts::HostsFile {
            hosts: configs,
            last,
        })
        .map_err(|source| JoinForgeError::SaveHosts {
            hosts: invites.iter().map(|invite| invite.host.clone()).collect(),
            source,
        })?;
    save_signed_in_account(files)
}

fn save_signed_in_account(files: &EditorFiles) -> Result<(), JoinForgeError> {
    use artisan_account::network::{AccountProfile, Jwk, MachineEntry};
    let profile = AccountProfile {
        account_id: ACCOUNT_ID.to_owned(),
        email: "capture@example.invalid".to_owned(),
        display_name: "Capture".to_owned(),
        username: None,
        avatar_url: None,
        totp_enrolled: false,
        machines: vec![MachineEntry {
            id: MACHINE_ID.to_owned(),
            label: "Capture Forge".to_owned(),
            public_key: Jwk {
                kty: "OKP".to_owned(),
                crv: "Ed25519".to_owned(),
                x: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            },
            created_at: "2026-01-01T00:00:00.000Z".to_owned(),
            revoked_at: None,
            last_seen_at: None,
        }],
        latest_config_version: None,
    };
    let account = serde_json::json!({
        "base_url": CORE_URL,
        "session_token": "capture-session",
        "profile": profile,
    });
    let path = files.dir.join("account.json");
    let bytes = serde_json::to_vec_pretty(&account).map_err(JoinForgeError::EncodeAccount)?;
    std::fs::write(&path, bytes).map_err(|source| JoinForgeError::WriteAccount { path, source })?;
    crate::core::save_onboarding_for_tests(files, CORE_URL, ACCOUNT_ID, MACHINE_ID).map_err(
        |source| JoinForgeError::Onboarding {
            editor: files.dir.clone(),
            source,
        },
    )
}
