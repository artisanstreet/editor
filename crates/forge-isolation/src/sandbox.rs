#[cfg(target_os = "linux")]
#[allow(
    unsafe_code,
    reason = "Landlock is entered through raw syscalls between fork and exec"
)]
mod landlock;

#[cfg(target_os = "linux")]
pub use landlock::{Landlock, Ruleset, RulesetError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Enforced(Landlock),
    Unavailable(Unavailable),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    KernelWithoutLandlock,
    Windows,
    MacOs,
    OtherOs,
}

#[cfg(target_os = "linux")]
pub fn support() -> Support {
    static SUPPORT: std::sync::OnceLock<Support> = std::sync::OnceLock::new();
    *SUPPORT.get_or_init(|| {
        landlock::detect().map_or(
            Support::Unavailable(Unavailable::KernelWithoutLandlock),
            Support::Enforced,
        )
    })
}

#[cfg(windows)]
pub const fn support() -> Support {
    Support::Unavailable(Unavailable::Windows)
}

#[cfg(target_os = "macos")]
pub const fn support() -> Support {
    Support::Unavailable(Unavailable::MacOs)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub const fn support() -> Support {
    Support::Unavailable(Unavailable::OtherOs)
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Never {}

#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Landlock(Never);

#[cfg(not(target_os = "linux"))]
impl Landlock {
    pub const fn writes_only_to(
        self,
        _roots: &[std::path::PathBuf],
    ) -> Result<Ruleset, RulesetError> {
        match self.0 {}
    }
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone)]
pub struct Ruleset(Never);

#[cfg(all(unix, not(target_os = "linux")))]
impl Ruleset {
    pub(crate) const fn restrict_self(&self) -> std::io::Result<()> {
        match self.0 {}
    }
}

#[cfg(not(target_os = "linux"))]
pub type RulesetError = std::convert::Infallible;
