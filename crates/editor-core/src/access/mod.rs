//! The account-only entry gate: what the Editor may show, derived from the
//! account phase and this device's onboarding record.

mod gate;

pub use gate::Dial;

use std::path::{Path, PathBuf};

use anyhow::Result;
use artisan_account::network::AccountProfile;
use serde::{Deserialize, Serialize};

use crate::account::{AccountMachine, AccountPhase};
use crate::hosts::{read_json, write_json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryGate {
    /// A saved session with no saved profile, waiting for Core.
    Restoring,
    /// No session, or a sign-in in progress.
    SignedOut,
    /// Signed in; no Forge has finished setup on this device yet.
    NeedsSetup {
        account_id: String,
        email: String,
    },
    Ready {
        account_id: String,
    },
}

/// The Forge that finished setup for this account on this device, scoped by
/// Core origin so dev and production accounts never mix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingRecord {
    pub account_id: String,
    pub base_url: String,
    pub machine_id: String,
}

/// The account's Forges that are not removed.
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

pub fn entry_gate(phase: &AccountPhase, onboarding: Option<&OnboardingRecord>) -> EntryGate {
    match phase {
        AccountPhase::Restoring => EntryGate::Restoring,
        AccountPhase::Authenticated { profile, .. } => {
            let onboarded = onboarding.is_some_and(|record| {
                record.account_id == profile.account_id
                    && live_machine(profile, &record.machine_id).is_some()
            });
            if onboarded {
                EntryGate::Ready {
                    account_id: profile.account_id.clone(),
                }
            } else {
                EntryGate::NeedsSetup {
                    account_id: profile.account_id.clone(),
                    email: profile.email.clone(),
                }
            }
        }
        AccountPhase::SignedOut
        | AccountPhase::Requesting
        | AccountPhase::Pending { .. }
        | AccountPhase::EmailApproved { .. }
        | AccountPhase::Finalizing
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
