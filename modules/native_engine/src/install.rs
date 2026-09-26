//! Forge-managed engine installation authority.
//!
//! One engine-agnostic authority installs, versions, verifies, and resolves
//! every engine CLI (Claude Code, Codex, Grok Build, Cursor Agent,
//! `OpenCode2`). Children:
//!
//! - `catalog` fixes each engine's vendor feed, integrity source, floor, and
//!   artifact layout per platform (no versions);
//! - `version`, `feed`, and `transport` resolve versions and digests from the
//!   vendor feeds through the single network seam;
//! - `spec` owns validated paths, the exclusive install lock, and the shared
//!   use lease;
//! - `state` and `selection` own the bounded install-state and selection
//!   documents;
//! - `authority` owns inspection and verified active-generation resolution;
//! - `archive`, `pipeline`, and `operations` install, switch, roll back, and
//!   list versions;
//! - `launch` resolves spawn targets and builds the engine environment;
//! - `opencode2` is the engine-fixed façade used by `OpenCode2` profiles.

#[path = "install/archive.rs"]
mod archive;
#[path = "install/authority.rs"]
mod authority;
#[path = "install/catalog.rs"]
mod catalog;
#[path = "install/feed.rs"]
mod feed;
#[path = "install/launch.rs"]
mod launch;
#[path = "install/opencode2.rs"]
mod opencode2;
#[path = "install/operations.rs"]
mod operations;
#[path = "install/pipeline.rs"]
mod pipeline;
#[path = "install/selection.rs"]
mod selection;
#[path = "install/spec.rs"]
mod spec;
#[path = "install/state.rs"]
mod state;
#[path = "install/transport.rs"]
mod transport;
#[path = "install/version.rs"]
mod version;

pub use archive::ArchiveError;
pub use authority::{EngineInspection, ManagedEngineAuthority, ResolvedGeneration};
pub use catalog::{
    ArtifactPlan, Distribution, Feed, HostPlatform, Layout, ManagedEngine, UnsupportedReason,
    VersionFilter,
};
pub use feed::{ArtifactDigest, FeedError, FeedRequest, ReleaseArtifact};
pub use launch::{
    LaunchSource, LaunchTarget, SeatedLaunch, apply_managed_environment, build_environment,
    engine_home, managed_database, managed_environment_for, register_managed_database,
    resolve_launch_target, resolve_launch_target_in,
};
pub use opencode2::NativeOpenCode2Authority;
pub use operations::{
    EngineOperations, InstallError, InstallProgress, SwitchOutcome, VersionListing,
};
pub use selection::EngineSelection;
pub use spec::{
    EngineIdle, EngineUseLease, ManagedInstallLock, ManagedInstallLockError,
    ManagedInstallPathError, ManagedInstallPaths,
};
pub use state::{
    MAX_PREVIOUS_GENERATIONS, ManagedEngineError, ManagedGeneration, ManagedStateError,
    ManagedToolchainState,
};
pub use transport::{HttpsTransport, ReleaseTransport, TransportError};
pub use version::EngineVersion;

#[cfg(test)]
pub(crate) use archive::tests as archive_fixtures;

#[cfg(test)]
#[path = "install/install_tests.rs"]
mod tests;
