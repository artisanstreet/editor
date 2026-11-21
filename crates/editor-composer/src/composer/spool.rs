use super::draft_session::KeptDraft;
use editor_core::{
    HostId,
    config_file::{self, ReadConfigError, WriteConfigError},
};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::{
    collections::{BTreeSet, HashMap},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak},
};

pub(crate) trait SpoolDisk: Send + Sync {
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn remove_tree(&self, path: &Path) -> std::io::Result<()>;
}

struct LocalDisk;

impl SpoolDisk for LocalDisk {
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }

    fn remove_tree(&self, path: &Path) -> std::io::Result<()> {
        std::fs::remove_dir_all(path)
    }
}

struct Owner {
    dir: PathBuf,
    disk: Arc<dyn SpoolDisk>,
    queues: Mutex<HashMap<String, Arc<Queue>>>,
}

#[derive(Clone)]
pub struct DraftSpool {
    owner: Arc<Owner>,
}

impl PartialEq for DraftSpool {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner)
    }
}

impl Eq for DraftSpool {}

impl std::fmt::Debug for DraftSpool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftSpool")
            .field("dir", &self.owner.dir)
            .finish_non_exhaustive()
    }
}

struct Order {
    issued: u64,
    serving: u64,
    abandoned: BTreeSet<u64>,
    migrated: bool,
}

impl Order {
    fn advance(&mut self) {
        self.serving += 1;
        while self.abandoned.remove(&self.serving) {
            self.serving += 1;
        }
    }
}

struct Queue {
    order: Mutex<Order>,
    served: Condvar,
}

impl Queue {
    fn order(&self) -> MutexGuard<'_, Order> {
        self.order.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub(crate) struct Turn {
    owner: Arc<Owner>,
    queue: Arc<Queue>,
    host: HostId,
    scope: String,
    stem: String,
    number: u64,
    served: bool,
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut order = self.queue.order();
        match (self.served, order.serving == self.number) {
            (true, _) | (false, true) => order.advance(),
            (false, false) => {
                order.abandoned.insert(self.number);
                return;
            }
        }
        drop(order);
        self.queue.served.notify_all();
    }
}

pub(crate) struct Serving {
    turn: Turn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BesideSlot(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Slot {
    Record,
    Beside(BesideSlot),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Holder {
    Owner(Slot),
    Newcomer,
}

#[derive(Debug)]
pub(crate) struct BesideDraft {
    pub(crate) slot: BesideSlot,
    pub(crate) spooled: SpooledDraft,
}

#[derive(Debug)]
pub struct SpooledDraft {
    pub(crate) draft: KeptDraft,
    pub(crate) images: Vec<Result<Vec<u8>, TakeSpooledImageError>>,
}

#[derive(Debug, thiserror::Error)]
pub enum KeepSpooledDraftError {
    #[error("create the directory {path}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("list the drafts kept in {path}")]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write the pasted image {path}")]
    WriteImage {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write the draft")]
    Draft(#[source] WriteConfigError),
    #[error("flush the folder {path} to disk")]
    Sync {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("move the written draft {from} to {to}")]
    Publish {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum TakeSpooledDraftError {
    #[error("list the drafts kept in {path}")]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the draft kept on this device is unreadable, so it was moved to {set_aside}")]
    SetAside {
        set_aside: PathBuf,
        #[source]
        source: Box<ReadConfigError>,
    },
    #[error("the draft kept on this device at {path} is unreadable ({unreadable}) and stays there")]
    Left {
        path: PathBuf,
        unreadable: Box<ReadConfigError>,
        #[source]
        source: Box<SetAsideError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum KeptBesideError {
    #[error("list the drafts kept in {path}")]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the draft kept beside another is unreadable, so it was moved to {set_aside}")]
    SetAside {
        set_aside: PathBuf,
        #[source]
        source: Box<ReadConfigError>,
    },
    #[error("the draft kept beside another at {path} is unreadable ({unreadable}) and stays there")]
    Left {
        path: PathBuf,
        unreadable: Box<ReadConfigError>,
        #[source]
        source: Box<SetAsideError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum TakeSpooledImageError {
    #[error("read the pasted image {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the pasted image {path} does not match its digest")]
    Damaged { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum SetAsideError {
    #[error("list the drafts kept in {path}")]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("move {from} to {to}")]
    Move {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ForgetSpooledDraftError {
    #[error("list the drafts kept in {path}")]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("move the discarded draft {from} to {to}")]
    Tombstone {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove {path}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("flush the folder {path} to disk")]
    Sync {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl ForgetSpooledDraftError {
    pub(crate) const fn slot_freed(&self) -> bool {
        match self {
            Self::List { .. } | Self::Tombstone { .. } => false,
            Self::Remove { .. } | Self::Sync { .. } => true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum MigrateError {
    #[error("list the drafts kept in {path}")]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("create the folder {path} for drafts kept in the old layout")]
    CreateFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("copy {from} to {to}")]
    Copy {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("flush the folder {path} to disk")]
    Sync {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("move the copied draft {from} to {to}")]
    Publish {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove {path}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
struct ImageDigest(String);

#[derive(Debug, thiserror::Error)]
#[error("{0:?} is not a SHA-256 hex digest")]
struct NotADigest(String);

impl TryFrom<String> for ImageDigest {
    type Error = NotADigest;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        if text.len() == 64
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            Ok(Self(text))
        } else {
            Err(NotADigest(text))
        }
    }
}

impl From<ImageDigest> for String {
    fn from(digest: ImageDigest) -> Self {
        digest.0
    }
}

impl ImageDigest {
    fn of(bytes: &[u8]) -> Self {
        Self(hex::encode(sha2::Sha256::digest(bytes)))
    }

    fn file_name(&self) -> String {
        format!("{}.image", self.0)
    }
}

#[derive(Serialize, Deserialize)]
struct Record {
    host: HostId,
    scope: String,
    #[serde(flatten)]
    draft: KeptDraft,
    #[serde(default)]
    images: Vec<ImageDigest>,
}

const RECORD: &str = "record.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Unit {
    slot: Slot,
    generation: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Unit(Unit),
    Tombstone,
    Staging,
    Other,
}

impl Entry {
    fn of(name: &str) -> Self {
        let unit = |slot: &str, generation: &str| {
            let slot = match slot {
                "record" => Some(Slot::Record),
                slot => slot
                    .strip_prefix("kept-")
                    .and_then(|number| number.parse().ok())
                    .map(|number| Slot::Beside(BesideSlot(number))),
            };
            slot.zip(generation.parse().ok())
                .map(|(slot, generation)| Self::Unit(Unit { slot, generation }))
        };
        match name.split_once('.') {
            Some((slot, generation)) => unit(slot, generation).unwrap_or(Self::Other),
            None if name.starts_with("rejected-") => Self::Tombstone,
            None if name.starts_with("staging-") => Self::Staging,
            None => Self::Other,
        }
    }
}

impl Unit {
    fn name(self) -> String {
        match self.slot {
            Slot::Record => format!("record.{}", self.generation),
            Slot::Beside(BesideSlot(number)) => format!("kept-{number}.{}", self.generation),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Legacy {
    Record,
    Image(ImageDigest),
    Beside(BesideSlot),
    Tombstone,
}

impl Legacy {
    fn of(stem: &str, name: &str) -> Option<Self> {
        let rest = name.strip_prefix(stem)?.strip_prefix('.')?;
        match rest {
            "json" => Some(Self::Record),
            rest => {
                if let Some(digest) = rest.strip_suffix(".image") {
                    return ImageDigest::try_from(digest.to_owned())
                        .ok()
                        .map(Self::Image);
                }
                if let Some(number) = rest.strip_prefix("kept-") {
                    return number
                        .parse()
                        .ok()
                        .map(|number| Self::Beside(BesideSlot(number)));
                }
                rest.strip_prefix("rejected-").map(|_| Self::Tombstone)
            }
        }
    }
}

fn owners() -> MutexGuard<'static, HashMap<PathBuf, Weak<Owner>>> {
    static OWNERS: OnceLock<Mutex<HashMap<PathBuf, Weak<Owner>>>> = OnceLock::new();
    OWNERS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

impl DraftSpool {
    #[must_use]
    pub fn new(editor_dir: &Path) -> Self {
        Self::at(editor_dir.join("drafts"), || Arc::new(LocalDisk))
    }

    #[cfg(test)]
    pub(crate) fn with_disk(editor_dir: &Path, disk: Arc<dyn SpoolDisk>) -> Self {
        let expected = Arc::clone(&disk);
        let spool = Self::at(editor_dir.join("drafts"), move || disk);
        assert!(
            Arc::ptr_eq(&spool.owner.disk, &expected),
            "invariant: a drafts folder is written through one disk"
        );
        spool
    }

    fn at(dir: PathBuf, disk: impl FnOnce() -> Arc<dyn SpoolDisk>) -> Self {
        let mut owners = owners();
        owners.retain(|_, owner| owner.strong_count() > 0);
        if let Some(owner) = owners.get(&dir).and_then(Weak::upgrade) {
            return Self { owner };
        }
        let owner = Arc::new(Owner {
            dir: dir.clone(),
            disk: disk(),
            queues: Mutex::default(),
        });
        owners.insert(dir, Arc::downgrade(&owner));
        Self { owner }
    }

    pub(crate) fn turn(&self, host: &HostId, scope: &str) -> Turn {
        let stem = stem(host, scope);
        let queue = Arc::clone(
            self.owner
                .queues
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(stem.clone())
                .or_insert_with(|| {
                    Arc::new(Queue {
                        order: Mutex::new(Order {
                            issued: 0,
                            serving: 1,
                            abandoned: BTreeSet::new(),
                            migrated: false,
                        }),
                        served: Condvar::new(),
                    })
                }),
        );
        let number = {
            let mut order = queue.order();
            order.issued += 1;
            order.issued
        };
        Turn {
            owner: Arc::clone(&self.owner),
            queue,
            host: host.clone(),
            scope: scope.to_owned(),
            stem,
            number,
            served: false,
        }
    }

    pub fn take(
        &self,
        host: &HostId,
        scope: &str,
    ) -> Result<Option<SpooledDraft>, TakeSpooledDraftError> {
        self.turn(host, scope).wait().take()
    }
}

impl Turn {
    pub(crate) fn wait(mut self) -> Serving {
        let mut order = self.queue.order();
        while order.serving != self.number {
            order = self
                .queue
                .served
                .wait(order)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let migrated = order.migrated;
        drop(order);
        self.served = true;
        let serving = Serving { turn: self };
        if !migrated {
            match serving.migrate() {
                Ok(()) => serving.turn.queue.order().migrated = true,
                Err(error) => tracing::error!(
                    host = %serving.turn.host,
                    scope = %serving.turn.scope,
                    error = %::diagnostics::chain(&error),
                    "couldn't move drafts kept in the old layout into the new one; trying again on the next save"
                ),
            }
        }
        serving
    }
}

impl Serving {
    pub(crate) const fn number(&self) -> u64 {
        self.turn.number
    }

    fn root(&self) -> PathBuf {
        self.turn.owner.dir.join(&self.turn.stem)
    }

    fn disk(&self) -> &dyn SpoolDisk {
        &*self.turn.owner.disk
    }

    fn entries(&self) -> std::io::Result<Vec<(Entry, PathBuf)>> {
        let root = self.root();
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        entries
            .map(|entry| {
                let entry = entry?;
                let entry_kind = entry.file_name().to_str().map_or(Entry::Other, Entry::of);
                Ok((entry_kind, entry.path()))
            })
            .collect()
    }

    fn units(&self) -> std::io::Result<Vec<Unit>> {
        let mut units: Vec<Unit> = self
            .entries()?
            .into_iter()
            .filter_map(|(entry, _)| match entry {
                Entry::Unit(unit) => Some(unit),
                Entry::Tombstone | Entry::Staging | Entry::Other => None,
            })
            .collect();
        units.sort_by_key(|unit| (unit.slot, unit.generation));
        Ok(units)
    }

    fn current(units: &[Unit], slot: Slot) -> Option<Unit> {
        units.iter().copied().rfind(|unit| unit.slot == slot)
    }

    fn unit_path(&self, unit: Unit) -> PathBuf {
        self.root().join(unit.name())
    }

    pub(crate) fn slot_path(&self, slot: Slot) -> PathBuf {
        self.units()
            .ok()
            .and_then(|units| Self::current(&units, slot))
            .map_or_else(|| self.root(), |unit| self.unit_path(unit))
    }

    fn create_root(&self) -> Result<(), KeepSpooledDraftError> {
        let root = self.root();
        if root.is_dir() {
            return Ok(());
        }
        let failed = |path: &Path| {
            let path = path.to_owned();
            move |source| KeepSpooledDraftError::CreateDirectory { path, source }
        };
        let dir = &self.turn.owner.dir;
        std::fs::create_dir_all(&root).map_err(failed(&root))?;
        dir.parent()
            .map_or(Ok(()), sync_folder)
            .and_then(|()| sync_folder(dir))
            .map_err(failed(&root))
    }

    pub(crate) fn keep(
        &self,
        draft: KeptDraft,
        images: &[Arc<[u8]>],
        holder: Holder,
    ) -> Result<Slot, KeepSpooledDraftError> {
        self.create_root()?;
        let root = self.root();
        let units = self.units().map_err(|source| KeepSpooledDraftError::List {
            path: root.clone(),
            source,
        })?;
        let slot = match holder {
            Holder::Owner(slot) => slot,
            Holder::Newcomer if Self::current(&units, Slot::Record).is_some() => {
                let number = units
                    .iter()
                    .filter_map(|unit| match unit.slot {
                        Slot::Beside(BesideSlot(number)) => Some(number),
                        Slot::Record => None,
                    })
                    .max()
                    .unwrap_or(0);
                Slot::Beside(BesideSlot(number + 1))
            }
            Holder::Newcomer => Slot::Record,
        };
        let unit = Unit {
            slot,
            generation: Self::current(&units, slot).map_or(1, |unit| unit.generation + 1),
        };
        let staging = tempfile::Builder::new()
            .prefix("staging-")
            .tempdir_in(&root)
            .map_err(|source| KeepSpooledDraftError::CreateDirectory {
                path: root.clone(),
                source,
            })?;
        let digests = images
            .iter()
            .map(|bytes| {
                let digest = ImageDigest::of(bytes);
                write_image(&staging.path().join(digest.file_name()), bytes)?;
                Ok(digest)
            })
            .collect::<Result<Vec<_>, KeepSpooledDraftError>>()?;
        config_file::write(
            &staging.path().join(RECORD),
            &Record {
                host: self.turn.host.clone(),
                scope: self.turn.scope.clone(),
                draft,
                images: digests,
            },
        )
        .map_err(KeepSpooledDraftError::Draft)?;
        sync_folder(staging.path()).map_err(|source| KeepSpooledDraftError::Sync {
            path: staging.path().to_owned(),
            source,
        })?;
        let to = self.unit_path(unit);
        self.disk().rename(staging.path(), &to).map_err(|source| {
            KeepSpooledDraftError::Publish {
                from: staging.path().to_owned(),
                to: to.clone(),
                source,
            }
        })?;
        drop(staging.keep());
        if let Err(source) = sync_folder(&root) {
            tracing::warn!(
                error = %source,
                "kept a draft on this device, but it may not survive a power cut"
            );
        }
        for older in units.iter().filter(|older| older.slot == slot) {
            let path = self.unit_path(*older);
            if let Err(error) = std::fs::remove_dir_all(&path) {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "kept the draft on this device but couldn't remove the version it replaces; removing it on the next save"
                );
            }
        }
        Ok(slot)
    }

    pub(crate) fn take(&self) -> Result<Option<SpooledDraft>, TakeSpooledDraftError> {
        let units = self.units().map_err(|source| TakeSpooledDraftError::List {
            path: self.root(),
            source,
        })?;
        let Some(unit) = Self::current(&units, Slot::Record) else {
            return Ok(None);
        };
        let folder = self.unit_path(unit);
        match read_record(&folder, &self.turn.host, &self.turn.scope) {
            Ok(spooled) => Ok(spooled),
            Err(unreadable) => Err(match self.set_aside(&folder) {
                Ok(set_aside) => TakeSpooledDraftError::SetAside {
                    set_aside,
                    source: Box::new(unreadable),
                },
                Err(source) => TakeSpooledDraftError::Left {
                    path: folder.join(RECORD),
                    unreadable: Box::new(unreadable),
                    source: Box::new(source),
                },
            }),
        }
    }

    pub(crate) fn kept_beside(&self) -> Vec<Result<BesideDraft, KeptBesideError>> {
        let units = match self.units() {
            Ok(units) => units,
            Err(source) => {
                return vec![Err(KeptBesideError::List {
                    path: self.root(),
                    source,
                })];
            }
        };
        let mut slots: Vec<BesideSlot> = units
            .iter()
            .filter_map(|unit| match unit.slot {
                Slot::Beside(slot) => Some(slot),
                Slot::Record => None,
            })
            .collect();
        slots.dedup();
        slots
            .into_iter()
            .filter_map(|slot| {
                let unit = Self::current(&units, Slot::Beside(slot))?;
                let folder = self.unit_path(unit);
                match read_record(&folder, &self.turn.host, &self.turn.scope) {
                    Ok(Some(spooled)) => Some(Ok(BesideDraft { slot, spooled })),
                    Ok(None) => None,
                    Err(unreadable) => Some(Err(match self.set_aside(&folder) {
                        Ok(set_aside) => KeptBesideError::SetAside {
                            set_aside,
                            source: Box::new(unreadable),
                        },
                        Err(source) => KeptBesideError::Left {
                            path: folder.join(RECORD),
                            unreadable: Box::new(unreadable),
                            source: Box::new(source),
                        },
                    })),
                }
            })
            .collect()
    }

    pub(crate) fn forget(&self, slot: Slot) -> Result<(), ForgetSpooledDraftError> {
        let root = self.root();
        let entries = self
            .entries()
            .map_err(|source| ForgetSpooledDraftError::List {
                path: root.clone(),
                source,
            })?;
        let mut units: Vec<Unit> = entries
            .iter()
            .filter_map(|(entry, _)| match entry {
                Entry::Unit(unit) if unit.slot == slot => Some(*unit),
                Entry::Unit(_) | Entry::Tombstone | Entry::Staging | Entry::Other => None,
            })
            .collect();
        if units.is_empty() {
            return Ok(());
        }
        units.sort_by_key(|unit| unit.generation);
        let taken = entries
            .iter()
            .filter_map(|(_, path)| path.file_name()?.to_str().map(str::to_owned))
            .collect::<Vec<_>>();
        let mut tombstones = Vec::new();
        let mut free = (1..=taken.len() + units.len()).map(|number| format!("rejected-{number}"));
        for unit in units {
            let name = free
                .by_ref()
                .find(|name| !taken.contains(name))
                .expect("invariant: some tombstone number is free");
            let from = self.unit_path(unit);
            let tombstone = root.join(name);
            self.disk().rename(&from, &tombstone).map_err(|source| {
                ForgetSpooledDraftError::Tombstone {
                    from,
                    to: tombstone.clone(),
                    source,
                }
            })?;
            tombstones.push(tombstone);
        }
        self.sync_root()?;
        for tombstone in &tombstones {
            self.remove_tree(tombstone)?;
        }
        self.sync_root()?;
        self.remove_root_if_empty();
        Ok(())
    }

    pub(crate) fn sweep_tombstones(&self) -> Result<(), ForgetSpooledDraftError> {
        let entries = self
            .entries()
            .map_err(|source| ForgetSpooledDraftError::List {
                path: self.root(),
                source,
            })?;
        let mut swept = false;
        for (entry, path) in entries {
            match entry {
                Entry::Tombstone | Entry::Staging => {
                    self.remove_tree(&path)?;
                    swept = true;
                }
                Entry::Unit(_) | Entry::Other => {}
            }
        }
        if swept {
            self.sync_root()?;
            self.remove_root_if_empty();
        }
        Ok(())
    }

    fn remove_tree(&self, path: &Path) -> Result<(), ForgetSpooledDraftError> {
        removed(path, self.disk().remove_tree(path))
    }

    fn sync_root(&self) -> Result<(), ForgetSpooledDraftError> {
        let root = self.root();
        sync_folder(&root).map_err(|source| ForgetSpooledDraftError::Sync { path: root, source })
    }

    fn remove_root_if_empty(&self) {
        match std::fs::remove_dir(self.root()) {
            Ok(()) => {
                if let Err(error) = sync_folder(&self.turn.owner.dir) {
                    tracing::warn!(%error, "removed an empty drafts folder, but it may come back after a power cut");
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => tracing::warn!(
                folder = %self.root().display(),
                %error,
                "couldn't remove an empty drafts folder; trying again on the next removal"
            ),
        }
    }

    fn set_aside(&self, unit: &Path) -> Result<PathBuf, SetAsideError> {
        let root = self.root();
        let taken = self
            .entries()
            .map_err(|source| SetAsideError::List {
                path: root.clone(),
                source,
            })?
            .into_iter()
            .filter_map(|(_, path)| path.file_name()?.to_str().map(str::to_owned))
            .collect::<Vec<_>>();
        let folder = (1..=taken.len() + 1)
            .map(|number| format!("unreadable-{number}"))
            .find(|name| !taken.contains(name))
            .map(|name| root.join(name))
            .expect("invariant: some set-aside number is free");
        self.disk()
            .rename(unit, &folder)
            .map_err(|source| SetAsideError::Move {
                from: unit.to_owned(),
                to: folder.clone(),
                source,
            })?;
        if let Err(source) = sync_folder(&root) {
            tracing::warn!(
                error = %source,
                "moved an unreadable draft aside, but the move may not survive a power cut"
            );
        }
        Ok(folder.join(RECORD))
    }

    fn migrate(&self) -> Result<(), MigrateError> {
        let dir = &self.turn.owner.dir;
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(MigrateError::List {
                    path: dir.clone(),
                    source,
                });
            }
        };
        let listed = |source| MigrateError::List {
            path: dir.clone(),
            source,
        };
        let mut legacy = Vec::new();
        for entry in entries {
            let path = entry.map_err(listed)?.path();
            if let Some(found) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| Legacy::of(&self.turn.stem, name))
            {
                legacy.push((found, path));
            }
        }
        if legacy.is_empty() {
            return Ok(());
        }
        let root = self.root();
        std::fs::create_dir_all(&root).map_err(|source| MigrateError::CreateFolder {
            path: root.clone(),
            source,
        })?;
        let units = self.units().map_err(|source| MigrateError::List {
            path: root.clone(),
            source,
        })?;
        let images: Vec<(PathBuf, String)> = legacy
            .iter()
            .filter_map(|(found, path)| match found {
                Legacy::Image(digest) if path.is_file() => Some((path.clone(), digest.file_name())),
                Legacy::Image(_) | Legacy::Record | Legacy::Beside(_) | Legacy::Tombstone => None,
            })
            .collect();
        for (found, path) in &legacy {
            match found {
                Legacy::Record if path.is_file() => {
                    if Self::current(&units, Slot::Record).is_none() {
                        let mut files = images.clone();
                        files.push((path.clone(), RECORD.to_owned()));
                        self.publish_copy(Slot::Record, &files)?;
                    }
                    remove_file(path)?;
                    for (image, _) in &images {
                        remove_file(image)?;
                    }
                }
                Legacy::Beside(slot) if path.is_dir() => {
                    let record = path.join(format!("{}.json", self.turn.stem));
                    if record.is_file() && Self::current(&units, Slot::Beside(*slot)).is_none() {
                        let mut files = Vec::new();
                        for entry in std::fs::read_dir(path).map_err(listed)? {
                            let image = entry.map_err(listed)?.path();
                            if let Some(Legacy::Image(digest)) = image
                                .file_name()
                                .and_then(|name| name.to_str())
                                .and_then(|name| Legacy::of(&self.turn.stem, name))
                            {
                                files.push((image, digest.file_name()));
                            }
                        }
                        files.push((record, RECORD.to_owned()));
                        self.publish_copy(Slot::Beside(*slot), &files)?;
                    }
                    std::fs::remove_dir_all(path).map_err(|source| MigrateError::Remove {
                        path: path.clone(),
                        source,
                    })?;
                }
                Legacy::Tombstone => {
                    if let Err(error) = self.remove_tree(path) {
                        tracing::warn!(
                            error = %::diagnostics::chain(&error),
                            "couldn't remove a draft discarded in the old layout"
                        );
                    }
                }
                Legacy::Record | Legacy::Image(_) | Legacy::Beside(_) => {}
            }
        }
        sync_folder(dir).map_err(|source| MigrateError::Sync {
            path: dir.clone(),
            source,
        })
    }

    fn publish_copy(&self, slot: Slot, files: &[(PathBuf, String)]) -> Result<(), MigrateError> {
        let root = self.root();
        let staging = tempfile::Builder::new()
            .prefix("staging-")
            .tempdir_in(&root)
            .map_err(|source| MigrateError::CreateFolder {
                path: root.clone(),
                source,
            })?;
        for (from, name) in files {
            let to = staging.path().join(name);
            std::fs::hard_link(from, &to)
                .or_else(|_| std::fs::copy(from, &to).map(drop))
                .map_err(|source| MigrateError::Copy {
                    from: from.clone(),
                    to: to.clone(),
                    source,
                })?;
        }
        sync_folder(staging.path()).map_err(|source| MigrateError::Sync {
            path: staging.path().to_owned(),
            source,
        })?;
        let to = self.unit_path(Unit {
            slot,
            generation: 1,
        });
        self.disk()
            .rename(staging.path(), &to)
            .map_err(|source| MigrateError::Publish {
                from: staging.path().to_owned(),
                to,
                source,
            })?;
        drop(staging.keep());
        sync_folder(&root).map_err(|source| MigrateError::Sync { path: root, source })
    }
}

fn read_record(
    folder: &Path,
    host: &HostId,
    scope: &str,
) -> Result<Option<SpooledDraft>, ReadConfigError> {
    let record: Option<Record> = config_file::read(&folder.join(RECORD))?;
    let Some(record) = record.filter(|record| &record.host == host && record.scope == scope) else {
        return Ok(None);
    };
    let images = record
        .images
        .iter()
        .map(|digest| {
            let path = folder.join(digest.file_name());
            let bytes = std::fs::read(&path).map_err(|source| TakeSpooledImageError::Read {
                path: path.clone(),
                source,
            })?;
            if ImageDigest::of(&bytes) == *digest {
                Ok(bytes)
            } else {
                Err(TakeSpooledImageError::Damaged { path })
            }
        })
        .collect();
    Ok(Some(SpooledDraft {
        draft: record.draft,
        images,
    }))
}

fn write_image(path: &Path, bytes: &[u8]) -> Result<(), KeepSpooledDraftError> {
    let failed = |source| KeepSpooledDraftError::WriteImage {
        path: path.to_owned(),
        source,
    };
    let mut file = std::fs::File::create(path).map_err(failed)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(failed)
}

fn removed(path: &Path, removal: std::io::Result<()>) -> Result<(), ForgetSpooledDraftError> {
    match removal {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ForgetSpooledDraftError::Remove {
            path: path.to_owned(),
            source,
        }),
    }
}

fn remove_file(path: &Path) -> Result<(), MigrateError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(MigrateError::Remove {
            path: path.to_owned(),
            source,
        }),
    }
}

#[cfg(unix)]
fn sync_folder(folder: &Path) -> std::io::Result<()> {
    std::fs::File::open(folder).and_then(|folder| folder.sync_all())
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "Windows journals folder changes itself and std can't flush a folder there, so this matches the Unix flush's signature"
)]
fn sync_folder(_folder: &Path) -> std::io::Result<()> {
    Ok(())
}

fn stem(host: &HostId, scope: &str) -> String {
    format!("{}-{}", file_safe(&host.0), file_safe(scope))
}

fn file_safe(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => char::from(byte).to_string(),
            byte => format!("_{byte:02x}"),
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/composer/spool.rs"]
pub(super) mod tests;
