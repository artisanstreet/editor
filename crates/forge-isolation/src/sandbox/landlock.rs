use ::landlock::{
    ABI, AccessFs, PathBeneath, PathFd, PathFdError, RulesetAttr, RulesetCreatedAttr,
    path_beneath_rules,
};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Arc;

const WRITES: ABI = ABI::V8;
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

    pub fn writes_only_to(self, roots: &[PathBuf]) -> Result<Ruleset, RulesetError> {
        let writes = AccessFs::from_write(WRITES);
        let created = ::landlock::Ruleset::default()
            .handle_access(writes)
            .and_then(::landlock::Ruleset::create)
            .map_err(RulesetError::Create)?;
        let mut present = roots.iter().filter(|root| {
            !matches!(
                std::fs::symlink_metadata(root),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            )
        });
        let created = present.try_fold(created, |created, root| {
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
