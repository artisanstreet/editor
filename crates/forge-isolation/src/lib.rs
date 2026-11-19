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

#[cfg(unix)]
pub fn contain(command: &mut std::process::Command, sandbox: Option<&Ruleset>) {
    unix::contain(command, sandbox.cloned());
}

#[cfg(not(unix))]
pub const fn contain(_command: &mut std::process::Command, _sandbox: Option<&Ruleset>) {}

#[cfg(windows)]
pub fn started(process_id: u32) {
    windows::demote(process_id);
}

#[cfg(target_os = "linux")]
pub fn started(process_id: u32) {
    linux::started(process_id);
}

#[cfg(not(any(windows, target_os = "linux")))]
pub const fn started(_process_id: u32) {}
