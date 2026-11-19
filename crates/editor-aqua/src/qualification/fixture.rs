use crate::core::HostId;
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
const MARKER: &[u8] = b"artisan-qualification-v1\n";

pub type ForgeFailure = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum MarkerError {
    #[error("{} is not an absolute path", path.display())]
    Relative { path: PathBuf },
    #[error("resolve {}", path.display())]
    Resolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read the qualification marker in {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} carries no valid qualification marker", path.display())]
    Invalid { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureTreeError {
    #[error("check the tree root")]
    Marker(#[source] MarkerError),
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} is neither a regular file nor a directory", path.display())]
    NotRegular { path: PathBuf },
    #[error("{} changed while it was read", path.display())]
    FileChanged { path: PathBuf },
    #[error("the tree at {} changed between two captures", root.display())]
    Unstable { root: PathBuf },
}

#[derive(Debug, thiserror::Error)]
#[error("encode the manifest")]
pub struct EncodeError(#[source] serde_json::Error);

#[derive(Debug, thiserror::Error)]
pub enum SeparateError {
    #[error("check a qualification directory")]
    Marker(#[source] MarkerError),
    #[error("{} is not canonical or passes through a symlink", path.display())]
    NotCanonical { path: PathBuf },
    #[error("{} and {} overlap", left.display(), right.display())]
    Overlap { left: PathBuf, right: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum CopyTreeError {
    #[error("the manifest names {}, which leaves the tree", path.display())]
    InvalidPath { path: PathBuf },
    #[error("create {}", path.display())]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("copy {}", path.display())]
    Copy {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("capture a copied tree")]
    Capture(#[source] CaptureTreeError),
    #[error("{} changed while it was copied", path.display())]
    SourceChanged { path: PathBuf },
    #[error("the copy at {} differs from its source", path.display())]
    CopyDiffers { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("open {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("checkpoint {}", path.display())]
    Checkpoint {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("{} is busy and could not be checkpointed", path.display())]
    Busy { path: PathBuf },
    #[error("read the size of {}", path.display())]
    Wal {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} still holds pages after the checkpoint", path.display())]
    WalNotEmpty { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum ProduceSnapshotError {
    #[error("check the Forge and snapshot directories")]
    Directories(#[source] SeparateError),
    #[error("list the snapshot directory {}", path.display())]
    ListSnapshot {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the snapshot directory {} is not empty", path.display())]
    SnapshotNotEmpty { path: PathBuf },
    #[error("Forge's blob directory {} has no parent data directory", path.display())]
    NoDataDirectory { path: PathBuf },
    #[error("resolve Forge's data directory {}", path.display())]
    ResolveDataDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Forge serves {}, not the pinned path {}", actual.display(), pinned.display())]
    NotPinned { actual: PathBuf, pinned: PathBuf },
    #[error("seed Forge")]
    Seed(#[source] SeedError),
    #[error("stop Forge")]
    Shutdown(#[source] ShutdownError),
    #[error("checkpoint Forge's database")]
    Checkpoint(#[source] CheckpointError),
    #[error("capture the seeded tree")]
    Capture(#[source] CaptureTreeError),
    #[error("copy the seeded tree")]
    Copy(#[source] CopyTreeError),
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreSnapshotError {
    #[error("check the Forge and snapshot directories")]
    Directories(#[source] SeparateError),
    #[error("{} is not the pinned path {}", requested.display(), pinned.display())]
    NotPinned { requested: PathBuf, pinned: PathBuf },
    #[error("capture a tree")]
    Capture(#[source] CaptureTreeError),
    #[error("the snapshot at {} changed", path.display())]
    SnapshotChanged { path: PathBuf },
    #[error("checkpoint Forge's database")]
    Checkpoint(#[source] CheckpointError),
    #[error("remove {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("copy the snapshot back")]
    Copy(#[source] CopyTreeError),
    #[error("{} has no parent folder to stage a restore beside", path.display())]
    NoParent { path: PathBuf },
    #[error("{} is left over but isn't a qualification folder", path.display())]
    Leftover {
        path: PathBuf,
        #[source]
        source: MarkerError,
    },
    #[error("create the staging folder {}", path.display())]
    Stage {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("move {} to {}", from.display(), to.display())]
    Swap {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("move {} back to {} after {failed}", from.display(), to.display())]
    RollBack {
        from: PathBuf,
        to: PathBuf,
        failed: std::io::Error,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ReadCheckedError {
    #[error("the expected SHA-256 of {} is not 64 lowercase hex digits", path.display())]
    InvalidDigest { path: PathBuf },
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("hash mismatch: {}", path.display())]
    Mismatch { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum EditorSettingsError {
    #[error("check the Editor settings")]
    Read(#[source] ReadCheckedError),
    #[error("check whether {} exists", path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Editor settings must be absent, but {} exists", path.display())]
    Present { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum LoadBootstrapError {
    #[error("read the qualification request {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode the qualification request {}", path.display())]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("qualification listens on {address}, not a fixed loopback port")]
    Listen { address: SocketAddr },
    #[error("the qualification window {width}x{height} is too small or not finite")]
    Dimensions { width: f32, height: f32 },
    #[error("the qualification host and thread ids are required")]
    MissingIds,
    #[error("check a qualification directory")]
    Marker(#[source] MarkerError),
    #[error("resolve the pinned Forge path {}", path.display())]
    ResolvePinned {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Forge's directory {} is not the pinned seed path {}", forge.display(), pinned.display())]
    NotPinned { forge: PathBuf, pinned: PathBuf },
    #[error("the Editor directory {} and Forge directory {} overlap", editor.display(), forge.display())]
    Overlap { editor: PathBuf, forge: PathBuf },
    #[error("list the Editor directory {}", path.display())]
    ListEditor {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the Editor directory is not fresh: it holds {}", path.display())]
    EditorNotFresh { path: PathBuf },
    #[error("check the Editor settings")]
    EditorSettings(#[source] EditorSettingsError),
    #[error("read the size of {}", path.display())]
    Wal {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the Forge seed WAL {} is not checkpointed", path.display())]
    WalNotEmpty { path: PathBuf },
    #[error("check the seed database")]
    Database(#[source] ReadCheckedError),
    #[error("check the fixture")]
    FixtureInput(#[source] ReadCheckedError),
    #[error("parse the fixture")]
    Fixture(#[source] FixtureError),
    #[error("check the store snapshot")]
    SnapshotInput(#[source] ReadCheckedError),
    #[error("decode the store snapshot {}", path.display())]
    DecodeSnapshot {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("thread {thread} is not in the fixture")]
    UnknownThread { thread: ThreadId },
    #[error("start Forge")]
    Start(#[source] StartForgeError),
    #[error("Forge started from a store that differs from the verified seed snapshot")]
    Diverged(#[source] SnapshotError),
    #[error("Forge bound {actual}, not {expected}")]
    Address {
        expected: SocketAddr,
        actual: SocketAddr,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyInputsError {
    #[error("check the fixture")]
    Fixture(#[source] FixtureError),
    #[error("check the store snapshot")]
    Snapshot(#[source] ReadCheckedError),
    #[error("check the Editor settings")]
    EditorSettings(#[source] EditorSettingsError),
}

#[derive(Debug, thiserror::Error)]
pub enum StartForgeError {
    #[error("start Forge in {}", data_dir.display())]
    Start {
        data_dir: PathBuf,
        #[source]
        source: ForgeFailure,
    },
    #[error("read the address Forge bound")]
    Address(#[source] ForgeFailure),
}

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("create a Forge pairing invitation")]
    Invite(#[source] ForgeFailure),
    #[error("read the Forge certificate fingerprint")]
    Fingerprint(#[source] artisan_api::tls::DecodeCertificateError),
}

#[derive(Debug, thiserror::Error)]
pub enum ShutdownError {
    #[error("join the Forge task")]
    Join(#[source] tokio::task::JoinError),
    #[error("stop Forge")]
    Stop(#[source] ForgeFailure),
    #[error("Forge's store still had live handles after {SNAPSHOT_TIMEOUT:?}")]
    LiveHandles,
    #[error("close Forge's store")]
    Store(#[source] forge_store::ShutdownError),
}

pub struct ForgeServer {
    pub store: forge_store::Store,
    pub address: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<(), ForgeFailure>>>,
}

impl std::fmt::Debug for ForgeServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ForgeServer")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct PairingInput {
    pub address: String,
    pub code: String,
    pub fingerprint: String,
    pub expires_at: artisan_api::Timestamp,
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

fn read_error(path: PathBuf) -> impl FnOnce(std::io::Error) -> CaptureTreeError {
    move |source| CaptureTreeError::Read { path, source }
}

impl TreeManifest {
    pub fn capture_stable(root: &Path) -> Result<Self, CaptureTreeError> {
        let root = marked_directory(root).map_err(CaptureTreeError::Marker)?;
        let first = Self::capture(&root)?;
        let second = Self::capture(&root)?;
        if first != second {
            return Err(CaptureTreeError::Unstable { root });
        }
        Ok(second)
    }

    fn capture(root: &Path) -> Result<Self, CaptureTreeError> {
        fn visit(
            root: &Path,
            relative: &Path,
            manifest: &mut TreeManifest,
        ) -> Result<(), CaptureTreeError> {
            let directory = root.join(relative);
            for entry in std::fs::read_dir(&directory).map_err(read_error(directory.clone()))? {
                let entry = entry.map_err(read_error(directory.clone()))?;
                let path = relative.join(entry.file_name());
                let kind = entry.file_type().map_err(read_error(entry.path()))?;
                if kind.is_dir() {
                    manifest.directories.insert(path.clone());
                    visit(root, &path, manifest)?;
                    continue;
                }
                if !kind.is_file() {
                    return Err(CaptureTreeError::NotRegular { path });
                }
                let before = entry.metadata().map_err(read_error(entry.path()))?;
                let bytes = std::fs::read(entry.path()).map_err(read_error(entry.path()))?;
                let after = entry.metadata().map_err(read_error(entry.path()))?;
                let modified = |metadata: &std::fs::Metadata| {
                    metadata.modified().map_err(read_error(entry.path()))
                };
                if before.len() != after.len()
                    || modified(&before)? != modified(&after)?
                    || after.len() != bytes.len() as u64
                {
                    return Err(CaptureTreeError::FileChanged { path });
                }
                manifest.files.insert(
                    path,
                    TreeFile {
                        size: after.len(),
                        sha256: hex::encode(Sha256::digest(&bytes)),
                    },
                );
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

    pub fn digest(&self) -> Result<String, EncodeError> {
        Ok(hex::encode(Sha256::digest(
            serde_json::to_vec(self).map_err(EncodeError)?,
        )))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedSnapshot {
    pub logical_path: PathBuf,
    pub tree: TreeManifest,
    pub store: StoreSnapshot,
}

fn separate_directories(left: &Path, right: &Path) -> Result<(PathBuf, PathBuf), SeparateError> {
    let canonical_left = marked_directory(left).map_err(SeparateError::Marker)?;
    let canonical_right = marked_directory(right).map_err(SeparateError::Marker)?;
    for (given, canonical) in [(left, &canonical_left), (right, &canonical_right)] {
        if given != canonical {
            return Err(SeparateError::NotCanonical {
                path: given.to_owned(),
            });
        }
    }
    if canonical_left.starts_with(&canonical_right) || canonical_right.starts_with(&canonical_left)
    {
        return Err(SeparateError::Overlap {
            left: canonical_left,
            right: canonical_right,
        });
    }
    Ok((canonical_left, canonical_right))
}

fn require_empty_tree(path: &Path) -> Result<(), ProduceSnapshotError> {
    let list = |source| ProduceSnapshotError::ListSnapshot {
        path: path.to_owned(),
        source,
    };
    for entry in std::fs::read_dir(path).map_err(list)? {
        if entry.map_err(list)?.file_name() != ".artisan-qualification" {
            return Err(ProduceSnapshotError::SnapshotNotEmpty {
                path: path.to_owned(),
            });
        }
    }
    Ok(())
}

fn copy_tree(
    source: &Path,
    destination: &Path,
    manifest: &TreeManifest,
) -> Result<(), CopyTreeError> {
    if let Some(path) = manifest
        .directories
        .iter()
        .chain(manifest.files.keys())
        .find(|path| {
            !path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        })
    {
        return Err(CopyTreeError::InvalidPath { path: path.clone() });
    }
    for directory in &manifest.directories {
        let path = destination.join(directory);
        std::fs::create_dir_all(&path)
            .map_err(|source| CopyTreeError::CreateDirectory { path, source })?;
    }
    for file in manifest.files.keys() {
        std::fs::copy(source.join(file), destination.join(file)).map_err(|error| {
            CopyTreeError::Copy {
                path: file.clone(),
                source: error,
            }
        })?;
    }
    if TreeManifest::capture_stable(source).map_err(CopyTreeError::Capture)? != *manifest {
        return Err(CopyTreeError::SourceChanged {
            path: source.to_owned(),
        });
    }
    if TreeManifest::capture_stable(destination).map_err(CopyTreeError::Capture)? != *manifest {
        return Err(CopyTreeError::CopyDiffers {
            path: destination.to_owned(),
        });
    }
    Ok(())
}

fn checkpoint_database(root: &Path) -> Result<(), CheckpointError> {
    let path = root.join("forge.sqlite3");
    let connection =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|source| CheckpointError::Open {
            path: path.clone(),
            source,
        })?;
    let checkpoint = |source| CheckpointError::Checkpoint {
        path: path.clone(),
        source,
    };
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(checkpoint)?;
    let (busy, _, _): (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(checkpoint)?;
    if busy != 0 {
        return Err(CheckpointError::Busy { path });
    }
    connection
        .execute_batch("BEGIN EXCLUSIVE; COMMIT;")
        .map_err(checkpoint)?;
    drop(connection);
    let wal = root.join("forge.sqlite3-wal");
    if wal.exists() {
        let length = std::fs::metadata(&wal)
            .map_err(|source| CheckpointError::Wal {
                path: wal.clone(),
                source,
            })?
            .len();
        if length != 0 {
            return Err(CheckpointError::WalNotEmpty { path: wal });
        }
    }
    Ok(())
}

impl SeedSnapshot {
    pub async fn produce(
        server: ForgeServer,
        fixture: &Fixture,
        logical_path: &Path,
        snapshot_dir: &Path,
    ) -> Result<Self, ProduceSnapshotError> {
        let (logical_path, snapshot_dir) = separate_directories(logical_path, snapshot_dir)
            .map_err(ProduceSnapshotError::Directories)?;
        require_empty_tree(&snapshot_dir)?;
        let blobs = server.store.blob_dir();
        let data = blobs
            .parent()
            .ok_or_else(|| ProduceSnapshotError::NoDataDirectory {
                path: blobs.to_owned(),
            })?;
        let served =
            canonical_path(data).map_err(|source| ProduceSnapshotError::ResolveDataDirectory {
                path: data.to_owned(),
                source,
            })?;
        if served != logical_path {
            return Err(ProduceSnapshotError::NotPinned {
                actual: served,
                pinned: logical_path,
            });
        }
        let store = fixture
            .seed(&server.store)
            .await
            .map_err(ProduceSnapshotError::Seed)?;
        server
            .shutdown_unique()
            .await
            .map_err(ProduceSnapshotError::Shutdown)?;
        checkpoint_database(&logical_path).map_err(ProduceSnapshotError::Checkpoint)?;
        let tree =
            TreeManifest::capture_stable(&logical_path).map_err(ProduceSnapshotError::Capture)?;
        copy_tree(&logical_path, &snapshot_dir, &tree).map_err(ProduceSnapshotError::Copy)?;
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
    ) -> Result<(), RestoreSnapshotError> {
        let (logical_path, snapshot_dir) = separate_directories(logical_path, snapshot_dir)
            .map_err(RestoreSnapshotError::Directories)?;
        if logical_path != self.logical_path {
            return Err(RestoreSnapshotError::NotPinned {
                requested: logical_path,
                pinned: self.logical_path.clone(),
            });
        }
        if TreeManifest::capture_stable(&snapshot_dir).map_err(RestoreSnapshotError::Capture)?
            != self.tree
        {
            return Err(RestoreSnapshotError::SnapshotChanged { path: snapshot_dir });
        }
        checkpoint_database(&logical_path).map_err(RestoreSnapshotError::Checkpoint)?;
        let staging = beside(&logical_path, "restoring")?;
        let previous = beside(&logical_path, "previous")?;
        clear_leftover(&staging)?;
        clear_leftover(&previous)?;
        std::fs::create_dir(&staging)
            .and_then(|()| std::fs::write(staging.join(".artisan-qualification"), MARKER))
            .map_err(|source| RestoreSnapshotError::Stage {
                path: staging.clone(),
                source,
            })?;
        if let Err(error) = copy_tree(&snapshot_dir, &staging, &self.tree) {
            clear_leftover(&staging)?;
            return Err(RestoreSnapshotError::Copy(error));
        }
        std::fs::rename(&logical_path, &previous).map_err(|source| RestoreSnapshotError::Swap {
            from: logical_path.clone(),
            to: previous.clone(),
            source,
        })?;
        if let Err(failed) = std::fs::rename(&staging, &logical_path) {
            return Err(match std::fs::rename(&previous, &logical_path) {
                Ok(()) => RestoreSnapshotError::Swap {
                    from: staging,
                    to: logical_path,
                    source: failed,
                },
                Err(source) => RestoreSnapshotError::RollBack {
                    from: previous,
                    to: logical_path,
                    failed,
                    source,
                },
            });
        }
        clear_leftover(&previous)
    }
}

fn beside(path: &Path, purpose: &str) -> Result<PathBuf, RestoreSnapshotError> {
    let name = path
        .file_name()
        .ok_or_else(|| RestoreSnapshotError::NoParent {
            path: path.to_owned(),
        })?;
    let mut sibling = name.to_owned();
    sibling.push(format!(".{purpose}"));
    Ok(path.with_file_name(sibling))
}

fn clear_leftover(path: &Path) -> Result<(), RestoreSnapshotError> {
    if !path.exists() {
        return Ok(());
    }
    let marked = marked_directory(path).map_err(|source| RestoreSnapshotError::Leftover {
        path: path.to_owned(),
        source,
    })?;
    let removal = |path: PathBuf| move |source| RestoreSnapshotError::Remove { path, source };
    let marker = marked.join(".artisan-qualification");
    for entry in std::fs::read_dir(&marked).map_err(removal(marked.clone()))? {
        let entry = entry.map_err(removal(marked.clone()))?;
        let path = entry.path();
        if path == marker {
            continue;
        }
        let kind = entry.file_type().map_err(removal(path.clone()))?;
        if kind.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        }
        .map_err(removal(path))?;
    }
    std::fs::remove_file(&marker).map_err(removal(marker))?;
    std::fs::remove_dir(&marked).map_err(removal(marked))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BootstrapRequest {
    fixture: PathBuf,
    fixture_sha256: String,
    snapshot: PathBuf,
    snapshot_sha256: String,
    forge_dir: PathBuf,
    forge_logical_path: PathBuf,
    seed_database_sha256: String,
    editor_dir: PathBuf,
    editor_settings_sha256: Option<String>,
    listen: SocketAddr,
    host_id: String,
    thread_id: String,
    width: f32,
    height: f32,
}

#[derive(Debug)]
struct VerifiedInputs {
    fixture: PathBuf,
    snapshot: PathBuf,
    snapshot_sha256: String,
    editor_settings_sha256: Option<String>,
}

#[derive(Debug)]
pub struct VerifiedBootstrap {
    inputs: VerifiedInputs,
    host: HostId,
    thread: ThreadId,
    dimensions: (f32, f32),
    fixture: ImmutableFixture,
    snapshot: StoreSnapshot,
    server: ForgeServer,
    editor_dir: PathBuf,
}

fn read_checked(path: &Path, expected: &str) -> Result<Vec<u8>, ReadCheckedError> {
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ReadCheckedError::InvalidDigest {
            path: path.to_owned(),
        });
    }
    let bytes = std::fs::read(path).map_err(|source| ReadCheckedError::Read {
        path: path.to_owned(),
        source,
    })?;
    if hex::encode(Sha256::digest(&bytes)) != expected {
        return Err(ReadCheckedError::Mismatch {
            path: path.to_owned(),
        });
    }
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

pub(crate) fn marked_directory(path: &Path) -> Result<PathBuf, MarkerError> {
    if !path.is_absolute() {
        return Err(MarkerError::Relative {
            path: path.to_owned(),
        });
    }
    let canonical = canonical_path(path).map_err(|source| MarkerError::Resolve {
        path: path.to_owned(),
        source,
    })?;
    let marker = std::fs::read(canonical.join(".artisan-qualification")).map_err(|source| {
        MarkerError::Read {
            path: canonical.clone(),
            source,
        }
    })?;
    if marker != MARKER {
        return Err(MarkerError::Invalid { path: canonical });
    }
    Ok(canonical)
}

fn verify_editor_settings(
    editor_dir: &Path,
    expected: Option<&str>,
) -> Result<(), EditorSettingsError> {
    let path = editor_dir.join("settings.json");
    match expected {
        Some(expected) => read_checked(&path, expected)
            .map(drop)
            .map_err(EditorSettingsError::Read),
        None => match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(EditorSettingsError::Inspect { path, source }),
            Ok(_) => Err(EditorSettingsError::Present { path }),
        },
    }
}

impl VerifiedBootstrap {
    pub async fn load(path: &Path) -> Result<Self, LoadBootstrapError> {
        let bytes = std::fs::read(path).map_err(|source| LoadBootstrapError::Read {
            path: path.to_owned(),
            source,
        })?;
        let request: BootstrapRequest =
            serde_json::from_slice(&bytes).map_err(|source| LoadBootstrapError::Decode {
                path: path.to_owned(),
                source,
            })?;
        if !request.listen.ip().is_loopback() || request.listen.port() == 0 {
            return Err(LoadBootstrapError::Listen {
                address: request.listen,
            });
        }
        if !(request.width.is_finite()
            && request.height.is_finite()
            && request.width >= 480.0
            && request.height >= 420.0)
        {
            return Err(LoadBootstrapError::Dimensions {
                width: request.width,
                height: request.height,
            });
        }
        if request.host_id.trim().is_empty() || request.thread_id.trim().is_empty() {
            return Err(LoadBootstrapError::MissingIds);
        }
        let forge_dir = marked_directory(&request.forge_dir).map_err(LoadBootstrapError::Marker)?;
        let pinned = canonical_path(&request.forge_logical_path).map_err(|source| {
            LoadBootstrapError::ResolvePinned {
                path: request.forge_logical_path.clone(),
                source,
            }
        })?;
        if forge_dir != pinned {
            return Err(LoadBootstrapError::NotPinned {
                forge: forge_dir,
                pinned,
            });
        }
        let editor_dir =
            marked_directory(&request.editor_dir).map_err(LoadBootstrapError::Marker)?;
        if editor_dir.starts_with(&forge_dir) || forge_dir.starts_with(&editor_dir) {
            return Err(LoadBootstrapError::Overlap {
                editor: editor_dir,
                forge: forge_dir,
            });
        }
        let list = |source| LoadBootstrapError::ListEditor {
            path: editor_dir.clone(),
            source,
        };
        for entry in std::fs::read_dir(&editor_dir).map_err(list)? {
            let entry = entry.map_err(list)?;
            if entry.file_name() != ".artisan-qualification" && entry.file_name() != "settings.json"
            {
                return Err(LoadBootstrapError::EditorNotFresh { path: entry.path() });
            }
        }
        verify_editor_settings(&editor_dir, request.editor_settings_sha256.as_deref())
            .map_err(LoadBootstrapError::EditorSettings)?;
        let wal = forge_dir.join("forge.sqlite3-wal");
        if wal.exists() {
            let length = std::fs::metadata(&wal)
                .map_err(|source| LoadBootstrapError::Wal {
                    path: wal.clone(),
                    source,
                })?
                .len();
            if length != 0 {
                return Err(LoadBootstrapError::WalNotEmpty { path: wal });
            }
        }
        read_checked(
            &forge_dir.join("forge.sqlite3"),
            &request.seed_database_sha256,
        )
        .map_err(LoadBootstrapError::Database)?;
        let fixture = ImmutableFixture::parse(
            read_checked(&request.fixture, &request.fixture_sha256)
                .map_err(LoadBootstrapError::FixtureInput)?,
        )
        .map_err(LoadBootstrapError::Fixture)?;
        let snapshot: StoreSnapshot = serde_json::from_slice(
            &read_checked(&request.snapshot, &request.snapshot_sha256)
                .map_err(LoadBootstrapError::SnapshotInput)?,
        )
        .map_err(|source| LoadBootstrapError::DecodeSnapshot {
            path: request.snapshot.clone(),
            source,
        })?;
        let thread = ThreadId::from(request.thread_id);
        if !fixture
            .fixture()
            .threads
            .iter()
            .any(|candidate| candidate.id == thread)
        {
            return Err(LoadBootstrapError::UnknownThread { thread });
        }
        let server = ForgeServer::start(forge_dir, request.listen)
            .await
            .map_err(LoadBootstrapError::Start)?;
        snapshot
            .verify_stable(&server.store, fixture.fixture())
            .await
            .map_err(LoadBootstrapError::Diverged)?;
        if server.address != request.listen {
            return Err(LoadBootstrapError::Address {
                expected: request.listen,
                actual: server.address,
            });
        }
        Ok(Self {
            inputs: VerifiedInputs {
                fixture: request.fixture,
                snapshot: request.snapshot,
                snapshot_sha256: request.snapshot_sha256,
                editor_settings_sha256: request.editor_settings_sha256,
            },
            host: HostId(request.host_id),
            thread,
            dimensions: (request.width, request.height),
            fixture,
            snapshot,
            server,
            editor_dir,
        })
    }

    pub(crate) fn verify_inputs(&self) -> Result<(), VerifyInputsError> {
        self.fixture
            .verify_unchanged(&self.inputs.fixture)
            .map_err(VerifyInputsError::Fixture)?;
        read_checked(&self.inputs.snapshot, &self.inputs.snapshot_sha256)
            .map_err(VerifyInputsError::Snapshot)?;
        verify_editor_settings(
            &self.editor_dir,
            self.inputs.editor_settings_sha256.as_deref(),
        )
        .map_err(VerifyInputsError::EditorSettings)
    }

    #[must_use]
    pub fn fixture_path(&self) -> &Path {
        &self.inputs.fixture
    }

    #[must_use]
    pub fn editor_dir(&self) -> &Path {
        &self.editor_dir
    }

    #[must_use]
    pub(crate) const fn host(&self) -> &HostId {
        &self.host
    }

    #[must_use]
    pub(crate) const fn thread(&self) -> &ThreadId {
        &self.thread
    }

    #[must_use]
    pub(crate) const fn dimensions(&self) -> (f32, f32) {
        self.dimensions
    }

    pub(crate) async fn pairing_input(&self) -> Result<PairingInput, PairingError> {
        self.server.pairing_input().await
    }

    #[must_use]
    pub fn into_server(self) -> ForgeServer {
        self.server
    }
}

impl ForgeServer {
    pub async fn start(data_dir: PathBuf, listen: SocketAddr) -> Result<Self, StartForgeError> {
        let server = forge_http::start(forge_http::ServeConfig {
            data_dir: data_dir.clone(),
            listen,
        })
        .await
        .map_err(|source| StartForgeError::Start {
            data_dir,
            source: source.into(),
        })?;
        let address = server
            .local_addr()
            .map_err(|source| StartForgeError::Address(source.into()))?;
        let store = server.store().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            server
                .run_until(async {
                    match stopped.await {
                        Ok(()) | Err(tokio::sync::oneshot::error::RecvError { .. }) => {}
                    }
                })
                .await;
            Ok::<(), ForgeFailure>(())
        });
        Ok(Self {
            store,
            address,
            stop: Some(stop),
            task: Some(task),
        })
    }

    pub async fn pairing_input(&self) -> Result<PairingInput, PairingError> {
        let invite = forge_http::invite(&self.store)
            .await
            .map_err(|source| PairingError::Invite(source.into()))?;
        let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate)
            .map_err(PairingError::Fingerprint)?;
        Ok(PairingInput {
            address: self.address.to_string(),
            code: invite.code,
            fingerprint,
            expires_at: invite.expires_at,
        })
    }

    async fn stop(&mut self) -> Result<(), ShutdownError> {
        if let Some(stop) = self.stop.take() {
            match stop.send(()) {
                Ok(()) | Err(()) => {}
            }
        }
        if let Some(task) = self.task.take() {
            task.await
                .map_err(ShutdownError::Join)?
                .map_err(ShutdownError::Stop)?;
        }
        Ok(())
    }

    pub async fn shutdown(mut self) -> Result<(), ShutdownError> {
        self.stop().await
    }

    pub async fn shutdown_unique(mut self) -> Result<(), ShutdownError> {
        self.stop().await?;
        let store = self.store.clone();
        drop(self);
        tokio::time::timeout(SNAPSHOT_TIMEOUT, async {
            while !store.qualification_is_unique() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_elapsed| ShutdownError::LiveHandles)?;
        store.shutdown_unique().map_err(ShutdownError::Store)
    }
}

impl Drop for ForgeServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            match stop.send(()) {
                Ok(()) | Err(()) => {}
            }
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

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("read the fixture {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode the fixture")]
    Decode(#[source] serde_json::Error),
    #[error("check the fixture")]
    Invalid(#[source] InvalidFixture),
    #[error("the fixture {} changed", path.display())]
    Changed { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidFixture {
    #[error("fixture version {version} is not supported")]
    Version { version: u32 },
    #[error("project id {project:?} is empty or repeated")]
    Project { project: String },
    #[error("project {project} has an empty path")]
    ProjectPath { project: String },
    #[error("thread {thread} comes before its parent {parent}")]
    ParentAfterChild { thread: ThreadId, parent: ThreadId },
    #[error("thread id {thread:?} is empty or repeated")]
    Thread { thread: ThreadId },
    #[error("thread {thread} refers to the unknown project {project}")]
    ThreadProject { thread: ThreadId, project: String },
    #[error("run id {run:?} is empty or repeated")]
    Run { run: RunId },
    #[error("run {run} refers to the unknown thread {thread}")]
    RunThread { run: RunId, thread: ThreadId },
    #[error("run {run} is live, which a restart-stable seed cannot hold")]
    LiveRun { run: RunId },
    #[error("item {item} does not follow its parent {parent} in thread {thread}")]
    ItemParent {
        item: ItemId,
        parent: ItemId,
        thread: ThreadId,
    },
    #[error("item id {item:?} is empty or repeated")]
    Item { item: ItemId },
    #[error("item {item} refers to the unknown thread {thread}")]
    ItemThread { item: ItemId, thread: ThreadId },
    #[error("item {item} refers to run {run}, which is unknown or in another thread")]
    ItemRun { item: ItemId, run: RunId },
    #[error("a setting key is empty")]
    SettingKey,
}

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    #[error("check the fixture")]
    Invalid(#[source] InvalidFixture),
    #[error("read Forge's initial state")]
    State(#[source] forge_store::StateError),
    #[error("Forge's seed store is not empty")]
    NotEmpty,
    #[error("Forge's blob directory {} has no parent data directory", path.display())]
    NoDataDirectory { path: PathBuf },
    #[error("Forge holds an unexpected built-in project")]
    UnexpectedBuiltin,
    #[error("the fixture's scratch project {project} does not match Forge's built-in one")]
    Scratch { project: String },
    #[error("seed project {project}")]
    Project {
        project: String,
        #[source]
        source: forge_store::CreateProjectError,
    },
    #[error("seed thread {thread}")]
    Thread {
        thread: ThreadId,
        #[source]
        source: forge_store::CreateThreadError,
    },
    #[error("seed run {run}")]
    Run {
        run: RunId,
        #[source]
        source: forge_store::UpsertRunError,
    },
    #[error("seed item {item}")]
    Item {
        item: ItemId,
        #[source]
        source: forge_store::UpsertItemError,
    },
    #[error("seed setting {key}")]
    Setting {
        key: String,
        #[source]
        source: forge_store::SetSettingError,
    },
    #[error("capture the seeded store")]
    Snapshot(#[source] SnapshotError),
    #[error("the seeded items differ from the fixture")]
    Items,
    #[error("the seeded projects or threads differ from the fixture")]
    ProjectsOrThreads,
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("read Forge's state")]
    State(#[source] forge_store::StateError),
    #[error("Forge's state has an unread page of threads")]
    UnobservedThreadPage,
    #[error("read a page of items")]
    Page(#[source] forge_store::ItemsPageError),
    #[error("the item cursor of thread {thread} did not advance past {position}")]
    CursorStuck { thread: ThreadId, position: u64 },
    #[error("Forge returned an empty page of thread {thread} that says more follow")]
    EmptyPage { thread: ThreadId },
    #[error("Forge changed while it was read")]
    HeadChanged,
    #[error("two snapshots at head {head} differ")]
    DifferAtHead { head: u64 },
    #[error("no consistent snapshot within {SNAPSHOT_TIMEOUT:?}")]
    TimedOut,
    #[error("Forge's store changed from head {expected} to {actual}")]
    Changed { expected: u64, actual: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum CheckpointDigestError {
    #[error("a checkpoint needs a name and a route")]
    Unnamed,
    #[error("a state key is empty")]
    EmptyKey,
    #[error("pending operations must be named, sorted and unique")]
    Pending,
    #[error("encode the checkpoint")]
    Encode(#[source] serde_json::Error),
}

#[derive(Debug)]
pub struct ImmutableFixture {
    input: Vec<u8>,
    fixture: Fixture,
}

impl ImmutableFixture {
    pub fn read(path: &Path) -> Result<Self, FixtureError> {
        let input = std::fs::read(path).map_err(|source| FixtureError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(input)
    }

    pub fn parse(input: Vec<u8>) -> Result<Self, FixtureError> {
        let fixture: Fixture = serde_json::from_slice(&input).map_err(FixtureError::Decode)?;
        fixture.validate().map_err(FixtureError::Invalid)?;
        Ok(Self { input, fixture })
    }

    pub(crate) fn sha256(&self) -> String {
        hex::encode(Sha256::digest(&self.input))
    }

    pub fn fixture(&self) -> &Fixture {
        &self.fixture
    }

    pub fn verify_unchanged(&self, path: &Path) -> Result<(), FixtureError> {
        let current = std::fs::read(path).map_err(|source| FixtureError::Read {
            path: path.to_owned(),
            source,
        })?;
        if current != self.input {
            return Err(FixtureError::Changed {
                path: path.to_owned(),
            });
        }
        Ok(())
    }
}

impl Fixture {
    pub fn validate(&self) -> Result<(), InvalidFixture> {
        if self.version != 1 {
            return Err(InvalidFixture::Version {
                version: self.version,
            });
        }
        let mut projects = BTreeSet::new();
        for project in &self.projects {
            let id = project.id.as_str();
            if id.trim().is_empty() || !projects.insert(id) {
                return Err(InvalidFixture::Project {
                    project: id.to_owned(),
                });
            }
            if project.path.trim().is_empty() {
                return Err(InvalidFixture::ProjectPath {
                    project: id.to_owned(),
                });
            }
        }
        let mut threads = BTreeSet::new();
        for thread in &self.threads {
            if let Some(parent) = &thread.parent_thread_id
                && !threads.contains(parent)
            {
                return Err(InvalidFixture::ParentAfterChild {
                    thread: thread.id.clone(),
                    parent: parent.clone(),
                });
            }
            if thread.id.as_str().trim().is_empty() || !threads.insert(&thread.id) {
                return Err(InvalidFixture::Thread {
                    thread: thread.id.clone(),
                });
            }
            if !projects.contains(thread.project_id.as_str()) {
                return Err(InvalidFixture::ThreadProject {
                    thread: thread.id.clone(),
                    project: thread.project_id.as_str().to_owned(),
                });
            }
        }
        let mut runs = BTreeMap::new();
        for run in &self.runs {
            if run.id.as_str().trim().is_empty() || runs.insert(&run.id, &run.thread_id).is_some() {
                return Err(InvalidFixture::Run {
                    run: run.id.clone(),
                });
            }
            if !threads.contains(&run.thread_id) {
                return Err(InvalidFixture::RunThread {
                    run: run.id.clone(),
                    thread: run.thread_id.clone(),
                });
            }
            if run.status().is_live() {
                return Err(InvalidFixture::LiveRun {
                    run: run.id.clone(),
                });
            }
        }
        let mut items = BTreeMap::new();
        for item in &self.items {
            if let Some(parent) = &item.parent_id
                && items.get(parent) != Some(&&item.thread_id)
            {
                return Err(InvalidFixture::ItemParent {
                    item: item.id.clone(),
                    parent: parent.clone(),
                    thread: item.thread_id.clone(),
                });
            }
            if item.id.as_str().trim().is_empty()
                || items.insert(&item.id, &item.thread_id).is_some()
            {
                return Err(InvalidFixture::Item {
                    item: item.id.clone(),
                });
            }
            if !threads.contains(&item.thread_id) {
                return Err(InvalidFixture::ItemThread {
                    item: item.id.clone(),
                    thread: item.thread_id.clone(),
                });
            }
            if let Some(run) = &item.run_id
                && runs.get(run) != Some(&&item.thread_id)
            {
                return Err(InvalidFixture::ItemRun {
                    item: item.id.clone(),
                    run: run.clone(),
                });
            }
        }
        if self.settings.keys().any(|key| key.trim().is_empty()) {
            return Err(InvalidFixture::SettingKey);
        }
        Ok(())
    }

    pub async fn seed(&self, store: &forge_store::Store) -> Result<StoreSnapshot, SeedError> {
        self.validate().map_err(SeedError::Invalid)?;
        let initial = store.state().await.map_err(SeedError::State)?;
        if !(initial.threads.is_empty()
            && initial.runs.is_empty()
            && initial.threads_next.is_none())
        {
            return Err(SeedError::NotEmpty);
        }
        let blobs = store.blob_dir();
        let scratch_path = blobs
            .parent()
            .ok_or_else(|| SeedError::NoDataDirectory {
                path: blobs.to_owned(),
            })?
            .join("scratch");
        if !(initial.projects.len() <= 1
            && initial.projects.iter().all(|project| {
                project.id.is_scratch()
                    && project.name == "No project"
                    && !project.removed
                    && Path::new(&project.path) == scratch_path
            }))
        {
            return Err(SeedError::UnexpectedBuiltin);
        }
        let builtins: BTreeSet<_> = initial
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect();
        if let Some(project) = self.projects.iter().find(|project| {
            project.id.is_scratch()
                && builtins.contains(&project.id)
                && (Path::new(&project.path) != scratch_path
                    || project.name.as_deref() != Some("No project"))
        }) {
            return Err(SeedError::Scratch {
                project: project.id.as_str().to_owned(),
            });
        }
        for project in &self.projects {
            store
                .create_project(project.clone())
                .await
                .map_err(|source| SeedError::Project {
                    project: project.id.as_str().to_owned(),
                    source,
                })?;
        }
        for thread in &self.threads {
            store
                .create_thread(thread.clone())
                .await
                .map_err(|source| SeedError::Thread {
                    thread: thread.id.clone(),
                    source,
                })?;
        }
        for run in &self.runs {
            store
                .upsert_run(run.clone())
                .await
                .map_err(|source| SeedError::Run {
                    run: run.id.clone(),
                    source,
                })?;
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
                .map_err(|source| SeedError::Item {
                    item: item.id.clone(),
                    source,
                })?;
        }
        for (key, value) in &self.settings {
            store
                .set_setting(key.clone(), value.clone())
                .await
                .map_err(|source| SeedError::Setting {
                    key: key.clone(),
                    source,
                })?;
        }
        let snapshot = StoreSnapshot::capture_stable(store, self)
            .await
            .map_err(SeedError::Snapshot)?;
        let expected: BTreeSet<_> = self.items.iter().map(|item| item.id.clone()).collect();
        let actual: BTreeSet<_> = snapshot
            .items
            .values()
            .flatten()
            .map(|item| item.id.clone())
            .collect();
        if actual != expected {
            return Err(SeedError::Items);
        }
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
        if expected_projects != actual_projects || expected_threads != actual_threads {
            return Err(SeedError::ProjectsOrThreads);
        }
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
    async fn capture(store: &forge_store::Store, fixture: &Fixture) -> Result<Self, SnapshotError> {
        tokio::time::timeout(SNAPSHOT_TIMEOUT, Self::capture_retry(store, fixture))
            .await
            .map_err(|_elapsed| SnapshotError::TimedOut)?
    }

    pub async fn capture_stable(
        store: &forge_store::Store,
        fixture: &Fixture,
    ) -> Result<Self, SnapshotError> {
        tokio::time::timeout(SNAPSHOT_TIMEOUT, async {
            loop {
                let first = Self::capture_retry(store, fixture).await?;
                let second = Self::capture_retry(store, fixture).await?;
                if first.head != second.head {
                    tokio::task::yield_now().await;
                    continue;
                }
                if first != second {
                    return Err(SnapshotError::DifferAtHead { head: second.head });
                }
                return Ok(second);
            }
        })
        .await
        .map_err(|_elapsed| SnapshotError::TimedOut)?
    }

    async fn capture_retry(
        store: &forge_store::Store,
        fixture: &Fixture,
    ) -> Result<Self, SnapshotError> {
        loop {
            match Self::capture_once(store, fixture).await {
                Err(SnapshotError::HeadChanged) => tokio::task::yield_now().await,
                result => return result,
            }
        }
    }

    async fn capture_once(
        store: &forge_store::Store,
        fixture: &Fixture,
    ) -> Result<Self, SnapshotError> {
        let state = store.state().await.map_err(SnapshotError::State)?;
        let head = state.head;
        if state.threads_next.is_some() {
            return Err(SnapshotError::UnobservedThreadPage);
        }
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
                    .map_err(SnapshotError::Page)?;
                match page.items.last() {
                    Some(last) if last.position > after => after = last.position,
                    Some(_) => {
                        return Err(SnapshotError::CursorStuck {
                            thread: thread.id.clone(),
                            position: after,
                        });
                    }
                    None if page.more => {
                        return Err(SnapshotError::EmptyPage {
                            thread: thread.id.clone(),
                        });
                    }
                    None => {}
                }
                all.extend(page.items);
                if !page.more {
                    break;
                }
            }
            items.insert(thread.id.as_str().to_owned(), all);
        }
        if store.head() != head {
            return Err(SnapshotError::HeadChanged);
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

    pub async fn verify_stable(
        &self,
        store: &forge_store::Store,
        fixture: &Fixture,
    ) -> Result<(), SnapshotError> {
        let next = Self::capture(store, fixture).await?;
        if &next != self {
            return Err(SnapshotError::Changed {
                expected: self.head,
                actual: next.head,
            });
        }
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
    pub fn digest(&self) -> Result<String, CheckpointDigestError> {
        if self.name.trim().is_empty() || self.route.trim().is_empty() {
            return Err(CheckpointDigestError::Unnamed);
        }
        if self
            .scroll
            .keys()
            .chain(self.observations.keys())
            .any(|key| key.trim().is_empty())
        {
            return Err(CheckpointDigestError::EmptyKey);
        }
        if !(self.pending.iter().all(|item| !item.trim().is_empty())
            && self.pending.windows(2).all(|pair| pair[0] < pair[1]))
        {
            return Err(CheckpointDigestError::Pending);
        }
        let value = canonical(serde_json::to_value(self).map_err(CheckpointDigestError::Encode)?);
        Ok(hex::encode(Sha256::digest(
            serde_json::to_vec(&value).map_err(CheckpointDigestError::Encode)?,
        )))
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
        value @ (serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_)) => value,
    }
}
