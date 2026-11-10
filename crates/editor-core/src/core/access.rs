use std::path::{Path, PathBuf};

use anyhow::Result;
use artisan_account::network::AccountProfile;
use serde::{Deserialize, Serialize};

use crate::core::account::{AccountMachine, AccountPhase};
use crate::core::hosts::{read_json, write_json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryGate {
    Restoring,
    SignedOut,
    Ready { account_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingRecord {
    pub account_id: String,
    pub base_url: String,
    pub machine_id: String,
}

pub fn live_machines(profile: &AccountProfile) -> impl Iterator<Item = &AccountMachine> {
    profile
        .machines
        .iter()
        .filter(|machine| machine.revoked_at.is_none())
}

pub fn live_machine<'a>(
    profile: &'a AccountProfile,
    machine_id: &str,
) -> Option<&'a AccountMachine> {
    live_machines(profile).find(|machine| machine.id == machine_id)
}

pub fn entry_gate(phase: &AccountPhase) -> EntryGate {
    match phase {
        AccountPhase::Restoring => EntryGate::Restoring,
        AccountPhase::Authenticated { profile, .. } => EntryGate::Ready {
            account_id: profile.account_id.clone(),
        },
        AccountPhase::SignedOut
        | AccountPhase::Requesting
        | AccountPhase::Browser { .. }
        | AccountPhase::Pending { .. }
        | AccountPhase::EmailApproved { .. }
        | AccountPhase::Finalizing { .. }
        | AccountPhase::Error { .. } => EntryGate::SignedOut,
    }
}

fn onboarding_path(data_dir: &Path, base_url: &str, account_id: &str) -> PathBuf {
    use base64::Engine as _;
    let origin = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(base_url.as_bytes());
    data_dir
        .join("accounts")
        .join(format!("{origin}-{account_id}.json"))
}

pub fn load_onboarding(
    data_dir: &Path,
    base_url: &str,
    account_id: &str,
) -> Result<Option<OnboardingRecord>> {
    read_json(&onboarding_path(data_dir, base_url, account_id))
}

pub fn save_onboarding(data_dir: &Path, record: &OnboardingRecord) -> Result<()> {
    write_json(
        &onboarding_path(data_dir, &record.base_url, &record.account_id),
        record,
    )
}
