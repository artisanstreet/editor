use artisan_api::EngineId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const SNAPSHOT_FOLDER: &str = "guarded-config";
const COPIES: &str = "files";
const MANIFEST: &str = "manifest.json";
const CLAUDE_STATE: &str = ".claude/.claude.json";
const CLAUDE_STATE_COPY: &str = "claude-mcp-servers.json";
const LIVE_MARKERS: &str = "guarded-config-live";
const BUSY_LOCK: &str = "guarded-config-busy";

const fn surfaces(engine: EngineId) -> &'static [&'static str] {
    match engine {
        EngineId::Claude => &[
            ".claude/settings.json",
            ".claude/settings.local.json",
            ".claude/plugins",
            ".claude/hooks",
            ".claude/commands",
            ".claude/agents",
        ],
        EngineId::Codex => &[
            ".codex/config.toml",
            ".codex/hooks.json",
            ".codex/plugins",
            ".codex/rules",
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
    #[error("restore {engine}'s configuration at {}", path.display())]
    Restore {
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
    File(Stamp),
    Folder,
    Link(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    device: u64,
    inode: u64,
    length: u64,
    changed: i64,
    changed_nanos: i64,
    mode: u32,
}

type Manifest = BTreeMap<PathBuf, Entry>;

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

fn open_lock(engine: EngineId, home: &Path, name: &str) -> Result<fs::File, GuardError> {
    let path = beside(home, name);
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|source| GuardError::Lock {
            engine,
            path,
            source,
        })
}

fn hold(engine: EngineId, home: &Path, name: &str) -> Result<fs::File, GuardError> {
    let file = open_lock(engine, home, name)?;
    fs2::FileExt::lock_exclusive(&file).map_err(|source| GuardError::Lock {
        engine,
        path: beside(home, name),
        source,
    })?;
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
            remove(&entry.path()).map_err(locking(&entry.path()))?;
        }
    }
    Ok(exposed)
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
    if !snapshot.join(MANIFEST).is_file() {
        take_snapshot(engine, home, &snapshot)?;
    }
    Ok(exposure)
}

pub(crate) fn restore(engine: EngineId, home: &Path) -> Result<(), GuardError> {
    let snapshot = snapshot_folder(home);
    if !snapshot.join(MANIFEST).is_file() {
        return Ok(());
    }
    let _busy = hold(engine, home, BUSY_LOCK)?;
    let manifest_path = snapshot.join(MANIFEST);
    let recorded = match fs::read(&manifest_path) {
        Ok(bytes) => {
            serde_json::from_slice::<Manifest>(&bytes).map_err(|source| GuardError::Manifest {
                engine,
                path: manifest_path.clone(),
                source,
            })?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(GuardError::Restore {
                engine,
                path: manifest_path,
                source,
            });
        }
    };
    let restoring = |path: &Path| {
        let path = path.to_path_buf();
        move |source| GuardError::Restore {
            engine,
            path,
            source,
        }
    };
    let current = walk(home, engine).map_err(restoring(home))?;
    let copies = snapshot.join(COPIES);
    let planted = current
        .iter()
        .filter(|(relative, entry)| recorded.get(*relative) != Some(*entry));
    for (relative, _) in planted {
        let live_path = home.join(relative);
        tracing::warn!(%engine, path = %live_path.display(), "engine configuration changed while a sandboxed run could write it; restoring Forge's trusted copy");
        remove(&live_path).map_err(restoring(&live_path))?;
    }
    let missing = recorded
        .iter()
        .filter(|(relative, entry)| current.get(*relative) != Some(*entry));
    for (relative, entry) in missing {
        let live_path = home.join(relative);
        put_back(&copies.join(relative), &live_path, entry).map_err(restoring(&live_path))?;
    }
    restore_claude_servers(engine, home, &snapshot)?;
    if still_exposed(engine, home)? {
        let manifest = walk(home, engine).map_err(restoring(home))?;
        write_manifest(engine, &snapshot, &manifest)?;
    } else {
        fs::remove_dir_all(&snapshot).map_err(restoring(&snapshot))?;
    }
    Ok(())
}

pub(crate) fn adopt(engine: EngineId, home: &Path, relative: &str) -> Result<(), GuardError> {
    let snapshot = snapshot_folder(home);
    if !snapshot.join(MANIFEST).is_file() {
        return Ok(());
    }
    let _busy = hold(engine, home, BUSY_LOCK)?;
    let manifest_path = snapshot.join(MANIFEST);
    let bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(GuardError::Snapshot {
                engine,
                path: manifest_path,
                source,
            });
        }
    };
    let mut manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|source| GuardError::Manifest {
            engine,
            path: manifest_path.clone(),
            source,
        })?;
    let live_path = home.join(relative);
    let copy = snapshot.join(COPIES).join(relative);
    let snapshotting = |path: &Path| {
        let path = path.to_path_buf();
        move |source| GuardError::Snapshot {
            engine,
            path,
            source,
        }
    };
    if let Some(parent) = copy.parent() {
        fs::create_dir_all(parent).map_err(snapshotting(parent))?;
    }
    fs::copy(&live_path, &copy).map_err(snapshotting(&live_path))?;
    let entry = entry(&live_path).map_err(snapshotting(&live_path))?;
    manifest.insert(PathBuf::from(relative), entry);
    write_manifest(engine, &snapshot, &manifest)
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
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(snapshotting(snapshot)(error));
        }
        _ => {}
    }
    let copies = snapshot.join(COPIES);
    fs::create_dir_all(&copies).map_err(snapshotting(&copies))?;
    let manifest = walk(home, engine).map_err(snapshotting(home))?;
    for (relative, entry) in &manifest {
        let source = home.join(relative);
        put_back(&source, &copies.join(relative), entry).map_err(snapshotting(&source))?;
    }
    if engine == EngineId::Claude {
        let servers = claude_servers(engine, &home.join(CLAUDE_STATE))?;
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

fn walk(home: &Path, engine: EngineId) -> io::Result<Manifest> {
    let mut manifest = Manifest::new();
    for surface in surfaces(engine) {
        collect(home, Path::new(surface), &mut manifest)?;
    }
    Ok(manifest)
}

fn collect(home: &Path, relative: &Path, manifest: &mut Manifest) -> io::Result<()> {
    let path = home.join(relative);
    let found = match entry(&path) {
        Ok(found) => found,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if found == Entry::Folder {
        for child in fs::read_dir(&path)? {
            collect(home, &relative.join(child?.file_name()), manifest)?;
        }
    }
    manifest.insert(relative.to_path_buf(), found);
    Ok(())
}

fn entry(path: &Path) -> io::Result<Entry> {
    let metadata = fs::symlink_metadata(path)?;
    let kind = metadata.file_type();
    Ok(if kind.is_symlink() {
        Entry::Link(fs::read_link(path)?)
    } else if kind.is_dir() {
        Entry::Folder
    } else {
        Entry::File(Stamp {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            changed: metadata.ctime(),
            changed_nanos: metadata.ctime_nsec(),
            mode: metadata.mode(),
        })
    })
}

fn remove(path: &Path) -> io::Result<()> {
    let removed = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) => Err(error),
    };
    match removed {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn put_back(from: &Path, to: &Path, entry: &Entry) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    match entry {
        Entry::Folder => fs::create_dir_all(to),
        Entry::Link(target) => {
            remove(to)?;
            std::os::unix::fs::symlink(target, to)
        }
        Entry::File(stamp) => {
            remove(to)?;
            fs::copy(from, to)?;
            fs::set_permissions(to, std::os::unix::fs::PermissionsExt::from_mode(stamp.mode))
        }
    }
}

fn claude_servers(engine: EngineId, path: &Path) -> Result<HashMap<String, Value>, GuardError> {
    let state = match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice::<Value>(&bytes).map_err(|source| GuardError::State {
                engine,
                path: path.to_path_buf(),
                source,
            })?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(source) => {
            return Err(GuardError::Snapshot {
                engine,
                path: path.to_path_buf(),
                source,
            });
        }
    };
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
    Ok(top.into_iter().chain(projects).collect())
}

fn restore_claude_servers(
    engine: EngineId,
    home: &Path,
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
    let path = home.join(CLAUDE_STATE);
    if claude_servers(engine, &path)? == trusted {
        return Ok(());
    }
    tracing::warn!(%engine, path = %path.display(), "Claude's MCP servers changed while a sandboxed run could write them; restoring Forge's trusted copy");
    let restoring = |source| GuardError::Restore {
        engine,
        path: path.clone(),
        source,
    };
    let bytes = fs::read(&path).map_err(restoring)?;
    let mut state: Value = serde_json::from_slice(&bytes).map_err(|source| GuardError::State {
        engine,
        path: path.clone(),
        source,
    })?;
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
    let temporary = path.with_extension("artisan-guard");
    fs::write(&temporary, bytes)
        .and_then(|()| fs::rename(&temporary, &path))
        .map_err(restoring)
}

#[cfg(test)]
#[path = "../../tests/sandbox/guard.rs"]
mod tests;
