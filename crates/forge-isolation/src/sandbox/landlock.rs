use super::{Coverage, Reach};
use ::landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, RulesetAttr, RulesetCreatedAttr,
    Scope, path_beneath_rules,
};
use rustix::fs::{FileType, Mode, OFlags, ResolveFlags};
use std::ffi::OsString;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

const CREATE_RULESET_VERSION: libc::c_uint = 1;
const MAX_LINKS: usize = 40;
const DEVICES: [&str; 6] = [
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/tty",
    "/dev/ptmx",
    "/dev/pts",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Landlock {
    abi: i32,
}

impl Coverage {
    #[must_use]
    pub fn of(abi: i32) -> Self {
        let abi = ABI::from(abi);
        let writes = AccessFs::from_write(abi);
        let scopes = Scope::from_all(abi);
        let reach = |blocked: bool| if blocked { Reach::Blocked } else { Reach::Open };
        Self {
            truncation: reach(writes.contains(AccessFs::Truncate)),
            socket_files: reach(writes.contains(AccessFs::ResolveUnix)),
            abstract_sockets: reach(scopes.contains(Scope::AbstractUnixSocket)),
            signals: reach(scopes.contains(Scope::Signal)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ruleset(Arc<OwnedFd>);

#[derive(Debug, thiserror::Error)]
pub enum RulesetError {
    #[error("start a Landlock ruleset that handles writes")]
    Create(#[source] ::landlock::RulesetError),
    #[error("resolve writable folder {} for the sandbox", path.display())]
    Resolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writable folder {} resolves through a link in {}, which the sandbox can write", path.display(), link_folder.display())]
    LinkedThroughWritable { path: PathBuf, link_folder: PathBuf },
    #[error("open writable folder {} for the sandbox", path.display())]
    OpenRoot {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writable folder {} was moved to {} while the sandbox was built", path.display(), opened.display())]
    Moved { path: PathBuf, opened: PathBuf },
    #[error("let the sandbox write beneath {}", path.display())]
    Allow {
        path: PathBuf,
        #[source]
        source: ::landlock::RulesetError,
    },
    #[error("let the sandbox write to the terminal and null devices")]
    Devices(#[source] ::landlock::RulesetError),
    #[error("let the sandbox reach socket {}", path.display())]
    Socket {
        path: PathBuf,
        #[source]
        source: ::landlock::RulesetError,
    },
    #[error("open socket {} for the sandbox", path.display())]
    OpenSocket {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub(super) fn detect() -> Option<Landlock> {
    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0_usize,
            CREATE_RULESET_VERSION,
        )
    };
    match i32::try_from(version) {
        Ok(abi) if abi > 0 => Some(Landlock { abi }),
        Ok(_) => None,
        Err(error) => unreachable!("invariant: Landlock ABI {version} fits an i32: {error}"),
    }
}

impl Landlock {
    #[must_use]
    pub const fn abi(self) -> i32 {
        self.abi
    }

    #[must_use]
    pub fn coverage(self) -> Coverage {
        Coverage::of(self.abi)
    }

    pub fn writes_only_to(
        self,
        roots: &[PathBuf],
        private: &Path,
        sockets: &[PathBuf],
    ) -> Result<Ruleset, RulesetError> {
        let abi = ABI::from(self.abi);
        let writes = AccessFs::from_write(abi);
        let mut beneath_roots = writes;
        beneath_roots.remove(AccessFs::ResolveUnix);
        let scopes = Scope::from_all(abi);
        let roots = outermost(&resolve_all(roots)?)?;
        let private = private_folder(private, &roots)?;
        let sockets = if writes.contains(AccessFs::ResolveUnix) {
            reachable(resolve_all(sockets)?, &roots)?
        } else {
            Vec::new()
        };
        let ruleset = ::landlock::Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(writes)
            .map_err(RulesetError::Create)?;
        let ruleset = if scopes.is_empty() {
            ruleset
        } else {
            ruleset.scope(scopes).map_err(RulesetError::Create)?
        };
        let created = ruleset.create().map_err(RulesetError::Create)?;
        let created = roots.iter().try_fold(created, |created, root| {
            let folder = open_exactly(&root.canonical, OFlags::DIRECTORY).map_err(|source| {
                RulesetError::OpenRoot {
                    path: root.requested.clone(),
                    source,
                }
            })?;
            confirm(&folder, root)?;
            created
                .add_rule(PathBeneath::new(folder, beneath_roots))
                .map_err(|source| RulesetError::Allow {
                    path: root.requested.clone(),
                    source,
                })
        })?;
        let own = open_exactly(&private.canonical, OFlags::DIRECTORY).map_err(|source| {
            RulesetError::OpenRoot {
                path: private.requested.clone(),
                source,
            }
        })?;
        confirm(&own, &private)?;
        let created = created
            .add_rule(PathBeneath::new(own, writes))
            .map_err(|source| RulesetError::Allow {
                path: private.requested.clone(),
                source,
            })?;
        let created = created
            .add_rules(path_beneath_rules(DEVICES, beneath_roots))
            .map_err(RulesetError::Devices)?;
        let created = sockets.iter().try_fold(created, |created, socket| {
            let file = open_exactly(&socket.canonical, OFlags::empty()).map_err(|source| {
                RulesetError::OpenSocket {
                    path: socket.requested.clone(),
                    source,
                }
            })?;
            confirm(&file, socket)?;
            let kind = rustix::fs::fstat(&file)
                .map(|stat| FileType::from_raw_mode(stat.st_mode))
                .map_err(|source| RulesetError::OpenSocket {
                    path: socket.requested.clone(),
                    source: source.into(),
                })?;
            if kind != FileType::Socket {
                tracing::warn!(path = %socket.requested.display(), "a socket the sandbox would reach isn't a socket; Auto runs can't use it");
                return Ok(created);
            }
            created
                .add_rule(PathBeneath::new(file, AccessFs::ResolveUnix))
                .map_err(|source| RulesetError::Socket {
                    path: socket.requested.clone(),
                    source,
                })
        })?;
        let descriptor: Option<OwnedFd> = created.into();
        let descriptor = descriptor.unwrap_or_else(|| {
            unreachable!(
                "invariant: a kernel reporting Landlock ABI {} creates rulesets",
                self.abi
            )
        });
        Ok(Ruleset(Arc::new(descriptor)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Resolved {
    requested: PathBuf,
    canonical: PathBuf,
    link_folders: Vec<PathBuf>,
}

fn resolve_all(paths: &[PathBuf]) -> Result<Vec<Resolved>, RulesetError> {
    paths
        .iter()
        .filter_map(|path| resolve(path).transpose())
        .collect()
}

fn resolve(requested: &Path) -> Result<Option<Resolved>, RulesetError> {
    let failed = |source: std::io::Error| RulesetError::Resolve {
        path: requested.to_path_buf(),
        source,
    };
    if !requested.is_absolute() {
        return Err(failed(std::io::Error::from(rustix::io::Errno::INVAL)));
    }
    let mut pending = parts(requested);
    let mut canonical = PathBuf::from("/");
    let mut link_folders = Vec::new();
    while let Some(part) = pending.pop() {
        if part == ".." {
            canonical.pop();
            continue;
        }
        let next = canonical.join(&part);
        let metadata = match std::fs::symlink_metadata(&next) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(failed(error)),
        };
        if !metadata.file_type().is_symlink() {
            canonical = next;
            continue;
        }
        if link_folders.len() == MAX_LINKS {
            return Err(failed(std::io::Error::from(rustix::io::Errno::LOOP)));
        }
        let target = std::fs::read_link(&next).map_err(failed)?;
        link_folders.push(canonical.clone());
        if target.is_absolute() {
            canonical = PathBuf::from("/");
        }
        pending.extend(parts(&target));
    }
    Ok(Some(Resolved {
        requested: requested.to_path_buf(),
        canonical,
        link_folders,
    }))
}

fn parts(path: &Path) -> Vec<OsString> {
    path.components()
        .rev()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_os_string()),
            Component::ParentDir => Some(OsString::from("..")),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => None,
        })
        .collect()
}

fn strictly_beneath(path: &Path, folder: &Path) -> bool {
    path != folder && path.starts_with(folder)
}

fn outermost(resolved: &[Resolved]) -> Result<Vec<Resolved>, RulesetError> {
    let lexical: Vec<Resolved> = resolved
        .iter()
        .enumerate()
        .filter(|(index, root)| {
            !resolved.iter().enumerate().any(|(other, outer)| {
                strictly_beneath(&root.requested, &outer.requested)
                    || (root.requested == outer.requested && other < *index)
            })
        })
        .map(|(_, root)| root.clone())
        .collect();
    let kept: Vec<Resolved> = lexical
        .iter()
        .enumerate()
        .filter(|(index, root)| {
            !lexical.iter().enumerate().any(|(other, outer)| {
                strictly_beneath(&root.canonical, &outer.canonical)
                    || (root.canonical == outer.canonical && other < *index)
            })
        })
        .map(|(_, root)| root.clone())
        .collect();
    kept.iter()
        .try_for_each(|root| outside_writable(root, &kept))
        .map(|()| kept)
}

fn private_folder(private: &Path, roots: &[Resolved]) -> Result<Resolved, RulesetError> {
    let resolved = resolve(private)?.ok_or_else(|| RulesetError::Resolve {
        path: private.to_path_buf(),
        source: std::io::Error::from(rustix::io::Errno::NOENT),
    })?;
    outside_writable(&resolved, roots)?;
    Ok(resolved)
}

fn reachable(sockets: Vec<Resolved>, roots: &[Resolved]) -> Result<Vec<Resolved>, RulesetError> {
    sockets
        .iter()
        .try_for_each(|socket| outside_writable(socket, roots))
        .map(|()| sockets)
}

fn outside_writable(path: &Resolved, roots: &[Resolved]) -> Result<(), RulesetError> {
    let writable = |folder: &Path| roots.iter().any(|root| folder.starts_with(&root.canonical));
    match path.link_folders.iter().find(|folder| writable(folder)) {
        Some(folder) => Err(RulesetError::LinkedThroughWritable {
            path: path.requested.clone(),
            link_folder: folder.clone(),
        }),
        None => Ok(()),
    }
}

fn open_exactly(canonical: &Path, kind: OFlags) -> std::io::Result<OwnedFd> {
    rustix::fs::openat2(
        rustix::fs::CWD,
        canonical,
        OFlags::PATH | OFlags::CLOEXEC | kind,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(std::io::Error::from)
}

fn confirm(opened: &OwnedFd, expected: &Resolved) -> Result<(), RulesetError> {
    let link = PathBuf::from(format!("/proc/self/fd/{}", opened.as_fd().as_raw_fd()));
    let actual = std::fs::read_link(&link).map_err(|source| RulesetError::OpenRoot {
        path: expected.requested.clone(),
        source,
    })?;
    if actual == expected.canonical {
        Ok(())
    } else {
        Err(RulesetError::Moved {
            path: expected.requested.clone(),
            opened: actual,
        })
    }
}

impl Ruleset {
    pub(crate) fn restrict_self(&self) -> std::io::Result<()> {
        unsafe {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::syscall(libc::SYS_landlock_restrict_self, self.0.as_raw_fd(), 0_u32) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}
