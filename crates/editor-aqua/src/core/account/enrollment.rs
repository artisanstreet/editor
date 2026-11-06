use anyhow::{Context as _, Result, bail};
use artisan_account::network::AccountProfile;

use serde::Deserialize;

use crate::core::ae::Ae;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AeBinding {
    Unenrolled,
    Pending,
    Claimed {
        account_id: String,
        email: Option<String>,
    },
    Enrolled {
        machine_id: String,
        account_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalForge {
    Unknown,
    Unenrolled,
    Pending,
    Claimed { email: Option<String> },
    Enrolled { machine_id: String },
    Revoked { machine_id: String },
    OtherAccount,
}

pub(super) fn local_forge(binding: &AeBinding, profile: &AccountProfile) -> LocalForge {
    let ours = |account_id: &str| account_id == profile.account_id;
    match binding {
        AeBinding::Unenrolled => LocalForge::Unenrolled,
        AeBinding::Pending => LocalForge::Pending,
        AeBinding::Claimed { account_id, email } if ours(account_id) => LocalForge::Claimed {
            email: email.clone(),
        },
        AeBinding::Enrolled {
            machine_id,
            account_id,
        } if ours(account_id) => {
            let revoked = profile
                .machines
                .iter()
                .any(|machine| &machine.id == machine_id && machine.revoked_at.is_some());
            if revoked {
                LocalForge::Revoked {
                    machine_id: machine_id.clone(),
                }
            } else {
                LocalForge::Enrolled {
                    machine_id: machine_id.clone(),
                }
            }
        }
        AeBinding::Claimed { .. } | AeBinding::Enrolled { .. } => LocalForge::OtherAccount,
    }
}

#[derive(Deserialize)]
struct StatusReport {
    enrolled: bool,
    #[serde(default)]
    machine_id: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Enrollment {
    pub(super) code: String,
}

fn parse_status(printed: &str) -> Result<AeBinding> {
    let status: StatusReport =
        serde_json::from_str(printed).context("read what `ae account status` printed")?;
    Ok(
        match (
            status.enrolled,
            status.machine_id,
            status.account_id,
            status.status.as_deref(),
        ) {
            (true, Some(machine_id), Some(account_id), _) => AeBinding::Enrolled {
                machine_id,
                account_id,
            },
            (false, _, Some(account_id), Some("claimed" | "confirmed")) => AeBinding::Claimed {
                account_id,
                email: status.email,
            },
            (false, _, _, Some("pending")) => AeBinding::Pending,
            (false, _, _, Some("none" | "expired" | "voided")) => AeBinding::Unenrolled,
            _ => bail!("`ae account status` reported a state this Editor doesn't know: {printed}"),
        },
    )
}

pub(super) async fn read_status(ae: &Ae, core: &str) -> Result<AeBinding> {
    parse_status(
        &ae.run(&["account", "--core", core, "status", "--json"])
            .await?,
    )
}
