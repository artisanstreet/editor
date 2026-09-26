//! Opt-in installs of the real vendor archives.
//!
//! Skipped unless `ARTISAN_REAL_ENGINE_ARCHIVES` names a directory holding
//! downloaded vendor artifacts named `codex-<version>-linux-x64.tgz` and
//! `cursor-<version>-linux-x64.tar.gz`. Each archive is served through an
//! in-memory transport exactly as the vendor feed would resolve it, installed
//! through the production pipeline, and its entry executable is run with
//! `--version` in the managed environment. Default test runs stay offline.

use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha512};

use super::*;
use crate::engine_core::{
    FeedRequest, HostPlatform, ManagedEngine, ManagedEngineAuthority, resolve_launch_target_in,
};

const ARCHIVES: &str = "ARTISAN_REAL_ENGINE_ARCHIVES";

struct RealVendor(HashMap<String, Vec<u8>>);

impl ReleaseTransport for RealVendor {
    fn fetch(&self, request: &FeedRequest) -> Result<Vec<u8>, TransportError> {
        self.0
            .get(&request.url)
            .cloned()
            .ok_or(TransportError::Rejected)
    }

    fn download(
        &self,
        url: &str,
        bound_bytes: u64,
        sink: &mut dyn Write,
    ) -> Result<u64, TransportError> {
        let body = self.0.get(url).ok_or(TransportError::Rejected)?;
        if body.len() as u64 > bound_bytes {
            return Err(TransportError::TooLarge);
        }
        sink.write_all(body).map_err(|_| TransportError::Sink)?;
        Ok(body.len() as u64)
    }
}

/// Returns the archive and its version, or `None` when the opt-in directory
/// is not configured.
fn real_archive(prefix: &str, suffix: &str) -> Option<(String, Vec<u8>)> {
    let directory = PathBuf::from(std::env::var_os(ARCHIVES)?);
    let entry = fs::read_dir(&directory)
        .expect("the real archive directory is readable")
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
        })
        .unwrap_or_else(|| panic!("{prefix}<version>{suffix} is missing from {ARCHIVES}"));
    let name = entry.file_name().into_string().unwrap();
    let version = name[prefix.len()..name.len() - suffix.len()].to_owned();
    Some((version, fs::read(entry.path()).unwrap()))
}

fn install_and_run(engine: ManagedEngine, vendor: &RealVendor, expected_version: &str) {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("forge.db");
    let authority = ManagedEngineAuthority::for_platform(engine, HostPlatform::LinuxX64);
    let outcome = EngineOperations::new(authority, &database, vendor).ensure_selected(&|_| {});
    assert!(
        matches!(outcome, Ok(SwitchOutcome::Activated(_))),
        "{engine}: {outcome:?}"
    );
    let variable = |name: &str| (name == "PATH").then(|| OsString::from("/usr/bin:/bin"));
    let target = resolve_launch_target_in(engine, &database, &variable).unwrap();
    let output = Command::new(target.executable())
        .arg("--version")
        .env_clear()
        .envs(target.environment().unwrap())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(expected_version),
        "{engine} --version: {:?} {stdout} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(target.executable().starts_with(root.path()));
    assert!(is_executable(target.executable()));
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

#[test]
fn the_real_codex_npm_archive_installs_and_runs() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return;
    }
    let Some((version, tarball)) = real_archive("codex-", "-linux-x64.tgz") else {
        return;
    };
    let integrity = format!("sha512-{}", STANDARD.encode(Sha512::digest(&tarball)));
    let url = format!("https://registry.npmjs.org/@openai/codex/-/codex-{version}-linux-x64.tgz");
    let vendor = RealVendor(HashMap::from([
        (
            "https://registry.npmjs.org/-/package/@openai%2fcodex/dist-tags".to_owned(),
            format!(r#"{{"latest":"{version}"}}"#).into_bytes(),
        ),
        (
            format!("https://registry.npmjs.org/@openai%2fcodex/{version}-linux-x64"),
            format!(
                r#"{{"version":"{version}-linux-x64","dist":{{"tarball":"{url}","integrity":"{integrity}"}}}}"#
            )
            .into_bytes(),
        ),
        (url, tarball),
    ]));
    install_and_run(ManagedEngine::Codex, &vendor, &version);
}

#[test]
fn the_real_cursor_package_installs_and_runs() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return;
    }
    let Some((version, tarball)) = real_archive("cursor-", "-linux-x64.tar.gz") else {
        return;
    };
    let url =
        format!("https://downloads.cursor.com/lab/{version}/linux/x64/agent-cli-package.tar.gz");
    let installer = format!(
        "DOWNLOAD_URL=\"https://downloads.cursor.com/lab/{version}/${{OS}}/${{ARCH}}/agent-cli-package.tar.gz\"\n"
    );
    let vendor = RealVendor(HashMap::from([
        (
            "https://cursor.com/install".to_owned(),
            installer.into_bytes(),
        ),
        (url, tarball),
    ]));
    install_and_run(ManagedEngine::Cursor, &vendor, &version);
}
