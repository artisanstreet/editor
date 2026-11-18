use artisan_api::{ItemId, ThreadId};
use fs2::FileExt as _;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    pub control: String,
    pub token: String,
    pub address: std::net::SocketAddr,
    pub pid: u32,
    pub protocol: u32,
    pub schema: usize,
}

#[derive(Clone, Debug)]
pub struct Registry {
    root: PathBuf,
}

#[derive(Debug)]
struct Held(File);

impl Drop for Held {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            tracing::warn!("release a runtime registry lock: {error}");
        }
    }
}

#[derive(Debug)]
pub struct UpgradeLock {
    _held: Held,
}

#[derive(Debug)]
struct Registration {
    held: Option<Held>,
    metadata: PathBuf,
    lock: PathBuf,
}

impl Drop for Registration {
    fn drop(&mut self) {
        for path in [&self.metadata, &self.lock] {
            if let Err(error) = std::fs::remove_file(path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(path = %path.display(), "remove a generation's registry file: {error}");
            }
        }
        drop(self.held.take());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Standby,
    Active,
    Draining,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holds {
    Reported(BTreeSet<ThreadId>),
    Unreported(BTreeSet<ThreadId>),
}

impl Holds {
    const fn threads(&self) -> &BTreeSet<ThreadId> {
        match self {
            Self::Reported(threads) | Self::Unreported(threads) => threads,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Peer {
    pub(crate) member: Member,
    pub(crate) holds: Holds,
}

#[derive(Clone, Debug)]
pub struct Overlap {
    pub phase: Phase,
    pub peers: HashMap<String, Peer>,
}

impl Overlap {
    #[must_use]
    pub fn overlapping(&self) -> bool {
        self.phase == Phase::Draining || !self.peers.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct Generation {
    pub registry: Registry,
    pub member: Member,
    _registration: Arc<Registration>,
    overlap: Arc<watch::Sender<Overlap>>,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryFileError {
    #[error("read the registry file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse the registry file {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("write the registry file {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("encode the registry file {path}")]
    Encode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("lock the registry file {path}")]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("create the registry folder {path}")]
    CreateFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("restrict the registry folder {path} to this user")]
    Restrict {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("list the registry folder {path}")]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove the retired registry entry {path}")]
    Retire {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("generation {generation} is already registered")]
    AlreadyRegistered { generation: String },
    #[error("list the generations in {path} off the async workers")]
    Task {
        path: PathBuf,
        #[source]
        source: tokio::task::JoinError,
    },
}

pub fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, RegistryFileError> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|source| RegistryFileError::Parse {
                    path: path.to_owned(),
                    source,
                })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(RegistryFileError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

pub fn write(path: &Path, value: &impl Serialize) -> Result<(), RegistryFileError> {
    let failed = |source| RegistryFileError::Write {
        path: path.to_owned(),
        source,
    };
    let Some(parent) = path.parent() else {
        unreachable!(
            "invariant: registry file {} lives inside a folder",
            path.display()
        );
    };
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(failed)?;
    serde_json::to_writer(file.as_file_mut(), value).map_err(|source| {
        RegistryFileError::Encode {
            path: path.to_owned(),
            source,
        }
    })?;
    file.as_file().sync_all().map_err(failed)?;
    file.persist(path).map_err(|error| failed(error.error))?;
    Ok(())
}

fn contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

fn try_lock(file: File, path: &Path) -> Result<Option<Held>, RegistryFileError> {
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(Held(file))),
        Err(error) if contended(&error) => Ok(None),
        Err(source) => Err(RegistryFileError::Lock {
            path: path.to_owned(),
            source,
        }),
    }
}

fn create_and_lock(path: &Path) -> Result<Option<Held>, RegistryFileError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|source| RegistryFileError::Lock {
            path: path.to_owned(),
            source,
        })?;
    try_lock(file, path)
}

enum Liveness {
    Alive,
    Gone(Option<Held>),
}

fn liveness(lock: &Path) -> Result<Liveness, RegistryFileError> {
    let file = match OpenOptions::new().read(true).write(true).open(lock) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Liveness::Gone(None));
        }
        Err(source) => {
            return Err(RegistryFileError::Lock {
                path: lock.to_owned(),
                source,
            });
        }
    };
    Ok(match try_lock(file, lock)? {
        Some(held) => Liveness::Gone(Some(held)),
        None => Liveness::Alive,
    })
}

fn create_folder(path: &Path) -> Result<(), RegistryFileError> {
    std::fs::create_dir_all(path).map_err(|source| RegistryFileError::CreateFolder {
        path: path.to_owned(),
        source,
    })
}

fn retire(path: &Path) -> Result<(), RegistryFileError> {
    let removed = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match removed {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RegistryFileError::Retire {
            path: path.to_owned(),
            source,
        }),
    }
}

const RETIRED: [&str; 4] = ["threads", "slots", "admission.lock", "services.lock"];

impl Registry {
    pub fn open(data: &Path) -> Result<Self, RegistryFileError> {
        let root = data.join("runtime");
        create_folder(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).map_err(
                |source| RegistryFileError::Restrict {
                    path: root.clone(),
                    source,
                },
            )?;
        }
        for folder in ["generations", "binaries"] {
            create_folder(&root.join(folder))?;
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn active(&self) -> Result<Option<Member>, RegistryFileError> {
        read(&self.root.join("active.json"))
    }

    pub fn promote(&self, member: &Member) -> Result<(), RegistryFileError> {
        write(&self.root.join("active.json"), member)
    }

    pub fn retire_unused(&self) -> Result<(), RegistryFileError> {
        for name in RETIRED {
            retire(&self.root.join(name))?;
        }
        self.peers().map(drop)
    }

    pub fn peers(&self) -> Result<Vec<Member>, RegistryFileError> {
        let folder = self.root.join("generations");
        let listed = |source| RegistryFileError::List {
            path: folder.clone(),
            source,
        };
        let mut members = Vec::new();
        for entry in std::fs::read_dir(&folder).map_err(listed)? {
            let path = entry.map_err(listed)?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let lock = path.with_extension("lock");
            match liveness(&lock)? {
                Liveness::Alive => {
                    if let Some(member) = read(&path)? {
                        members.push(member);
                    }
                }
                Liveness::Gone(held) => {
                    retire(&path)?;
                    retire(&lock)?;
                    drop(held);
                }
            }
        }
        Ok(members)
    }

    pub async fn list_peers(&self) -> Result<Vec<Member>, RegistryFileError> {
        let registry = self.clone();
        tokio::task::spawn_blocking(move || registry.peers())
            .await
            .map_err(|source| RegistryFileError::Task {
                path: self.root.join("generations"),
                source,
            })?
    }

    pub fn register(&self, mut member: Member) -> Result<Generation, RegistryFileError> {
        member.id = ItemId::new().to_string();
        let path = self.root.join("generations").join(&member.id);
        let lock = path.with_extension("lock");
        let metadata = path.with_extension("json");
        let held = create_and_lock(&lock)?.ok_or_else(|| RegistryFileError::AlreadyRegistered {
            generation: member.id.clone(),
        })?;
        let registration = Registration {
            held: Some(held),
            metadata,
            lock,
        };
        write(&registration.metadata, &member)?;
        Ok(Generation {
            registry: self.clone(),
            member,
            _registration: Arc::new(registration),
            overlap: Arc::new(watch::Sender::new(Overlap {
                phase: Phase::Standby,
                peers: HashMap::new(),
            })),
        })
    }

    pub fn upgrade_lock(&self) -> Result<Option<UpgradeLock>, RegistryFileError> {
        Ok(create_and_lock(&self.root.join("upgrade.lock"))?
            .map(|held| UpgradeLock { _held: held }))
    }
}

impl Generation {
    #[must_use]
    fn phase(&self) -> Phase {
        self.overlap.borrow().phase
    }

    #[must_use]
    pub fn accepting(&self) -> bool {
        self.phase() == Phase::Active
    }

    #[must_use]
    pub fn draining(&self) -> bool {
        self.phase() == Phase::Draining
    }

    pub fn set_phase(&self, phase: Phase) {
        self.overlap.send_if_modified(|overlap| {
            let changed = overlap.phase != phase;
            overlap.phase = phase;
            changed
        });
    }

    #[must_use]
    pub fn watch(&self) -> watch::Receiver<Overlap> {
        self.overlap.subscribe()
    }

    #[must_use]
    pub fn overlapping(&self) -> bool {
        self.overlap.borrow().overlapping()
    }

    #[must_use]
    pub(crate) fn owner(&self, thread: &ThreadId) -> Option<Member> {
        self.overlap
            .borrow()
            .peers
            .values()
            .find(|peer| peer.holds.threads().contains(thread))
            .map(|peer| peer.member.clone())
    }

    #[must_use]
    pub(crate) fn foreign_threads(&self) -> BTreeSet<ThreadId> {
        self.overlap
            .borrow()
            .peers
            .values()
            .flat_map(|peer| peer.holds.threads().iter().cloned())
            .collect()
    }

    #[must_use]
    pub(crate) fn peers(&self) -> Vec<Peer> {
        self.overlap.borrow().peers.values().cloned().collect()
    }

    pub(crate) fn track(&self, peers: Vec<Peer>) {
        self.overlap.send_modify(|overlap| {
            overlap.peers = peers
                .into_iter()
                .map(|peer| (peer.member.id.clone(), peer))
                .collect();
        });
    }

    pub(crate) fn reported(&self, generation: &str, threads: BTreeSet<ThreadId>) {
        self.overlap.send_if_modified(|overlap| {
            let Some(peer) = overlap.peers.get_mut(generation) else {
                return false;
            };
            let holds = Holds::Reported(threads);
            let changed = peer.holds != holds;
            peer.holds = holds;
            changed
        });
    }

    pub(crate) fn forget(&self, generation: &str) -> BTreeSet<ThreadId> {
        let mut released = BTreeSet::new();
        self.overlap.send_if_modified(|overlap| {
            overlap
                .peers
                .remove(generation)
                .map(|peer| {
                    released = match peer.holds {
                        Holds::Reported(threads) | Holds::Unreported(threads) => threads,
                    };
                })
                .is_some()
        });
        released
    }
}
