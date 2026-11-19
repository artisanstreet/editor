use super::{Coverage, Reach};
use ::landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, PathFdError, RulesetAttr,
    RulesetCreatedAttr, Scope, path_beneath_rules,
};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Arc;

const CREATE_RULESET_VERSION: libc::c_uint = 1;
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
    #[error("open writable folder {} for the sandbox", path.display())]
    OpenRoot {
        path: PathBuf,
        #[source]
        source: PathFdError,
    },
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
        source: PathFdError,
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
    i32::try_from(version)
        .ok()
        .filter(|abi| *abi > 0)
        .map(|abi| Landlock { abi })
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
        sockets: &[PathBuf],
    ) -> Result<Ruleset, RulesetError> {
        let abi = ABI::from(self.abi);
        let writes = AccessFs::from_write(abi);
        let scopes = Scope::from_all(abi);
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
        let created = present(roots).try_fold(created, |created, root| {
            let folder = PathFd::new(root).map_err(|source| RulesetError::OpenRoot {
                path: root.clone(),
                source,
            })?;
            created
                .add_rule(PathBeneath::new(folder, writes))
                .map_err(|source| RulesetError::Allow {
                    path: root.clone(),
                    source,
                })
        })?;
        let created = created
            .add_rules(path_beneath_rules(DEVICES, writes))
            .map_err(RulesetError::Devices)?;
        let mut reachable = present(sockets).filter(|_| writes.contains(AccessFs::ResolveUnix));
        let created = reachable.try_fold(created, |created, socket| {
            let file = PathFd::new(socket).map_err(|source| RulesetError::OpenSocket {
                path: socket.clone(),
                source,
            })?;
            created
                .add_rule(PathBeneath::new(file, AccessFs::ResolveUnix))
                .map_err(|source| RulesetError::Socket {
                    path: socket.clone(),
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

fn present(paths: &[PathBuf]) -> impl Iterator<Item = &PathBuf> {
    paths.iter().filter(|path| {
        !matches!(
            std::fs::symlink_metadata(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        )
    })
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
