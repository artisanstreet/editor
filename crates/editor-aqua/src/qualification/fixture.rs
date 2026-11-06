use anyhow::{Context, Result, ensure};
use artisan_api::{
    CreateProject, CreateThread, Item, ItemId, ItemKind, ItemsQuery, Project, Run, RunId, Status,
    Thread, ThreadId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct HeadChanged;

impl std::fmt::Display for HeadChanged {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Forge changed during snapshot")
    }
}

impl std::error::Error for HeadChanged {}

pub struct ForgeServer {
    pub store: forge_store::Store,
    pub address: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
}

pub struct PairingInput {
    pub address: String,
    pub code: String,
    pub fingerprint: String,
    pub expires_at: artisan_api::Timestamp,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InitialItem {
    pub id: String,
    pub position: u64,
    pub seq: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InitialCheckpoint {
    pub name: String,
    pub account: String,
    pub host: String,
    pub thread: String,
    pub agent: Option<String>,
    pub items: Vec<InitialItem>,
}

#[derive(Serialize)]
pub struct InitialPayload<O, P> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub version: u32,
    pub generation: u64,
    pub checkpoint: InitialCheckpoint,
    pub raw: InitialRaw<O, P>,
}

#[derive(Serialize)]
pub struct InitialRaw<O, P> {
    pub observation: O,
    pub paint: P,
}

pub fn initial_items(items: &[Item]) -> Vec<InitialItem> {
    items
        .iter()
        .map(|item| InitialItem {
            id: item.id.to_string(),
            position: item.position,
            seq: item.seq,
        })
        .collect()
}

pub fn supported_initial_items(items: &[Item]) -> bool {
    !items.is_empty() && items.iter().all(|item| {
        item.parent_id.is_none() && item.run_id.is_none() && item.status == Status::Done &&
        matches!(&item.kind, ItemKind::AssistantMessage { text: artisan_api::TextContent::Inline(text), .. }
            if !text.trim().is_empty() && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte.is_ascii_whitespace()))
    })
}

pub struct InitialReadiness<'a> {
    pub actual: &'a [Item],
    pub expected: &'a [Item],
    pub prepared: &'a [String],
    pub page_loaded: bool,
    pub page_in_flight: bool,
    pub page_exhausted: bool,
    pub page_gap: bool,
    pub load_error: bool,
    pub landing: bool,
    pub work_in_flight: bool,
}

pub fn initial_content_ready(state: &InitialReadiness<'_>) -> bool {
    state.actual == state.expected
        && supported_initial_items(state.expected)
        && state.page_loaded
        && !state.page_in_flight
        && state.page_exhausted
        && !state.page_gap
        && !state.load_error
        && !state.landing
        && !state.work_in_flight
        && state
            .expected
            .iter()
            .all(|item| state.prepared.contains(&item.id.to_string()))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TreeFile {
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TreeManifest {
    pub directories: BTreeSet<PathBuf>,
    pub files: BTreeMap<PathBuf, TreeFile>,
}

impl TreeManifest {
    pub fn capture_stable(root: &Path) -> Result<Self> {
        let root = marked_directory(root)?;
        let first = Self::capture(&root)?;
        let second = Self::capture(&root)?;
        ensure!(first == second, "qualification tree changed during capture");
        Ok(second)
    }

    fn capture(root: &Path) -> Result<Self> {
        fn visit(root: &Path, relative: &Path, manifest: &mut TreeManifest) -> Result<()> {
            for entry in std::fs::read_dir(root.join(relative))? {
                let entry = entry?;
                let path = relative.join(entry.file_name());
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    manifest.directories.insert(path.clone());
                    visit(root, &path, manifest)?;
                } else {
                    ensure!(
                        kind.is_file(),
                        "qualification tree contains a non-regular entry: {}",
                        path.display()
                    );
                    let before = entry.metadata()?;
                    let bytes = std::fs::read(entry.path())?;
                    let after = entry.metadata()?;
                    ensure!(
                        before.len() == after.len()
                            && before.modified()? == after.modified()?
                            && after.len() == bytes.len() as u64,
                        "qualification file changed during capture: {}",
                        path.display()
                    );
                    manifest.files.insert(
                        path,
                        TreeFile {
                            size: after.len(),
                            sha256: hex::encode(Sha256::digest(&bytes)),
                        },
                    );
                }
            }
            Ok(())
        }
        let mut manifest = Self {
            directories: BTreeSet::new(),
            files: BTreeMap::new(),
        };
        visit(root, Path::new(""), &mut manifest)?;
        Ok(manifest)
    }

    pub fn digest(&self) -> Result<String> {
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(self)?)))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedSnapshot {
    pub logical_path: PathBuf,
    pub tree: TreeManifest,
    pub store: StoreSnapshot,
}

fn separate_directories(left: &Path, right: &Path) -> Result<(PathBuf, PathBuf)> {
    let canonical_left = marked_directory(left)?;
    let canonical_right = marked_directory(right)?;
    ensure!(
        left == canonical_left && right == canonical_right,
        "qualification paths must be canonical and contain no symlinks"
    );
    let left = canonical_left;
    let right = canonical_right;
    ensure!(
        !left.starts_with(&right) && !right.starts_with(&left),
        "qualification directories overlap"
    );
    Ok((left, right))
}

fn require_empty_tree(path: &Path) -> Result<()> {
    ensure!(
        std::fs::read_dir(path)?
            .all(|entry| entry.is_ok_and(|entry| entry.file_name() == ".artisan-qualification")),
        "qualification snapshot directory is not empty"
    );
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path, manifest: &TreeManifest) -> Result<()> {
    for path in manifest.directories.iter().chain(manifest.files.keys()) {
        ensure!(
            path.components()
                .all(|component| matches!(component, std::path::Component::Normal(_))),
            "invalid qualification tree path"
        );
    }
    for directory in &manifest.directories {
        std::fs::create_dir_all(destination.join(directory))?;
    }
    for file in manifest.files.keys() {
        std::fs::copy(source.join(file), destination.join(file))?;
    }
    ensure!(
        TreeManifest::capture_stable(source)? == *manifest,
        "qualification source tree changed while copying"
    );
    ensure!(
        TreeManifest::capture_stable(destination)? == *manifest,
        "qualification copied tree differs from source"
    );
    Ok(())
}

fn checkpoint_database(root: &Path) -> Result<()> {
    let path = root.join("forge.sqlite3");
    let connection =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(Duration::ZERO)?;
    let (busy, _, _): (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    ensure!(busy == 0, "Forge database checkpoint is busy");
    connection.execute_batch("BEGIN EXCLUSIVE; COMMIT;")?;
    drop(connection);
    let wal = root.join("forge.sqlite3-wal");
    if wal.exists() {
        ensure!(
            std::fs::metadata(wal)?.len() == 0,
            "Forge WAL is not empty after checkpoint"
        );
    }
    Ok(())
}

impl SeedSnapshot {
    pub async fn produce(
        server: ForgeServer,
        fixture: &Fixture,
        logical_path: &Path,
        snapshot_dir: &Path,
    ) -> Result<Self> {
        let (logical_path, snapshot_dir) = separate_directories(logical_path, snapshot_dir)?;
        require_empty_tree(&snapshot_dir)?;
        ensure!(
            canonical_path(
                server
                    .store
                    .blob_dir()
                    .parent()
                    .context("Forge data directory is unavailable")?
            )?
                == logical_path,
            "Forge server does not own the pinned logical path"
        );
        let store = fixture.seed(&server.store).await?;
        server.shutdown_unique().await?;
        checkpoint_database(&logical_path)?;
        let tree = TreeManifest::capture_stable(&logical_path)?;
        copy_tree(&logical_path, &snapshot_dir, &tree)?;
        Ok(Self {
            logical_path,
            tree,
            store,
        })
    }

    pub fn restore_after_all_forge_handles_closed(
        &self,
        snapshot_dir: &Path,
        logical_path: &Path,
    ) -> Result<()> {
        let (logical_path, snapshot_dir) = separate_directories(logical_path, snapshot_dir)?;
        ensure!(
            logical_path == self.logical_path,
            "Forge restore path differs from pinned logical path"
        );
        ensure!(
            TreeManifest::capture_stable(&snapshot_dir)? == self.tree,
            "qualification snapshot tree changed"
        );
        checkpoint_database(&logical_path)?;
        let old = TreeManifest::capture_stable(&logical_path)?;
        for file in old.files.keys() {
            if file != Path::new(".artisan-qualification") {
                std::fs::remove_file(logical_path.join(file))?;
            }
        }
        for directory in old.directories.iter().rev() {
            std::fs::remove_dir(logical_path.join(directory))?;
        }
        copy_tree(&snapshot_dir, &logical_path, &self.tree)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapRequest {
    pub fixture: PathBuf,
    pub fixture_sha256: String,
    pub snapshot: PathBuf,
    pub snapshot_sha256: String,
    pub forge_dir: PathBuf,
    pub forge_logical_path: PathBuf,
    pub seed_database_sha256: String,
    pub editor_dir: PathBuf,
    pub editor_settings_sha256: Option<String>,
    pub listen: SocketAddr,
    pub host_id: String,
    pub thread_id: String,
    pub width: f32,
    pub height: f32,
}

pub struct VerifiedBootstrap {
    pub request: BootstrapRequest,
    pub fixture: ImmutableFixture,
    pub snapshot: StoreSnapshot,
    pub server: ForgeServer,
    pub editor_dir: PathBuf,
}

fn read_checked(path: &Path, expected: &str) -> Result<Vec<u8>> {
    ensure!(
        expected.len() == 64
            && expected
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid expected SHA-256"
    );
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    ensure!(
        hex::encode(Sha256::digest(&bytes)) == expected,
        "qualification input hash mismatch: {}",
        path.display()
    );
    Ok(bytes)
}

pub fn strip_verbatim(path: PathBuf) -> PathBuf {
    path.to_str()
        .and_then(|text| text.strip_prefix(r"\\?\"))
        .filter(|rest| {
            let bytes = rest.as_bytes();
            bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
        })
        .map(PathBuf::from)
        .unwrap_or(path)
}

pub fn canonical_path(path: &Path) -> std::io::Result<PathBuf> {
    path.canonicalize().map(strip_verbatim)
}

fn marked_directory(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute(),
        "qualification directory must be absolute"
    );
    let canonical =
        canonical_path(path).with_context(|| format!("resolve {}", path.display()))?;
    ensure!(
        std::fs::read(canonical.join(".artisan-qualification"))? == b"artisan-qualification-v1\n",
        "qualification directory marker is invalid"
    );
    Ok(canonical)
}

fn verify_editor_settings(editor_dir: &Path, expected: Option<&str>) -> Result<()> {
    let path = editor_dir.join("settings.json");
    match expected {
        Some(expected) => {
            read_checked(&path, expected)?;
        }
        None => match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("check qualification Editor settings"),
            Ok(_) => anyhow::bail!("qualification Editor settings must be absent"),
        },
    }
    Ok(())
}

impl VerifiedBootstrap {
    pub async fn load(path: &Path) -> Result<Self> {
        let request: BootstrapRequest =
            serde_json::from_slice(&std::fs::read(path).context("read qualification request")?)
                .context("decode qualification request")?;
        ensure!(
            request.listen.ip().is_loopback() && request.listen.port() != 0,
            "qualification requires a fixed loopback port"
        );
        ensure!(
            request.width.is_finite()
                && request.height.is_finite()
                && request.width >= 480.0
                && request.height >= 420.0,
            "invalid qualification window dimensions"
        );
        ensure!(
            !request.host_id.trim().is_empty() && !request.thread_id.trim().is_empty(),
            "qualification host and thread IDs are required"
        );
        let forge_dir = marked_directory(&request.forge_dir)?;
        let expected_path =
            canonical_path(&request.forge_logical_path).context("resolve pinned Forge path")?;
        ensure!(
            forge_dir == expected_path,
            "Forge logical path does not match the pinned seed"
        );
        let editor_dir = marked_directory(&request.editor_dir)?;
        ensure!(
            !editor_dir.starts_with(&forge_dir) && !forge_dir.starts_with(&editor_dir),
            "qualification data directories overlap"
        );
        for entry in std::fs::read_dir(&editor_dir)? {
            let entry = entry?;
            ensure!(
                entry.file_name() == ".artisan-qualification"
                    || entry.file_name() == "settings.json",
                "qualification Editor directory is not fresh"
            );
        }
        verify_editor_settings(&editor_dir, request.editor_settings_sha256.as_deref())?;
        let wal = forge_dir.join("forge.sqlite3-wal");
        if wal.exists() {
            ensure!(
                std::fs::metadata(wal)?.len() == 0,
                "Forge seed WAL is not checkpointed"
            );
        }
        read_checked(
            &forge_dir.join("forge.sqlite3"),
            &request.seed_database_sha256,
        )?;
        let fixture =
            ImmutableFixture::parse(read_checked(&request.fixture, &request.fixture_sha256)?)?;
        let snapshot: StoreSnapshot =
            serde_json::from_slice(&read_checked(&request.snapshot, &request.snapshot_sha256)?)
                .context("decode verified Forge snapshot")?;
        ensure!(
            fixture
                .fixture()
                .threads
                .iter()
                .any(|thread| thread.id.as_str() == request.thread_id),
            "qualification thread is not in the fixture"
        );
        let server = ForgeServer::start(forge_dir, request.listen).await?;
        snapshot
            .verify_stable(&server.store, fixture.fixture())
            .await
            .context("Forge startup does not match the verified seed snapshot")?;
        ensure!(
            server.address == request.listen,
            "Forge bound an unexpected address"
        );
        Ok(Self {
            request,
            fixture,
            snapshot,
            server,
            editor_dir,
        })
    }

    pub fn verify_inputs(&self) -> Result<()> {
        self.fixture.verify_unchanged(&self.request.fixture)?;
        read_checked(&self.request.snapshot, &self.request.snapshot_sha256)?;
        verify_editor_settings(
            &self.editor_dir,
            self.request.editor_settings_sha256.as_deref(),
        )?;
        Ok(())
    }
}

impl ForgeServer {
    pub async fn start(data_dir: PathBuf, listen: SocketAddr) -> Result<Self> {
        let server = forge_http::start(forge_http::ServeConfig { data_dir, listen })
            .await
            .context("start qualification Forge")?;
        let address = server
            .local_addr()
            .context("read qualification Forge address")?;
        let store = server.store().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            server
                .run_until(async {
                    let _ = stopped.await;
                })
                .await
        });
        Ok(Self {
            store,
            address,
            stop: Some(stop),
            task: Some(task),
        })
    }

    pub async fn pairing_input(&self) -> Result<PairingInput> {
        let invite = forge_http::invite(&self.store)
            .await
            .context("create Forge pairing invitation")?;
        let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate)
            .context("read Forge certificate fingerprint")?;
        Ok(PairingInput {
            address: self.address.to_string(),
            code: invite.code,
            fingerprint,
            expires_at: invite.expires_at,
        })
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await
                .context("join qualification Forge")?
                .context("stop qualification Forge")?;
        }
        Ok(())
    }

    pub async fn shutdown_unique(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await
                .context("join qualification Forge")?
                .context("stop qualification Forge")?;
        }
        let store = self.store.clone();
        drop(self);
        tokio::time::timeout(SNAPSHOT_TIMEOUT, async {
            while !store.qualification_is_unique() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("Forge store still has live handles")?;
        store.shutdown_unique()
    }
}

impl Drop for ForgeServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub version: u32,
    pub projects: Vec<CreateProject>,
    pub threads: Vec<CreateThread>,
    pub runs: Vec<Run>,
    pub items: Vec<FixtureItem>,
    #[serde(default)]
    pub settings: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureItem {
    pub id: ItemId,
    pub thread_id: ThreadId,
    pub run_id: Option<RunId>,
    pub parent_id: Option<ItemId>,
    pub native_id: Option<String>,
    pub status: Status,
    pub kind: ItemKind,
}

pub struct ImmutableFixture {
    input: Vec<u8>,
    fixture: Fixture,
}

impl ImmutableFixture {
    pub fn read(path: &Path) -> Result<Self> {
        let input =
            std::fs::read(path).with_context(|| format!("read fixture {}", path.display()))?;
        Self::parse(input)
    }

    pub fn parse(input: Vec<u8>) -> Result<Self> {
        let fixture: Fixture =
            serde_json::from_slice(&input).context("decode qualification fixture")?;
        fixture.validate()?;
        Ok(Self { input, fixture })
    }

    pub fn sha256(&self) -> String {
        hex::encode(Sha256::digest(&self.input))
    }

    pub fn fixture(&self) -> &Fixture {
        &self.fixture
    }

    pub fn verify_unchanged(&self, path: &Path) -> Result<()> {
        let current =
            std::fs::read(path).with_context(|| format!("read fixture {}", path.display()))?;
        ensure!(current == self.input, "qualification fixture changed");
        Ok(())
    }
}

impl Fixture {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported qualification fixture version"
        );
        let mut projects = BTreeSet::new();
        for project in &self.projects {
            ensure!(
                !project.id.as_str().trim().is_empty() && projects.insert(project.id.as_str()),
                "project IDs must be nonempty and unique"
            );
            ensure!(!project.path.trim().is_empty(), "project path is empty");
        }
        let mut threads = BTreeMap::new();
        for thread in &self.threads {
            if let Some(parent) = &thread.parent_thread_id {
                ensure!(
                    threads.contains_key(parent.as_str()),
                    "parent thread must precede its child"
                );
            }
            ensure!(
                !thread.id.as_str().trim().is_empty()
                    && threads
                        .insert(thread.id.as_str(), thread.project_id.as_str())
                        .is_none(),
                "thread IDs must be nonempty and unique"
            );
            ensure!(
                projects.contains(thread.project_id.as_str()),
                "thread refers to an unknown project"
            );
        }
        let mut runs = BTreeMap::new();
        for run in &self.runs {
            ensure!(
                !run.id.as_str().trim().is_empty()
                    && runs
                        .insert(run.id.as_str(), run.thread_id.as_str())
                        .is_none(),
                "run IDs must be nonempty and unique"
            );
            ensure!(
                threads.contains_key(run.thread_id.as_str()),
                "run refers to an unknown thread"
            );
            ensure!(
                !run.status.is_live(),
                "live runs are not supported in a restart-stable seed"
            );
        }
        let mut items = BTreeMap::new();
        for item in &self.items {
            if let Some(parent) = &item.parent_id {
                ensure!(
                    items.get(parent.as_str()) == Some(&item.thread_id.as_str()),
                    "parent item must precede its child in the same thread"
                );
            }
            ensure!(
                !item.id.as_str().trim().is_empty()
                    && items
                        .insert(item.id.as_str(), item.thread_id.as_str())
                        .is_none(),
                "item IDs must be nonempty and unique"
            );
            ensure!(
                threads.contains_key(item.thread_id.as_str()),
                "item refers to an unknown thread"
            );
            if let Some(run) = &item.run_id {
                ensure!(
                    runs.get(run.as_str()) == Some(&item.thread_id.as_str()),
                    "item refers to an unknown or mismatched run"
                );
            }
        }
        ensure!(
            self.settings.keys().all(|key| !key.trim().is_empty()),
            "setting key is empty"
        );
        Ok(())
    }

    pub async fn seed(&self, store: &forge_store::Store) -> Result<StoreSnapshot> {
        self.validate()?;
        let initial = store.state().await.context("read initial Forge state")?;
        ensure!(
            initial.threads.is_empty() && initial.runs.is_empty() && initial.threads_next.is_none(),
            "Forge seed store is not empty"
        );
        let scratch_path = store
            .blob_dir()
            .parent()
            .context("Forge data directory is unavailable")?
            .join("scratch");
        ensure!(
            initial.projects.len() <= 1
                && initial
                    .projects
                    .iter()
                    .all(|project| project.id.is_scratch()
                        && project.name == "No project"
                        && !project.removed
                        && Path::new(&project.path) == scratch_path),
            "Forge contains an unexpected built-in project"
        );
        let builtins: BTreeSet<_> = initial
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect();
        for project in &self.projects {
            if project.id.is_scratch() && builtins.contains(&project.id) {
                ensure!(
                    Path::new(&project.path) == scratch_path
                        && project.name.as_deref() == Some("No project"),
                    "fixture scratch project does not match Forge's built-in project"
                );
            }
        }
        for project in &self.projects {
            store
                .create_project(project.clone())
                .await
                .context("seed project")?;
        }
        for thread in &self.threads {
            store
                .create_thread(thread.clone())
                .await
                .context("seed thread")?;
        }
        for run in &self.runs {
            store.upsert_run(run.clone()).await.context("seed run")?;
        }
        for item in &self.items {
            store
                .upsert_item(forge_store::NewItem {
                    id: item.id.clone(),
                    thread_id: item.thread_id.clone(),
                    run_id: item.run_id.clone(),
                    parent_id: item.parent_id.clone(),
                    native_id: item.native_id.clone(),
                    status: item.status,
                    kind: item.kind.clone(),
                })
                .await
                .context("seed item")?;
        }
        for (key, value) in &self.settings {
            store
                .set_setting(key.clone(), value.clone())
                .await
                .context("seed setting")?;
        }
        let snapshot = StoreSnapshot::capture_stable(store, self).await?;
        let expected: BTreeSet<_> = self.items.iter().map(|item| item.id.clone()).collect();
        let actual: BTreeSet<_> = snapshot
            .items
            .values()
            .flatten()
            .map(|item| item.id.clone())
            .collect();
        ensure!(actual == expected, "seeded items do not match fixture");
        let mut expected_projects: BTreeSet<_> = self
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect();
        expected_projects.extend(builtins);
        let actual_projects: BTreeSet<_> = snapshot
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect();
        let expected_threads: BTreeSet<_> = self
            .threads
            .iter()
            .map(|thread| thread.id.clone())
            .collect();
        let actual_threads: BTreeSet<_> = snapshot
            .threads
            .iter()
            .map(|thread| thread.id.clone())
            .collect();
        ensure!(
            expected_projects == actual_projects && expected_threads == actual_threads,
            "seeded projects or threads do not match fixture"
        );
        Ok(snapshot)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoreSnapshot {
    pub head: u64,
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
    pub runs: Vec<Run>,
    pub settings: BTreeMap<String, serde_json::Value>,
    pub items: BTreeMap<String, Vec<Item>>,
}

impl StoreSnapshot {
    pub async fn capture(store: &forge_store::Store, fixture: &Fixture) -> Result<Self> {
        tokio::time::timeout(SNAPSHOT_TIMEOUT, Self::capture_retry(store, fixture))
            .await
            .context("timed out waiting for a consistent Forge snapshot")?
    }

    pub async fn capture_stable(store: &forge_store::Store, fixture: &Fixture) -> Result<Self> {
        tokio::time::timeout(SNAPSHOT_TIMEOUT, async {
            loop {
                let first = Self::capture_retry(store, fixture).await?;
                let second = Self::capture_retry(store, fixture).await?;
                if first.head != second.head {
                    tokio::task::yield_now().await;
                    continue;
                }
                ensure!(first == second, "Forge snapshots differ at the same head");
                return Ok(second);
            }
        })
        .await
        .context("timed out waiting for two consistent Forge snapshots")?
    }

    async fn capture_retry(store: &forge_store::Store, fixture: &Fixture) -> Result<Self> {
        loop {
            match Self::capture_once(store, fixture).await {
                Err(error) if error.is::<HeadChanged>() => tokio::task::yield_now().await,
                result => return result,
            }
        }
    }

    async fn capture_once(store: &forge_store::Store, fixture: &Fixture) -> Result<Self> {
        let state = store.state().await.context("read Forge snapshot")?;
        let head = state.head;
        ensure!(
            state.threads_next.is_none(),
            "Forge snapshot has an unobserved thread page"
        );
        let mut items = BTreeMap::new();
        for thread in &fixture.threads {
            let mut after = 0;
            let mut all = Vec::new();
            loop {
                let page = store
                    .items_page(
                        &thread.id,
                        ItemsQuery {
                            before: None,
                            after: Some(after),
                            limit: Some(200),
                        },
                    )
                    .await
                    .context("read Forge item page")?;
                if let Some(last) = page.items.last() {
                    ensure!(last.position > after, "Forge item cursor did not advance");
                    after = last.position;
                } else {
                    ensure!(!page.more, "Forge returned an empty nonterminal item page");
                }
                all.extend(page.items);
                if !page.more {
                    break;
                }
            }
            items.insert(thread.id.as_str().to_owned(), all);
        }
        if store.head() != head {
            return Err(HeadChanged.into());
        }
        Ok(Self {
            head,
            projects: state.projects,
            threads: state.threads,
            runs: state.runs,
            settings: state.settings.into_iter().collect(),
            items,
        })
    }

    pub async fn verify_stable(&self, store: &forge_store::Store, fixture: &Fixture) -> Result<()> {
        let next = Self::capture(store, fixture).await?;
        ensure!(&next == self, "Forge snapshot changed");
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointState {
    pub name: String,
    pub route: String,
    pub focus: Option<String>,
    pub pointer: Option<(i32, i32)>,
    pub scroll: BTreeMap<String, (i64, i64)>,
    pub animation_time_ns: u64,
    pub pending: Vec<String>,
    pub observations: BTreeMap<String, serde_json::Value>,
}

impl CheckpointState {
    pub fn digest(&self) -> Result<String> {
        ensure!(
            !self.name.trim().is_empty() && !self.route.trim().is_empty(),
            "checkpoint name and route are required"
        );
        ensure!(
            self.scroll
                .keys()
                .chain(self.observations.keys())
                .all(|key| !key.trim().is_empty()),
            "state keys cannot be empty"
        );
        ensure!(
            self.pending.iter().all(|item| !item.trim().is_empty())
                && self.pending.windows(2).all(|pair| pair[0] < pair[1]),
            "pending operations must be named, sorted and unique"
        );
        let value = canonical(serde_json::to_value(self)?);
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(&value)?)))
    }
}

fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical).collect())
        }
        value => value,
    }
}
