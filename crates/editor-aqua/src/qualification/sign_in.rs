use crate::core::config_file::WriteConfigError;
use crate::core::hosts::{self, PairError};
use crate::core::{
    Ae, AeError, Clock, Core, EditorFiles, HostId, IdentityError, OpenCoreError, RelayRoute, RunAe,
    Services,
};
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
}

#[derive(Debug, thiserror::Error)]
pub enum OpenSignedInError {
    #[error("open the signed-in Editor's core")]
    Open(#[source] OpenCoreError),
    #[error("the paired Forges {waiting:?} did not connect within {within:?}")]
    Connect {
        waiting: Vec<HostId>,
        within: std::time::Duration,
    },
}

const CONNECT_WITHIN: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Debug, Clone, Copy)]
struct UnenrolledAe;

impl RunAe for UnenrolledAe {
    fn run(
        &self,
        args: Vec<String>,
    ) -> futures::future::BoxFuture<'static, Result<String, AeError>> {
        let answer = match args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["account", .., "status", "--json"] => {
                Ok(r#"{"enrolled":false,"status":"none"}"#.to_owned())
            }
            _ => Err(AeError::Failed {
                command: args.join(" "),
                detail: "the signed-in fixture has no `ae`".to_owned(),
            }),
        };
        Box::pin(std::future::ready(answer))
    }
}

pub fn open_signed_in(files: EditorFiles) -> Result<Core, OpenSignedInError> {
    let saved = files
        .hosts()
        .map_err(|source| OpenSignedInError::Open(OpenCoreError::Hosts(source)))?;
    let services = Services::new(
        CORE_URL,
        Ae::new(UnenrolledAe),
        RelayRoute::Off,
        Clock::system(),
    );
    let mut core = Core::with_services(files, services).map_err(OpenSignedInError::Open)?;
    let deadline = std::time::Instant::now() + CONNECT_WITHIN;
    loop {
        core.drain_with(|_, _| {});
        let waiting: Vec<HostId> = saved
            .hosts
            .iter()
            .filter(|config| core.host(&config.id).is_none())
            .map(|config| config.id.clone())
            .collect();
        if waiting.is_empty() {
            return Ok(core);
        }
        if std::time::Instant::now() >= deadline {
            return Err(OpenSignedInError::Connect {
                waiting,
                within: CONNECT_WITHIN,
            });
        }
        std::thread::park_timeout(std::time::Duration::from_millis(1));
    }
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
        "Capture",
        &[ForgeInvite {
            host: host.clone(),
            name: "Forge".to_owned(),
            pairing: pairing.clone(),
        }],
    )?;
    Ok(host)
}

pub fn join_forges(
    files: &EditorFiles,
    display_name: &str,
    invites: &[ForgeInvite],
) -> Result<(), JoinForgeError> {
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
    save_signed_in_account(files, display_name, invites)
}

fn save_signed_in_account(
    files: &EditorFiles,
    display_name: &str,
    invites: &[ForgeInvite],
) -> Result<(), JoinForgeError> {
    use artisan_account::network::{AccountProfile, Jwk, MachineEntry};
    let profile = AccountProfile {
        account_id: ACCOUNT_ID.to_owned(),
        email: "capture@example.invalid".to_owned(),
        display_name: display_name.to_owned(),
        username: None,
        avatar_url: None,
        totp_enrolled: false,
        machines: invites
            .iter()
            .map(|invite| MachineEntry {
                id: invite.host.0.clone(),
                label: invite.name.clone(),
                public_key: Jwk {
                    kty: "OKP".to_owned(),
                    crv: "Ed25519".to_owned(),
                    x: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
                },
                created_at: "2026-01-01T00:00:00.000Z".to_owned(),
                revoked_at: None,
                last_seen_at: None,
            })
            .collect(),
        latest_config_version: None,
    };
    let account = serde_json::json!({
        "base_url": CORE_URL,
        "session_token": "capture-session",
        "profile": profile,
    });
    let path = files.dir.join("account.json");
    let bytes = serde_json::to_vec_pretty(&account).map_err(JoinForgeError::EncodeAccount)?;
    std::fs::write(&path, bytes).map_err(|source| JoinForgeError::WriteAccount { path, source })
}
