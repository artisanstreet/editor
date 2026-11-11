#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
#[allow(unsafe_code, reason = "setiopolicy_np is a C call")]
mod macos;
#[cfg(unix)]
#[allow(unsafe_code, reason = "pre_exec runs between fork and exec")]
mod unix;
#[cfg(windows)]
#[allow(unsafe_code, reason = "Win32 process calls")]
mod windows;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied(String),
    Skipped { measure: String, reason: String },
}

#[cfg(any(target_os = "linux", windows))]
impl Outcome {
    pub(crate) fn of<E: std::fmt::Display>(measure: &str, result: Result<(), E>) -> Self {
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

#[cfg(unix)]
pub fn contain(command: &mut std::process::Command) {
    unix::contain(command);
}

#[cfg(not(unix))]
pub const fn contain(_command: &mut std::process::Command) {}

#[cfg(windows)]
pub fn started(process_id: u32) {
    windows::demote(process_id);
}

#[cfg(not(windows))]
pub const fn started(_process_id: u32) {}
