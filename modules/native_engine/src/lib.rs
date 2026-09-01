#![forbid(unsafe_code)]

#[path = "install.rs"]
mod engine_core;
#[path = "files.rs"]
mod io;
#[path = "profile.rs"]
mod resolver;
#[cfg(windows)]
mod windows_private;

pub use engine_core::{
    NativeOpenCode2Authority, NativeOpenCode2Error, NativeOpenCode2InstallLock,
    NativeOpenCode2InstallLockError, NativeOpenCode2InstallPathError, NativeOpenCode2InstallPaths,
    NativeOpenCode2InstallSpec, NativeOpenCode2State, NativeOpenCode2StateError,
    OpenCode2Inspection, ResolvedOpenCode2Generation, platform_supported,
};
pub use io::{
    AtomicReplaceOutcome, NativeFileError, VerifiedFileIdentity, ensure_directory,
    ensure_private_directory, read_bounded, replace_file, validate_private_directory,
    verify_directory, verify_file,
};
pub use resolver::{
    NativeOpenCode2ProfileError, NativeOpenCode2ProfileLaunchError, OpenCode2Profile,
    ProfileHomeKind, ProfileRegistrationOutcome, VerifiedOpenCode2ProfileLaunch,
};
