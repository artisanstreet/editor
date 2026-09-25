//! Installer authority: workflow, guarded filesystem, and durable state.
//!
//! This file keeps the crate-facing installation surface and the module's
//! behavior tests, split into three private children:
//!
//! - `authority` owns the exclusive installer lock, the pending-operation
//!   markers, identity-checked filesystem primitives, and the release stage
//!   lease.
//! - `workflow` owns manifest fetch, artifact download and extraction,
//!   activation, and the repair/diagnose/uninstall/prepare-update commands.
//! - `state` owns `installation.json` persistence, activation-pointer
//!   recovery, integration records, and validated root cleanup.

mod authority;
mod path_registry;
mod prune;
mod release;
mod source;
mod state;
mod workflow;

pub(crate) use authority::hash_file;
pub use prune::{PruneReport, prune};
pub use release::install;
pub use source::{RELEASE_MANIFEST_NAME, RELEASE_SIGNATURE_NAME, ReleaseSource};
pub use workflow::{
    InstallIntegrationOptions, InstallOptions, diagnose, prepare_update, repair, uninstall,
};

#[cfg(test)]
pub(crate) use self::source::platform_libc;

#[cfg(test)]
use {
    self::authority::{
        EntryKind, INSTALLER_LOCK_NAME, InstallerLock, PendingMarker, PendingMarkerKind, RootMode,
        StageLease, complete_install, ordinary_path_identity, pending_marker_path,
    },
    self::state::{
        activation_pointer_paths, inspect_activation_pointer, recover_activation_pointer_swap,
        remove_path_in_root, remove_validated_activation_pointer,
    },
    self::workflow::{
        FIRST_RUN_CONFIGURATION_COMMANDS, installed_components, should_restore_retired_forge,
        versioned_installer_path,
    },
    crate::{error::InstallerError, processes::Retirement},
};

#[cfg(all(test, windows))]
use self::path_registry::prepend_windows_path_entry;

#[cfg(test)]
mod tests;
