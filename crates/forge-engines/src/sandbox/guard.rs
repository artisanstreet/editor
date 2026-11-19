use artisan_api::EngineId;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf};

const SNAPSHOT_FOLDER: &str = "guarded-config";
const COPIES: &str = "files";
const MANIFEST: &str = "manifest.json";
const CLAUDE_FOLDER: &str = ".claude";
const CLAUDE_STATE: &str = ".claude.json";
const CLAUDE_STATE_COPY: &str = "claude-mcp-servers.json";
const LIVE_MARKERS: &str = "guarded-config-live";
const BUSY_LOCK: &str = "guarded-config-busy";
const TEMPORARY_PREFIX: &str = ".artisan-guard-";
const PLANTED_PREFIX: &str = ".artisan-planted-";
const RANDOM_BYTES: usize = 16;
const MAX_DEPTH: usize = 128;
const FOLDER_MODE: u32 = 0o700;
const CLAUDE_STATE_MODE: u32 = 0o600;
const PERMISSION_BITS: u32 = 0o7777;

const fn surfaces(engine: EngineId) -> &'static [&'static str] {
    match engine {
        EngineId::Claude => &[
            ".claude/settings.json",
            ".claude/settings.local.json",
            ".claude/plugins",
            ".claude/hooks",
            ".claude/commands",
            ".claude/agents",
            ".claude/CLAUDE.md",
            ".artisan/claude-mcp.json",
        ],
        EngineId::Codex => &[
            ".codex/config.toml",
            ".codex/hooks.json",
            ".codex/plugins",
            ".codex/rules",
            ".codex/AGENTS.md",
            ".codex/AGENTS.override.md",
        ],
        EngineId::OpenCode => &[
            "config/opencode/opencode.json",
            "config/opencode/opencode.jsonc",
            "config/opencode/config.json",
            "config/opencode/plugin",
            "config/opencode/plugins",
            "config/opencode/tool",
            "config/opencode/tools",
            "config/opencode/command",
            "config/opencode/commands",
            "config/opencode/agent",
            "config/opencode/agents",
            "config/opencode/AGENTS.md",
        ],
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("lock {engine}'s guarded configuration with {}", path.display())]
    Lock {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("snapshot {engine}'s configuration in {}", path.display())]
    Snapshot {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{engine}'s configuration folder {} is a link, so Forge can't guard what's beneath it", path.display())]
    LinkedFolder { engine: EngineId, path: PathBuf },
    #[error("{engine}'s configuration {} is nested over {MAX_DEPTH} folders deep", path.display())]
    TooDeep { engine: EngineId, path: PathBuf },
    #[error("restore {engine}'s configuration at {}", path.display())]
    Restore {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Forge's trusted copy of {engine}'s {} no longer matches its record", path.display())]
    CopyChanged { engine: EngineId, path: PathBuf },
    #[error("write {engine}'s configuration {}", path.display())]
    Write {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("read {engine}'s configuration {}", path.display())]
    Read {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("read {engine}'s guarded configuration record {}", path.display())]
    Manifest {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("read {engine}'s MCP servers in {}", path.display())]
    State {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Entry {
    Folder,
    Link(PathBuf),
    File {
        digest: String,
        length: u64,
        mode: u32,
    },
}

type Manifest = BTreeMap<PathBuf, Entry>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Found {
    Missing,
    Folder,
    Link(PathBuf),
    File { length: u64, mode: u32 },
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Outside,
    Containing,
    Guarded,
}

fn scope(engine: EngineId, relative: &Path) -> Scope {
    let surfaces = surfaces(engine).iter().map(Path::new);
    let mut scope = Scope::Outside;
    for surface in surfaces {
        if relative.starts_with(surface) {
            return Scope::Guarded;
        }
        if surface.starts_with(relative) {
            scope = Scope::Containing;
        }
    }
    scope
}

fn contained_names(engine: EngineId, relative: &Path) -> BTreeSet<OsString> {
    surfaces(engine)
        .iter()
        .filter_map(|surface| Path::new(surface).strip_prefix(relative).ok())
        .filter_map(|rest| rest.components().next())
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_os_string()),
            Component::RootDir
            | Component::CurDir
            | Component::ParentDir
            | Component::Prefix(_) => None,
        })
        .collect()
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn random_name(prefix: &str) -> io::Result<OsString> {
    let mut bytes = [0_u8; RANDOM_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(OsString::from(format!("{prefix}{}", hex::encode(bytes))))
}

#[derive(Debug)]
struct Folder(OwnedFd);

impl Folder {
    fn found(&self, name: &OsStr) -> io::Result<Found> {
        let stat = match rustix::fs::statat(&self.0, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(Found::Missing),
            Err(error) => return Err(error.into()),
        };
        Ok(match FileType::from_raw_mode(stat.st_mode) {
            FileType::Directory => Found::Folder,
            FileType::Symlink => {
                let target = rustix::fs::readlinkat(&self.0, name, Vec::new())?;
                Found::Link(PathBuf::from(OsString::from_vec(target.into_bytes())))
            }
            FileType::RegularFile => Found::File {
                length: stat.st_size.unsigned_abs(),
                mode: stat.st_mode & PERMISSION_BITS,
            },
            FileType::Fifo
            | FileType::Socket
            | FileType::CharacterDevice
            | FileType::BlockDevice
            | FileType::Unknown => Found::Other,
        })
    }

    fn open(&self, name: &OsStr) -> io::Result<Self> {
        rustix::fs::openat(
            &self.0,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(Self)
        .map_err(io::Error::from)
    }

    fn names(&self) -> io::Result<BTreeSet<OsString>> {
        let mut names = BTreeSet::new();
        for entry in rustix::fs::Dir::read_from(&self.0)? {
            let name = entry?.file_name().to_bytes().to_vec();
            if name != b"." && name != b".." {
                names.insert(OsString::from_vec(name));
            }
        }
        Ok(names)
    }

    fn read(&self, name: &OsStr, length: u64) -> io::Result<Vec<u8>> {
        let file = rustix::fs::openat(
            &self.0,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        if FileType::from_raw_mode(rustix::fs::fstat(&file)?.st_mode) != FileType::RegularFile {
            return Err(io::Error::other("not a regular file"));
        }
        let mut bytes = Vec::new();
        fs::File::from(file)
            .take(length.saturating_add(1))
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn holds(&self, name: &OsStr, length: u64, wanted: &str) -> io::Result<bool> {
        let bytes = self.read(name, length)?;
        Ok(bytes.len() as u64 == length && digest(&bytes) == wanted)
    }

    fn make_folder(&self, name: &OsStr) -> io::Result<Self> {
        rustix::fs::mkdirat(&self.0, name, Mode::from_raw_mode(FOLDER_MODE))?;
        self.open(name)
    }

    fn put_link(&self, name: &OsStr, target: &Path) -> io::Result<()> {
        rustix::fs::symlinkat(target, &self.0, name).map_err(io::Error::from)
    }

    fn put_file(&self, name: &OsStr, bytes: &[u8], mode: u32) -> io::Result<()> {
        let temporary = random_name(TEMPORARY_PREFIX)?;
        let file = rustix::fs::openat(
            &self.0,
            &temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(mode),
        )?;
        let written = rustix::fs::fchmod(&file, Mode::from_raw_mode(mode))
            .map_err(io::Error::from)
            .and_then(|()| fs::File::from(file).write_all(bytes))
            .and_then(|()| {
                rustix::fs::renameat(&self.0, &temporary, &self.0, name).map_err(io::Error::from)
            });
        if written.is_err() {
            match rustix::fs::unlinkat(&self.0, &temporary, AtFlags::empty()) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => {}
                Err(error) => return Err(error.into()),
            }
        }
        written
    }

    fn remove(&self, name: &OsStr, home: &Home) -> io::Result<()> {
        match self.found(name)? {
            Found::Missing => Ok(()),
            Found::Folder => {
                let planted = random_name(PLANTED_PREFIX)?;
                rustix::fs::renameat(&self.0, name, &home.folder.0, &planted)?;
                fs::remove_dir_all(home.path.join(&planted))
            }
            Found::Link(_) | Found::File { .. } | Found::Other => {
                match rustix::fs::unlinkat(&self.0, name, AtFlags::empty()) {
                    Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
                    Err(error) => Err(error.into()),
                }
            }
        }
    }
}

#[derive(Debug)]
struct Home {
    path: PathBuf,
    folder: Folder,
}

impl Home {
    fn open(path: &Path) -> io::Result<Self> {
        let folder = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Ok(Self {
            path: path.to_path_buf(),
            folder: Folder(folder),
        })
    }

    fn open_existing(path: &Path) -> io::Result<Option<Self>> {
        match Self::open(path) {
            Ok(home) => Ok(Some(home)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn reach(&self, relative: &Path, create: bool) -> io::Result<Option<Folder>> {
        let mut folder = Folder(self.folder.0.try_clone()?);
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(io::Error::other(format!(
                    "{} isn't a plain relative path",
                    relative.display()
                )));
            };
            folder = match (folder.found(name)?, create) {
                (Found::Folder, _) => folder.open(name)?,
                (Found::Missing | Found::Link(_) | Found::File { .. } | Found::Other, false) => {
                    return Ok(None);
                }
                (Found::Missing, true) => folder.make_folder(name)?,
                (Found::Link(_) | Found::File { .. } | Found::Other, true) => {
                    tracing::warn!(path = %self.path.join(relative).display(), "replacing what a sandboxed run put where Forge keeps an engine's configuration");
                    folder.remove(name, self)?;
                    folder.make_folder(name)?
                }
            };
        }
        Ok(Some(folder))
    }
}

static EXPOSURES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct Exposure {
    engine: EngineId,
    home: PathBuf,
    marker: PathBuf,
    ends: std::sync::Mutex<Ending>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    WithItsRuns,
    WithForge,
}

impl Exposure {
    pub(crate) fn outlive_forge(&self) {
        *self
            .ends
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Ending::WithForge;
    }
}

impl Drop for Exposure {
    fn drop(&mut self) {
        let ending = *self
            .ends
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if ending == Ending::WithForge {
            tracing::warn!(marker = %self.marker.display(), "a sandboxed run's processes may still be alive; its engine's configuration stays guarded until Forge exits");
            return;
        }
        if let Err(error) = fs::remove_file(&self.marker) {
            tracing::warn!(marker = %self.marker.display(), %error, "couldn't record that a sandboxed run ended; its engine's configuration stays guarded");
        }
        if let Err(failure) = restore(self.engine, &self.home) {
            let error: &dyn std::error::Error = &failure;
            tracing::warn!(engine = %self.engine, home = %self.home.display(), error, "couldn't restore the engine's configuration after a sandboxed run; the next launch restores it");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Holder {
    process: u32,
    started: u64,
}

impl Holder {
    fn current() -> io::Result<Self> {
        let process = std::process::id();
        started(process)?.map_or_else(
            || Err(io::Error::other("this process has no start time in /proc")),
            |started| Ok(Self { process, started }),
        )
    }

    fn parse(name: &str) -> Option<Self> {
        let mut parts = name.split('-');
        match (
            parts.next().map(str::parse::<u32>),
            parts.next().map(str::parse::<u64>),
        ) {
            (Some(Ok(process)), Some(Ok(started))) => Some(Self { process, started }),
            _ => None,
        }
    }

    fn alive(self) -> io::Result<bool> {
        Ok(started(self.process)? == Some(self.started))
    }
}

fn started(process: u32) -> io::Result<Option<u64>> {
    let stat = match fs::read_to_string(format!("/proc/{process}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let field = stat
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .ok_or_else(|| io::Error::other(format!("/proc/{process}/stat has no start time")))?;
    field.parse().map(Some).map_err(io::Error::other)
}

fn beside(home: &Path, name: &str) -> PathBuf {
    let own = home
        .file_name()
        .expect("invariant: an engine home is a named folder");
    let mut sibling = own.to_os_string();
    sibling.push(".");
    sibling.push(name);
    home.with_file_name(sibling)
}

fn snapshot_folder(home: &Path) -> PathBuf {
    beside(home, SNAPSHOT_FOLDER)
}

fn hold(engine: EngineId, home: &Path, name: &str) -> Result<fs::File, GuardError> {
    let path = beside(home, name);
    let locking = |source| GuardError::Lock {
        engine,
        path: path.clone(),
        source,
    };
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(locking)?;
    fs2::FileExt::lock_exclusive(&file).map_err(locking)?;
    Ok(file)
}

fn still_exposed(engine: EngineId, home: &Path) -> Result<bool, GuardError> {
    let markers = beside(home, LIVE_MARKERS);
    let locking = |path: &Path| {
        let path = path.to_path_buf();
        move |source| GuardError::Lock {
            engine,
            path,
            source,
        }
    };
    let entries = match fs::read_dir(&markers) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(locking(&markers)(error)),
    };
    let mut exposed = false;
    for entry in entries {
        let entry = entry.map_err(locking(&markers))?;
        let holder = Holder::parse(&entry.file_name().to_string_lossy());
        let alive = match holder {
            Some(holder) => holder.alive().map_err(locking(&entry.path()))?,
            None => false,
        };
        if alive {
            exposed = true;
        } else {
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(locking(&entry.path())(error)),
            }
        }
    }
    Ok(exposed)
}

fn read_manifest(engine: EngineId, snapshot: &Path) -> Result<Option<Manifest>, GuardError> {
    let path = snapshot.join(MANIFEST);
    match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|source| GuardError::Manifest {
                    engine,
                    path,
                    source,
                })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(GuardError::Restore {
            engine,
            path,
            source,
        }),
    }
}

fn write_manifest(
    engine: EngineId,
    snapshot: &Path,
    manifest: &Manifest,
) -> Result<(), GuardError> {
    let path = snapshot.join(MANIFEST);
    let bytes = serde_json::to_vec(manifest).map_err(|source| GuardError::Manifest {
        engine,
        path: path.clone(),
        source,
    })?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)
        .and_then(|()| fs::rename(&temporary, &path))
        .map_err(|source| GuardError::Snapshot {
            engine,
            path,
            source,
        })
}

pub(crate) fn expose(engine: EngineId, home: &Path) -> Result<Exposure, GuardError> {
    let _busy = hold(engine, home, BUSY_LOCK)?;
    let markers = beside(home, LIVE_MARKERS);
    let marking = |source| GuardError::Lock {
        engine,
        path: markers.clone(),
        source,
    };
    let holder = Holder::current().map_err(marking)?;
    let exposure = EXPOSURES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let marker = markers.join(format!("{}-{}-{exposure}", holder.process, holder.started));
    fs::create_dir_all(&markers)
        .and_then(|()| fs::write(&marker, []))
        .map_err(marking)?;
    let exposure = Exposure {
        engine,
        home: home.to_path_buf(),
        marker,
        ends: std::sync::Mutex::new(Ending::WithItsRuns),
    };
    let snapshot = snapshot_folder(home);
    if read_manifest(engine, &snapshot)?.is_none() {
        take_snapshot(engine, home, &snapshot)?;
    }
    Ok(exposure)
}

fn take_snapshot(engine: EngineId, home: &Path, snapshot: &Path) -> Result<(), GuardError> {
    let snapshotting = |path: &Path| {
        let path = path.to_path_buf();
        move |source| GuardError::Snapshot {
            engine,
            path,
            source,
        }
    };
    match fs::remove_dir_all(snapshot) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(snapshotting(snapshot)(error)),
    }
    let copies = snapshot.join(COPIES);
    fs::create_dir_all(&copies).map_err(snapshotting(&copies))?;
    let opened = Home::open_existing(home).map_err(snapshotting(home))?;
    let mut manifest = Manifest::new();
    if let Some(opened) = &opened {
        let walk = Walk {
            engine,
            home: opened,
            copies: &copies,
        };
        walk.record(&opened.folder, Path::new(""), &mut manifest, 0)?;
    }
    if engine == EngineId::Claude {
        let servers = match &opened {
            Some(opened) => claude_servers(engine, opened)?,
            None => HashMap::new(),
        };
        let path = snapshot.join(CLAUDE_STATE_COPY);
        let bytes = serde_json::to_vec(&servers).map_err(|source| GuardError::State {
            engine,
            path: path.clone(),
            source,
        })?;
        fs::write(&path, bytes).map_err(snapshotting(&path))?;
    }
    write_manifest(engine, snapshot, &manifest)
}

struct Walk<'a> {
    engine: EngineId,
    home: &'a Home,
    copies: &'a Path,
}

impl Walk<'_> {
    fn snapshotting(&self, relative: &Path) -> impl FnOnce(io::Error) -> GuardError + use<> {
        let engine = self.engine;
        let path = self.home.path.join(relative);
        move |source| GuardError::Snapshot {
            engine,
            path,
            source,
        }
    }

    fn restoring(&self, relative: &Path) -> impl FnOnce(io::Error) -> GuardError + use<> {
        let engine = self.engine;
        let path = self.home.path.join(relative);
        move |source| GuardError::Restore {
            engine,
            path,
            source,
        }
    }

    fn in_scope(&self, folder: &Folder, relative: &Path) -> io::Result<BTreeSet<OsString>> {
        match scope(self.engine, relative) {
            Scope::Guarded => folder.names(),
            Scope::Containing | Scope::Outside => Ok(contained_names(self.engine, relative)),
        }
    }

    fn deeper(&self, relative: &Path, depth: usize) -> Result<usize, GuardError> {
        if depth == MAX_DEPTH {
            return Err(GuardError::TooDeep {
                engine: self.engine,
                path: self.home.path.join(relative),
            });
        }
        Ok(depth + 1)
    }

    fn record(
        &self,
        folder: &Folder,
        relative: &Path,
        manifest: &mut Manifest,
        depth: usize,
    ) -> Result<(), GuardError> {
        let names = self
            .in_scope(folder, relative)
            .map_err(self.snapshotting(relative))?;
        for name in names {
            let child = relative.join(&name);
            let scope = scope(self.engine, &child);
            if scope == Scope::Outside {
                continue;
            }
            match folder.found(&name).map_err(self.snapshotting(&child))? {
                Found::Missing | Found::Other => {}
                Found::Folder => {
                    manifest.insert(child.clone(), Entry::Folder);
                    let inner = folder.open(&name).map_err(self.snapshotting(&child))?;
                    self.record(&inner, &child, manifest, self.deeper(&child, depth)?)?;
                }
                Found::Link(_) if scope == Scope::Containing => {
                    return Err(GuardError::LinkedFolder {
                        engine: self.engine,
                        path: self.home.path.join(&child),
                    });
                }
                Found::Link(target) => {
                    manifest.insert(child, Entry::Link(target));
                }
                Found::File { length, mode } => {
                    let bytes = folder
                        .read(&name, length)
                        .map_err(self.snapshotting(&child))?;
                    let copy = self.copies.join(&child);
                    copy.parent()
                        .map_or(Ok(()), fs::create_dir_all)
                        .and_then(|()| fs::write(&copy, &bytes))
                        .map_err(|source| GuardError::Snapshot {
                            engine: self.engine,
                            path: copy.clone(),
                            source,
                        })?;
                    manifest.insert(
                        child,
                        Entry::File {
                            digest: digest(&bytes),
                            length: bytes.len() as u64,
                            mode,
                        },
                    );
                }
            }
        }
        Ok(())
    }

    fn reconcile(
        &self,
        folder: &Folder,
        relative: &Path,
        manifest: &Manifest,
        depth: usize,
    ) -> Result<(), GuardError> {
        let recorded = manifest
            .keys()
            .filter(|path| path.parent() == Some(relative))
            .filter_map(|path| path.file_name().map(OsStr::to_os_string));
        let mut names = self
            .in_scope(folder, relative)
            .map_err(self.restoring(relative))?;
        names.extend(recorded);
        for name in names {
            let child = relative.join(&name);
            if scope(self.engine, &child) == Scope::Outside {
                continue;
            }
            self.reconcile_entry(folder, &name, &child, manifest, depth)?;
        }
        Ok(())
    }

    fn reconcile_entry(
        &self,
        folder: &Folder,
        name: &OsStr,
        child: &Path,
        manifest: &Manifest,
        depth: usize,
    ) -> Result<(), GuardError> {
        let restoring = || self.restoring(child);
        let found = folder.found(name).map_err(restoring())?;
        let recorded = manifest.get(child);
        let planted = match (recorded, &found) {
            (None, Found::Missing) => return Ok(()),
            (Some(Entry::Folder), Found::Folder) => {
                let inner = folder.open(name).map_err(restoring())?;
                return self.reconcile(&inner, child, manifest, self.deeper(child, depth)?);
            }
            (Some(Entry::Link(wanted)), Found::Link(target)) if wanted == target => {
                return Ok(());
            }
            (
                Some(Entry::File {
                    digest,
                    length,
                    mode,
                }),
                Found::File {
                    length: found_length,
                    mode: found_mode,
                },
            ) if length == found_length
                && mode == found_mode
                && folder.holds(name, *length, digest).map_err(restoring())? =>
            {
                return Ok(());
            }
            (_, Found::Missing) => false,
            (_, Found::Folder | Found::Link(_) | Found::File { .. } | Found::Other) => true,
        };
        if planted {
            tracing::warn!(engine = %self.engine, path = %self.home.path.join(child).display(), "engine configuration changed while a sandboxed run could write it; restoring Forge's trusted copy");
        }
        match recorded {
            None => folder.remove(name, self.home).map_err(restoring()),
            Some(Entry::Folder) => {
                folder.remove(name, self.home).map_err(restoring())?;
                let inner = folder.make_folder(name).map_err(restoring())?;
                self.reconcile(&inner, child, manifest, self.deeper(child, depth)?)
            }
            Some(Entry::Link(target)) => folder
                .remove(name, self.home)
                .and_then(|()| folder.put_link(name, target))
                .map_err(restoring()),
            Some(Entry::File {
                digest: wanted,
                mode,
                ..
            }) => {
                let copy = self.copies.join(child);
                let bytes = fs::read(&copy).map_err(|source| GuardError::Restore {
                    engine: self.engine,
                    path: copy.clone(),
                    source,
                })?;
                if digest(&bytes) != *wanted {
                    return Err(GuardError::CopyChanged {
                        engine: self.engine,
                        path: copy,
                    });
                }
                if matches!(found, Found::Folder) {
                    folder.remove(name, self.home).map_err(restoring())?;
                }
                folder.put_file(name, &bytes, *mode).map_err(restoring())
            }
        }
    }
}

pub(crate) fn restore(engine: EngineId, home: &Path) -> Result<(), GuardError> {
    let snapshot = snapshot_folder(home);
    if read_manifest(engine, &snapshot)?.is_none() {
        return Ok(());
    }
    let _busy = hold(engine, home, BUSY_LOCK)?;
    let Some(manifest) = read_manifest(engine, &snapshot)? else {
        return Ok(());
    };
    let restoring = |path: &Path| {
        let path = path.to_path_buf();
        move |source| GuardError::Restore {
            engine,
            path,
            source,
        }
    };
    fs::create_dir_all(home).map_err(restoring(home))?;
    let opened = Home::open(home).map_err(restoring(home))?;
    let copies = snapshot.join(COPIES);
    let walk = Walk {
        engine,
        home: &opened,
        copies: &copies,
    };
    walk.reconcile(&opened.folder, Path::new(""), &manifest, 0)?;
    restore_claude_servers(engine, &opened, &snapshot)?;
    if !still_exposed(engine, home)? {
        fs::remove_dir_all(&snapshot).map_err(restoring(&snapshot))?;
    }
    Ok(())
}

fn split(relative: &str) -> (&Path, &OsStr) {
    let path = Path::new(relative);
    let name = path
        .file_name()
        .unwrap_or_else(|| unreachable!("invariant: guarded file {relative} has a name"));
    (path.parent().unwrap_or_else(|| Path::new("")), name)
}

pub(crate) fn write(
    engine: EngineId,
    home: &Path,
    relative: &str,
    bytes: &[u8],
    mode: u32,
) -> Result<(), GuardError> {
    assert!(
        scope(engine, Path::new(relative)) == Scope::Guarded,
        "invariant: Forge writes only guarded {engine} configuration through the guard, not {relative}"
    );
    let _busy = hold(engine, home, BUSY_LOCK)?;
    let path = home.join(relative);
    let writing = |source| GuardError::Write {
        engine,
        path: path.clone(),
        source,
    };
    let (parent, name) = split(relative);
    fs::create_dir_all(home).map_err(writing)?;
    let opened = Home::open(home).map_err(writing)?;
    let folder = opened
        .reach(parent, true)
        .map_err(writing)?
        .expect("invariant: reaching a folder while creating it always yields it");
    let wanted = digest(bytes);
    let current = folder.found(name).map_err(writing)?;
    let unchanged = match current {
        Found::File {
            length,
            mode: found_mode,
        } => {
            length == bytes.len() as u64
                && found_mode == mode
                && folder.holds(name, length, &wanted).map_err(writing)?
        }
        Found::Missing | Found::Folder | Found::Link(_) | Found::Other => false,
    };
    if !unchanged {
        if matches!(current, Found::Folder) {
            folder.remove(name, &opened).map_err(writing)?;
        }
        folder.put_file(name, bytes, mode).map_err(writing)?;
    }
    let snapshot = snapshot_folder(home);
    let Some(mut manifest) = read_manifest(engine, &snapshot)? else {
        return Ok(());
    };
    let copy = snapshot.join(COPIES).join(relative);
    copy.parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(&copy, bytes))
        .map_err(|source| GuardError::Snapshot {
            engine,
            path: copy.clone(),
            source,
        })?;
    for ancestor in Path::new(relative).ancestors().skip(1) {
        if !ancestor.as_os_str().is_empty() {
            manifest.insert(ancestor.to_path_buf(), Entry::Folder);
        }
    }
    manifest.insert(
        PathBuf::from(relative),
        Entry::File {
            digest: wanted,
            length: bytes.len() as u64,
            mode,
        },
    );
    write_manifest(engine, &snapshot, &manifest)
}

pub(crate) fn read(
    engine: EngineId,
    home: &Path,
    relative: &str,
) -> Result<Option<Vec<u8>>, GuardError> {
    let path = home.join(relative);
    let reading = |source| GuardError::Read {
        engine,
        path: path.clone(),
        source,
    };
    let (parent, name) = split(relative);
    let Some(opened) = Home::open_existing(home).map_err(reading)? else {
        return Ok(None);
    };
    let Some(folder) = opened.reach(parent, false).map_err(reading)? else {
        return Ok(None);
    };
    match folder.found(name).map_err(reading)? {
        Found::File { length, .. } => folder.read(name, length).map(Some).map_err(reading),
        Found::Missing | Found::Folder | Found::Link(_) | Found::Other => Ok(None),
    }
}

pub(crate) fn remove(engine: EngineId, home: &Path, relative: &str) -> Result<(), GuardError> {
    assert!(
        scope(engine, Path::new(relative)) == Scope::Guarded,
        "invariant: Forge removes only guarded {engine} configuration through the guard, not {relative}"
    );
    let _busy = hold(engine, home, BUSY_LOCK)?;
    let path = home.join(relative);
    let removing = |source| GuardError::Write {
        engine,
        path: path.clone(),
        source,
    };
    let (parent, name) = split(relative);
    let Some(opened) = Home::open_existing(home).map_err(removing)? else {
        return Ok(());
    };
    if let Some(folder) = opened.reach(parent, false).map_err(removing)? {
        folder.remove(name, &opened).map_err(removing)?;
    }
    let snapshot = snapshot_folder(home);
    let Some(mut manifest) = read_manifest(engine, &snapshot)? else {
        return Ok(());
    };
    if manifest.remove(Path::new(relative)).is_some() {
        write_manifest(engine, &snapshot, &manifest)?;
    }
    Ok(())
}

fn claude_folder(home: &Home) -> io::Result<Option<Folder>> {
    match home.folder.found(OsStr::new(CLAUDE_FOLDER))? {
        Found::Folder => home.folder.open(OsStr::new(CLAUDE_FOLDER)).map(Some),
        Found::Missing | Found::Link(_) | Found::File { .. } | Found::Other => Ok(None),
    }
}

fn claude_state(engine: EngineId, home: &Home) -> Result<Option<(Folder, Value)>, GuardError> {
    let path = home.path.join(CLAUDE_FOLDER).join(CLAUDE_STATE);
    let reading = |source| GuardError::Restore {
        engine,
        path: path.clone(),
        source,
    };
    let Some(folder) = claude_folder(home).map_err(reading)? else {
        return Ok(None);
    };
    let name = OsStr::new(CLAUDE_STATE);
    let bytes = match folder.found(name).map_err(reading)? {
        Found::File { length, .. } => folder.read(name, length).map_err(reading)?,
        Found::Missing => return Ok(Some((folder, Value::Object(serde_json::Map::new())))),
        Found::Folder | Found::Link(_) | Found::Other => {
            tracing::warn!(%engine, path = %path.display(), "Claude's state file was replaced by something else while a sandboxed run could write it; removing it");
            folder.remove(name, home).map_err(reading)?;
            return Ok(Some((folder, Value::Object(serde_json::Map::new()))));
        }
    };
    let state = serde_json::from_slice::<Value>(&bytes).map_err(|source| GuardError::State {
        engine,
        path: path.clone(),
        source,
    })?;
    Ok(Some((folder, state)))
}

fn servers_of(state: &Value) -> HashMap<String, Value> {
    let top = state
        .get("mcpServers")
        .map(|servers| (String::new(), servers.clone()));
    let projects = state
        .get("projects")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(project, settings)| {
            settings
                .get("mcpServers")
                .map(|servers| (project.clone(), servers.clone()))
        });
    top.into_iter().chain(projects).collect()
}

fn claude_servers(engine: EngineId, home: &Home) -> Result<HashMap<String, Value>, GuardError> {
    Ok(claude_state(engine, home)?.map_or_else(HashMap::new, |(_, state)| servers_of(&state)))
}

fn restore_claude_servers(
    engine: EngineId,
    home: &Home,
    snapshot: &Path,
) -> Result<(), GuardError> {
    if engine != EngineId::Claude {
        return Ok(());
    }
    let copy = snapshot.join(CLAUDE_STATE_COPY);
    let trusted: HashMap<String, Value> = match fs::read(&copy) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| GuardError::State {
            engine,
            path: copy.clone(),
            source,
        })?,
        Err(source) => {
            return Err(GuardError::Restore {
                engine,
                path: copy,
                source,
            });
        }
    };
    let Some((folder, mut state)) = claude_state(engine, home)? else {
        return Ok(());
    };
    if servers_of(&state) == trusted {
        return Ok(());
    }
    let path = home.path.join(CLAUDE_FOLDER).join(CLAUDE_STATE);
    tracing::warn!(%engine, path = %path.display(), "Claude's MCP servers changed while a sandboxed run could write them; restoring Forge's trusted copy");
    if !state.is_object() {
        state = Value::Object(serde_json::Map::new());
    }
    let object = state
        .as_object_mut()
        .expect("invariant: Claude's state was just made an object");
    match trusted.get("") {
        Some(servers) => object.insert("mcpServers".to_owned(), servers.clone()),
        None => object.remove("mcpServers"),
    };
    if let Some(projects) = object.get_mut("projects").and_then(Value::as_object_mut) {
        for (project, settings) in projects.iter_mut() {
            if let Some(settings) = settings.as_object_mut() {
                match trusted.get(project) {
                    Some(servers) => settings.insert("mcpServers".to_owned(), servers.clone()),
                    None => settings.remove("mcpServers"),
                };
            }
        }
    }
    let bytes = serde_json::to_vec(&state).map_err(|source| GuardError::State {
        engine,
        path: path.clone(),
        source,
    })?;
    folder
        .put_file(OsStr::new(CLAUDE_STATE), &bytes, CLAUDE_STATE_MODE)
        .map_err(|source| GuardError::Restore {
            engine,
            path,
            source,
        })
}

#[cfg(test)]
#[path = "../../tests/sandbox/guard.rs"]
mod tests;
