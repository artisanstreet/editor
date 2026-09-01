use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    archive,
    background_process::{background_command, detached_background_command},
    error::{InstallerError, Result, io},
    integrations::{
        OwnedIntegration, apply_protocol, prepare_protocol, remove_protocol, verify_protocol,
    },
    manifest::{Artifact, TrustKey, fetch},
    platform::Platform,
    processes::{Retirement, RetirementPolicy, retire_superseded},
    shortcuts,
};

const ABSOLUTE_ARTIFACT_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const NATIVE_PAYLOAD_LABEL: &str = "native payload";
/// First install configures and verifies Forge, but deliberately leaves launch
/// to the editor's background handoff. That gives the window exact ownership
/// of the process it caused and lets normal window close stop that Forge. An
/// explicit autostart task remains independently owned by `ae setup --autostart`.
const FIRST_RUN_CONFIGURATION_COMMANDS: [&[&str]; 3] = [&["setup"], &["doctor"], &["status"]];

/// Owns exactly one stage directory after its atomic creation succeeds.
/// Cleanup is explicit because its failure must be reported to the caller.
#[derive(Debug)]
struct StageLease {
    path: PathBuf,
    armed: bool,
}

impl StageLease {
    fn acquire(path: PathBuf, release_version: &str) -> Result<Self> {
        match std::fs::create_dir(&path) {
            Ok(()) => Ok(Self { path, armed: true }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(InstallerError::ExistingRelease(release_version.to_owned()))
            }
            Err(error) => Err(io(&path)(error)),
        }
    }

    fn transfer_to(&mut self, release: &Path) -> Result<()> {
        std::fs::rename(&self.path, release).map_err(io(release))?;
        self.armed = false;
        Ok(())
    }

    fn cleanup(&mut self) -> Result<()> {
        if !self.armed {
            return Ok(());
        }
        match remove_owned_stage(&self.path) {
            Ok(()) => {
                self.armed = false;
                Ok(())
            }
            Err(_) => Err(InstallerError::StageCleanupIncomplete),
        }
    }

    fn finish(&self) -> Result<()> {
        if self.armed {
            Err(InstallerError::StageCleanupIncomplete)
        } else {
            Ok(())
        }
    }
}

fn complete_install(stage: &mut StageLease, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => stage.finish(),
        Err(original) => match stage.cleanup() {
            Ok(()) => Err(original),
            Err(cleanup) => Err(cleanup),
        },
    }
}

fn remove_owned_stage(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "owned stage is not an ordinary directory",
        ));
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn metadata_is_symlink_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub struct InstallIntegrationOptions {
    /// Whether this install may own the `artisan://` handler. A secondary
    /// install beside an existing installation must leave the handler with
    /// its current owner rather than fail on finding it taken.
    pub register_protocol: bool,
    /// Whether this install owns the desktop and Start Menu launchers.
    pub register_shortcuts: bool,
}

pub struct InstallOptions {
    pub manifest_url: Url,
    pub signature_url: Url,
    pub platform: Platform,
    pub install_root: PathBuf,
    pub trust: TrustKey,
    pub run_setup: bool,
    pub integrations: InstallIntegrationOptions,
    /// `None` leaves superseded editor and Forge processes running. A policy
    /// retires them and controls whether a stuck Forge may be ended.
    pub retirement: Option<RetirementPolicy>,
}

#[allow(clippy::too_many_lines)]
pub async fn install(options: InstallOptions) -> Result<()> {
    // Plain HTTP is permitted only from this machine's own loopback, which
    // cannot be intercepted off-host. A locally built, locally signed release
    // is installed by serving its output directory on 127.0.0.1; every remote
    // manifest still requires TLS, and the signature check applies to both.
    let loopback_manifest = options.manifest_url.host().is_some_and(|host| match host {
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
        url::Host::Domain(domain) => domain == "localhost",
    });
    let client = reqwest::Client::builder()
        .https_only(!loopback_manifest)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(InstallerError::ManifestRequest)?;
    let artifact_base_url = options
        .manifest_url
        .join("./")
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    let manifest = fetch(
        &client,
        options.manifest_url.clone(),
        options.signature_url.clone(),
        &options.trust,
    )
    .await?;
    let current_version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    let minimum_version = semver::Version::parse(&manifest.minimum_installer_version)
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    if current_version < minimum_version {
        return Err(InstallerError::InstallerTooOld {
            current: current_version.to_string(),
            minimum: minimum_version.to_string(),
        });
    }
    let product_version = semver::Version::parse(&manifest.product_version)
        .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
    let compatibility_version =
        semver::Version::parse(&manifest.editor_forge_compatibility_version)
            .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
    let minimum_cli_version = semver::Version::parse(&manifest.minimum_cli_version)
        .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
    if product_version != compatibility_version || product_version < minimum_cli_version {
        return Err(InstallerError::InvalidRelease(
            "product, Editor/Forge compatibility, and minimum CLI versions disagree".to_owned(),
        ));
    }
    std::fs::create_dir_all(&options.install_root).map_err(io(&options.install_root))?;
    let existing_release = options
        .install_root
        .join("versions")
        .join(&manifest.product_version);
    if existing_release.is_dir() {
        let lifecycle_ae = release_cli(&existing_release)?;
        let existing_protocol = read_existing_protocol(&options.install_root)?;
        let bootstrap = versioned_installer_path(&existing_release);
        if !bootstrap.is_file() {
            return Err(InstallerError::MissingInstaller(bootstrap));
        }
        let retirement = retire_for(&options, &existing_release, &lifecycle_ae)?;
        let stable_ae = install_stable_cli(&options.install_root, &existing_release)?;
        let protocol = if options.integrations.register_protocol {
            prepare_protocol(&options.platform, &stable_ae, existing_protocol.as_ref())?
        } else {
            None
        };
        let launchers = planned_shortcuts(&options, &stable_ae, &existing_release);
        activate(
            &options.install_root,
            &existing_release,
            &manifest,
            &options,
            &stable_ae,
            protocol.as_ref(),
            &shortcut_records(&launchers)?,
        )?;
        if options.integrations.register_protocol {
            apply_protocol(&options.platform, &stable_ae, existing_protocol.as_ref())?;
        }
        shortcuts::apply(&launchers)?;
        restore_retired_forge(&options, &existing_release, retirement)?;
        invoke_ae(&existing_release, &["--version"])?;
        return Ok(());
    }
    let stage = options.install_root.join(format!(
        ".stage-{}-{}",
        manifest.product_version,
        std::process::id()
    ));
    let mut stage_lease = StageLease::acquire(stage.clone(), &manifest.product_version)?;

    let result = async {
        let artifact = manifest
            .artifacts
            .iter()
            .find(|artifact| {
                artifact.platform == options.platform.os
                    && artifact.architecture == options.platform.arch
                    && (options.platform.os != "linux"
                        || artifact.libc.as_deref() == Some(platform_libc()))
            })
            .ok_or_else(|| InstallerError::MissingArtifact {
                component: NATIVE_PAYLOAD_LABEL.to_owned(),
                target: options.platform.target(),
            })?;
        let artifact_url = artifact_base_url
            .join(&artifact.file_name)
            .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
        install_artifact(&client, artifact, artifact_url, &stage).await?;
        // The tree is final: record per-file digests so `ae doctor` can
        // detect payload drift after activation.
        crate::payload::write_manifest(&stage)?;

        let release = options
            .install_root
            .join("versions")
            .join(&manifest.product_version);
        if release.exists() {
            return Err(InstallerError::ExistingRelease(
                manifest.product_version.clone(),
            ));
        }
        if let Some(parent) = release.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        stage_lease.transfer_to(&release)?;
        let lifecycle_ae = release_cli(&release)?;
        let existing_protocol = read_existing_protocol(&options.install_root)?;
        let bootstrap = versioned_installer_path(&release);
        if !bootstrap.is_file() {
            return Err(InstallerError::MissingInstaller(bootstrap));
        }
        let retirement = retire_for(&options, &release, &lifecycle_ae)?;
        let stable_ae = install_stable_cli(&options.install_root, &release)?;
        let protocol = if options.integrations.register_protocol {
            prepare_protocol(&options.platform, &stable_ae, existing_protocol.as_ref())?
        } else {
            None
        };
        let launchers = planned_shortcuts(&options, &stable_ae, &release);
        activate(
            &options.install_root,
            &release,
            &manifest,
            &options,
            &stable_ae,
            protocol.as_ref(),
            &shortcut_records(&launchers)?,
        )?;
        if options.integrations.register_protocol {
            apply_protocol(&options.platform, &stable_ae, existing_protocol.as_ref())?;
        }
        shortcuts::apply(&launchers)?;
        if options.run_setup {
            run_setup_sequence(&release)?;
        } else {
            restore_retired_forge(&options, &release, retirement)?;
        }
        Ok(())
    }
    .await;
    complete_install(&mut stage_lease, result)
}

async fn install_artifact(
    client: &reqwest::Client,
    artifact: &Artifact,
    artifact_url: Url,
    stage: &Path,
) -> Result<()> {
    if artifact.size == 0 || artifact.size > ABSOLUTE_ARTIFACT_LIMIT {
        return Err(InstallerError::ArtifactTooLarge {
            url: artifact_url.clone(),
        });
    }
    let response = client
        .get(artifact_url.clone())
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| InstallerError::ArtifactRequest {
            url: artifact_url.clone(),
            source,
        })?;
    if response
        .content_length()
        .is_some_and(|size| size > artifact.size)
    {
        return Err(InstallerError::ArtifactTooLarge {
            url: artifact_url.clone(),
        });
    }
    let download = stage.join(format!(".{}.download", artifact.id));
    let mut file = File::create(&download).map_err(io(&download))?;
    let mut response = response;
    let mut downloaded = 0_u64;
    let mut hasher = Sha256::new();
    while let Some(chunk) =
        response
            .chunk()
            .await
            .map_err(|source| InstallerError::ArtifactRequest {
                url: artifact_url.clone(),
                source,
            })?
    {
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > artifact.size {
            return Err(InstallerError::ArtifactTooLarge {
                url: artifact_url.clone(),
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(io(&download))?;
    }
    if downloaded != artifact.size {
        return Err(InstallerError::ArtifactSizeMismatch {
            expected: artifact.size,
            actual: downloaded,
        });
    }
    file.sync_all().map_err(io(&download))?;
    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(InstallerError::ChecksumMismatch(artifact_url));
    }
    archive::extract(&download, artifact.format, stage, &artifact.archive_entries)?;
    std::fs::remove_file(&download).map_err(io(&download))?;
    Ok(())
}

pub(crate) fn platform_libc() -> &'static str {
    if cfg!(target_env = "musl") {
        "musl"
    } else {
        "glibc"
    }
}

#[derive(Debug, Serialize)]
struct Components {
    editor: bool,
    forge: bool,
}

fn installed_components() -> Components {
    Components {
        editor: true,
        forge: true,
    }
}

/// The launchers this run owns, or none when the caller opted out.
fn planned_shortcuts(
    options: &InstallOptions,
    stable_ae: &Path,
    release: &Path,
) -> Vec<shortcuts::ShortcutTarget> {
    if options.integrations.register_shortcuts {
        shortcuts::targets(&options.platform, stable_ae, release)
    } else {
        Vec::new()
    }
}

fn shortcut_records(targets: &[shortcuts::ShortcutTarget]) -> Result<Vec<OwnedIntegration>> {
    targets
        .iter()
        .map(shortcuts::ShortcutTarget::owned)
        .collect()
}

/// Closes the old editor and proves Forge can stop before activation changes
/// any durable pointer or integration. A busy Forge therefore cancels the
/// update while the prior installation remains authoritative.
fn retire_for(options: &InstallOptions, release: &Path, stable_ae: &Path) -> Result<Retirement> {
    let Some(policy) = options.retirement else {
        return Ok(Retirement::default());
    };

    let retirement = retire_superseded(&options.install_root, release, stable_ae, policy)?;
    if !retirement.is_empty() {
        println!(
            "retired superseded instances: {} editor, {} forge",
            retirement.editors_closed, retirement.forges_stopped
        );
    }
    Ok(retirement)
}

/// A maintenance update has no editor launch after the installer returns, so
/// preserve a Forge that was running before the update by starting the newly
/// activated version. Setup-driven installs deliberately skip this: their
/// caller opens the editor, whose background handoff must own the Forge it
/// starts so window close can stop that exact process.
fn restore_retired_forge(
    options: &InstallOptions,
    release: &Path,
    retirement: Retirement,
) -> Result<()> {
    if should_restore_retired_forge(options.run_setup, retirement) {
        invoke_ae(release, &["start"])?;
    }
    Ok(())
}

fn run_setup_sequence(release: &Path) -> Result<()> {
    for arguments in FIRST_RUN_CONFIGURATION_COMMANDS {
        invoke_ae(release, arguments)?;
    }
    Ok(())
}

fn should_restore_retired_forge(run_setup: bool, retirement: Retirement) -> bool {
    !run_setup && retirement.forges_stopped > 0
}

fn activate(
    root: &Path,
    release: &Path,
    manifest: &crate::manifest::ReleaseManifest,
    options: &InstallOptions,
    stable_ae: &Path,
    protocol: Option<&OwnedIntegration>,
    launchers: &[OwnedIntegration],
) -> Result<()> {
    let next = root.join(".installation.json.tmp");
    let current = root.join("installation.json");
    let now = Utc::now().to_rfc3339();
    let mut integrations = serde_json::Map::from_iter([(
        "ae_path".to_owned(),
        serde_json::to_value(OwnedIntegration {
            path: stable_ae.display().to_string(),
            fingerprint: hash_file(&release.join("bin").join(if cfg!(windows) {
                "ae.exe"
            } else {
                "ae"
            }))?,
        })
        .map_err(InstallerError::InvalidPayload)?,
    )]);
    if let Some(protocol) = protocol {
        integrations.insert(
            "protocol".to_owned(),
            serde_json::to_value(protocol).map_err(InstallerError::InvalidPayload)?,
        );
    }
    if !launchers.is_empty() {
        integrations.insert(
            "shortcuts".to_owned(),
            serde_json::to_value(launchers).map_err(InstallerError::InvalidPayload)?,
        );
    }
    let contents = serde_json::json!({
        "format_version": 1,
        "install_root": root,
        "platform": options.platform.os,
        "architecture": options.platform.arch,
        "channel": manifest.channel.as_str(),
        "components": installed_components(),
        "integrations": integrations,
        "installed_at": now,
        "updated_at": now,
        "activation_state": "active",
        "finalization_state": "complete",
        "active_version": manifest.product_version.as_str(),
        "permanent_ae_path": stable_ae,
        "artifact": {
            "artifact_id": manifest.artifacts.iter()
                .find(|artifact| artifact.platform == options.platform.os
                    && artifact.architecture == options.platform.arch)
                .map_or("unknown", |artifact| artifact.id.as_str()),
            "sha256": manifest.artifacts.iter()
                .find(|artifact| artifact.platform == options.platform.os
                    && artifact.architecture == options.platform.arch)
                .map_or("", |artifact| artifact.sha256.as_str()),
            "signing_key_id": manifest.signing_identity.key_id.as_str(),
        },
        "transaction": { "state": "idle" }
    });
    let mut file = File::create(&next).map_err(io(&next))?;
    serde_json::to_writer(&mut file, &contents)
        .map_err(|error| InstallerError::Archive(error.to_string()))?;
    file.sync_all().map_err(io(&next))?;
    let previous = root.join(".installation.json.previous");
    if previous.exists() {
        std::fs::remove_file(&previous).map_err(io(&previous))?;
    }
    if current.exists() {
        std::fs::rename(&current, &previous).map_err(io(&current))?;
    }
    if let Err(source) = std::fs::rename(&next, &current) {
        if previous.exists() {
            let _ = std::fs::rename(&previous, &current);
        }
        return Err(io(&current)(source));
    }
    if previous.exists() {
        std::fs::remove_file(&previous).map_err(io(&previous))?;
    }
    Ok(())
}

fn install_stable_cli(root: &Path, release: &Path) -> Result<PathBuf> {
    let source = release_cli(release)?;
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).map_err(io(&bin))?;
    let stable = bin.join(if cfg!(windows) { "ae.exe" } else { "ae" });
    let temporary = bin.join(".ae.next");
    std::fs::copy(&source, &temporary).map_err(io(&temporary))?;
    if stable.exists() {
        if hash_file(&stable)? == hash_file(&source)? {
            std::fs::remove_file(&temporary).map_err(io(&temporary))?;
            integrate_path(&bin)?;
            return Ok(stable);
        }
        if let Err(remove_error) = std::fs::remove_file(&stable) {
            #[cfg(windows)]
            {
                let _ = remove_error;
                schedule_stable_cli_replacement(&temporary, &stable)?;
                integrate_path(&bin)?;
                return Ok(stable);
            }
            #[cfg(not(windows))]
            return Err(io(&stable)(remove_error));
        }
    }
    std::fs::rename(&temporary, &stable).map_err(io(&stable))?;
    integrate_path(&bin)?;
    Ok(stable)
}

fn release_cli(release: &Path) -> Result<PathBuf> {
    let executable = release
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !executable.is_file() {
        return Err(InstallerError::MissingCli(executable));
    }
    Ok(executable)
}

fn versioned_installer_path(release: &Path) -> PathBuf {
    release.join("bin").join(if cfg!(windows) {
        "installer.exe"
    } else {
        "installer"
    })
}

#[cfg(windows)]
fn schedule_stable_cli_replacement(source: &Path, destination: &Path) -> Result<()> {
    detached_background_command("cmd.exe")
        .args([
            "/d",
            "/s",
            "/c",
            "ping 127.0.0.1 -n 3 > nul & move /y \"%ARTISAN_AE_SOURCE%\" \"%ARTISAN_AE_DESTINATION%\" > nul",
        ])
        .env("ARTISAN_AE_SOURCE", source)
        .env("ARTISAN_AE_DESTINATION", destination)
        .spawn()
        .map_err(InstallerError::CleanupHelper)?;
    Ok(())
}

#[cfg(windows)]
fn integrate_path(bin: &Path) -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving the user PATH untouched instead of registering {}",
            bin.display()
        );
        return Ok(());
    }
    let environment = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            "Environment",
            winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
        )
        .map_err(io("HKCU\\Environment"))?;
    let current: String = environment.get_value("Path").unwrap_or_default();
    let candidate = bin.display().to_string();
    let next = prepend_windows_path_entry(&current, &candidate);
    if next != current {
        environment
            .set_value("Path", &next)
            .map_err(io("HKCU\\Environment\\Path"))?;
    }
    Ok(())
}

#[cfg(windows)]
fn prepend_windows_path_entry(current: &str, candidate: &str) -> String {
    std::iter::once(candidate)
        .chain(
            current
                .split(';')
                .filter(|entry| !entry.is_empty() && !entry.eq_ignore_ascii_case(candidate)),
        )
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(unix)]
fn integrate_path(bin: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving ~/.local/bin untouched instead of linking {}",
            bin.display()
        );
        return Ok(());
    }
    let home = std::env::var_os("HOME").ok_or(InstallerError::MissingHome)?;
    let command_bin = PathBuf::from(home).join(".local").join("bin");
    std::fs::create_dir_all(&command_bin).map_err(io(&command_bin))?;
    let link = command_bin.join("ae");
    let target = bin.join("ae");
    if link.symlink_metadata().is_ok() {
        if std::fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
            return Ok(());
        }
        return Err(InstallerError::InvalidInstallation(format!(
            "refusing to replace existing command at {}",
            link.display()
        )));
    }
    symlink(target, &link).map_err(io(&link))
}

pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(io(path))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(io(path))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[derive(Debug, Deserialize)]
struct InstalledState {
    activation_state: String,
    active_version: String,
    install_root: PathBuf,
    permanent_ae_path: PathBuf,
    #[serde(default)]
    integrations: InstalledIntegrations,
}

#[derive(Debug, Default, Deserialize)]
struct InstalledIntegrations {
    ae_path: Option<OwnedIntegration>,
    protocol: Option<OwnedIntegration>,
    /// Absent in installations predating installer-owned launchers, which is
    /// why removal and repair both treat an empty record as "not ours".
    #[serde(default)]
    shortcuts: Vec<OwnedIntegration>,
}

pub fn repair(root: &Path) -> Result<()> {
    let state = read_installed_state(root)?;
    validate_state_root(root, &state)?;
    let release = root.join("versions").join(&state.active_version);
    let bootstrap = versioned_installer_path(&release);
    if !bootstrap.is_file() {
        return Err(InstallerError::MissingInstaller(bootstrap));
    }
    let stable = install_stable_cli(root, &release)?;
    if stable != state.permanent_ae_path {
        return Err(InstallerError::InvalidInstallation(
            "permanent ae path is outside the bootstrap-owned layout".to_owned(),
        ));
    }
    let platform = Platform::detect()?;
    let existing_protocol = state.integrations.protocol.as_ref();
    // An installation with no recorded protocol ownership was installed with
    // `--skip-protocol` beside a primary installation. Repairing it must not
    // adopt the handler the primary owns — finding it registered elsewhere is
    // this installation's healthy state, not damage to fix.
    if existing_protocol.is_some() {
        let protocol = prepare_protocol(&platform, &stable, existing_protocol)?;
        if protocol.as_ref() != state.integrations.protocol.as_ref()
            && let Some(protocol) = protocol.as_ref()
        {
            persist_protocol_record(root, protocol)?;
        }
        apply_protocol(&platform, &stable, existing_protocol)?;
    }
    // Launchers are rewritten rather than merely checked: their icon is taken
    // from the versioned editor executable, so every update leaves the
    // previous release's path behind in an otherwise healthy shortcut.
    let launchers = shortcuts::targets(&platform, &stable, &release);
    if !state.integrations.shortcuts.is_empty() || !launchers.is_empty() {
        let records = shortcut_records(&launchers)?;
        shortcuts::apply(&launchers)?;
        persist_shortcut_records(root, &records)?;
    }
    invoke_ae_diagnostic(&release, &["doctor"])
}

pub fn diagnose(root: &Path) -> Result<()> {
    let state = read_installed_state(root)?;
    validate_state_root(root, &state)?;
    let stable = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if stable != state.permanent_ae_path || !stable.is_file() {
        return Err(InstallerError::InvalidInstallation(
            "permanent ae path is missing or outside the bootstrap-owned layout".to_owned(),
        ));
    }
    verify_protocol(
        &Platform::detect()?,
        &stable,
        state.integrations.protocol.as_ref(),
    )
}

fn invoke_ae_diagnostic(release: &Path, arguments: &[&str]) -> Result<()> {
    let executable = release
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !executable.is_file() {
        return Err(InstallerError::MissingCli(executable));
    }
    // Doctor reports Forge-instance problems independently. Repair owns the
    // installation invariants above and must not recurse through `--fix`.
    let _status = background_command(&executable)
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(io(&executable))?;
    Ok(())
}

pub fn uninstall(root: &Path, remove_data: bool) -> Result<()> {
    let state = read_installed_state(root)?;
    validate_state_root(root, &state)?;
    remove_protocol(
        &Platform::detect()?,
        &state.permanent_ae_path,
        state.integrations.protocol.as_ref(),
    )?;
    shortcuts::remove(
        &shortcuts::targets(
            &Platform::detect()?,
            &state.permanent_ae_path,
            &root.join("versions").join(&state.active_version),
        ),
        &state.integrations.shortcuts,
    )?;
    if let Some(integration) = state.integrations.ae_path {
        let path = Path::new(&integration.path);
        if path.is_file() && hash_file(path)? == integration.fingerprint {
            std::fs::remove_file(path).map_err(io(path))?;
        }
    }
    remove_path_integration(&root.join("bin"))?;
    remove_path_in_root(root, &root.join("bin"))?;
    remove_path_in_root(root, &root.join("installation.json"))?;
    if remove_data {
        // The home hosts one Forge instance at its root; legacy `profiles/`
        // trees predate the single-instance layout and are removed alongside.
        for name in [
            "config.json",
            "secrets.json",
            "state.json",
            "forge.log",
            "data",
            "profiles",
        ] {
            remove_path_in_root(root, &root.join(name))?;
        }
    }
    schedule_installation_cleanup(root, remove_data)
}

#[cfg(windows)]
fn remove_path_integration(bin: &Path) -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving the user PATH untouched instead of removing {}",
            bin.display()
        );
        return Ok(());
    }
    let environment = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            "Environment",
            winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
        )
        .map_err(io("HKCU\\Environment"))?;
    let current: String = environment.get_value("Path").unwrap_or_default();
    let candidate = bin.display().to_string();
    let next = current
        .split(';')
        .filter(|entry| !entry.eq_ignore_ascii_case(&candidate))
        .collect::<Vec<_>>()
        .join(";");
    if next != current {
        environment
            .set_value("Path", &next)
            .map_err(io("HKCU\\Environment\\Path"))?;
    }
    Ok(())
}

#[cfg(unix)]
fn remove_path_integration(bin: &Path) -> Result<()> {
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving ~/.local/bin untouched instead of unlinking {}",
            bin.display()
        );
        return Ok(());
    }
    let home = std::env::var_os("HOME").ok_or(InstallerError::MissingHome)?;
    let link = PathBuf::from(home).join(".local").join("bin").join("ae");
    if link.symlink_metadata().is_ok()
        && std::fs::read_link(&link).ok().as_deref() == Some(bin.join("ae").as_path())
    {
        std::fs::remove_file(&link).map_err(io(&link))?;
    }
    Ok(())
}

fn read_installed_state(root: &Path) -> Result<InstalledState> {
    let path = root.join("installation.json");
    let bytes = std::fs::read(&path).map_err(io(&path))?;
    serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)
}

fn read_existing_protocol(root: &Path) -> Result<Option<OwnedIntegration>> {
    let path = root.join("installation.json");
    if !path.is_file() {
        return Ok(None);
    }
    read_installed_state(root).map(|state| state.integrations.protocol)
}

fn persist_protocol_record(root: &Path, protocol: &OwnedIntegration) -> Result<()> {
    let current = root.join("installation.json");
    let next = root.join(".installation.json.protocol");
    let bytes = std::fs::read(&current).map_err(io(&current))?;
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)?;
    let integrations = document
        .get_mut("integrations")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            InstallerError::InvalidInstallation(
                "installation manifest integrations are missing".to_owned(),
            )
        })?;
    integrations.insert(
        "protocol".to_owned(),
        serde_json::to_value(protocol).map_err(InstallerError::InvalidPayload)?,
    );
    let mut file = File::create(&next).map_err(io(&next))?;
    serde_json::to_writer(&mut file, &document).map_err(InstallerError::InvalidPayload)?;
    file.sync_all().map_err(io(&next))?;

    let previous = root.join(".installation.json.protocol.previous");
    if previous.exists() {
        std::fs::remove_file(&previous).map_err(io(&previous))?;
    }
    std::fs::rename(&current, &previous).map_err(io(&current))?;
    if let Err(source) = std::fs::rename(&next, &current) {
        let _ = std::fs::rename(&previous, &current);
        return Err(io(&current)(source));
    }
    std::fs::remove_file(&previous).map_err(io(&previous))
}

/// Records the launchers a repair just rewrote, through the same
/// write-and-swap the protocol record uses so an interrupted repair never
/// leaves the manifest half-written.
fn persist_shortcut_records(root: &Path, launchers: &[OwnedIntegration]) -> Result<()> {
    let current = root.join("installation.json");
    let next = root.join(".installation.json.shortcuts");
    let bytes = std::fs::read(&current).map_err(io(&current))?;
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)?;
    let integrations = document
        .get_mut("integrations")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            InstallerError::InvalidInstallation(
                "installation manifest integrations are missing".to_owned(),
            )
        })?;
    integrations.insert(
        "shortcuts".to_owned(),
        serde_json::to_value(launchers).map_err(InstallerError::InvalidPayload)?,
    );
    let mut file = File::create(&next).map_err(io(&next))?;
    serde_json::to_writer(&mut file, &document).map_err(InstallerError::InvalidPayload)?;
    file.sync_all().map_err(io(&next))?;

    let previous = root.join(".installation.json.shortcuts.previous");
    if previous.exists() {
        std::fs::remove_file(&previous).map_err(io(&previous))?;
    }
    std::fs::rename(&current, &previous).map_err(io(&current))?;
    if let Err(source) = std::fs::rename(&next, &current) {
        let _ = std::fs::rename(&previous, &current);
        return Err(io(&current)(source));
    }
    std::fs::remove_file(&previous).map_err(io(&previous))
}

fn validate_state_root(root: &Path, state: &InstalledState) -> Result<()> {
    if state.activation_state != "active" || state.install_root != root {
        return Err(InstallerError::InvalidInstallation(
            "installation manifest does not own the requested root".to_owned(),
        ));
    }
    Ok(())
}

fn remove_path_in_root(root: &Path, path: &Path) -> Result<()> {
    if !path.starts_with(root) || path == root {
        return Err(InstallerError::InvalidInstallation(
            "refusing removal outside the installation root".to_owned(),
        ));
    }
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(io(path))?;
    } else if path.exists() {
        std::fs::remove_file(path).map_err(io(path))?;
    }
    Ok(())
}

fn schedule_installation_cleanup(root: &Path, remove_data: bool) -> Result<()> {
    let versions = root.join("versions");
    #[cfg(windows)]
    {
        let mut command = detached_background_command("cmd.exe");
        let script = if remove_data {
            "ping 127.0.0.1 -n 3 > nul & rmdir /s /q \"%ARTISAN_ROOT%\""
        } else {
            "ping 127.0.0.1 -n 3 > nul & rmdir /s /q \"%ARTISAN_VERSIONS%\""
        };
        command.args(["/d", "/s", "/c", script]);
        command.env("ARTISAN_VERSIONS", versions);
        command.env("ARTISAN_ROOT", root);
        command.spawn().map_err(InstallerError::CleanupHelper)?;
    }
    #[cfg(unix)]
    {
        let target = if remove_data {
            root
        } else {
            versions.as_path()
        };
        std::process::Command::new("sh")
            .args(["-c", "sleep 1; rm -rf -- \"$1\"", "artisan-uninstall"])
            .arg(target)
            .spawn()
            .map_err(InstallerError::CleanupHelper)?;
    }
    Ok(())
}

fn invoke_ae(release: &Path, arguments: &[&str]) -> Result<()> {
    let executable = release
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !executable.is_file() {
        return Err(InstallerError::MissingCli(executable));
    }
    let status = background_command(&executable)
        .args(arguments)
        .status()
        .map_err(io(&executable))?;
    if !status.success() {
        return Err(InstallerError::CliFailed {
            command: arguments.join(" "),
            status: status.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::{InstallerError, StageLease, complete_install};

    #[cfg(windows)]
    use super::prepend_windows_path_entry;

    #[cfg(unix)]
    fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }

    #[test]
    fn sha256_representation_matches_release_contract() {
        let mut hasher = Sha256::new();
        hasher.update(b"artisan");
        assert_eq!(
            hex::encode(hasher.finalize()),
            "0b74ed7ff22b86fd0838fd29a78940a8d54377951e968867948a57b3e53646fc"
        );
    }

    #[test]
    fn permanent_lifecycle_binary_has_a_stable_archive_location() {
        let root = tempdir().expect("temp");
        let release = root.path().join("versions").join("1.2.3");
        let expected =
            root.path()
                .join("versions")
                .join("1.2.3")
                .join("bin")
                .join(if cfg!(windows) {
                    "installer.exe"
                } else {
                    "installer"
                });
        assert_eq!(super::versioned_installer_path(&release), expected);
        assert!(!expected.ends_with(if cfg!(windows) {
            "ae-installer.exe"
        } else {
            "ae-installer"
        }));
    }

    #[test]
    fn first_install_leaves_forge_launch_to_the_editor_handoff() {
        assert_eq!(super::FIRST_RUN_CONFIGURATION_COMMANDS[0], ["setup"]);
        assert!(
            super::FIRST_RUN_CONFIGURATION_COMMANDS
                .iter()
                .flat_map(|arguments| arguments.iter())
                .all(|argument| *argument != "start")
        );
        assert!(!super::should_restore_retired_forge(
            true,
            super::Retirement {
                editors_closed: 1,
                forges_stopped: 1,
            },
        ));
    }

    #[test]
    fn maintenance_update_restores_a_previously_running_forge() {
        assert!(super::should_restore_retired_forge(
            false,
            super::Retirement {
                editors_closed: 0,
                forges_stopped: 1,
            },
        ));
        assert!(!super::should_restore_retired_forge(
            false,
            super::Retirement::default(),
        ));
    }

    #[test]
    fn installation_manifest_components_are_always_enabled() {
        assert_eq!(
            serde_json::to_value(super::installed_components()).expect("component projection"),
            serde_json::json!({"editor": true, "forge": true})
        );
    }

    #[test]
    fn failed_install_removes_only_its_owned_stage() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        let sibling = root.path().join(".stage-1.2.3-sibling");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::create_dir(&sibling).expect("sibling stage");
        fs::write(stage.join("partial"), b"partial payload").expect("partial payload");

        let result = complete_install(
            &mut lease,
            Err(InstallerError::Archive(
                "post-acquisition failure".to_owned(),
            )),
        );

        assert!(matches!(
            result,
            Err(InstallerError::Archive(message)) if message == "post-acquisition failure"
        ));
        assert!(!stage.exists());
        assert!(sibling.is_dir());
    }

    #[test]
    fn pre_existing_stage_collision_is_rejected_and_untouched() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        fs::create_dir(&stage).expect("pre-existing stage");
        let marker = stage.join("marker");
        fs::write(&marker, b"keep").expect("collision marker");

        let result = StageLease::acquire(stage.clone(), "1.2.3");

        assert!(matches!(
            result,
            Err(InstallerError::ExistingRelease(version)) if version == "1.2.3"
        ));
        assert!(stage.is_dir());
        assert_eq!(fs::read(marker).expect("collision marker"), b"keep");
    }

    #[test]
    fn missing_owned_stage_cleanup_is_idempotent() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::remove_dir(&stage).expect("remove stage before cleanup");

        assert!(lease.cleanup().is_ok());
        assert!(!lease.armed);
        assert!(lease.cleanup().is_ok());
    }

    #[test]
    fn cleanup_refuses_a_regular_file_target() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::remove_dir(&stage).expect("remove stage before replacement");
        fs::write(&stage, b"do not remove").expect("file replacement");

        let result = lease.cleanup();

        assert!(matches!(
            result,
            Err(InstallerError::StageCleanupIncomplete)
        ));
        assert_eq!(fs::read(stage).expect("file target"), b"do not remove");
    }

    #[test]
    fn cleanup_refuses_a_link_or_reparse_target() {
        let root = tempdir().expect("temp");
        let target = root.path().join("target");
        fs::create_dir(&target).expect("link target");
        let stage = root.path().join(".stage-1.2.3-owned");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::remove_dir(&stage).expect("remove stage before replacement");
        if !create_directory_link(&target, &stage) {
            eprintln!("SKIP: directory links are not supported on this host");
            return;
        }

        let result = lease.cleanup();

        assert!(matches!(
            result,
            Err(InstallerError::StageCleanupIncomplete)
        ));
        assert!(stage.symlink_metadata().is_ok());
        assert!(target.is_dir());
    }

    #[test]
    fn cleanup_failure_takes_precedence_and_is_path_free() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::remove_dir(&stage).expect("remove stage before replacement");
        fs::write(&stage, b"preserve").expect("file replacement");
        let original = format!("original failure at {}", stage.display());

        let error = complete_install(&mut lease, Err(InstallerError::Archive(original)))
            .expect_err("cleanup failure");

        assert!(matches!(error, InstallerError::StageCleanupIncomplete));
        assert_eq!(error.to_string(), "staging cleanup could not be completed");
        assert!(!error.to_string().contains(&stage.display().to_string()));
        assert_eq!(fs::read(stage).expect("file target"), b"preserve");
    }

    #[test]
    fn successful_transfer_disarms_lease_before_later_failure() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        let release_parent = root.path().join("versions");
        let release = release_parent.join("1.2.3");
        fs::create_dir(&release_parent).expect("release parent");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::write(stage.join("payload"), b"release payload").expect("payload");

        lease.transfer_to(&release).expect("stage transfer");
        let result = complete_install(
            &mut lease,
            Err(InstallerError::Archive("activation failure".to_owned())),
        );

        assert!(matches!(
            result,
            Err(InstallerError::Archive(message)) if message == "activation failure"
        ));
        assert!(!lease.armed);
        assert!(!stage.exists());
        assert_eq!(
            fs::read(release.join("payload")).expect("release payload"),
            b"release payload"
        );
    }

    #[cfg(windows)]
    #[test]
    fn stable_cli_precedes_stale_path_entries() {
        let stable = r"C:\Users\test\AppData\Local\Artisan\bin";
        let legacy = r"C:\Users\test\AppData\Local\Programs\artisan-editor\resources\artisan-forge";

        assert_eq!(
            prepend_windows_path_entry(&format!("{legacy};{stable};C:\\Windows"), stable),
            format!("{stable};{legacy};C:\\Windows")
        );
    }
}
