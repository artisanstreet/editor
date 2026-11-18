//! `ae account`: enroll this Forge machine into an AST account.
//!
//! The machine keypair is local (persisted in Forge's data dir, not in the
//! database). Core owns enrollment lifecycle; this CLI only builds the exact
//! proofs Core specifies, polls the authenticated status, and — after the
//! operator explicitly approves the shown accountId + verified email —
//! confirms the exact account/key binding. No parallel enrollment logic.

use anyhow::{Context, Result, bail};
use artisan_account::MachineKeypair;
use artisan_paths::App;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

#[derive(Debug, Default, Serialize, Deserialize)]
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

impl AccountFile {
    fn present_fields(&self) -> String {
        let fields: Vec<&str> = [
            (self.secret_b64u.is_some(), "secret_b64u"),
            (self.public_b64u.is_some(), "public_b64u"),
            (self.enrollment_id.is_some(), "enrollment_id"),
            (self.machine_id.is_some(), "machine_id"),
            (self.account_id.is_some(), "account_id"),
        ]
        .into_iter()
        .filter_map(|(present, name)| present.then_some(name))
        .collect();
        fields.join(", ")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Account {
    Unkeyed,
    Keyed {
        key: MachineKeypair,
    },
    Pending {
        key: MachineKeypair,
        enrollment_id: String,
    },
    Enrolled {
        key: MachineKeypair,
        machine_id: String,
        account_id: String,
    },
}

#[derive(Debug, thiserror::Error)]
enum AccountFileError {
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{} holds {fields}, which is not a state `ae account` writes; move it aside and enroll again", path.display())]
    Inconsistent { path: PathBuf, fields: String },
    #[error("{} holds an unusable machine key: {problem}", path.display())]
    Key {
        path: PathBuf,
        problem: &'static str,
    },
    #[error("save {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Account {
    fn decode(path: &Path, file: AccountFile) -> Result<Self, AccountFileError> {
        let fields = file.present_fields();
        let inconsistent = || AccountFileError::Inconsistent {
            path: path.to_owned(),
            fields: fields.clone(),
        };
        let AccountFile {
            secret_b64u,
            public_b64u,
            enrollment_id,
            machine_id,
            account_id,
        } = file;
        let key = match (secret_b64u, public_b64u) {
            (None, None) => None,
            (Some(secret), Some(public)) => Some(decode_key(path, &secret, &public)?),
            (Some(_), None) | (None, Some(_)) => return Err(inconsistent()),
        };
        match (key, enrollment_id, machine_id, account_id) {
            (None, None, None, None) => Ok(Self::Unkeyed),
            (Some(key), None, None, None) => Ok(Self::Keyed { key }),
            (Some(key), Some(enrollment_id), None, None) => {
                Ok(Self::Pending { key, enrollment_id })
            }
            (Some(key), None, Some(machine_id), Some(account_id)) => Ok(Self::Enrolled {
                key,
                machine_id,
                account_id,
            }),
            (_, _, _, _) => Err(inconsistent()),
        }
    }

    fn encode(&self) -> AccountFile {
        use base64::Engine as _;
        let keyed = |key: &MachineKeypair| AccountFile {
            secret_b64u: Some(
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.secret()),
            ),
            public_b64u: Some(key.public_b64u()),
            ..AccountFile::default()
        };
        match self {
            Self::Unkeyed => AccountFile::default(),
            Self::Keyed { key } => keyed(key),
            Self::Pending { key, enrollment_id } => AccountFile {
                enrollment_id: Some(enrollment_id.clone()),
                ..keyed(key)
            },
            Self::Enrolled {
                key,
                machine_id,
                account_id,
            } => AccountFile {
                machine_id: Some(machine_id.clone()),
                account_id: Some(account_id.clone()),
                ..keyed(key)
            },
        }
    }

    const fn key(&self) -> Option<&MachineKeypair> {
        match self {
            Self::Unkeyed => None,
            Self::Keyed { key } | Self::Pending { key, .. } | Self::Enrolled { key, .. } => {
                Some(key)
            }
        }
    }
}

fn decode_key(path: &Path, secret: &str, public: &str) -> Result<MachineKeypair, AccountFileError> {
    use base64::Engine as _;
    let problem = |problem| AccountFileError::Key {
        path: path.to_owned(),
        problem,
    };
    let secret: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(secret)
        .map_err(|_encoding| problem("the secret is not base64url"))?
        .try_into()
        .map_err(|_length| problem("the secret is not 32 bytes"))?;
    let public: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(public)
        .map_err(|_encoding| problem("the public key is not base64url"))?
        .try_into()
        .map_err(|_length| problem("the public key is not 32 bytes"))?;
    let key =
        MachineKeypair::from_secret(secret).map_err(|_secret| problem("the secret is invalid"))?;
    if key.public() != &public {
        return Err(problem("the public key does not match the secret"));
    }
    Ok(key)
}

fn account_path() -> Result<PathBuf> {
    Ok(App::Forge.data()?.join("ast_account.json"))
}

fn load_account(path: &Path) -> Result<Account, AccountFileError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Account::Unkeyed),
        Err(source) => {
            return Err(AccountFileError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    let file = serde_json::from_slice(&bytes).map_err(|source| AccountFileError::Parse {
        path: path.to_owned(),
        source,
    })?;
    Account::decode(path, file)
}

fn save_account(path: &Path, account: &Account) -> Result<(), AccountFileError> {
    let bytes = serde_json::to_vec_pretty(&account.encode())
        .expect("invariant: an account file of strings always serializes");
    write_private(path, &bytes).map_err(|source| AccountFileError::Write {
        path: path.to_owned(),
        source,
    })
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let folder = path
        .parent()
        .expect("invariant: the account file lives in Forge's data folder");
    std::fs::create_dir_all(folder)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".ast_account-")
        .tempfile_in(folder)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    std::fs::File::open(folder)?.sync_all()?;
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
        AccountCommand::Enroll {
            label,
            json,
            replace,
        } => enroll(args.core.as_deref(), label.as_deref(), *json, *replace),
        AccountCommand::Status { json } => status(args.core.as_deref(), *json),
        AccountCommand::Confirm { yes, account_id } => {
            confirm(args.core.as_deref(), *yes, account_id.as_deref())
        }
        AccountCommand::Sync => sync_config_now(args.core.as_deref()),
    }
}

fn keyed(path: &Path, account: Account) -> Result<(Account, MachineKeypair)> {
    if let Some(key) = account.key() {
        let key = key.clone();
        return Ok((account, key));
    }
    let key = MachineKeypair::generate().context("generate a machine key")?;
    let account = Account::Keyed { key: key.clone() };
    save_account(path, &account)?;
    Ok((account, key))
}

fn label_default(label: Option<&str>) -> String {
    label
        .map_or_else(
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
    let path = account_path()?;
    let account = load_account(&path)?;
    let enrolled = matches!(account, Account::Enrolled { .. });
    if enrolled && !replace {
        bail!("this Forge is already enrolled; see `ae account status`");
    }
    let restart = replace && enrolled && crate::service::is_running();
    let key = if replace {
        MachineKeypair::generate().context("generate a machine key")?
    } else {
        keyed(&path, account)?.1
    };
    let label = label_default(label);
    if label.is_empty() {
        bail!("give this machine a label with --label");
    }
    let created = runtime()?.block_on(async {
        client(core)?
            .enroll_start(key.public(), &label, key.secret())
            .await
    })?;
    save_account(
        &path,
        &Account::Pending {
            key,
            enrollment_id: created.enrollment_id.clone(),
        },
    )?;
    if restart {
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
    let (key, enrollment_id) = match load_account(&account_path()?)? {
        Account::Enrolled {
            machine_id,
            account_id,
            ..
        } => {
            if json {
                println!("{}", enrolled_report(&machine_id, Some(&account_id)));
            } else {
                println!("Enrolled: machine {machine_id} in account {account_id}.");
            }
            return Ok(());
        }
        Account::Unkeyed | Account::Keyed { .. } => {
            if json {
                println!("{}", no_enrollment_report());
                return Ok(());
            }
            bail!("no enrollment started; run `ae account enroll`");
        }
        Account::Pending { key, enrollment_id } => (key, enrollment_id),
    };
    let status = runtime()?.block_on(async {
        client(core)?
            .enrollment_status(&enrollment_id, key.secret())
            .await
    })?;
    if json {
        println!("{}", pending_report(&enrollment_id, &status));
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
    let path = account_path()?;
    let (key, enrollment_id) = match load_account(&path)? {
        Account::Enrolled {
            machine_id,
            account_id,
            ..
        } => {
            check_expected_account(expected_account, Some(&account_id))?;
            println!("Already enrolled as {machine_id}.");
            return Ok(());
        }
        Account::Unkeyed | Account::Keyed { .. } => {
            bail!("no enrollment started; run `ae account enroll`")
        }
        Account::Pending { key, enrollment_id } => (key, enrollment_id),
    };
    let status = runtime()?.block_on(async {
        client(core)?
            .enrollment_status(&enrollment_id, key.secret())
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
        client(core)?
            .confirm_enrollment(&enrollment_id, &account_id, key.public(), key.secret())
            .await
    })?;
    let machine_id = confirmed.machine_id.clone();
    save_account(
        &path,
        &Account::Enrolled {
            key,
            machine_id: confirmed.machine_id,
            account_id: confirmed.account_id,
        },
    )?;
    if crate::service::is_running() {
        crate::service::restart()?;
    }
    crate::ui::success(format!("Enrolled this Forge as {machine_id}."));
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
        Some(actual) => {
            bail!("account mismatch: expected {expected}, found {actual}; refusing to confirm")
        }
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
fn pending_report(enrollment_id: &str, status: &artisan_account::PairStatus) -> serde_json::Value {
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
    if identity.account_id.is_none() {
        bail!("no account recorded; re-run `ae account confirm`");
    }
    // A serving Forge owns the store and syncs on startup; opening the store
    // here would recover (cancel) its live runs.
    if crate::service::is_running() {
        crate::service::restart()?;
        crate::ui::success("Restarted Forge; it syncs the account config on startup.");
        return Ok(());
    }
    let _lock = forge_http::lock_data_dir(&data_dir)?;
    let store = forge_store::Store::open(&data_dir).context("open the Forge store")?;
    let engines = forge_engines::Engines::new(&data_dir);
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build the Core HTTP client")?;
    let outcome = runtime()?.block_on(async {
        let hub = forge_runs::Hub::new(store, engines, data_dir.clone());
        let outcome = forge_http::sync::sync_config(
            &http,
            &forge_http::core::core_base_url(core),
            &identity.machine_id,
            &identity.secret,
            &hub,
            &data_dir,
        )
        .await;
        hub.shutdown();
        outcome
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
#[path = "../tests/account/state.rs"]
mod tests;
