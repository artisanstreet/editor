use super::draft_session::KeptDraft;
use editor_core::{
    HostId,
    config_file::{self, ReadConfigError, WriteConfigError},
};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::{
    collections::HashSet,
    io::Write as _,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftSpool {
    dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BesideSlot(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    #[error("write the pasted image {path}")]
    WriteImage {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write the draft")]
    Draft(#[source] WriteConfigError),
}

#[derive(Debug, thiserror::Error)]
pub enum TakeSpooledDraftError {
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
    #[error("the draft kept beside another at {path} is unreadable and stays there")]
    Unreadable {
        path: PathBuf,
        #[source]
        source: Box<ReadConfigError>,
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
    #[error("create the folder {path} to move the unreadable draft into")]
    CreateFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("list the pasted images in {path}")]
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
    #[error("flush the folder {path} to disk")]
    Sync {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ForgetSpooledDraftError {
    #[error("remove {path}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("list the pasted images in {path}")]
    List {
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

impl DraftSpool {
    #[must_use]
    pub fn new(editor_dir: &Path) -> Self {
        Self {
            dir: editor_dir.join("drafts"),
        }
    }

    pub(crate) fn keep(
        &self,
        host: &HostId,
        scope: &str,
        draft: KeptDraft,
        images: &[std::sync::Arc<[u8]>],
        holder: Holder,
    ) -> Result<Slot, KeepSpooledDraftError> {
        self.create_folder()?;
        let stem = stem(host, scope);
        let slot = match holder {
            Holder::Owner(slot) => slot,
            Holder::Newcomer if occupied(&self.dir.join(format!("{stem}.json"))) => {
                Slot::Beside(self.new_beside_slot(&stem)?)
            }
            Holder::Newcomer => Slot::Record,
        };
        let folder = self.folder(&stem, slot);
        if let Slot::Beside(_) = slot {
            create_beside_folder(&folder)?;
        }
        let record = folder.join(format!("{stem}.json"));
        let kept = self.write_record(&folder, &stem, &record, host, scope, draft, images)?;
        if let Err(error) = remove_images(&folder, &stem, &kept) {
            tracing::warn!(
                host = %host,
                %scope,
                error = %::diagnostics::chain(&error),
                "kept the draft on this device but couldn't remove images it no longer holds"
            );
        }
        if let Slot::Beside(_) = slot
            && let Err(source) = sync_folder(&self.dir)
        {
            tracing::warn!(
                error = %source,
                "kept a draft beside another, but its folder may not survive a power cut"
            );
        }
        Ok(slot)
    }

    fn folder(&self, stem: &str, slot: Slot) -> PathBuf {
        match slot {
            Slot::Record => self.dir.clone(),
            Slot::Beside(BesideSlot(number)) => self.dir.join(format!("{stem}.kept-{number}")),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "a record is one host's scope with its draft and images, written to one folder"
    )]
    fn write_record(
        &self,
        folder: &Path,
        stem: &str,
        record: &Path,
        host: &HostId,
        scope: &str,
        draft: KeptDraft,
        images: &[std::sync::Arc<[u8]>],
    ) -> Result<HashSet<PathBuf>, KeepSpooledDraftError> {
        let digests = images
            .iter()
            .map(|bytes| {
                let digest = ImageDigest::of(bytes);
                self.write_image(&image_path(folder, stem, &digest), bytes)?;
                Ok(digest)
            })
            .collect::<Result<Vec<_>, KeepSpooledDraftError>>()?;
        let kept = digests
            .iter()
            .map(|digest| image_path(folder, stem, digest))
            .collect();
        config_file::write(
            record,
            &Record {
                host: host.clone(),
                scope: scope.to_owned(),
                draft,
                images: digests,
            },
        )
        .map_err(KeepSpooledDraftError::Draft)?;
        Ok(kept)
    }

    fn new_beside_slot(&self, stem: &str) -> Result<BesideSlot, KeepSpooledDraftError> {
        (1_u64..)
            .map(|number| {
                let folder = self.folder(stem, Slot::Beside(BesideSlot(number)));
                match std::fs::create_dir(&folder) {
                    Ok(()) => Ok(Some(BesideSlot(number))),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
                    Err(source) => Err(KeepSpooledDraftError::CreateDirectory {
                        path: folder,
                        source,
                    }),
                }
            })
            .find_map(Result::transpose)
            .expect("invariant: some beside number is free")
    }

    pub fn take(
        &self,
        host: &HostId,
        scope: &str,
    ) -> Result<Option<SpooledDraft>, TakeSpooledDraftError> {
        let stem = stem(host, scope);
        let path = self.dir.join(format!("{stem}.json"));
        match read_record(&self.dir, &stem, &path, host, scope) {
            Ok(spooled) => Ok(spooled),
            Err(unreadable) => Err(match self.set_aside(&stem, &path) {
                Ok(set_aside) => TakeSpooledDraftError::SetAside {
                    set_aside,
                    source: Box::new(unreadable),
                },
                Err(source) => TakeSpooledDraftError::Left {
                    path,
                    unreadable: Box::new(unreadable),
                    source: Box::new(source),
                },
            }),
        }
    }

    pub(crate) fn kept_beside(
        &self,
        host: &HostId,
        scope: &str,
    ) -> Vec<Result<BesideDraft, KeptBesideError>> {
        let stem = stem(host, scope);
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(source) => {
                return vec![Err(KeptBesideError::List {
                    path: self.dir.clone(),
                    source,
                })];
            }
        };
        let prefix = format!("{stem}.kept-");
        let mut slots = Vec::new();
        let mut failures = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => {
                    let number = entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.strip_prefix(&prefix))
                        .and_then(|number| number.parse::<u64>().ok());
                    if let Some(number) = number
                        && entry.path().is_dir()
                    {
                        slots.push(BesideSlot(number));
                    }
                }
                Err(source) => failures.push(Err(KeptBesideError::List {
                    path: self.dir.clone(),
                    source,
                })),
            }
        }
        slots.sort_by_key(|BesideSlot(number)| *number);
        slots
            .into_iter()
            .filter_map(|slot| {
                let folder = self.folder(&stem, Slot::Beside(slot));
                let path = folder.join(format!("{stem}.json"));
                read_record(&folder, &stem, &path, host, scope)
                    .map_err(|source| KeptBesideError::Unreadable {
                        path,
                        source: Box::new(source),
                    })
                    .map(|spooled| spooled.map(|spooled| BesideDraft { slot, spooled }))
                    .transpose()
            })
            .chain(failures)
            .collect()
    }

    pub(crate) fn forget(
        &self,
        host: &HostId,
        scope: &str,
        slot: Slot,
    ) -> Result<(), ForgetSpooledDraftError> {
        let stem = stem(host, scope);
        match slot {
            Slot::Record => {
                remove(&self.dir.join(format!("{stem}.json")))?;
                remove_images(&self.dir, &stem, &HashSet::new())?;
            }
            Slot::Beside(_) => {
                let folder = self.folder(&stem, slot);
                match std::fs::remove_dir_all(&folder) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(ForgetSpooledDraftError::Remove {
                            path: folder,
                            source,
                        });
                    }
                }
            }
        }
        sync_folder(&self.dir).map_err(|source| ForgetSpooledDraftError::Sync {
            path: self.dir.clone(),
            source,
        })
    }

    fn create_folder(&self) -> Result<(), KeepSpooledDraftError> {
        if self.dir.is_dir() {
            return Ok(());
        }
        let failed = |source| KeepSpooledDraftError::CreateDirectory {
            path: self.dir.clone(),
            source,
        };
        std::fs::create_dir_all(&self.dir).map_err(failed)?;
        self.dir
            .parent()
            .map_or(Ok(()), sync_folder)
            .map_err(failed)
    }

    fn set_aside(&self, stem: &str, record: &Path) -> Result<PathBuf, SetAsideError> {
        let folder = (1_u64..)
            .map(|number| {
                let folder = self.dir.join(format!("{stem}.unreadable-{number}"));
                match std::fs::create_dir(&folder) {
                    Ok(()) => Ok(Some(folder)),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
                    Err(source) => Err(SetAsideError::CreateFolder {
                        path: folder,
                        source,
                    }),
                }
            })
            .find_map(Result::transpose)
            .expect("invariant: some set-aside number is free")?;
        let listed = |source| SetAsideError::List {
            path: self.dir.clone(),
            source,
        };
        for entry in std::fs::read_dir(&self.dir).map_err(listed)? {
            let path = entry.map_err(listed)?.path();
            if let Some(name) = path.file_name().filter(|_| is_image_of(&path, stem)) {
                move_into(&path, &folder.join(name))?;
            }
        }
        let set_aside = folder.join(
            record
                .file_name()
                .expect("invariant: a kept draft's record path names a file"),
        );
        sync_folder(&folder).map_err(|source| SetAsideError::Sync {
            path: folder.clone(),
            source,
        })?;
        move_into(record, &set_aside)?;
        for synced in [&folder, &self.dir] {
            if let Err(source) = sync_folder(synced) {
                let error = SetAsideError::Sync {
                    path: synced.clone(),
                    source,
                };
                tracing::warn!(
                    error = %::diagnostics::chain(&error),
                    "moved an unreadable draft aside, but the move may not survive a power cut"
                );
            }
        }
        Ok(set_aside)
    }

    fn write_image(&self, path: &Path, bytes: &[u8]) -> Result<(), KeepSpooledDraftError> {
        let failed = |source| KeepSpooledDraftError::WriteImage {
            path: path.to_owned(),
            source,
        };
        let mut temporary = tempfile::NamedTempFile::new_in(&self.dir).map_err(failed)?;
        temporary
            .write_all(bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(failed)?;
        temporary
            .persist(path)
            .map(drop)
            .map_err(|error| failed(error.error))
    }
}

fn read_record(
    folder: &Path,
    stem: &str,
    path: &Path,
    host: &HostId,
    scope: &str,
) -> Result<Option<SpooledDraft>, ReadConfigError> {
    let record: Option<Record> = config_file::read(path)?;
    let Some(record) = record.filter(|record| &record.host == host && record.scope == scope) else {
        return Ok(None);
    };
    let images = record
        .images
        .iter()
        .map(|digest| {
            let path = image_path(folder, stem, digest);
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

fn create_beside_folder(folder: &Path) -> Result<(), KeepSpooledDraftError> {
    match std::fs::create_dir(folder) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(source) => Err(KeepSpooledDraftError::CreateDirectory {
            path: folder.to_owned(),
            source,
        }),
    }
}

fn remove_images(
    folder: &Path,
    stem: &str,
    kept: &HashSet<PathBuf>,
) -> Result<(), ForgetSpooledDraftError> {
    let listed = |source| ForgetSpooledDraftError::List {
        path: folder.to_owned(),
        source,
    };
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(listed(source)),
    };
    for entry in entries {
        let path = entry.map_err(listed)?.path();
        if is_image_of(&path, stem) && !kept.contains(&path) {
            remove(&path)?;
        }
    }
    Ok(())
}

fn image_path(folder: &Path, stem: &str, digest: &ImageDigest) -> PathBuf {
    folder.join(format!("{stem}.{}.image", digest.0))
}

fn occupied(record: &Path) -> bool {
    std::fs::symlink_metadata(record).map_or_else(
        |error| error.kind() != std::io::ErrorKind::NotFound,
        |_| true,
    )
}

fn is_image_of(path: &Path, stem: &str) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "image")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(stem))
            .is_some_and(|rest| rest.starts_with('.'))
}

fn move_into(from: &Path, to: &Path) -> Result<(), SetAsideError> {
    std::fs::rename(from, to).map_err(|source| SetAsideError::Move {
        from: from.to_owned(),
        to: to.to_owned(),
        source,
    })
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

fn remove(path: &Path) -> Result<(), ForgetSpooledDraftError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ForgetSpooledDraftError::Remove {
            path: path.to_owned(),
            source,
        }),
    }
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
mod tests;
