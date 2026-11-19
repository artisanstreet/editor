use crate::paths::{self, LocateError};
use crate::service::RestartError;
use crate::ui::AskError;
use artisan_account::{MachineKeyError, MachineKeypair};
use artisan_paths::App;
use clap::{Args, Subcommand};
use forge_http::core::{BuildCoreClientError, CoreCallError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub(crate) struct AccountArgs {
    #[arg(
        long,
        help = "Core base URL (else `AST_CORE_BASE_URL`, else production default)"
    )]
    pub core: Option<String>,
    #[command(subcommand)]
    pub command: AccountCommand,
}

#[derive(Subcommand)]
pub(crate) enum AccountCommand {
    #[command(about = "Start an enrollment: prints a code to claim in a signed-in session")]
    Enroll {
        #[arg(long, help = "Human label for this machine (default: hostname)")]
        label: Option<String>,
        #[arg(
            long,
            help = "Print machine-readable enrollment (code, ids, expiry) instead of human text, for the Editor's binding check"
        )]
        json: bool,
        #[arg(long, help = "Replace a removed enrollment with a fresh machine key")]
        replace: bool,
    },
    #[command(about = "Poll the enrollment: shows pending/claimed + account when claimed")]
    Status {
        #[arg(
            long,
            help = "Print machine-readable JSON (stable keys; never secrets) instead of human text, for the Editor's binding check"
        )]
        json: bool,
    },
    #[command(about = "Confirm after explicit local approval of the shown account")]
    Confirm {
        #[arg(
            long,
            short,
            help = "Approve without asking (the account was already shown + approved)"
        )]
        yes: bool,
        #[arg(
            long,
            help = "Only confirm into this account id; refuse on mismatch before the RPC"
        )]
        account_id: Option<String>,
    },
    #[command(about = "Fetch and apply the latest account config bundle now")]
    Sync,
}

#[derive(Default, Serialize, Deserialize)]
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
pub(crate) enum AccountFileError {
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

#[derive(Debug, thiserror::Error)]
pub(crate) enum AccountError {
    #[error(transparent)]
    Enroll(#[from] EnrollError),
    #[error(transparent)]
    Status(#[from] StatusError),
    #[error(transparent)]
    Confirm(#[from] ConfirmError),
    #[error(transparent)]
    Sync(#[from] SyncError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum EnrollError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    File(#[from] AccountFileError),
    #[error("this Forge is already enrolled; see `ae account status`")]
    AlreadyEnrolled,
    #[error("generate a machine key")]
    Key(#[source] MachineKeyError),
    #[error("give this machine a label with --label")]
    NoLabel,
    #[error("start the account runtime")]
    Runtime(#[source] std::io::Error),
    #[error(transparent)]
    Client(#[from] BuildCoreClientError),
    #[error("start an enrollment for {label:?}")]
    Start {
        label: String,
        #[source]
        source: CoreCallError,
    },
    #[error("restart Forge with the new machine key")]
    Restart(#[source] RestartError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum StatusError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    File(#[from] AccountFileError),
    #[error("no enrollment started; run `ae account enroll`")]
    NotStarted,
    #[error("start the account runtime")]
    Runtime(#[source] std::io::Error),
    #[error(transparent)]
    Client(#[from] BuildCoreClientError),
    #[error("read the status of enrollment {enrollment_id}")]
    Status {
        enrollment_id: String,
        #[source]
        source: CoreCallError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfirmError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    File(#[from] AccountFileError),
    #[error(transparent)]
    Mismatch(#[from] AccountMismatch),
    #[error("no enrollment started; run `ae account enroll`")]
    NotStarted,
    #[error("start the account runtime")]
    Runtime(#[source] std::io::Error),
    #[error(transparent)]
    Client(#[from] BuildCoreClientError),
    #[error("read the status of enrollment {enrollment_id}")]
    Status {
        enrollment_id: String,
        #[source]
        source: CoreCallError,
    },
    #[error("the enrollment is not claimed yet; claim the code in a signed-in session first")]
    Unclaimed,
    #[error(
        "refusing to enroll without explicit approval: re-run with --yes to confirm account {account_id}"
    )]
    NeedsApproval { account_id: String },
    #[error(transparent)]
    Ask(#[from] AskError),
    #[error("confirm enrollment {enrollment_id} into account {account_id}")]
    Confirm {
        enrollment_id: String,
        account_id: String,
        #[source]
        source: CoreCallError,
    },
    #[error("restart Forge as an enrolled machine")]
    Restart(#[source] RestartError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AccountMismatch {
    #[error("--account-id must not be blank")]
    Blank,
    #[error("account mismatch: expected {expected}, found {actual}; refusing to confirm")]
    Other { expected: String, actual: String },
    #[error("account mismatch: expected {expected}, found none; refusing to confirm")]
    Missing { expected: String },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SyncError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("read this machine's identity")]
    Identity(#[source] forge_http::sync::MachineIdentityError),
    #[error("this Forge is not enrolled; run `ae account enroll` first")]
    NotEnrolled,
    #[error("no account recorded; re-run `ae account confirm`")]
    NoAccount,
    #[error("restart Forge to sync the account config")]
    Restart(#[source] RestartError),
    #[error(transparent)]
    Lock(#[from] forge_http::LockDataError),
    #[error("open the Forge store in {}", data.display())]
    Store {
        data: PathBuf,
        #[source]
        source: Box<forge_store::OpenStoreError>,
    },
    #[error("build the Core HTTP client")]
    Http(#[source] reqwest::Error),
    #[error("start the account runtime")]
    Runtime(#[source] std::io::Error),
    #[error("sync the account config of machine {machine_id}")]
    Sync {
        machine_id: String,
        #[source]
        source: forge_http::sync::SyncConfigError,
    },
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

fn account_path() -> Result<PathBuf, LocateError> {
    Ok(paths::data(App::Forge)?.join("ast_account.json"))
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

fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

fn client(core: Option<&str>) -> Result<forge_http::core::CoreClient, BuildCoreClientError> {
    forge_http::core::CoreClient::new(&forge_http::core::core_base_url(core))
}

pub(crate) fn run(args: &AccountArgs) -> Result<(), AccountError> {
    match &args.command {
        AccountCommand::Enroll {
            label,
            json,
            replace,
        } => Ok(enroll(
            args.core.as_deref(),
            label.as_deref(),
            *json,
            *replace,
        )?),
        AccountCommand::Status { json } => Ok(status(args.core.as_deref(), *json)?),
        AccountCommand::Confirm { yes, account_id } => {
            Ok(confirm(args.core.as_deref(), *yes, account_id.as_deref())?)
        }
        AccountCommand::Sync => Ok(sync_config_now(args.core.as_deref())?),
    }
}

fn keyed(path: &Path, account: Account) -> Result<(Account, MachineKeypair), EnrollError> {
    if let Some(key) = account.key() {
        let key = key.clone();
        return Ok((account, key));
    }
    let key = MachineKeypair::generate().map_err(EnrollError::Key)?;
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

fn enroll(
    core: Option<&str>,
    label: Option<&str>,
    json: bool,
    replace: bool,
) -> Result<(), EnrollError> {
    let path = account_path()?;
    let account = load_account(&path)?;
    let enrolled = matches!(account, Account::Enrolled { .. });
    if enrolled && !replace {
        return Err(EnrollError::AlreadyEnrolled);
    }
    let restart = replace && enrolled && crate::service::is_running();
    let key = if replace {
        MachineKeypair::generate().map_err(EnrollError::Key)?
    } else {
        keyed(&path, account)?.1
    };
    let label = label_default(label);
    if label.is_empty() {
        return Err(EnrollError::NoLabel);
    }
    let client = client(core)?;
    let created = runtime()
        .map_err(EnrollError::Runtime)?
        .block_on(client.enroll_start(key.public(), &label, key.secret()))
        .map_err(|source| EnrollError::Start {
            label: label.clone(),
            source,
        })?;
    save_account(
        &path,
        &Account::Pending {
            key,
            enrollment_id: created.enrollment_id.clone(),
        },
    )?;
    if restart {
        crate::service::restart().map_err(EnrollError::Restart)?;
    }
    if json {
        crate::ui::print(format_args!("{}", enroll_report(&created)));
        return Ok(());
    }
    crate::ui::print(format_args!(
        "Claim this code in a signed-in session: {} (expires {})",
        created.code, created.expires_at
    ));
    crate::ui::print(format_args!(
        "Then run `ae account status`, and `ae account confirm`."
    ));
    Ok(())
}

fn enroll_report(created: &forge_http::core::EnrollmentCreated) -> serde_json::Value {
    serde_json::json!({
        "code": created.code.clone(),
        "enrollment_id": created.enrollment_id.clone(),
        "expires_at": created.expires_at.clone(),
        "key_fingerprint": created.key_fingerprint.clone(),
    })
}

fn status(core: Option<&str>, json: bool) -> Result<(), StatusError> {
    let (key, enrollment_id) = match load_account(&account_path()?)? {
        Account::Enrolled {
            machine_id,
            account_id,
            ..
        } => {
            if json {
                crate::ui::print(format_args!(
                    "{}",
                    enrolled_report(&machine_id, Some(&account_id))
                ));
            } else {
                crate::ui::print(format_args!(
                    "Enrolled: machine {machine_id} in account {account_id}."
                ));
            }
            return Ok(());
        }
        Account::Unkeyed | Account::Keyed { .. } => {
            if json {
                crate::ui::print(format_args!("{}", no_enrollment_report()));
                return Ok(());
            }
            return Err(StatusError::NotStarted);
        }
        Account::Pending { key, enrollment_id } => (key, enrollment_id),
    };
    let client = client(core)?;
    let status = runtime()
        .map_err(StatusError::Runtime)?
        .block_on(client.enrollment_status(&enrollment_id, key.secret()))
        .map_err(|source| StatusError::Status {
            enrollment_id: enrollment_id.clone(),
            source,
        })?;
    if json {
        crate::ui::print(format_args!("{}", pending_report(&enrollment_id, &status)));
        return Ok(());
    }
    crate::ui::print(format_args!("Enrollment status: {}", status.status));
    match (status.account_id, status.email) {
        (Some(account_id), email) => {
            crate::ui::print(format_args!("Claimed by account: {account_id}"));
            if let Some(email) = email {
                crate::ui::print(format_args!("Verified email: {email}"));
            }
            if let Some(display_name) = status.display_name {
                crate::ui::print(format_args!("Display name: {display_name}"));
            }
            crate::ui::print(format_args!(
                "If that identity is correct, run `ae account confirm`."
            ));
        }
        (None, _) => crate::ui::print(format_args!("Not claimed yet; claim the code first.")),
    }
    Ok(())
}

fn confirm(
    core: Option<&str>,
    yes: bool,
    expected_account: Option<&str>,
) -> Result<(), ConfirmError> {
    let path = account_path()?;
    let (key, enrollment_id) = match load_account(&path)? {
        Account::Enrolled {
            machine_id,
            account_id,
            ..
        } => {
            check_expected_account(expected_account, Some(&account_id))?;
            crate::ui::print(format_args!("Already enrolled as {machine_id}."));
            return Ok(());
        }
        Account::Unkeyed | Account::Keyed { .. } => return Err(ConfirmError::NotStarted),
        Account::Pending { key, enrollment_id } => (key, enrollment_id),
    };
    let client = client(core)?;
    let runtime = runtime().map_err(ConfirmError::Runtime)?;
    let status = runtime
        .block_on(client.enrollment_status(&enrollment_id, key.secret()))
        .map_err(|source| ConfirmError::Status {
            enrollment_id: enrollment_id.clone(),
            source,
        })?;
    let account_id = status.account_id.clone().ok_or(ConfirmError::Unclaimed)?;
    check_expected_account(expected_account, Some(&account_id))?;
    crate::ui::print(format_args!("Enrollment claimed by account: {account_id}"));
    if let Some(email) = status.email.as_deref() {
        crate::ui::print(format_args!("Verified email: {email}"));
    }
    if let Some(display_name) = status.display_name.as_deref() {
        crate::ui::print(format_args!("Display name: {display_name}"));
    }
    let approved = if yes {
        true
    } else if !crate::ui::interactive() {
        return Err(ConfirmError::NeedsApproval { account_id });
    } else {
        ask(&account_id)?
    };
    if !approved {
        crate::ui::print(format_args!("Not confirmed."));
        return Ok(());
    }
    let confirmed = runtime
        .block_on(client.confirm_enrollment(
            &enrollment_id,
            &account_id,
            key.public(),
            key.secret(),
        ))
        .map_err(|source| ConfirmError::Confirm {
            enrollment_id: enrollment_id.clone(),
            account_id: account_id.clone(),
            source,
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
        crate::service::restart().map_err(ConfirmError::Restart)?;
    }
    crate::ui::success(format!("Enrolled this Forge as {machine_id}."));
    Ok(())
}

fn check_expected_account(
    expected: Option<&str>,
    actual: Option<&str>,
) -> Result<(), AccountMismatch> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let expected = expected.trim();
    if expected.is_empty() {
        return Err(AccountMismatch::Blank);
    }
    match actual {
        Some(actual) if actual == expected => Ok(()),
        Some(actual) => Err(AccountMismatch::Other {
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        }),
        None => Err(AccountMismatch::Missing {
            expected: expected.to_owned(),
        }),
    }
}

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

fn enrolled_report(machine_id: &str, account_id: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "enrolled": true,
        "machine_id": machine_id,
        "account_id": account_id,
    })
}

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

fn ask(account_id: &str) -> Result<bool, AskError> {
    cliclack::confirm(format!(
        "Enroll this Forge into account {account_id}? The code alone is not enough: this binds the local key."
    ))
    .initial_value(true)
    .interact()
    .map_err(crate::ui::prompt_error)
}

fn sync_config_now(core: Option<&str>) -> Result<(), SyncError> {
    let data_dir = paths::data(App::Forge)?;
    let Some(identity) =
        forge_http::sync::load_machine_identity(&data_dir).map_err(SyncError::Identity)?
    else {
        return Err(SyncError::NotEnrolled);
    };
    if identity.account_id.is_none() {
        return Err(SyncError::NoAccount);
    }
    if crate::service::is_running() {
        crate::service::restart().map_err(SyncError::Restart)?;
        crate::ui::success("Restarted Forge; it syncs the account config on startup.");
        return Ok(());
    }
    let _lock = forge_http::lock_data_dir(&data_dir)?;
    let store = forge_store::Store::open_current(&data_dir).map_err(|source| SyncError::Store {
        data: data_dir.clone(),
        source: Box::new(source),
    })?;
    let engines = forge_engines::Engines::new(&data_dir);
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(SyncError::Http)?;
    let outcome = runtime()
        .map_err(SyncError::Runtime)?
        .block_on(async {
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
        })
        .map_err(|source| SyncError::Sync {
            machine_id: identity.machine_id.clone(),
            source,
        })?;
    match outcome {
        forge_http::sync::ConfigOutcome::Updated { state, kept_local } => {
            crate::ui::success(format!("Applied config bundle version {}.", state.version));
            for kept in kept_local {
                crate::ui::warning(format!(
                    "Kept the MCP server {} created on this machine; the account's server of the same name was not applied. Rename one of them to use both.",
                    kept.name
                ));
            }
        }
        forge_http::sync::ConfigOutcome::Current(state) => {
            crate::ui::print(format_args!(
                "Config is current (version {}).",
                state.version
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/account/state.rs"]
mod tests;
