//! `ae account`: enroll this Forge machine into an AST account.
//!
//! The machine keypair is local (persisted in Forge's data dir, not in the
//! database). Core owns enrollment lifecycle; this CLI only builds the exact
//! proofs Core specifies, polls the authenticated status, and — after the
//! operator explicitly approves the shown accountId + verified email —
//! confirms the exact account/key binding. No parallel enrollment logic.

use anyhow::{Context, Result, bail};
use artisan_paths::App;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Args)]
pub struct AccountArgs {
    /// Core base URL (else `AST_CORE_BASE_URL`, else production default).
    #[arg(long)]
    pub core: Option<String>,
    #[command(subcommand)]
    pub command: AccountCommand,
}

#[derive(Subcommand)]
pub enum AccountCommand {
    /// Start an enrollment: prints a code to claim in a signed-in session.
    Enroll {
        /// Human label for this machine (default: hostname).
        #[arg(long)]
        label: Option<String>,
    },
    /// Poll the enrollment: shows pending/claimed + account when claimed.
    Status,
    /// Confirm after explicit local approval of the shown account.
    Confirm {
        /// Approve without asking (the account was already shown + approved).
        #[arg(long, short)]
        yes: bool,
        /// Only confirm into this account id; refuse on mismatch before the RPC.
        #[arg(long)]
        account_id: Option<String>,
    },
    /// Fetch and apply the latest account config bundle now.
    Sync,
}

/// What `ae account` persists beside `forge.sqlite3` (0600 on unix).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct AccountFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret_b64u: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_b64u: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enrollment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    machine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
}

fn account_path() -> Result<PathBuf> {
    Ok(App::Forge.data()?.join("ast_account.json"))
}

fn load_account() -> Result<AccountFile> {
    let path = account_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("read {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AccountFile::default()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn save_account(account: &AccountFile) -> Result<()> {
    use std::io::Write as _;
    let path = account_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("write {}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, account)?;
    file.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("start the account runtime")
}

fn client(core: Option<&str>) -> Result<forge_http::core::CoreClient> {
    forge_http::core::CoreClient::new(&forge_http::core::core_base_url(core))
}

pub fn run(args: &AccountArgs) -> Result<()> {
    match &args.command {
        AccountCommand::Enroll { label } => enroll(args.core.as_deref(), label.as_deref()),
        AccountCommand::Status => status(args.core.as_deref()),
        AccountCommand::Confirm { yes, account_id } => {
            confirm(args.core.as_deref(), *yes, account_id.as_deref())
        }
        AccountCommand::Sync => sync_config_now(args.core.as_deref()),
    }
}

fn keypair(account: &mut AccountFile) -> Result<artisan_account::MachineKeypair> {
    use base64::Engine as _;
    if let (Some(secret), Some(public)) = (
        account.secret_b64u.as_deref(),
        account.public_b64u.as_deref(),
    ) {
        use base64::Engine as _;
        let secret: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(secret)
            .context("read the machine secret")?
            .try_into()
            .map_err(|_secret| anyhow::anyhow!("the machine secret is not 32 bytes"))?;
        let public: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(public)
            .context("read the machine key")?
            .try_into()
            .map_err(|_key| anyhow::anyhow!("the machine key is not 32 bytes"))?;
        let keypair = artisan_account::MachineKeypair::from_secret(secret)
            .map_err(|_secret| anyhow::anyhow!("the machine secret is invalid"))?;
        if keypair.public() != &public {
            bail!("the stored machine key does not match its secret");
        }
        return Ok(keypair);
    }
    let keypair =
        artisan_account::MachineKeypair::generate().map_err(|_rand| anyhow::anyhow!("random key failed"))?;
    account.secret_b64u = Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(keypair.secret()));
    account.public_b64u = Some(keypair.public_b64u());
    save_account(account)?;
    Ok(keypair)
}

fn label_default(label: Option<&str>) -> String {
    label.map_or_else(
        || {
            gethostname::gethostname()
                .to_string_lossy()
                .trim()
                .to_owned()
        },
        str::to_owned,
    )
    .chars()
    .take(64)
    .collect::<String>()
    .trim()
    .to_owned()
}

fn enroll(core: Option<&str>, label: Option<&str>) -> Result<()> {
    let mut account = load_account()?;
    if account.machine_id.is_some() {
        bail!("this Forge is already enrolled; see `ae account status`");
    }
    let keypair = keypair(&mut account)?;
    let label = label_default(label);
    if label.is_empty() {
        bail!("give this machine a label with --label");
    }
    let created = runtime()?.block_on(async {
        client(core)?
            .enroll_start(keypair.public(), &label, keypair.secret())
            .await
    })?;
    account.enrollment_id = Some(created.enrollment_id);
    save_account(&account)?;
    println!(
        "Claim this code in a signed-in session: {} (expires {})",
        created.code, created.expires_at
    );
    println!("Then run `ae account status`, and `ae account confirm`.");
    Ok(())
}

fn status(core: Option<&str>) -> Result<()> {
    let account = load_account()?;
    let enrollment_id = account
        .enrollment_id
        .as_deref()
        .context("no enrollment started; run `ae account enroll`")?;
    if let Some(machine_id) = account.machine_id.as_deref() {
        println!(
            "Enrolled: machine {machine_id} in account {}.",
            account.account_id.as_deref().unwrap_or("?")
        );
        return Ok(());
    }
    let keypair = keypair(&mut load_account()?)?;
    let status = runtime()?.block_on(async {
        client(core)?
            .enrollment_status(enrollment_id, keypair.secret())
            .await
    })?;
    println!("Enrollment status: {}", status.status);
    match (status.account_id, status.email) {
        (Some(account_id), email) => {
            println!("Claimed by account: {account_id}");
            if let Some(email) = email {
                println!("Verified email: {email}");
            }
            if let Some(display_name) = status.display_name {
                println!("Display name: {display_name}");
            }
            println!("If that identity is correct, run `ae account confirm`.");
        }
        (None, _) => println!("Not claimed yet; claim the code first."),
    }
    Ok(())
}

fn confirm(core: Option<&str>, yes: bool, expected_account: Option<&str>) -> Result<()> {
    let mut account = load_account()?;
    if let Some(machine_id) = account.machine_id.clone() {
        check_expected_account(expected_account, account.account_id.as_deref())?;
        println!("Already enrolled as {machine_id}.");
        return Ok(());
    }
    let enrollment_id = account
        .enrollment_id
        .clone()
        .context("no enrollment started; run `ae account enroll`")?;
    let keypair = keypair(&mut account)?;
    let status = runtime()?.block_on(async {
        client(core)?
            .enrollment_status(&enrollment_id, keypair.secret())
            .await
    })?;
    let account_id = status.account_id.clone().context(
        "the enrollment is not claimed yet; claim the code in a signed-in session first",
    )?;
    check_expected_account(expected_account, Some(&account_id))?;
    println!("Enrollment claimed by account: {account_id}");
    if let Some(email) = status.email.as_deref() {
        println!("Verified email: {email}");
    }
    if let Some(display_name) = status.display_name.as_deref() {
        println!("Display name: {display_name}");
    }
    let approved = if yes {
        true
    } else if !crate::ui::interactive() {
        bail!(
            "refusing to enroll without explicit approval: re-run with --yes to confirm account {account_id}"
        );
    } else {
        ask(&account_id)?
    };
    if !approved {
        println!("Not confirmed.");
        return Ok(());
    }
    let confirmed = runtime()?.block_on(async {
        client(core)?.confirm_enrollment(
            &enrollment_id,
            &account_id,
            keypair.public(),
            keypair.secret(),
        )
        .await
    })?;
    account.machine_id = Some(confirmed.machine_id.clone());
    account.account_id = Some(confirmed.account_id);
    account.enrollment_id = None;
    save_account(&account)?;
    crate::ui::success(format!(
        "Enrolled this Forge as {}.",
        confirmed.machine_id
    ));
    Ok(())
}

/// Refuses to confirm into an unexpected account. `None` expected means no
/// constraint (standalone CLI); a blank flag never silently disables the
/// check, and a missing stored/claimed id never satisfies a constraint.
fn check_expected_account(expected: Option<&str>, actual: Option<&str>) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let expected = expected.trim();
    if expected.is_empty() {
        bail!("--account-id must not be blank");
    }
    match actual {
        Some(actual) if actual == expected => Ok(()),
        Some(actual) => bail!(
            "account mismatch: expected {expected}, found {actual}; refusing to confirm"
        ),
        None => bail!("account mismatch: expected {expected}, found none; refusing to confirm"),
    }
}

fn ask(account_id: &str) -> Result<bool> {
    cliclack::confirm(format!(
        "Enroll this Forge into account {account_id}? The code alone is not enough: this binds the local key."
    ))
    .initial_value(true)
    .interact()
    .map_err(crate::ui::prompt_error)
}

/// Fetch and apply the latest account config bundle now (same path the
/// Forge daemon runs on its interval).
fn sync_config_now(core: Option<&str>) -> Result<()> {
    let data_dir = artisan_paths::App::Forge.data()?;
    let Some(identity) = forge_http::sync::load_machine_identity(&data_dir)? else {
        bail!("this Forge is not enrolled; run `ae account enroll` first");
    };
    let Some(account_id) = identity.account_id else {
        bail!("no account recorded; re-run `ae account confirm`");
    };
    let _ = account_id;
    let store = forge_store::Store::open(&data_dir).context("open the Forge store")?;
    let engines = forge_engines::Engines::new(&data_dir);
    let hub = forge_runs::Hub::new(store, engines, data_dir.clone());
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build the Core HTTP client")?;
    let outcome = runtime()?.block_on(async {
        forge_http::sync::sync_config(
            &http,
            &forge_http::core::core_base_url(core),
            &identity.machine_id,
            &identity.secret,
            &hub,
            &data_dir,
        )
        .await
    })?;
    match outcome {
        forge_http::sync::ConfigOutcome::Updated(state) => {
            crate::ui::success(format!("Applied config bundle version {}.", state.version));
        }
        forge_http::sync::ConfigOutcome::Current(state) => {
            println!("Config is current (version {}).", state.version);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_fall_back_to_a_bounded_hostname() {
        assert_eq!(label_default(Some("  srv-1  ")), "srv-1");
        assert!(label_default(Some("")).is_empty());
        assert!(label_default(None).len() <= 64);
    }

    #[test]
    fn account_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ast_account.json");
        let account = AccountFile {
            machine_id: Some("mch_1".into()),
            account_id: Some("acct_1".into()),
            ..AccountFile::default()
        };
        std::fs::write(&path, serde_json::to_string(&account).unwrap()).unwrap();
        let read: AccountFile =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(read, account);
    }

    #[test]
    fn expected_account_gate_rejects_mismatch_before_rpc() {
        assert!(check_expected_account(None, None).is_ok());
        assert!(check_expected_account(None, Some("acct_a")).is_ok());
        assert!(check_expected_account(Some("acct_a"), Some("acct_a")).is_ok());
        assert!(check_expected_account(Some(" acct_a "), Some("acct_a")).is_ok());
        assert!(check_expected_account(Some("acct_a"), Some("acct_b")).is_err());
        assert!(check_expected_account(Some("acct_a"), None).is_err());
        assert!(check_expected_account(Some(""), Some("acct_a")).is_err());
        assert!(check_expected_account(Some("   "), Some("acct_a")).is_err());
    }
}
