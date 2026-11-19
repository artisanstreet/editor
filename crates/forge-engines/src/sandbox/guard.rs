use super::home::{Create, Folder, Found, Home, MAX_DEPTH};
use artisan_api::EngineId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const SNAPSHOT_FOLDER: &str = "guarded-config";
const COPIES: &str = "files";
const MANIFEST: &str = "manifest.json";
const CLAUDE_FOLDER: &str = ".claude";
const CLAUDE_STATE: &str = ".claude.json";
const CLAUDE_STATE_COPY: &str = "claude-mcp-servers.json";
const LIVE_MARKERS: &str = "guarded-config-live";
const BUSY_LOCK: &str = "guarded-config-busy";
const CLAUDE_STATE_MODE: u32 = 0o600;

const fn surfaces(engine: EngineId) -> &'static [&'static str] {
    match engine {
        EngineId::Claude => &[
            ".claude/settings.json",
            ".claude/settings.local.json",
            ".claude/plugins",
            ".claude/hooks",
            ".claude/commands",
            ".claude/agents",
            ".claude/skills",
            ".claude/CLAUDE.md",
            ".artisan/claude-mcp.json",
        ],
        EngineId::Codex => &[
            ".codex/config.toml",
            ".codex/hooks.json",
            ".codex/plugins",
            ".codex/rules",
            ".codex/skills",
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
            "config/opencode/skill",
            "config/opencode/skills",
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
    #[error("move {engine}'s {} out to {}", path.display(), to.display())]
    Take {
        engine: EngineId,
        path: PathBuf,
        to: PathBuf,
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

fn holds(folder: &Folder, name: &OsStr, length: u64, wanted: &str) -> io::Result<bool> {
    let bytes = folder.read(name, length)?;
    Ok(bytes.len() as u64 == length && digest(&bytes) == wanted)
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
    if forge_isolation::leftover_runs_alive() {
        return Ok(true);
    }
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
            self.record_one(folder, &name, &relative.join(&name), manifest, depth)?;
        }
        Ok(())
    }

    fn record_one(
        &self,
        folder: &Folder,
        name: &OsStr,
        child: &Path,
        manifest: &mut Manifest,
        depth: usize,
    ) -> Result<(), GuardError> {
        let scope = scope(self.engine, child);
        if scope == Scope::Outside {
            return Ok(());
        }
        match folder.found(name).map_err(self.snapshotting(child))? {
            Found::Missing | Found::Other => {}
            Found::Folder => {
                manifest.insert(child.to_path_buf(), Entry::Folder);
                let inner = folder.open(name).map_err(self.snapshotting(child))?;
                self.record(&inner, child, manifest, self.deeper(child, depth)?)?;
            }
            Found::Link(_) if scope == Scope::Containing => {
                return Err(GuardError::LinkedFolder {
                    engine: self.engine,
                    path: self.home.path.join(child),
                });
            }
            Found::Link(target) => {
                manifest.insert(child.to_path_buf(), Entry::Link(target));
            }
            Found::File { length, mode } => {
                let bytes = folder
                    .read(name, length)
                    .map_err(self.snapshotting(child))?;
                let copy = self.copies.join(child);
                copy.parent()
                    .map_or(Ok(()), fs::create_dir_all)
                    .and_then(|()| fs::write(&copy, &bytes))
                    .map_err(|source| GuardError::Snapshot {
                        engine: self.engine,
                        path: copy.clone(),
                        source,
                    })?;
                manifest.insert(
                    child.to_path_buf(),
                    Entry::File {
                        digest: digest(&bytes),
                        length: bytes.len() as u64,
                        mode,
                    },
                );
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
            (None, Found::Folder) if scope(self.engine, child) == Scope::Containing => {
                let inner = folder.open(name).map_err(restoring())?;
                return self.reconcile(&inner, child, manifest, self.deeper(child, depth)?);
            }
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
                && holds(folder, name, *length, digest).map_err(restoring())? =>
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
            None => folder.remove(name).map_err(restoring()),
            Some(Entry::Folder) => {
                folder.remove(name).map_err(restoring())?;
                let inner = folder.make_folder(name).map_err(restoring())?;
                self.reconcile(&inner, child, manifest, self.deeper(child, depth)?)
            }
            Some(Entry::Link(target)) => folder
                .remove(name)
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
                    folder.remove(name).map_err(restoring())?;
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

fn split(relative: &Path) -> (&Path, &OsStr) {
    let name = relative.file_name().unwrap_or_else(|| {
        unreachable!("invariant: guarded entry {} has a name", relative.display())
    });
    (relative.parent().unwrap_or_else(|| Path::new("")), name)
}

fn guarded_only(engine: EngineId, relative: &Path) {
    assert!(
        scope(engine, relative) == Scope::Guarded,
        "invariant: Forge changes only guarded {engine} configuration through the guard, not {}",
        relative.display()
    );
}

struct Held {
    _busy: fs::File,
    opened: Home,
}

fn held(engine: EngineId, home: &Path, relative: &Path) -> Result<Held, GuardError> {
    let busy = hold(engine, home, BUSY_LOCK)?;
    let writing = |source| GuardError::Write {
        engine,
        path: home.join(relative),
        source,
    };
    fs::create_dir_all(home).map_err(writing)?;
    let opened = Home::open(home).map_err(writing)?;
    Ok(Held {
        _busy: busy,
        opened,
    })
}

fn record_change(
    engine: EngineId,
    home: &Path,
    change: impl FnOnce(&mut Manifest, &Path) -> Result<(), GuardError>,
) -> Result<(), GuardError> {
    let snapshot = snapshot_folder(home);
    let Some(mut manifest) = read_manifest(engine, &snapshot)? else {
        return Ok(());
    };
    change(&mut manifest, &snapshot.join(COPIES))?;
    write_manifest(engine, &snapshot, &manifest)
}

fn record_ancestors(manifest: &mut Manifest, relative: &Path) {
    for ancestor in relative.ancestors().skip(1) {
        if !ancestor.as_os_str().is_empty() {
            manifest.insert(ancestor.to_path_buf(), Entry::Folder);
        }
    }
}

fn forget(manifest: &mut Manifest, relative: &Path) {
    manifest.retain(|path, _| !path.starts_with(relative));
}

pub(crate) struct Editing {
    engine: EngineId,
    home: PathBuf,
    relative: PathBuf,
    held: Held,
    base: Option<Vec<u8>>,
}

pub(crate) fn edit(engine: EngineId, home: &Path, relative: &Path) -> Result<Editing, GuardError> {
    guarded_only(engine, relative);
    let held = held(engine, home, relative)?;
    let snapshot = snapshot_folder(home);
    let base = match read_manifest(engine, &snapshot)? {
        Some(manifest) => trusted_copy(engine, &snapshot, &manifest, relative)?,
        None => live_file(engine, &held.opened, relative)?,
    };
    Ok(Editing {
        engine,
        home: home.to_path_buf(),
        relative: relative.to_path_buf(),
        held,
        base,
    })
}

fn trusted_copy(
    engine: EngineId,
    snapshot: &Path,
    manifest: &Manifest,
    relative: &Path,
) -> Result<Option<Vec<u8>>, GuardError> {
    let Some(Entry::File { digest: wanted, .. }) = manifest.get(relative) else {
        return Ok(None);
    };
    let copy = snapshot.join(COPIES).join(relative);
    let bytes = fs::read(&copy).map_err(|source| GuardError::Read {
        engine,
        path: copy.clone(),
        source,
    })?;
    if digest(&bytes) == *wanted {
        Ok(Some(bytes))
    } else {
        Err(GuardError::CopyChanged { engine, path: copy })
    }
}

fn live_file(
    engine: EngineId,
    opened: &Home,
    relative: &Path,
) -> Result<Option<Vec<u8>>, GuardError> {
    let reading = |source| GuardError::Read {
        engine,
        path: opened.path.join(relative),
        source,
    };
    let (parent, name) = split(relative);
    let Some(folder) = opened.reach(parent, Create::No).map_err(reading)? else {
        return Ok(None);
    };
    match folder.found(name).map_err(reading)? {
        Found::File { length, .. } => folder.read(name, length).map(Some).map_err(reading),
        Found::Missing | Found::Folder | Found::Link(_) | Found::Other => Ok(None),
    }
}

impl Editing {
    pub(crate) fn base(&self) -> Option<&[u8]> {
        self.base.as_deref()
    }

    pub(crate) fn write(self, bytes: &[u8], mode: u32) -> Result<(), GuardError> {
        put(
            self.engine,
            &self.home,
            &self.held,
            &self.relative,
            bytes,
            mode,
        )
    }
}

pub(crate) fn write(
    engine: EngineId,
    home: &Path,
    relative: &Path,
    bytes: &[u8],
    mode: u32,
) -> Result<(), GuardError> {
    guarded_only(engine, relative);
    let held = held(engine, home, relative)?;
    put(engine, home, &held, relative, bytes, mode)
}

fn put(
    engine: EngineId,
    home: &Path,
    held: &Held,
    relative: &Path,
    bytes: &[u8],
    mode: u32,
) -> Result<(), GuardError> {
    let writing = |source| GuardError::Write {
        engine,
        path: home.join(relative),
        source,
    };
    let (parent, name) = split(relative);
    let folder = held
        .opened
        .reach(parent, Create::Replacing)
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
                && holds(&folder, name, length, &wanted).map_err(writing)?
        }
        Found::Missing | Found::Folder | Found::Link(_) | Found::Other => false,
    };
    if !unchanged {
        if matches!(current, Found::Folder) {
            folder.remove(name).map_err(writing)?;
        }
        folder.put_file(name, bytes, mode).map_err(writing)?;
    }
    record_change(engine, home, |manifest, copies| {
        let copy = copies.join(relative);
        copy.parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&copy, bytes))
            .map_err(|source| GuardError::Snapshot {
                engine,
                path: copy.clone(),
                source,
            })?;
        record_ancestors(manifest, relative);
        manifest.insert(
            relative.to_path_buf(),
            Entry::File {
                digest: wanted,
                length: bytes.len() as u64,
                mode,
            },
        );
        Ok(())
    })
}

pub(crate) fn link(
    engine: EngineId,
    home: &Path,
    relative: &Path,
    target: &Path,
) -> Result<(), GuardError> {
    guarded_only(engine, relative);
    let held = held(engine, home, relative)?;
    let writing = |source| GuardError::Write {
        engine,
        path: home.join(relative),
        source,
    };
    let (parent, name) = split(relative);
    let folder = held
        .opened
        .reach(parent, Create::Replacing)
        .map_err(writing)?
        .expect("invariant: reaching a folder while creating it always yields it");
    if matches!(folder.found(name).map_err(writing)?, Found::Folder) {
        folder.remove(name).map_err(writing)?;
    }
    folder.put_link(name, target).map_err(writing)?;
    record_change(engine, home, |manifest, _| {
        record_ancestors(manifest, relative);
        manifest.insert(relative.to_path_buf(), Entry::Link(target.to_path_buf()));
        Ok(())
    })
}

pub(crate) fn remove(engine: EngineId, home: &Path, relative: &Path) -> Result<(), GuardError> {
    guarded_only(engine, relative);
    let held = held(engine, home, relative)?;
    let removing = |source| GuardError::Write {
        engine,
        path: home.join(relative),
        source,
    };
    let (parent, name) = split(relative);
    if let Some(folder) = held.opened.reach(parent, Create::No).map_err(removing)? {
        folder.remove(name).map_err(removing)?;
    }
    record_change(engine, home, |manifest, _| {
        forget(manifest, relative);
        Ok(())
    })
}

pub(crate) fn give(
    engine: EngineId,
    home: &Path,
    relative: &Path,
    from: &Path,
) -> Result<(), GuardError> {
    guarded_only(engine, relative);
    let held = held(engine, home, relative)?;
    let moving = |source| GuardError::Take {
        engine,
        path: from.to_path_buf(),
        to: home.join(relative),
        source,
    };
    let (parent, name) = split(relative);
    let folder = held
        .opened
        .reach(parent, Create::Replacing)
        .map_err(moving)?
        .expect("invariant: reaching a folder while creating it always yields it");
    let (source_folder, source_name) = split(from);
    let source = Home::open(source_folder).map_err(moving)?;
    source
        .folder
        .move_into_new(source_name, &folder, name)
        .map_err(moving)?;
    record_change(engine, home, |manifest, copies| {
        record_ancestors(manifest, relative);
        let walk = Walk {
            engine,
            home: &held.opened,
            copies,
        };
        walk.record_one(&folder, name, relative, manifest, 0)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Placed {
    Link(PathBuf),
    Other,
}

pub(crate) fn entries(
    engine: EngineId,
    home: &Path,
    relative: &Path,
) -> Result<Vec<(OsString, Placed)>, GuardError> {
    let reading = |source| GuardError::Read {
        engine,
        path: home.join(relative),
        source,
    };
    let Some(opened) = Home::open_existing(home).map_err(reading)? else {
        return Ok(Vec::new());
    };
    let Some(folder) = opened.reach(relative, Create::No).map_err(reading)? else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for name in folder.names().map_err(reading)? {
        let placed = match folder.found(&name).map_err(reading)? {
            Found::Missing => continue,
            Found::Link(target) => Placed::Link(target),
            Found::Folder | Found::File { .. } | Found::Other => Placed::Other,
        };
        entries.push((name, placed));
    }
    Ok(entries)
}

pub(crate) fn take(
    engine: EngineId,
    home: &Path,
    relative: &Path,
    to: &Path,
) -> Result<(), GuardError> {
    guarded_only(engine, relative);
    let held = held(engine, home, relative)?;
    let moving = |source| GuardError::Take {
        engine,
        path: home.join(relative),
        to: to.to_path_buf(),
        source,
    };
    let (parent, name) = split(relative);
    let Some(folder) = held.opened.reach(parent, Create::No).map_err(moving)? else {
        return Ok(());
    };
    let (into, to_name) = split(to);
    let destination = Home::open(into).map_err(moving)?;
    match folder.move_to(name, &destination.folder, to_name) {
        Ok(()) => {}
        Err(error) if rustix::io::Errno::from_io_error(&error) == Some(rustix::io::Errno::XDEV) => {
            folder.copy_tree_to(name, to, 0).map_err(moving)?;
            folder.remove(name).map_err(moving)?;
        }
        Err(error) => return Err(moving(error)),
    }
    record_change(engine, home, |manifest, _| {
        forget(manifest, relative);
        Ok(())
    })
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
            folder.remove(name).map_err(reading)?;
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
