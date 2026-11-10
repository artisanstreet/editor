use anyhow::{Context, Result, ensure};
use artisan_api::{ItemId, ThreadId};
use fs2::FileExt as _;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

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

pub struct Lease {
    _file: File,
    metadata: Option<PathBuf>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(path) = &self.metadata {
            std::fs::remove_file(path).ok();
        }
    }
}

#[derive(Clone)]
pub struct Generation {
    pub registry: Registry,
    pub member: Member,
    _lease: Arc<Lease>,
    accepting: Arc<AtomicBool>,
    draining: Arc<AtomicBool>,
}

fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("registry path has no parent")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(file.as_file_mut(), value)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn lock(path: &Path) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.into()),
    }
}

impl Registry {
    pub fn open(data: &Path) -> Result<Self> {
        let root = data.join("runtime");
        std::fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        }
        for folder in ["generations", "threads", "slots", "binaries"] {
            std::fs::create_dir_all(root.join(folder))?;
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn active(&self) -> Result<Option<Member>> {
        read(&self.root.join("active.json"))
    }

    pub fn promote(&self, member: &Member) -> Result<()> {
        let gate = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("admission.lock"))?;
        gate.lock_exclusive()?;
        write(&self.root.join("active.json"), member)
    }

    pub fn peers(&self) -> Result<Vec<Member>> {
        let mut members = Vec::new();
        for entry in std::fs::read_dir(self.root.join("generations"))? {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            if lock(&path.with_extension("lock"))?.is_some() {
                continue;
            }
            if let Some(member) = read(&path)? {
                members.push(member);
            }
        }
        Ok(members)
    }

    pub fn owner(&self, thread: &ThreadId) -> Result<Option<Member>> {
        ensure!(
            !thread.as_str().contains(['/', '\\']),
            "invalid thread identity"
        );
        let path = self.root.join("threads").join(hex::encode(thread.as_str()));
        if lock(&path.with_extension("lock"))?.is_some() {
            return Ok(None);
        }
        read(&path.with_extension("json"))
    }

    pub fn owners(&self) -> Result<Vec<(ThreadId, Member)>> {
        let mut owners = Vec::new();
        for entry in std::fs::read_dir(self.root.join("threads"))? {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let encoded = path
                .file_stem()
                .context("thread path has no id")?
                .to_str()
                .context("invalid thread filename")?;
            let id = ThreadId::from(String::from_utf8(hex::decode(encoded)?)?);
            if let Some(member) = self.owner(&id)? {
                owners.push((id, member));
            }
        }
        Ok(owners)
    }

    pub fn register(&self, mut member: Member) -> Result<Generation> {
        ensure!(
            member.id.is_empty(),
            "generation ids are assigned by the registry"
        );
        member.id = ItemId::new().to_string();
        let path = self.root.join("generations").join(&member.id);
        let file = lock(&path.with_extension("lock"))?.context("generation already exists")?;
        write(&path.with_extension("json"), &member)?;
        Ok(Generation {
            registry: self.clone(),
            member,
            _lease: Arc::new(Lease {
                _file: file,
                metadata: Some(path.with_extension("json")),
            }),
            accepting: Arc::new(AtomicBool::new(false)),
            draining: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn services_lock(&self) -> Result<Option<Lease>> {
        Ok(lock(&self.root.join("services.lock"))?.map(|file| Lease {
            _file: file,
            metadata: None,
        }))
    }

    pub fn upgrade_lock(&self) -> Result<Option<Lease>> {
        Ok(lock(&self.root.join("upgrade.lock"))?.map(|file| Lease {
            _file: file,
            metadata: None,
        }))
    }
}

impl Generation {
    pub fn set_accepting(&self, accepting: bool) {
        self.draining.store(!accepting, Ordering::Release);
        self.accepting.store(accepting, Ordering::Release);
    }

    pub fn draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    pub fn accepting(&self) -> bool {
        self.accepting.load(Ordering::Acquire)
            && self
                .registry
                .active()
                .ok()
                .flatten()
                .is_some_and(|member| member.id == self.member.id)
    }

    pub fn claim(&self, thread: &ThreadId) -> Result<Option<Lease>> {
        let gate = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.registry.root.join("admission.lock"))?;
        match fs2::FileExt::try_lock_shared(&gate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        if !self.accepting() {
            return Ok(None);
        }
        ensure!(
            !thread.as_str().contains(['/', '\\']),
            "invalid thread identity"
        );
        let path = self
            .registry
            .root
            .join("threads")
            .join(hex::encode(thread.as_str()));
        let Some(file) = lock(&path.with_extension("lock"))? else {
            return Ok(None);
        };
        if !self.accepting() {
            return Ok(None);
        }
        write(&path.with_extension("json"), &self.member)?;
        Ok(Some(Lease {
            _file: file,
            metadata: Some(path.with_extension("json")),
        }))
    }

    pub fn slot(&self) -> Result<Option<Lease>> {
        for slot in 0..8 {
            if let Some(file) = lock(&self.registry.root.join("slots").join(slot.to_string()))? {
                return Ok(Some(Lease {
                    _file: file,
                    metadata: None,
                }));
            }
        }
        Ok(None)
    }
}
