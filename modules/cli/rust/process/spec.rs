use std::{
    env,
    ffi::{OsStr, OsString},
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    CliError, Result, credentials::ForgeCredentialPaths, instance::NativeInstanceConfig,
    manifest::InstallationManifest,
};

use super::{MAX_READINESS_BYTES, READY_SCHEMA};
#[derive(Clone, Eq, PartialEq)]
pub struct ForgeLaunchSpec {
    pub(super) executable: PathBuf,
    pub(super) argv: Vec<OsString>,
    pub(super) readiness_path: PathBuf,
}

impl fmt::Debug for ForgeLaunchSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForgeLaunchSpec")
            .field("argv_count", &self.argv.len())
            .finish_non_exhaustive()
    }
}

impl ForgeLaunchSpec {
    pub fn new(
        manifest: &InstallationManifest,
        config: &NativeInstanceConfig,
        credentials: &ForgeCredentialPaths,
    ) -> Result<Self> {
        validate_credential_manifest(config, credentials)?;

        Ok(Self {
            executable: manifest.forge_executable(),
            argv: native_argv(
                config,
                credentials.certificate_paths(),
                credentials.private_key_path(),
                credentials.capability_path(),
            ),
            readiness_path: config.readiness_path().to_path_buf(),
        })
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn argv(&self) -> &[OsString] {
        &self.argv
    }

    pub fn readiness_path(&self) -> &Path {
        &self.readiness_path
    }
}

pub(crate) fn validate_credential_manifest(
    config: &NativeInstanceConfig,
    credentials: &ForgeCredentialPaths,
) -> Result<()> {
    let configured_manifest = config.credentials_manifest().to_path_buf();
    let credential_manifest = credentials.manifest_path().to_path_buf();
    if configured_manifest != credential_manifest {
        return Err(CliError::CredentialManifestMismatch {
            configured: configured_manifest,
            credentials: credential_manifest,
        });
    }
    Ok(())
}

/// The non-secret receipt a native Forge publishes after its listener and
/// certificate identity are ready.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ForgeReadiness {
    schema: String,
    endpoint: String,
    certificate_sha256: String,
    pid: std::num::NonZeroU32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgeReadinessFile {
    schema: String,
    endpoint: String,
    certificate_sha256: String,
    pid: u32,
}

impl ForgeReadiness {
    pub fn new(
        schema: impl Into<String>,
        endpoint: impl Into<String>,
        certificate_sha256: impl Into<String>,
        pid: u32,
    ) -> Result<Self> {
        let schema = schema.into();
        if schema != READY_SCHEMA {
            return Err(CliError::InvalidForgeReadiness {
                reason: "schema is not artisan-forge-ready-v1",
            });
        }

        let endpoint = endpoint.into();
        if !is_exact_loopback_endpoint(&endpoint) {
            return Err(CliError::InvalidForgeReadiness {
                reason: "endpoint is not an IPv4 127.0.0.1 address with a nonzero port",
            });
        }

        let certificate_sha256 = certificate_sha256.into();
        if !is_sha256_hex(&certificate_sha256) {
            return Err(CliError::InvalidForgeReadiness {
                reason: "certificate SHA-256 is not exactly 64 ASCII hex characters",
            });
        }

        let pid = std::num::NonZeroU32::new(pid).ok_or(CliError::InvalidForgeReadiness {
            reason: "PID must be nonzero",
        })?;

        Ok(Self {
            schema,
            endpoint,
            certificate_sha256,
            pid,
        })
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_READINESS_BYTES {
            return Err(CliError::InvalidForgeReadiness {
                reason: "receipt exceeds its size bound",
            });
        }
        serde_json::from_slice(bytes).map_err(|_| CliError::InvalidForgeReadiness {
            reason: "receipt JSON is malformed or has an unsupported shape",
        })
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn certificate_sha256(&self) -> &str {
        &self.certificate_sha256
    }

    pub fn pid(&self) -> u32 {
        self.pid.get()
    }
}

impl<'de> Deserialize<'de> for ForgeReadiness {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let file = ForgeReadinessFile::deserialize(deserializer)?;
        Self::new(
            file.schema,
            file.endpoint,
            file.certificate_sha256,
            file.pid,
        )
        .map_err(|error| <D::Error as serde::de::Error>::custom(error.to_string()))
    }
}

fn is_exact_loopback_endpoint(endpoint: &str) -> bool {
    let Some(port_text) = endpoint.strip_prefix("127.0.0.1:") else {
        return false;
    };
    if port_text.is_empty()
        || (port_text.len() > 1 && port_text.starts_with('0'))
        || !port_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    port_text.parse::<u16>().is_ok_and(|port| port != 0)
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The result a status caller can safely report without treating readiness as
/// authenticated lifecycle or busy state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForgeReadinessStatus {
    Missing,
    Invalid,
    Ready(ForgeReadiness),
}

pub(super) fn native_argv(
    config: &NativeInstanceConfig,
    certificate_paths: &[PathBuf],
    private_key_path: &Path,
    capability_path: &Path,
) -> Vec<OsString> {
    let mut argv = Vec::with_capacity(38 + certificate_paths.len() * 2);
    append_path(&mut argv, "--database", config.database_path());
    append_path(&mut argv, "--custody", config.custody_path());
    for certificate_path in certificate_paths {
        append_path(&mut argv, "--certificate-der", certificate_path);
    }
    append_path(&mut argv, "--private-key-der", private_key_path);
    append_path(&mut argv, "--bootstrap-capability", capability_path);
    append_path(&mut argv, "--ready-file", config.readiness_path());
    append_number(
        &mut argv,
        "--admission-timeout-ms",
        config.listener().admission_timeout_ms(),
    );
    append_number(
        &mut argv,
        "--handshake-timeout-ms",
        config.listener().handshake_timeout_ms(),
    );
    append_number(
        &mut argv,
        "--request-timeout-ms",
        config.listener().request_timeout_ms(),
    );
    append_number(
        &mut argv,
        "--drain-timeout-ms",
        config.listener().drain_timeout_ms(),
    );
    append_number(
        &mut argv,
        "--admission-capacity",
        u64::from(config.listener().admission_capacity().get()),
    );
    append_number(
        &mut argv,
        "--requests-per-connection",
        u64::from(config.listener().requests_per_connection().get()),
    );
    append_number(
        &mut argv,
        "--native-run-claim-lease-ms",
        config.native_run().claim_lease_ms(),
    );
    append_number(
        &mut argv,
        "--native-run-poll-interval-ms",
        config.native_run().poll_interval_ms(),
    );
    append_number(
        &mut argv,
        "--native-run-retry-backoff-ms",
        config.native_run().retry_backoff_ms(),
    );
    append_number(
        &mut argv,
        "--native-run-shutdown-budget-ms",
        config.native_run().shutdown_budget_ms(),
    );
    append_number(
        &mut argv,
        "--native-run-queue-capacity",
        u64::from(config.native_run().queue_capacity().get()),
    );
    append_number(
        &mut argv,
        "--native-run-max-command-retries",
        u64::from(config.native_run().max_command_retries().get()),
    );
    append_text(
        &mut argv,
        "--native-run-prompt-delivery",
        config.native_run().prompt_delivery(),
    );
    append_number(
        &mut argv,
        "--native-run-stream-after",
        config.native_run().stream_after(),
    );
    argv
}

fn append_path(argv: &mut Vec<OsString>, option: &str, path: &Path) {
    argv.push(OsString::from(option));
    argv.push(path.as_os_str().to_os_string());
}

fn append_number(argv: &mut Vec<OsString>, option: &str, value: u64) {
    argv.push(OsString::from(option));
    argv.push(value.to_string().into());
}

fn append_text(argv: &mut Vec<OsString>, option: &str, value: &str) {
    argv.push(OsString::from(option));
    argv.push(OsString::from(value));
}

pub(super) fn forge_command(spec: &ForgeLaunchSpec) -> Command {
    let mut command = Command::new(spec.executable());
    command.args(spec.argv());
    configure_native_environment(&mut command);
    command
}

fn configure_native_environment(command: &mut Command) {
    configure_environment(command, env::vars_os());
}

pub(super) fn configure_environment<I>(command: &mut Command, variables: I)
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    command.env_clear();
    for (key, value) in variables {
        if !is_forbidden_environment_key(&key) {
            command.env(key, value);
        }
    }
}

pub(super) fn is_forbidden_environment_key(key: &OsStr) -> bool {
    starts_with_ascii_case_insensitive(key, b"ARTISAN_")
        || starts_with_ascii_case_insensitive(key, b"NODE")
        || starts_with_ascii_case_insensitive(key, b"ELECTRON")
        || key
            .as_encoded_bytes()
            .eq_ignore_ascii_case(b"CODEX_SQLITE_HOME")
}

fn starts_with_ascii_case_insensitive(value: &OsStr, prefix: &[u8]) -> bool {
    value
        .as_encoded_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

pub(super) fn ensure_forge_executable(spec: &ForgeLaunchSpec) -> Result<()> {
    if spec.executable().is_file() {
        return Ok(());
    }
    Err(CliError::Installation(format!(
        "Forge binary is missing at {}",
        spec.executable().display()
    )))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartResult {
    AlreadyRunning,
    Spawned { pid: u32 },
    ForegroundExited,
}
