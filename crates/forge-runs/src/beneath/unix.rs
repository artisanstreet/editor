use super::{OpenBeneathError, Relative};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const FILE_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NONBLOCK)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

pub(super) fn directory(path: &Path) -> std::io::Result<std::fs::File> {
    rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(std::fs::File::from)
    .map_err(std::io::Error::from)
}

#[cfg(target_os = "linux")]
pub(super) fn open_file(
    root: &std::fs::File,
    relative: &Relative,
) -> Result<std::fs::File, OpenBeneathError> {
    use rustix::fs::ResolveFlags;
    let at = relative.path();
    match rustix::fs::openat2(
        root,
        &at,
        FILE_FLAGS,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    ) {
        Ok(file) => Ok(std::fs::File::from(file)),
        Err(Errno::NOSYS) => walk(root, relative),
        Err(Errno::LOOP) => Err(OpenBeneathError::Linked { at }),
        Err(errno) => Err(OpenBeneathError::Open {
            at,
            source: errno.into(),
        }),
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn open_file(
    root: &std::fs::File,
    relative: &Relative,
) -> Result<std::fs::File, OpenBeneathError> {
    walk(root, relative)
}

fn refusal(parent: BorrowedFd<'_>, name: &OsStr, at: PathBuf, errno: Errno) -> OpenBeneathError {
    let linked = matches!(errno, Errno::LOOP | Errno::NOTDIR | Errno::MLINK)
        && rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
            .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink);
    if linked {
        OpenBeneathError::Linked { at }
    } else {
        OpenBeneathError::Open {
            at,
            source: errno.into(),
        }
    }
}

pub(super) fn walk(
    root: &std::fs::File,
    relative: &Relative,
) -> Result<std::fs::File, OpenBeneathError> {
    let mut at = PathBuf::new();
    let mut held: Option<OwnedFd> = None;
    for name in &relative.parents {
        at.push(name);
        let parent = held.as_ref().map_or_else(|| root.as_fd(), AsFd::as_fd);
        let opened = rustix::fs::openat(parent, name.as_os_str(), DIRECTORY_FLAGS, Mode::empty())
            .map_err(|errno| refusal(parent, name, at.clone(), errno))?;
        held = Some(opened);
    }
    at.push(&relative.name);
    let parent = held.as_ref().map_or_else(|| root.as_fd(), AsFd::as_fd);
    rustix::fs::openat(parent, relative.name.as_os_str(), FILE_FLAGS, Mode::empty())
        .map(std::fs::File::from)
        .map_err(|errno| refusal(parent, &relative.name, at, errno))
}
