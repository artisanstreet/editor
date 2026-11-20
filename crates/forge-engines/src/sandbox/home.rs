use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf};

const TEMPORARY_PREFIX: &str = ".artisan-guard-";
const PLANTED_PREFIX: &str = ".artisan-planted-";
const RANDOM_BYTES: usize = 16;
pub(super) const MAX_DEPTH: usize = 128;
const FOLDER_MODE: u32 = 0o700;
const PERMISSION_BITS: u32 = 0o7777;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Found {
    Missing,
    Folder,
    Link(PathBuf),
    File { length: u64, mode: u32 },
    Other,
}

fn random_name(prefix: &str) -> io::Result<OsString> {
    let mut bytes = [0_u8; RANDOM_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(OsString::from(format!("{prefix}{}", hex::encode(bytes))))
}

fn ignore_missing(result: rustix::io::Result<()>) -> io::Result<()> {
    match result {
        Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn widened<Raw: Into<u32>>(raw: Raw) -> u32 {
    raw.into()
}

fn narrowed<Raw: TryFrom<u32>>(bits: u32) -> Raw {
    Raw::try_from(bits).unwrap_or_else(|_| {
        unreachable!("invariant: permission bits {bits:o} fit this system's mode_t")
    })
}

fn permissions(mode: u32) -> Mode {
    Mode::from_raw_mode(narrowed(mode & PERMISSION_BITS))
}

fn too_deep() -> io::Error {
    io::Error::other(format!("nested over {MAX_DEPTH} folders deep"))
}

#[derive(Debug)]
pub(super) struct Folder(OwnedFd);

impl Folder {
    pub(super) fn found(&self, name: &OsStr) -> io::Result<Found> {
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
                mode: widened(stat.st_mode) & PERMISSION_BITS,
            },
            FileType::Fifo
            | FileType::Socket
            | FileType::CharacterDevice
            | FileType::BlockDevice
            | FileType::Unknown => Found::Other,
        })
    }

    pub(super) fn open(&self, name: &OsStr) -> io::Result<Self> {
        rustix::fs::openat(
            &self.0,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(Self)
        .map_err(io::Error::from)
    }

    pub(super) fn names(&self) -> io::Result<BTreeSet<OsString>> {
        let mut names = BTreeSet::new();
        for entry in rustix::fs::Dir::read_from(&self.0)? {
            let name = entry?.file_name().to_bytes().to_vec();
            if name != b"." && name != b".." {
                names.insert(OsString::from_vec(name));
            }
        }
        Ok(names)
    }

    pub(super) fn read(&self, name: &OsStr, length: u64) -> io::Result<Vec<u8>> {
        let file = self.open_file(name, OFlags::RDONLY)?;
        let mut bytes = Vec::new();
        fs::File::from(file)
            .take(length.saturating_add(1))
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn open_file(&self, name: &OsStr, access: OFlags) -> io::Result<OwnedFd> {
        let file = rustix::fs::openat(
            &self.0,
            name,
            access | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        if FileType::from_raw_mode(rustix::fs::fstat(&file)?.st_mode) != FileType::RegularFile {
            return Err(io::Error::other("not a regular file"));
        }
        Ok(file)
    }

    pub(super) fn make_folder(&self, name: &OsStr) -> io::Result<Self> {
        rustix::fs::mkdirat(&self.0, name, permissions(FOLDER_MODE))?;
        self.open(name)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn put_link(&self, name: &OsStr, target: &Path) -> io::Result<()> {
        let temporary = random_name(TEMPORARY_PREFIX)?;
        rustix::fs::symlinkat(target, &self.0, &temporary)?;
        let renamed = rustix::fs::renameat(&self.0, &temporary, &self.0, name);
        if let Err(error) = renamed {
            ignore_missing(rustix::fs::unlinkat(&self.0, &temporary, AtFlags::empty()))?;
            return Err(error.into());
        }
        Ok(())
    }

    pub(super) fn put_file(&self, name: &OsStr, bytes: &[u8], mode: u32) -> io::Result<()> {
        let temporary = random_name(TEMPORARY_PREFIX)?;
        let file = rustix::fs::openat(
            &self.0,
            &temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            permissions(mode),
        )?;
        let written = rustix::fs::fchmod(&file, permissions(mode))
            .map_err(io::Error::from)
            .and_then(|()| fs::File::from(file).write_all(bytes))
            .and_then(|()| {
                rustix::fs::renameat(&self.0, &temporary, &self.0, name).map_err(io::Error::from)
            });
        if written.is_err() {
            ignore_missing(rustix::fs::unlinkat(&self.0, &temporary, AtFlags::empty()))?;
        }
        written
    }

    fn append(&self, name: &OsStr, bytes: &[u8]) -> io::Result<()> {
        let file = self.open_file(name, OFlags::WRONLY | OFlags::APPEND)?;
        fs::File::from(file).write_all(bytes)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn move_into_new(&self, name: &OsStr, into: &Self, to: &OsStr) -> io::Result<()> {
        rustix::fs::renameat_with(
            &self.0,
            name,
            &into.0,
            to,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::from)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn move_to(&self, name: &OsStr, into: &Self, to: &OsStr) -> io::Result<()> {
        rustix::fs::renameat(&self.0, name, &into.0, to).map_err(io::Error::from)
    }

    pub(super) fn remove(&self, name: &OsStr) -> io::Result<()> {
        match self.found(name)? {
            Found::Missing => Ok(()),
            Found::Folder => {
                let planted = random_name(PLANTED_PREFIX)?;
                rustix::fs::renameat(&self.0, name, &self.0, &planted)?;
                self.remove_tree(&planted, 0)
            }
            Found::Link(_) | Found::File { .. } | Found::Other => {
                ignore_missing(rustix::fs::unlinkat(&self.0, name, AtFlags::empty()))
            }
        }
    }

    fn remove_tree(&self, name: &OsStr, depth: usize) -> io::Result<()> {
        if depth == MAX_DEPTH {
            return Err(too_deep());
        }
        let inner = match self.open(name) {
            Ok(inner) => inner,
            Err(error)
                if matches!(
                    rustix::io::Errno::from_io_error(&error),
                    Some(rustix::io::Errno::NOTDIR | rustix::io::Errno::LOOP)
                ) =>
            {
                return ignore_missing(rustix::fs::unlinkat(&self.0, name, AtFlags::empty()));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for child in inner.names()? {
            match inner.found(&child)? {
                Found::Missing => {}
                Found::Folder => inner.remove_tree(&child, depth + 1)?,
                Found::Link(_) | Found::File { .. } | Found::Other => {
                    ignore_missing(rustix::fs::unlinkat(&inner.0, &child, AtFlags::empty()))?;
                }
            }
        }
        ignore_missing(rustix::fs::unlinkat(&self.0, name, AtFlags::REMOVEDIR))
    }

    #[cfg(target_os = "linux")]
    pub(super) fn copy_tree_to(&self, name: &OsStr, to: &Path, depth: usize) -> io::Result<()> {
        if depth == MAX_DEPTH {
            return Err(too_deep());
        }
        match self.found(name)? {
            Found::Missing | Found::Other => Ok(()),
            Found::Link(target) => std::os::unix::fs::symlink(target, to),
            Found::File { length, mode } => {
                let bytes = self.read(name, length)?;
                let mut options = fs::OpenOptions::new();
                std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
                options
                    .write(true)
                    .create_new(true)
                    .open(to)?
                    .write_all(&bytes)
            }
            Found::Folder => {
                fs::create_dir(to)?;
                let inner = self.open(name)?;
                for child in inner.names()? {
                    inner.copy_tree_to(&child, &to.join(&child), depth + 1)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct Home {
    pub(super) path: PathBuf,
    pub(super) folder: Folder,
}

impl Home {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
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

    fn create(path: &Path) -> io::Result<Self> {
        fs::create_dir_all(path)?;
        Self::open(path)
    }

    pub(super) fn open_existing(path: &Path) -> io::Result<Option<Self>> {
        match Self::open(path) {
            Ok(home) => Ok(Some(home)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) fn reach(&self, relative: &Path, create: Create<'_>) -> io::Result<Option<Folder>> {
        Ok(match self.walk(relative, create)? {
            Reach::Reached(folder) => Some(folder),
            Reach::Missing | Reach::Blocked(_) => None,
        })
    }

    fn walk(&self, relative: &Path, create: Create<'_>) -> io::Result<Reach> {
        let mut folder = Folder(self.folder.0.try_clone()?);
        let mut reached = PathBuf::new();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(io::Error::other(format!(
                    "{} isn't a plain relative path",
                    relative.display()
                )));
            };
            reached.push(name);
            folder = match (folder.found(name)?, create) {
                (Found::Folder, _) => folder.open(name)?,
                (Found::Missing | Found::File { .. } | Found::Other, Create::No)
                | (Found::File { .. } | Found::Other, Create::Missing) => {
                    return Ok(Reach::Missing);
                }
                (Found::Link(_), Create::No | Create::Missing) => {
                    return Ok(Reach::Blocked(Linked(self.path.join(&reached))));
                }
                (Found::Missing, Create::Missing | Create::Replacing(_)) => {
                    folder.make_folder(name)?
                }
                (
                    Found::Link(_) | Found::File { .. } | Found::Other,
                    Create::Replacing(planted),
                ) => {
                    if !planted(&reached) {
                        return Ok(Reach::Blocked(Linked(self.path.join(&reached))));
                    }
                    tracing::warn!(path = %self.path.join(&reached).display(), "replacing what a sandboxed run put where Forge keeps an engine's files");
                    folder.remove(name)?;
                    folder.make_folder(name)?
                }
            };
        }
        Ok(Reach::Reached(folder))
    }

    pub(super) fn reach_replacing(
        &self,
        relative: &Path,
        planted: &dyn Fn(&Path) -> bool,
    ) -> io::Result<Result<Folder, Linked>> {
        match self.walk(relative, Create::Replacing(planted))? {
            Reach::Reached(folder) => Ok(Ok(folder)),
            Reach::Blocked(linked) => Ok(Err(linked)),
            Reach::Missing => unreachable!(
                "invariant: reaching {} while replacing makes every missing folder",
                relative.display()
            ),
        }
    }

    pub(super) fn place(
        &self,
        relative: &Path,
        planted: &dyn Fn(&Path) -> bool,
    ) -> io::Result<Result<Placing, Linked>> {
        let (parent, name) = split(relative)?;
        let folder = match self.reach_replacing(parent, planted)? {
            Ok(folder) => folder,
            Err(linked) => return Ok(Err(linked)),
        };
        let found = folder.found(name)?;
        if matches!(found, Found::Link(_)) && !planted(relative) {
            return Ok(Err(Linked(self.path.join(relative))));
        }
        Ok(Ok(Placing {
            folder,
            name: name.to_os_string(),
            found,
        }))
    }

    pub(super) fn read_unlinked(
        &self,
        relative: &Path,
    ) -> io::Result<Result<Option<Vec<u8>>, Linked>> {
        let (parent, name) = split(relative)?;
        let folder = match self.walk(parent, Create::No)? {
            Reach::Reached(folder) => folder,
            Reach::Missing => return Ok(Ok(None)),
            Reach::Blocked(linked) => return Ok(Err(linked)),
        };
        match folder.found(name)? {
            Found::File { length, .. } => folder.read(name, length).map(|bytes| Ok(Some(bytes))),
            Found::Link(_) => Ok(Err(Linked(self.path.join(relative)))),
            Found::Missing | Found::Folder | Found::Other => Ok(Ok(None)),
        }
    }
}

#[derive(Debug)]
pub(super) struct Linked(pub(super) PathBuf);

enum Reach {
    Reached(Folder),
    Missing,
    Blocked(Linked),
}

#[derive(Debug)]
pub(super) struct Placing {
    pub(super) folder: Folder,
    pub(super) name: OsString,
    pub(super) found: Found,
}

impl Placing {
    pub(super) fn put_file(&self, bytes: &[u8], mode: u32) -> io::Result<()> {
        if matches!(self.found, Found::Folder) {
            self.folder.remove(&self.name)?;
        }
        self.folder.put_file(&self.name, bytes, mode)
    }
}

#[derive(Clone, Copy)]
pub(super) enum Create<'planted> {
    No,
    Missing,
    Replacing(&'planted dyn Fn(&Path) -> bool),
}

const fn forge_owned(_reached: &Path) -> bool {
    true
}

fn split(relative: &Path) -> io::Result<(&Path, &OsStr)> {
    let name = relative
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} names no file", relative.display())))?;
    Ok((relative.parent().unwrap_or_else(|| Path::new("")), name))
}

pub(crate) fn write(home: &Path, relative: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let (parent, name) = split(relative)?;
    let opened = Home::create(home)?;
    let folder = opened
        .reach(parent, Create::Replacing(&forge_owned))?
        .expect("invariant: Forge replaces anything in the way of its own files");
    if matches!(folder.found(name)?, Found::Folder) {
        folder.remove(name)?;
    }
    folder.put_file(name, bytes, mode)
}

pub(crate) fn read(home: &Path, relative: &Path) -> io::Result<Option<Vec<u8>>> {
    let (parent, name) = split(relative)?;
    let Some(opened) = Home::open_existing(home)? else {
        return Ok(None);
    };
    let Some(folder) = opened.reach(parent, Create::No)? else {
        return Ok(None);
    };
    match folder.found(name)? {
        Found::Missing => Ok(None),
        Found::File { length, .. } => folder.read(name, length).map(Some),
        Found::Link(_) => Err(io::Error::from(rustix::io::Errno::LOOP)),
        Found::Folder | Found::Other => Err(io::Error::other("not a regular file")),
    }
}

pub(crate) fn append(home: &Path, relative: &Path, bytes: &[u8]) -> io::Result<()> {
    let (parent, name) = split(relative)?;
    let opened = Home::open(home)?;
    let folder = opened
        .reach(parent, Create::No)?
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    folder.append(name, bytes)
}

pub(crate) fn remove(home: &Path, relative: &Path) -> io::Result<()> {
    let (parent, name) = split(relative)?;
    let Some(opened) = Home::open_existing(home)? else {
        return Ok(());
    };
    match opened.reach(parent, Create::No)? {
        Some(folder) => folder.remove(name),
        None => Ok(()),
    }
}

pub(crate) fn make_folder(home: &Path, relative: &Path) -> io::Result<()> {
    let opened = Home::create(home)?;
    opened.reach(relative, Create::Missing).map(drop)
}

pub(crate) fn make_own_folder(root: &Path, relative: &Path) -> io::Result<()> {
    let opened = Home::create(root)?;
    opened
        .reach(relative, Create::Replacing(&forge_owned))
        .map(drop)
}
