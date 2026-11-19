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

#[derive(Clone, Debug)]
pub struct DraftSpool {
    dir: PathBuf,
}

#[derive(Debug, PartialEq)]
pub struct SpooledDraft {
    pub(crate) draft: KeptDraft,
    pub(crate) images: Vec<Vec<u8>>,
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
    #[error("read the draft")]
    Draft(#[source] ReadConfigError),
    #[error("read the pasted image {path}")]
    ReadImage {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the pasted image {path} does not match its digest")]
    ImageDamaged { path: PathBuf },
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
    ) -> Result<(), KeepSpooledDraftError> {
        std::fs::create_dir_all(&self.dir).map_err(|source| {
            KeepSpooledDraftError::CreateDirectory {
                path: self.dir.clone(),
                source,
            }
        })?;
        let stem = stem(host, scope);
        let digests = images
            .iter()
            .map(|bytes| {
                let digest = ImageDigest::of(bytes);
                self.write_image(&self.image_path(&stem, &digest), bytes)?;
                Ok(digest)
            })
            .collect::<Result<Vec<_>, KeepSpooledDraftError>>()?;
        let kept: HashSet<PathBuf> = digests
            .iter()
            .map(|digest| self.image_path(&stem, digest))
            .collect();
        config_file::write(
            &self.dir.join(format!("{stem}.json")),
            &Record {
                host: host.clone(),
                scope: scope.to_owned(),
                draft,
                images: digests,
            },
        )
        .map_err(KeepSpooledDraftError::Draft)?;
        if let Err(error) = self.remove_images(&stem, &kept) {
            tracing::warn!(
                host = %host,
                %scope,
                error = %::diagnostics::chain(&error),
                "kept the draft on this device but couldn't remove images it no longer holds"
            );
        }
        Ok(())
    }

    pub fn take(
        &self,
        host: &HostId,
        scope: &str,
    ) -> Result<Option<SpooledDraft>, TakeSpooledDraftError> {
        let stem = stem(host, scope);
        let record: Option<Record> = config_file::read(&self.dir.join(format!("{stem}.json")))
            .map_err(TakeSpooledDraftError::Draft)?;
        let Some(record) = record.filter(|record| &record.host == host && record.scope == scope)
        else {
            return Ok(None);
        };
        let images = record
            .images
            .iter()
            .map(|digest| {
                let path = self.image_path(&stem, digest);
                let bytes =
                    std::fs::read(&path).map_err(|source| TakeSpooledDraftError::ReadImage {
                        path: path.clone(),
                        source,
                    })?;
                if ImageDigest::of(&bytes) == *digest {
                    Ok(bytes)
                } else {
                    Err(TakeSpooledDraftError::ImageDamaged { path })
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(SpooledDraft {
            draft: record.draft,
            images,
        }))
    }

    pub(crate) fn forget(&self, host: &HostId, scope: &str) -> Result<(), ForgetSpooledDraftError> {
        let stem = stem(host, scope);
        remove(&self.dir.join(format!("{stem}.json")))?;
        self.remove_images(&stem, &HashSet::new())
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

    fn remove_images(
        &self,
        stem: &str,
        kept: &HashSet<PathBuf>,
    ) -> Result<(), ForgetSpooledDraftError> {
        let listed = |source| ForgetSpooledDraftError::List {
            path: self.dir.clone(),
            source,
        };
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(listed(source)),
        };
        let prefix = format!("{stem}.");
        for entry in entries {
            let path = entry.map_err(listed)?.path();
            let ours = path
                .extension()
                .is_some_and(|extension| extension == "image")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix));
            if ours && !kept.contains(&path) {
                remove(&path)?;
            }
        }
        Ok(())
    }

    fn image_path(&self, stem: &str, digest: &ImageDigest) -> PathBuf {
        self.dir.join(format!("{stem}.{}.image", digest.0))
    }
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
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' => char::from(byte).to_string(),
            byte => format!("_{byte:02x}"),
        })
        .collect()
}
