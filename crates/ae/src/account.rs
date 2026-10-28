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
        /// Print machine-readable enrollment (code, ids, expiry) instead of
        /// human text, for the Editor's binding check.
        #[arg(long)]
        json: bool,
        /// Replace a removed enrollment with a fresh machine key.
        #[arg(long)]
        replace: bool,
    },
    /// Poll the enrollment: shows pending/claimed + account when claimed.
    Status {
        /// Print machine-readable JSON (stable keys; never secrets) instead
        /// of human text, for the Editor's binding check.
        #[arg(long)]
        json: bool,
    },
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
        AccountCommand::Enroll { label, json, replace } => {
            enroll(args.core.as_deref(), label.as_deref(), *json, *replace)
        }
        AccountCommand::Status { json } => status(args.core.as_deref(), *json),
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

fn enroll(core: Option<&str>, label: Option<&str>, json: bool, replace: bool) -> Result<()> {
    let mut account = load_account()?;
    if account.machine_id.is_some() && !replace {
        bail!("this Forge is already enrolled; see `ae account status`");
    }
    let restart = replace && account.machine_id.is_some() && crate::service::is_running();
    let keypair = if replace {
        use base64::Engine as _;
        // Core keeps revoked account/key pairs unique. Re-enrollment needs
        // a new key, not just a new code for the revoked key. Keep the old
        // binding on disk until Core accepts the replacement enrollment.
        let fresh = artisan_account::MachineKeypair::generate()
            .map_err(|_rand| anyhow::anyhow!("random key failed"))?;
        account = AccountFile {
            secret_b64u: Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(fresh.secret())),
            public_b64u: Some(fresh.public_b64u()),
            ..AccountFile::default()
        };
        fresh
    } else {
        keypair(&mut account)?
    };
    let label = label_default(label);
    if label.is_empty() {
        bail!("give this machine a label with --label");
    }
    let created = runtime()?.block_on(async {
        client(core)?
            .enroll_start(keypair.public(), &label, keypair.secret())
            .await
    })?;
    account.enrollment_id = Some(created.enrollment_id.clone());
    save_account(&account)?;
    if restart {
        // The old daemon holds its machine identity in memory. Restart
        // into the pending state so it observes the new confirmation.
        crate::service::restart()?;
    }
    if json {
        println!("{}", enroll_report(&created));
        return Ok(());
    }
    println!(
        "Claim this code in a signed-in session: {} (expires {})",
        created.code, created.expires_at
    );
    println!("Then run `ae account status`, and `ae account confirm`.");
    Ok(())
}

/// Machine-readable enrollment for the Editor: the exact Core-created
/// fields it needs to track the request. Never key material.
fn enroll_report(created: &forge_http::core::EnrollmentCreated) -> serde_json::Value {
    serde_json::json!({
        "code": created.code.clone(),
        "enrollment_id": created.enrollment_id.clone(),
        "expires_at": created.expires_at.clone(),
        "key_fingerprint": created.key_fingerprint.clone(),
    })
}

fn status(core: Option<&str>, json: bool) -> Result<()> {
    let account = load_account()?;
    // Enrolled machines report their binding even though confirming clears
    // the pending enrollment id.
    if let Some(machine_id) = account.machine_id.as_deref() {
        if json {
            println!(
                "{}",
                enrolled_report(machine_id, account.account_id.as_deref())
            );
            return Ok(());
        }
        println!(
            "Enrolled: machine {machine_id} in account {}.",
            account.account_id.as_deref().unwrap_or("?")
        );
        return Ok(());
    }
    let Some(enrollment_id) = account.enrollment_id.as_deref() else {
        if json {
            println!("{}", no_enrollment_report());
            return Ok(());
        }
        bail!("no enrollment started; run `ae account enroll`");
    };
    let keypair = keypair(&mut load_account()?)?;
    let status = runtime()?.block_on(async {
        client(core)?
            .enrollment_status(enrollment_id, keypair.secret())
            .await
    })?;
    if json {
        println!("{}", pending_report(enrollment_id, &status));
        return Ok(());
    }
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

/// No-enrollment report: the pending keys with nulls and `status "none"`,
/// exit 0, so the Editor distinguishes "nothing started" from failure.
/// Human output keeps the nonzero error.
fn no_enrollment_report() -> serde_json::Value {
    serde_json::json!({
        "enrolled": false,
        "enrollment_id": null,
        "status": "none",
        "account_id": null,
        "email": null,
        "display_name": null,
    })
}

/// Machine-readable enrollment state for the Editor's binding check.
/// Only identifiers Core already shows the operator: never secrets, keys,
/// codes, or tokens. Compact single line so WSL forwarding stays parseable.
fn enrolled_report(machine_id: &str, account_id: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "enrolled": true,
        "machine_id": machine_id,
        "account_id": account_id,
    })
}

/// Pending-enrollment report: the server's verbatim `status` plus the
/// claimed identity when present (`None` reads as JSON null).
fn pending_report(
    enrollment_id: &str,
    status: &artisan_account::PairStatus,
) -> serde_json::Value {
    serde_json::json!({
        "enrolled": false,
        "enrollment_id": enrollment_id,
        "status": status.status.clone(),
        "account_id": status.account_id.clone(),
        "email": status.email.clone(),
        "display_name": status.display_name.clone(),
    })
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

    fn sorted_keys(value: &serde_json::Value) -> Vec<String> {
        let mut keys: Vec<String> = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn status_json_reports_ids_with_exact_keys() {
        let enrolled = enrolled_report("mch_1", Some("acct_1"));
        assert_eq!(enrolled["enrolled"], true);
        assert_eq!(enrolled["machine_id"], "mch_1");
        assert_eq!(enrolled["account_id"], "acct_1");
        assert_eq!(sorted_keys(&enrolled), ["account_id", "enrolled", "machine_id"]);
        let pending = pending_report(
            "enr_1",
            &artisan_account::PairStatus {
                enrollment_id: None,
                status: "claimed".to_owned(),
                account_id: Some("acct_9".to_owned()),
                email: Some("op@example.invalid".to_owned()),
                display_name: None,
                expires_at: None,
            },
        );
        assert_eq!(pending["enrolled"], false);
        assert_eq!(pending["enrollment_id"], "enr_1");
        assert_eq!(pending["status"], "claimed");
        assert_eq!(pending["account_id"], "acct_9");
        assert_eq!(pending["email"], "op@example.invalid");
        assert!(pending["display_name"].is_null());
        assert_eq!(
            sorted_keys(&pending),
            [
                "account_id",
                "display_name",
                "email",
                "enrolled",
                "enrollment_id",
                "status"
            ]
        );
    }

    #[test]
    fn status_json_never_carries_secrets() {
        // Even with every secret field populated on disk, the reports only
        // see identifiers: the builders take ids, never the file.
        let secret = "SECRET-B64U-abcdefghijklmnopqrstuvwxyz012345";
        let public = "PUBLIC-B64U-abcdefghijklmnopqrstuvwxyz012345";
        let enrolled = enrolled_report("mch_1", Some("acct_1"));
        let pending = pending_report(
            "enr_1",
            &artisan_account::PairStatus {
                enrollment_id: None,
                status: "pending".to_owned(),
                account_id: None,
                email: None,
                display_name: None,
                expires_at: None,
            },
        );
        for report in [
            serde_json::to_string(&enrolled).unwrap(),
            serde_json::to_string(&pending).unwrap(),
        ] {
            assert!(!report.contains(secret));
            assert!(!report.contains(public));
            assert!(!report.contains("secret"));
            assert!(!report.contains("token"));
            assert!(!report.contains("code"));
        }
    }

    #[test]
    fn enroll_report_carries_the_created_dto_fields() {
        let report = enroll_report(&forge_http::core::EnrollmentCreated {
            enrollment_id: "enr_1".to_owned(),
            code: "ABCDEFGH".to_owned(),
            expires_at: "2030-01-01T00:00:00.000Z".to_owned(),
            key_fingerprint: "fp".to_owned(),
        });
        assert_eq!(report["code"], "ABCDEFGH");
        assert_eq!(report["enrollment_id"], "enr_1");
        assert_eq!(report["expires_at"], "2030-01-01T00:00:00.000Z");
        assert_eq!(report["key_fingerprint"], "fp");
        assert_eq!(
            sorted_keys(&report),
            ["code", "enrollment_id", "expires_at", "key_fingerprint"]
        );
    }

    #[test]
    fn no_enrollment_json_uses_pending_keys_with_nulls() {
        let report = no_enrollment_report();
        assert_eq!(report["enrolled"], false);
        assert!(report["enrollment_id"].is_null());
        assert_eq!(report["status"], "none");
        assert!(report["account_id"].is_null());
        assert!(report["email"].is_null());
        assert!(report["display_name"].is_null());
        assert_eq!(
            sorted_keys(&report),
            [
                "account_id",
                "display_name",
                "email",
                "enrolled",
                "enrollment_id",
                "status"
            ]
        );
    }
}
