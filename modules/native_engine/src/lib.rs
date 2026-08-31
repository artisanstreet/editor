#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

mod files;
mod install;
mod profile;
#[cfg(windows)]
mod windows_private;

pub use files::{
    AtomicReplaceOutcome, NativeFileError, VerifiedFileIdentity, ensure_directory,
    ensure_private_directory, read_bounded, replace_file, validate_private_directory,
    verify_directory, verify_file,
};
pub use install::{
    NativeOpenCode2Authority, NativeOpenCode2Error, NativeOpenCode2InstallLock,
    NativeOpenCode2InstallLockError, NativeOpenCode2InstallPathError, NativeOpenCode2InstallPaths,
    NativeOpenCode2InstallSpec, NativeOpenCode2State, NativeOpenCode2StateError,
    OpenCode2Inspection, ResolvedOpenCode2Generation, platform_supported,
};
pub use profile::{
    NativeOpenCode2ProfileError, NativeOpenCode2ProfileLaunchError, OpenCode2Profile,
    ProfileHomeKind, ProfileRegistrationOutcome, VerifiedOpenCode2ProfileLaunch,
};
