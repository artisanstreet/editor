use crate::sys::{self, RemoveError};
use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

pub(super) const ASIDE_SLOTS: usize = 100;
const JOURNAL: &str = ".install-journal";
const JOURNAL_DRAFT: &str = ".install-journal.draft";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
struct Name(String);

#[derive(Debug, thiserror::Error)]
#[error("{0:?} isn't the name of a file in the install folder")]
struct NotAName(String);

impl TryFrom<String> for Name {
    type Error = NotAName;

    fn try_from(name: String) -> Result<Self, NotAName> {
        let mut components = Path::new(&name).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(only)), None) if only == OsStr::new(&name) => Ok(Self(name)),
            _ => Err(NotAName(name)),
        }
    }
}

impl From<Name> for String {
    fn from(name: Name) -> Self {
        name.0
    }
}

impl Name {
    fn aside(&self, slot: usize) -> Self {
        Self(format!(".old-{slot}-{}", self.0))
    }

    fn of(&self, dir: &Path) -> PathBuf {
        dir.join(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Entry {
    name: Name,
    aside: Option<Name>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Journal {
    entries: Vec<Entry>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FreeAsideError {
    #[error("look at {}", path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("no free name to move {} aside among {ASIDE_SLOTS}", path.display())]
    Full { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReplaceError {
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} is not a Unicode file name", path.display())]
    NotUnicode { path: PathBuf },
    #[error(transparent)]
    NoAside(#[from] FreeAsideError),
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("move {} aside", path.display())]
    Aside {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("put {} in place", path.display())]
    Place {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{failed}; restoring the previous files also failed: {}", diagnostics::chain(.restore))]
    Restore {
        #[source]
        failed: Box<Self>,
        restore: Box<RestoreFailures>,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum JournalError {
    #[error("record the files being replaced in {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("read the record of an interrupted install in {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} doesn't read as the record of an interrupted install", path.display())]
    Unreadable {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("remove the record of a finished install {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: RemoveError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RestoreError {
    #[error("remove {} to put the previous version back", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: RemoveError,
    },
    #[error(transparent)]
    NoAside(#[from] FreeAsideError),
    #[error("move {} out of the way of the previous version", path.display())]
    Displace {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("put {} back as {}", aside.display(), target.display())]
    PutBack {
        aside: PathBuf,
        target: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Journal(#[from] JournalError),
}

#[derive(Debug)]
pub(crate) struct RestoreFailures {
    first: RestoreError,
    rest: Vec<RestoreError>,
}

impl RestoreFailures {
    fn of(mut failures: Vec<RestoreError>) -> Option<Self> {
        if failures.is_empty() {
            return None;
        }
        let first = failures.remove(0);
        Some(Self {
            first,
            rest: failures,
        })
    }
}

impl std::fmt::Display for RestoreFailures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the previous files couldn't all be put back")?;
        for also in &self.rest {
            write!(f, "; also {}", diagnostics::chain(also))?;
        }
        Ok(())
    }
}

impl std::error::Error for RestoreFailures {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.first)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RecoverError {
    #[error("look for an interrupted install in {}", path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("put back the files an interrupted install left in {}", dir.display())]
    Restore {
        dir: PathBuf,
        #[source]
        source: Box<RestoreFailures>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Recovered {
    Nothing,
    PutBack,
}

#[derive(Debug)]
#[must_use]
pub(super) struct Replaced {
    dir: PathBuf,
    journal: Journal,
}

impl Replaced {
    pub(super) fn keep(self) -> Result<(), JournalError> {
        forget(&self.dir)?;
        for entry in self.journal.entries {
            if let Some(aside) = entry.aside {
                discard(&aside.of(&self.dir));
            }
        }
        Ok(())
    }

    pub(super) fn undo(self) -> Result<(), RestoreFailures> {
        match RestoreFailures::of(restore(&self.dir, &self.journal)) {
            Some(failures) => Err(failures),
            None => Ok(()),
        }
    }
}

pub(super) fn replace(dir: &Path, incoming: &Path) -> Result<Replaced, ReplaceError> {
    replace_with(dir, incoming, |from, to| std::fs::rename(from, to))
}

pub(super) fn replace_with(
    dir: &Path,
    incoming: &Path,
    rename: impl Fn(&Path, &Path) -> io::Result<()>,
) -> Result<Replaced, ReplaceError> {
    let read = |source| ReplaceError::Read {
        path: incoming.to_owned(),
        source,
    };
    let mut names = Vec::new();
    for entry in std::fs::read_dir(incoming).map_err(read)? {
        let path = entry.map_err(read)?.path();
        let name = path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| ReplaceError::NotUnicode { path: path.clone() })?
            .to_owned();
        names.push(Name(name));
    }
    names.sort_by(|one, other| one.0.cmp(&other.0));
    let mut replaced = Replaced {
        dir: dir.to_owned(),
        journal: Journal::default(),
    };
    for name in &names {
        if let Err(error) = swap_in(&mut replaced, &incoming.join(&name.0), name, &rename) {
            return Err(match replaced.undo() {
                Ok(()) => error,
                Err(restore) => ReplaceError::Restore {
                    failed: Box::new(error),
                    restore: Box::new(restore),
                },
            });
        }
    }
    Ok(replaced)
}

pub(super) fn journal(dir: &Path) -> PathBuf {
    dir.join(JOURNAL)
}

pub(super) fn leftover(name: &OsStr) -> bool {
    name == JOURNAL_DRAFT || name.to_string_lossy().starts_with(".old-")
}

pub(super) fn recover(dir: &Path) -> Result<Recovered, RecoverError> {
    let path = journal(dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Recovered::Nothing),
        Err(source) => return Err(JournalError::Read { path, source }.into()),
    };
    let journal: Journal =
        serde_json::from_str(&text).map_err(|source| JournalError::Unreadable { path, source })?;
    discard(&dir.join(JOURNAL_DRAFT));
    match RestoreFailures::of(restore(dir, &journal)) {
        Some(source) => Err(RecoverError::Restore {
            dir: dir.to_owned(),
            source: Box::new(source),
        }),
        None => Ok(Recovered::PutBack),
    }
}

pub(super) fn interrupted(dir: &Path) -> Result<bool, RecoverError> {
    let path = dir.join(JOURNAL);
    path.try_exists()
        .map_err(|source| RecoverError::Inspect { path, source })
}

fn occupied(path: &Path) -> Result<bool, FreeAsideError> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(FreeAsideError::Inspect {
            path: path.to_owned(),
            source,
        }),
    }
}

fn free_aside(dir: &Path, name: &Name) -> Result<Name, FreeAsideError> {
    for slot in 0..ASIDE_SLOTS {
        let aside = name.aside(slot);
        if !occupied(&aside.of(dir))? {
            return Ok(aside);
        }
    }
    Err(FreeAsideError::Full { path: name.of(dir) })
}

fn swap_in(
    replaced: &mut Replaced,
    new: &Path,
    name: &Name,
    rename: &impl Fn(&Path, &Path) -> io::Result<()>,
) -> Result<(), ReplaceError> {
    let dir = replaced.dir.clone();
    let target = name.of(&dir);
    let aside = if occupied(&target)? {
        Some(free_aside(&dir, name)?)
    } else {
        None
    };
    replaced.journal.entries.push(Entry {
        name: name.clone(),
        aside: aside.clone(),
    });
    record(&dir, &replaced.journal)?;
    if let Some(aside) = &aside {
        set_aside(&target, &aside.of(&dir), rename).map_err(|source| ReplaceError::Aside {
            path: target.clone(),
            source,
        })?;
    }
    rename(new, &target).map_err(|source| ReplaceError::Place {
        path: target.clone(),
        source,
    })
}

fn set_aside(
    target: &Path,
    aside: &Path,
    rename: &impl Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if cfg!(unix) {
        match std::fs::hard_link(target, aside) {
            Ok(()) => return Ok(()),
            Err(error) => tracing::debug!(
                target = %target.display(),
                %error,
                "this folder doesn't take a second link to the file, so it is moved aside instead"
            ),
        }
    }
    rename(target, aside)
}

fn restore(dir: &Path, journal: &Journal) -> Vec<RestoreError> {
    let mut failures = Vec::new();
    for entry in journal.entries.iter().rev() {
        let restored = match &entry.aside {
            Some(aside) => put_back(dir, &entry.name, aside),
            None => clear(dir, &entry.name),
        };
        if let Err(failure) = restored {
            failures.push(failure);
        }
    }
    if failures.is_empty()
        && let Err(failure) = forget(dir)
    {
        failures.push(failure.into());
    }
    failures
}

fn clear(dir: &Path, name: &Name) -> Result<(), RestoreError> {
    let target = name.of(dir);
    if cfg!(windows) {
        return displace(dir, name);
    }
    sys::remove(&target).map_err(|source| RestoreError::Remove {
        path: target,
        source,
    })
}

fn displace(dir: &Path, name: &Name) -> Result<(), RestoreError> {
    let target = name.of(dir);
    if !occupied(&target)? {
        return Ok(());
    }
    let out_of_the_way = free_aside(dir, name)?.of(dir);
    std::fs::rename(&target, &out_of_the_way).map_err(|source| RestoreError::Displace {
        path: target,
        source,
    })?;
    discard(&out_of_the_way);
    Ok(())
}

fn put_back(dir: &Path, name: &Name, aside: &Name) -> Result<(), RestoreError> {
    let target = name.of(dir);
    let aside = aside.of(dir);
    let inspect = |source| RestoreError::PutBack {
        aside: aside.clone(),
        target: target.clone(),
        source,
    };
    let kept = match aside.symlink_metadata() {
        Ok(kept) => kept,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(inspect(source)),
    };
    let current = match target.symlink_metadata() {
        Ok(current) => Some(current),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(source) => return Err(inspect(source)),
    };
    if current
        .as_ref()
        .is_some_and(|current| same_file(&kept, current))
    {
        return sys::remove(&aside).map_err(|source| RestoreError::Remove {
            path: aside.clone(),
            source,
        });
    }
    if cfg!(windows) && current.is_some() {
        displace(dir, name)?;
    }
    std::fs::rename(&aside, &target).map_err(inspect)
}

#[cfg(unix)]
fn same_file(one: &std::fs::Metadata, other: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    (one.dev(), one.ino()) == (other.dev(), other.ino())
}

#[cfg(not(unix))]
const fn same_file(_one: &std::fs::Metadata, _other: &std::fs::Metadata) -> bool {
    false
}

fn record(dir: &Path, journal: &Journal) -> Result<(), JournalError> {
    let path = dir.join(JOURNAL);
    let draft = dir.join(JOURNAL_DRAFT);
    let failed = |source| JournalError::Write {
        path: path.clone(),
        source,
    };
    let text = serde_json::to_vec(journal).expect("invariant: the install journal encodes as JSON");
    let mut file = std::fs::File::create(&draft).map_err(failed)?;
    io::Write::write_all(&mut file, &text).map_err(failed)?;
    file.sync_all().map_err(failed)?;
    drop(file);
    std::fs::rename(&draft, &path).map_err(failed)?;
    sync_dir(dir).map_err(failed)
}

fn forget(dir: &Path) -> Result<(), JournalError> {
    let path = dir.join(JOURNAL);
    sys::remove(&path).map_err(|source| JournalError::Remove {
        path: path.clone(),
        source,
    })?;
    sync_dir(dir).map_err(|source| JournalError::Write { path, source })
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    if cfg!(unix) {
        std::fs::File::open(dir)?.sync_all()
    } else {
        Ok(())
    }
}

pub(super) fn discard(path: &Path) {
    if let Err(error) = sys::remove(path) {
        tracing::debug!(
            path = %path.display(),
            "the replaced file stays until the next install: {}",
            diagnostics::chain(&error)
        );
    }
}

#[cfg(test)]
#[path = "../../tests/install/swap.rs"]
mod tests;
