#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
#[expect(unsafe_code, reason = "setiopolicy_np is a C call")]
mod macos;
mod sandbox;
#[cfg(unix)]
#[expect(unsafe_code, reason = "pre_exec runs between fork and exec")]
mod unix;
#[cfg(windows)]
#[expect(unsafe_code, reason = "Win32 process calls")]
mod windows;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied(String),
    Skipped { measure: String, reason: String },
}

#[cfg(any(target_os = "linux", windows))]
impl Outcome {
    fn of<E: std::fmt::Display>(measure: &str, result: Result<(), E>) -> Self {
        match result {
            Ok(()) => Self::Applied(measure.to_owned()),
            Err(error) => Self::Skipped {
                measure: measure.to_owned(),
                reason: error.to_string(),
            },
        }
    }
}

#[cfg(target_os = "linux")]
pub fn protect_forge() -> Vec<Outcome> {
    linux::protect_forge()
}

#[cfg(windows)]
pub fn protect_forge() -> Vec<Outcome> {
    windows::protect_forge()
}

#[cfg(not(any(target_os = "linux", windows)))]
pub const fn protect_forge() -> Vec<Outcome> {
    Vec::new()
}

pub use sandbox::{
    Coverage, Landlock, Reach, Ruleset, RulesetError, Support, Unavailable, support,
};

#[cfg(target_os = "linux")]
pub use linux::{EndRunError, LeftoverError, RunGroupError};

#[cfg(not(target_os = "linux"))]
pub type RunGroupError = std::convert::Infallible;

#[cfg(not(target_os = "linux"))]
pub type EndRunError = std::convert::Infallible;

#[cfg(not(target_os = "linux"))]
pub type LeftoverError = std::convert::Infallible;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    OwnCgroup,
    Shared,
}

#[derive(Debug)]
pub struct EngineTree {
    #[cfg(target_os = "linux")]
    cgroup: linux::Cgroup,
}

impl EngineTree {
    #[cfg(target_os = "linux")]
    pub fn new(containment: Containment) -> Result<Self, RunGroupError> {
        let cgroup = match containment {
            Containment::OwnCgroup => linux::Cgroup::own()?,
            Containment::Shared => linux::Cgroup::shared(),
        };
        Ok(Self { cgroup })
    }

    #[cfg(not(target_os = "linux"))]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the same signature as Linux, where making the run's cgroup can fail"
    )]
    pub const fn new(_containment: Containment) -> Result<Self, RunGroupError> {
        Ok(Self {})
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn membership(&self) -> Option<std::ffi::CString> {
        self.cgroup.membership()
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn kill_switch(&self) -> Option<std::ffi::CString> {
        self.cgroup.kill_switch()
    }

    #[cfg(target_os = "linux")]
    pub fn end(&self) -> Result<(), EndRunError> {
        self.cgroup.end()
    }

    #[cfg(not(target_os = "linux"))]
    #[expect(
        clippy::unnecessary_wraps,
        clippy::unused_self,
        reason = "the same signature as Linux, where killing the run's cgroup can fail"
    )]
    pub const fn end(&self) -> Result<(), EndRunError> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub fn leftover_runs_alive() -> Result<bool, LeftoverError> {
    linux::leftovers_alive()
}

#[cfg(not(target_os = "linux"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as Linux, where reading a leftover run's cgroup can fail"
)]
pub const fn leftover_runs_alive() -> Result<bool, LeftoverError> {
    Ok(false)
}

#[cfg(unix)]
pub use unix::ContainError;

#[cfg(not(unix))]
pub type ContainError = std::convert::Infallible;

#[cfg(unix)]
pub fn contain(
    command: &mut std::process::Command,
    sandbox: Option<&Ruleset>,
    tree: &EngineTree,
) -> Result<(), ContainError> {
    unix::contain(command, sandbox.cloned(), tree)
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as Unix, where opening Forge's lifeline pipe can fail"
)]
pub const fn contain(
    _command: &mut std::process::Command,
    _sandbox: Option<&Ruleset>,
    _tree: &EngineTree,
) -> Result<(), ContainError> {
    Ok(())
}

#[cfg(windows)]
pub fn started(_tree: &EngineTree, process_id: u32) {
    windows::demote(process_id);
}

#[cfg(target_os = "linux")]
pub fn started(tree: &EngineTree, process_id: u32) {
    linux::started(&tree.cgroup, process_id);
}

#[cfg(not(any(windows, target_os = "linux")))]
pub const fn started(_tree: &EngineTree, _process_id: u32) {}
