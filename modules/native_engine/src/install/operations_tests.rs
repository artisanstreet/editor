use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256, Sha512};

use super::*;
use crate::engine_core::{
    EngineInspection, EngineUseLease, FeedRequest, HostPlatform, ManagedEngine,
    archive_fixtures::tar_gzip_with_modes,
};

/// In-memory vendor: every URL the operations may request, nothing else.
#[derive(Default)]
struct FixtureVendor {
    documents: HashMap<String, Vec<u8>>,
    requested: Mutex<Vec<String>>,
}

impl FixtureVendor {
    fn with(mut self, url: &str, body: impl Into<Vec<u8>>) -> Self {
        self.documents.insert(url.to_owned(), body.into());
        self
    }

    fn requested(&self) -> Vec<String> {
        self.requested.lock().unwrap().clone()
    }
}

impl ReleaseTransport for FixtureVendor {
    fn fetch(&self, request: &FeedRequest) -> Result<Vec<u8>, TransportError> {
        self.requested.lock().unwrap().push(request.url.clone());
        self.documents
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
        self.requested.lock().unwrap().push(url.to_owned());
        let body = self.documents.get(url).ok_or(TransportError::Rejected)?;
        if body.len() as u64 > bound_bytes {
            return Err(TransportError::TooLarge);
        }
        sink.write_all(body).map_err(|_| TransportError::Sink)?;
        Ok(body.len() as u64)
    }
}

const CLAUDE: &str = "https://downloads.claude.ai/claude-code-releases";

fn claude_vendor(latest: &str, releases: &[(&str, &[u8])]) -> FixtureVendor {
    let mut vendor = FixtureVendor::default().with(&format!("{CLAUDE}/latest"), latest);
    for (version, binary) in releases {
        let checksum = hex(&Sha256::digest(binary));
        vendor = vendor
            .with(
                &format!("{CLAUDE}/{version}/manifest.json"),
                format!(
                    r#"{{"version":"{version}","platforms":{{"linux-x64":{{"binary":"claude","checksum":"{checksum}","size":{}}}}}}}"#,
                    binary.len()
                ),
            )
            .with(&format!("{CLAUDE}/{version}/linux-x64/claude"), *binary);
    }
    vendor
}

fn claude() -> ManagedEngineAuthority {
    ManagedEngineAuthority::for_platform(ManagedEngine::Claude, HostPlatform::LinuxX64)
}

fn database(root: &Path) -> PathBuf {
    root.join("forge.db")
}

fn active_version(authority: ManagedEngineAuthority, database: &Path) -> String {
    authority
        .resolve_active(database)
        .unwrap()
        .version()
        .to_string()
}

fn hex(bytes: &[u8]) -> String {
    super::super::pipeline::hex_lower(bytes)
}

#[test]
fn latest_is_resolved_from_the_feed_and_installed_against_the_manifest_digest() {
    let root = tempfile::tempdir().unwrap();
    let database = database(root.path());
    let vendor = claude_vendor("2.1.283\n", &[("2.1.283", b"claude 2.1.283")]);
    let operations = EngineOperations::new(claude(), &database, &vendor);
    let progress = Mutex::new(Vec::new());
    let outcome = operations
        .ensure_selected(&|step| progress.lock().unwrap().push(step))
        .unwrap();
    assert_eq!(
        outcome,
        SwitchOutcome::Activated(EngineVersion::parse("2.1.283").unwrap())
    );
    let resolved = claude().resolve_active(&database).unwrap();
    assert_eq!(
        fs::read(resolved.executable_path()).unwrap(),
        b"claude 2.1.283"
    );
    assert!(
        resolved.executable_path().starts_with(
            root.path()
                .join("toolchain")
                .join("claude")
                .join("versions")
        )
    );
    assert!(
        progress
            .lock()
            .unwrap()
            .contains(&InstallProgress::Verifying)
    );
    assert_eq!(
        operations.ensure_selected(&|_| {}).unwrap(),
        SwitchOutcome::AlreadyActive(EngineVersion::parse("2.1.283").unwrap())
    );
}

#[test]
fn a_digest_mismatch_installs_nothing() {
    let root = tempfile::tempdir().unwrap();
    let database = database(root.path());
    let vendor = claude_vendor("2.1.283", &[("2.1.283", b"claude 2.1.283")]).with(
        &format!("{CLAUDE}/2.1.283/linux-x64/claude"),
        b"claude 2.1.999".to_vec(),
    );
    let operations = EngineOperations::new(claude(), &database, &vendor);
    assert_eq!(
        operations.ensure_selected(&|_| {}),
        Err(InstallError::IntegrityMismatch)
    );
    assert!(matches!(
        claude().inspect(&database),
        Ok(EngineInspection::NotInstalled)
    ));
    let versions = root.path().join("toolchain/claude/versions");
    assert_eq!(fs::read_dir(versions).unwrap().count(), 0);
}

#[test]
fn versions_below_the_floor_are_never_installed_or_selected() {
    let root = tempfile::tempdir().unwrap();
    let database = database(root.path());
    let vendor = claude_vendor("2.1.100", &[("2.1.100", b"old")]);
    let operations = EngineOperations::new(claude(), &database, &vendor);
    assert_eq!(
        operations.ensure_selected(&|_| {}),
        Err(InstallError::BelowFloor)
    );
    let held = EngineSelection::parse("2.1.219").unwrap();
    assert_eq!(
        operations.select(&held, &|_| {}),
        Err(InstallError::BelowFloor)
    );
    assert!(
        !vendor
            .requested()
            .iter()
            .any(|url| url.ends_with("/claude"))
    );
}

#[test]
fn a_held_selection_persists_and_never_consults_latest() {
    let root = tempfile::tempdir().unwrap();
    let database = database(root.path());
    let vendor = claude_vendor(
        "2.1.283",
        &[
            ("2.1.282", b"claude 2.1.282"),
            ("2.1.283", b"claude 2.1.283"),
        ],
    );
    let operations = EngineOperations::new(claude(), &database, &vendor);
    operations
        .select(&EngineSelection::parse("2.1.282").unwrap(), &|_| {})
        .unwrap();
    assert_eq!(active_version(claude(), &database), "2.1.282");
    let engine_root = root.path().join("toolchain/claude");
    assert_eq!(
        claude().read_selection(&engine_root),
        Ok(EngineSelection::parse("2.1.282").unwrap())
    );
    vendor.requested.lock().unwrap().clear();
    let again = EngineOperations::new(claude(), &database, &vendor);
    assert!(matches!(
        again.ensure_selected(&|_| {}),
        Ok(SwitchOutcome::AlreadyActive(_))
    ));
    assert!(vendor.requested().is_empty(), "{:?}", vendor.requested());

    operations
        .select(&EngineSelection::Latest, &|_| {})
        .unwrap();
    assert_eq!(active_version(claude(), &database), "2.1.283");
}

#[test]
fn a_switch_waits_while_the_engine_is_in_use_and_activates_when_idle() {
    let root = tempfile::tempdir().unwrap();
    let database = database(root.path());
    let vendor = claude_vendor(
        "2.1.282",
        &[
            ("2.1.282", b"claude 2.1.282"),
            ("2.1.283", b"claude 2.1.283"),
        ],
    );
    let operations = EngineOperations::new(claude(), &database, &vendor);
    operations.ensure_selected(&|_| {}).unwrap();
    let paths = claude().install_paths(&database).unwrap();
    let running = EngineUseLease::acquire(&paths).unwrap();

    let updated = claude_vendor(
        "2.1.283",
        &[
            ("2.1.282", b"claude 2.1.282"),
            ("2.1.283", b"claude 2.1.283"),
        ],
    );
    let update = EngineOperations::new(claude(), &database, &updated);
    assert_eq!(
        update.ensure_selected(&|_| {}).unwrap(),
        SwitchOutcome::Pending(EngineVersion::parse("2.1.283").unwrap())
    );
    assert_eq!(active_version(claude(), &database), "2.1.282");
    assert_eq!(
        update.activate_pending().unwrap(),
        Some(SwitchOutcome::Pending(
            EngineVersion::parse("2.1.283").unwrap()
        ))
    );

    drop(running);
    assert_eq!(
        update.activate_pending().unwrap(),
        Some(SwitchOutcome::Activated(
            EngineVersion::parse("2.1.283").unwrap()
        ))
    );
    assert_eq!(active_version(claude(), &database), "2.1.283");
    assert_eq!(update.activate_pending().unwrap(), None);
}

#[test]
fn rollback_restores_the_previous_generation_and_holds_it() {
    let root = tempfile::tempdir().unwrap();
    let database = database(root.path());
    let first = claude_vendor(
        "2.1.282",
        &[
            ("2.1.282", b"claude 2.1.282"),
            ("2.1.283", b"claude 2.1.283"),
        ],
    );
    EngineOperations::new(claude(), &database, &first)
        .ensure_selected(&|_| {})
        .unwrap();
    let second = claude_vendor(
        "2.1.283",
        &[
            ("2.1.282", b"claude 2.1.282"),
            ("2.1.283", b"claude 2.1.283"),
        ],
    );
    let operations = EngineOperations::new(claude(), &database, &second);
    operations.ensure_selected(&|_| {}).unwrap();
    assert_eq!(active_version(claude(), &database), "2.1.283");

    second.requested.lock().unwrap().clear();
    assert_eq!(
        operations.rollback().unwrap(),
        SwitchOutcome::Activated(EngineVersion::parse("2.1.282").unwrap())
    );
    assert_eq!(active_version(claude(), &database), "2.1.282");
    assert!(second.requested().is_empty(), "rollback must not download");
    let engine_root = root.path().join("toolchain/claude");
    assert_eq!(
        claude().read_selection(&engine_root),
        Ok(EngineSelection::parse("2.1.282").unwrap())
    );
    assert!(matches!(
        operations.ensure_selected(&|_| {}),
        Ok(SwitchOutcome::AlreadyActive(_))
    ));
    assert_eq!(
        operations.rollback().unwrap(),
        SwitchOutcome::Activated(EngineVersion::parse("2.1.283").unwrap())
    );
}

#[test]
fn retention_keeps_the_active_and_three_previous_generations() {
    let root = tempfile::tempdir().unwrap();
    let database = database(root.path());
    let releases: Vec<(String, Vec<u8>)> = (280..=285)
        .map(|patch| {
            (
                format!("2.1.{patch}"),
                format!("claude 2.1.{patch}").into_bytes(),
            )
        })
        .collect();
    for (version, _) in &releases {
        let listed: Vec<(&str, &[u8])> = releases
            .iter()
            .map(|(version, bytes)| (version.as_str(), bytes.as_slice()))
            .collect();
        let vendor = claude_vendor(version, &listed);
        EngineOperations::new(claude(), &database, &vendor)
            .ensure_selected(&|_| {})
            .unwrap();
    }
    let paths = claude().install_paths(&database).unwrap();
    let state = claude()
        .read_install_state(paths.engine_root())
        .unwrap()
        .unwrap();
    assert_eq!(state.active.version, "2.1.285");
    let previous: Vec<&str> = state.previous.iter().map(|g| g.version.as_str()).collect();
    assert_eq!(previous, ["2.1.284", "2.1.283", "2.1.282"]);
    assert_eq!(fs::read_dir(paths.versions_root()).unwrap().count(), 4);
}

#[test]
fn version_listing_marks_installed_active_and_below_floor() {
    let root = tempfile::tempdir().unwrap();
    let database = database(root.path());
    let vendor = claude_vendor("2.1.283", &[("2.1.283", b"claude 2.1.283")]).with(
        "https://registry.npmjs.org/@anthropic-ai%2fclaude-code",
        r#"{"versions":{"2.1.283":{},"2.1.282":{},"2.1.100":{},"2.1.283-next.1":{}}}"#,
    );
    let operations = EngineOperations::new(claude(), &database, &vendor);
    operations.ensure_selected(&|_| {}).unwrap();
    let listing = operations.list_versions().unwrap();
    let summary: Vec<(&str, bool, bool, bool)> = listing
        .iter()
        .map(|entry| {
            (
                entry.version.as_str(),
                entry.installed,
                entry.active,
                entry.below_floor,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("2.1.283", true, true, false),
            ("2.1.282", false, false, false),
            ("2.1.100", false, false, true),
        ]
    );
}

#[test]
fn codex_installs_its_package_tree_from_npm_integrity() {
    let root = tempfile::tempdir().unwrap();
    let database = database(root.path());
    let tarball = tar_gzip_with_modes(&[
        ("package/package.json", b"{}".as_slice(), b'0', 0o644),
        (
            "package/vendor/x86_64-unknown-linux-musl/bin/codex",
            b"codex 0.157.1".as_slice(),
            b'0',
            0o755,
        ),
        (
            "package/vendor/x86_64-unknown-linux-musl/codex-path/rg",
            b"rg".as_slice(),
            b'0',
            0o755,
        ),
    ]);
    let integrity = format!("sha512-{}", STANDARD.encode(Sha512::digest(&tarball)));
    let url = "https://registry.npmjs.org/@openai/codex/-/codex-0.157.1-linux-x64.tgz";
    let vendor = FixtureVendor::default()
        .with(
            "https://registry.npmjs.org/-/package/@openai%2fcodex/dist-tags",
            r#"{"latest":"0.157.1","linux-x64":"0.157.1-linux-x64"}"#,
        )
        .with(
            "https://registry.npmjs.org/@openai%2fcodex/0.157.1-linux-x64",
            format!(
                r#"{{"version":"0.157.1-linux-x64","dist":{{"tarball":"{url}","integrity":"{integrity}"}}}}"#
            ),
        )
        .with(url, tarball);
    let codex = ManagedEngineAuthority::for_platform(ManagedEngine::Codex, HostPlatform::LinuxX64);
    EngineOperations::new(codex, &database, &vendor)
        .ensure_selected(&|_| {})
        .unwrap();
    let resolved = codex.resolve_active(&database).unwrap();
    assert_eq!(
        fs::read(resolved.executable_path()).unwrap(),
        b"codex 0.157.1"
    );
    assert_eq!(resolved.tool_dirs().len(), 1);
    assert!(resolved.tool_dirs()[0].join("rg").is_file());
}

#[test]
fn unsupported_engines_never_touch_the_network() {
    let root = tempfile::tempdir().unwrap();
    let database = database(root.path());
    let vendor = FixtureVendor::default();
    for engine in [ManagedEngine::Grok, ManagedEngine::Cursor] {
        let authority = ManagedEngineAuthority::for_platform(engine, HostPlatform::LinuxX64);
        let operations = EngineOperations::new(authority, &database, &vendor);
        assert_eq!(
            operations.ensure_selected(&|_| {}),
            Err(InstallError::UnsupportedPlatform)
        );
        assert!(matches!(
            authority.inspect(&database),
            Ok(EngineInspection::UnsupportedPlatform(_))
        ));
    }
    assert!(vendor.requested().is_empty());
}
