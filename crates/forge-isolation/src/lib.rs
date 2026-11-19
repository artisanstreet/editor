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
pub use linux::{EndRunError, RunGroupError};

#[cfg(not(target_os = "linux"))]
pub type RunGroupError = std::convert::Infallible;

#[cfg(not(target_os = "linux"))]
pub type EndRunError = std::convert::Infallible;

#[derive(Debug)]
pub struct EngineTree {
    #[cfg(target_os = "linux")]
    group: Option<linux::RunGroup>,
}

impl EngineTree {
    #[cfg(target_os = "linux")]
    pub fn new() -> Result<Self, RunGroupError> {
        linux::RunGroup::create().map(|group| Self { group })
    }

    #[cfg(not(target_os = "linux"))]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the same signature as Linux, where making the run's cgroup can fail"
    )]
    pub const fn new() -> Result<Self, RunGroupError> {
        Ok(Self {})
    }

    #[cfg(target_os = "linux")]
    pub(crate) const fn group(&self) -> Option<&linux::RunGroup> {
        self.group.as_ref()
    }

    #[cfg(target_os = "linux")]
    pub fn end(&self) -> Result<(), EndRunError> {
        self.group.as_ref().map_or(Ok(()), linux::RunGroup::end)
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

#[cfg(unix)]
pub fn contain(command: &mut std::process::Command, sandbox: Option<&Ruleset>, tree: &EngineTree) {
    unix::contain(command, sandbox.cloned(), tree);
}

#[cfg(not(unix))]
pub const fn contain(
    _command: &mut std::process::Command,
    _sandbox: Option<&Ruleset>,
    _tree: &EngineTree,
) {
}

#[cfg(windows)]
pub fn started(_tree: &EngineTree, process_id: u32) {
    windows::demote(process_id);
}

#[cfg(target_os = "linux")]
pub fn started(tree: &EngineTree, process_id: u32) {
    linux::started(tree.group(), process_id);
}

#[cfg(not(any(windows, target_os = "linux")))]
pub const fn started(_tree: &EngineTree, _process_id: u32) {}
