//! Finite Hermes native service discovery and non-billable readiness probe.
//!
//! This module mirrors `modules/engines/src/hermes/service.ts` (`HermesExecutable`,
//! `HermesVersion`, `minimum_hermes_version`) without spawning any service,
//! opening any WebSocket, or synthesizing authentication. The only child
//! process ever executed here is `hermes --version`, bounded in bytes and
//! time, with kill plus reap on every path.
//!
//! Authentication stays profile-owned per the TypeScript engine descriptor
//! (`auth: unsupported`, owned by the installed Hermes profile): the probe
//! reports [`probe::HermesAuthState::Unknown`] with reason
//! [`inventory::AUTH_UNKNOWN_REASON`] and never anything else.
//!
//! Wiring note for the controller: this directory is not yet declared in
//! `modules/native_engine/src/lib.rs`, `Cargo.toml`, or `BUILD.bazel`. The
//! exact registrations are returned with the worker report.

pub mod inventory;
pub mod probe;
pub mod resolve;
pub mod version;

pub use inventory::{
    AUTH_UNKNOWN_REASON, DEFAULT_PROFILE_ID, HERMES_ENGINE_ID, HermesInstalledProfileLayout,
    HermesInventoryRequest, INSTALLED_PROFILE_AUTH_OWNER, MODEL_OPTIONS_METHOD,
    installed_profile_layout, live_inventory_request,
};
pub use probe::{
    DEFAULT_PROBE_DEADLINE, HermesAuthState, HermesProbe, HermesProbeError, HermesProbeInput,
    MAXIMUM_VERSION_OUTPUT_BYTES, PROBE_POLL_INTERVAL, ProbeLimits, SpawnedVersionOutput,
    check_minimum_version, default_probe_limits, evaluate_version_output, probe_installed_hermes,
    run_hermes_probe, spawn_capture,
};
pub use resolve::{
    HERMES_EXECUTABLE_ENV, HERMES_PATH_BINARY, HERMES_PATH_BINARY_WINDOWS, HermesExecutableSource,
    INSTALLED_WINDOWS_PARTS, ResolvedHermesExecutable, installed_hermes_path,
    resolve_hermes_executable, resolve_hermes_executable_from_parts,
};
pub use version::{
    HermesVersion, HermesVersionError, MINIMUM_HERMES_VERSION, parse_hermes_version,
};
