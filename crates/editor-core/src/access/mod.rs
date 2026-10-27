//! Account-only entry gate: startup restore vs sign-in vs setup vs ready.
//!
//! Small cohesive module so `lib.rs`/`account.rs` stay bounded. The gate is
//! derived from validated account state plus durable per-account onboarding,
//! never from a globally transferable boolean. Onboarding records are scoped
//! by Core API origin (`base_url`) so identical account ids on dev and prod
//! cannot collide.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use artisan_account::network::NetworkError;

pub mod gate;

/// Where the Editor may show workspace content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryGate {
    /// Saved session is validating; no workspace flash, no host connects.
    Restoring,
    /// No usable session (includes in-flight sign-in steps: the workspace
    /// stays unmounted until `Ready`).
    SignedOut,
    /// Authenticated but onboarding incomplete: only enroll/claim/confirm/
    /// connect flows are actionable, public host inventory stays hidden.
    NeedsSetup { account_id: String, email: String },
    /// Authenticated plus durable per-account onboarding verified.
    Ready { account_id: String },
}

/// Durable per-account/device onboarding: the machine whose live Relay
/// route proved setup, scoped by Core origin. The machine must still be
/// present and non-revoked in the live profile for `Ready`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingRecord {
    pub account_id: String,
    pub base_url: String,
    pub machine_id: String,
    pub completed_at: String,
}

/// Structured local Forge binding. Read from the local Forge data dir where
/// available; never parsed from human CLI text as a gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalForgeBinding {
    pub machine_id: Option<String>,
    pub account_id: Option<String>,
    pub enrolled: bool,
    pub claimed_by: Option<String>,
}

/// Scope onboarding files by Core origin so dev/prod account ids collide
/// never. `base64url(base_url)` is filesystem-safe and reversible.
fn encode_base_url(base_url: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(base_url.as_bytes())
}

/// Directory holding per-account onboarding records.
fn accounts_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("accounts")
}

/// Path for `(base_url, account_id)`. Both parts are required: the same
/// `acct_…` on a dev Core and on production are distinct records.
#[must_use]
pub fn onboarding_path(data_dir: &Path, base_url: &str, account_id: &str) -> PathBuf {
    accounts_dir(data_dir).join(format!(
        "{}-{account_id}.json",
        encode_base_url(base_url)
    ))
}

/// Loads the durable onboarding record for this origin+account, if any.
/// Corrupt files read as absent (setup repeats against live state).
#[must_use]
pub fn load_onboarding(
    data_dir: &Path,
    base_url: &str,
    account_id: &str,
) -> Option<OnboardingRecord> {
    let bytes = std::fs::read(onboarding_path(data_dir, base_url, account_id)).ok()?;
    let record: OnboardingRecord = serde_json::from_slice(&bytes).ok()?;
    if record.account_id != account_id || record.base_url != base_url {
        return None;
    }
    Some(record)
}

/// Persists the onboarding record atomically (0600 on unix).
pub fn save_onboarding(data_dir: &Path, record: &OnboardingRecord) -> anyhow::Result<()> {
    use std::io::Write as _;
    let path = onboarding_path(data_dir, &record.base_url, &record.account_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or(data_dir);
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer_pretty(tmp.as_file_mut(), record)?;
    tmp.flush()?;
    tmp.persist(&path).map_err(|e| anyhow::anyhow!("persist onboarding: {}", e.error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Session-invalidating Core failures: the saved session is dead and the UI
/// must lock. Transport and other codes keep the existing session.
#[must_use]
pub fn is_session_invalid(error: &NetworkError) -> bool {
    matches!(error, NetworkError::Core { code, .. } if matches!(code.as_str(), "unauthorized" | "forbidden" | "gone"))
}

/// Session death for business operations: only an explicit unauthorized or
/// expired session clears.
#[must_use]
pub fn is_session_dead(error: &NetworkError) -> bool {
    matches!(error, NetworkError::Core { code, .. } if matches!(code.as_str(), "unauthorized" | "gone"))
}
