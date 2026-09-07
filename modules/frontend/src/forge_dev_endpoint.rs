//! Development Forge endpoint policy for the native transport service.
//!
//! The shipping startup path launches a newly owned Forge from the installed
//! payload and installation manifest. That path cannot work from a `cargo`
//! checkout: there is no installed payload to verify. This module owns the
//! narrow, explicitly opted-in development alternative used by
//! `native_transport_service`: when `ARTISAN_DEV_FORGE_HOME` names an
//! absolute scratch home, the service connects to the manually started dev
//! Forge whose readiness receipt that home contains, reusing the exact QUIC
//! handshake, credential files, and request surface as the owned path.
//! Nothing here invents an RPC: the endpoint, certificate pin, bootstrap
//! capability, and reconnect store are the same values the owned path uses.
//!
//! Activation is strictly opt-in: without the environment variable the
//! module reports "not requested" and the owned path runs unchanged. A home
//! containing `installation.json` is refused so a debug build can never
//! mistake the real installation for a scratch home.

#![forbid(unsafe_code)]

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Environment variable selecting the dev Forge home.
///
/// When unset or empty, the service uses the owned Forge path unchanged.
/// When set, it must name an absolute scratch directory shared with the
/// manually started dev Forge (credential files plus readiness receipt).
pub const DEV_HOME_ENV: &str = "ARTISAN_DEV_FORGE_HOME";

/// Optional override for the dev readiness receipt path.
///
/// When unset or empty, the service reads `<home>/readiness/forge.json`,
/// which is the same relative location the native instance configuration
/// uses for freshly provisioned homes.
pub const DEV_READY_FILE_ENV: &str = "ARTISAN_DEV_FORGE_READY_FILE";

/// Bound for one dev readiness receipt read, matching the owned CLI bound.
pub const DEV_READY_MAX_BYTES: usize = 4_096;

/// How long the dev path waits for the dev Forge to publish readiness.
pub const DEV_READY_WAIT_MS: u64 = 15_000;

/// Poll interval while waiting for dev readiness.
pub const DEV_READY_POLL_MS: u64 = 100;

/// Whole-stage client limits for the dev session.
pub const DEV_CONNECT_TIMEOUT_MS: u64 = 5_000;
/// Whole-stage client limits for the dev session.
pub const DEV_HANDSHAKE_TIMEOUT_MS: u64 = 5_000;
/// Whole-stage client limits for the dev session.
pub const DEV_REQUEST_TIMEOUT_MS: u64 = 30_000;
/// Whole-stage client limits for the dev session.
pub const DEV_SHUTDOWN_TIMEOUT_MS: u64 = 2_000;

/// Lifetime admission budget for the dev session.
pub const DEV_ADMISSION_BUDGET: usize = 1_024;

/// Selects the dev Forge home from one environment value.
///
/// Returns `None` when the variable is unset or empty (the owned path runs
/// unchanged). A set value is returned verbatim, including relative paths;
/// the caller rejects those with a typed configuration failure.
#[must_use]
pub fn dev_home_from_value(value: Option<&OsStr>) -> Option<PathBuf> {
    value.and_then(|value| {
        if value.is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    })
}

/// Reads [`DEV_HOME_ENV`] from the process environment.
///
/// See [`dev_home_from_value`] for the selection contract.
#[must_use]
pub fn dev_home_from_env() -> Option<PathBuf> {
    dev_home_from_value(std::env::var_os(DEV_HOME_ENV).as_deref())
}

/// Reads [`DEV_READY_FILE_ENV`] from the process environment.
///
/// Returns `None` when the variable is unset or empty.
#[must_use]
pub fn dev_ready_override_from_env() -> Option<PathBuf> {
    dev_home_from_value(std::env::var_os(DEV_READY_FILE_ENV).as_deref())
}

/// Returns the readiness receipt path for one dev home.
///
/// A set, non-empty override is used verbatim; otherwise the receipt lives
/// at `<home>/readiness/forge.json`.
#[must_use]
pub fn dev_ready_path(home: &Path, override_path: Option<&Path>) -> PathBuf {
    override_path.map_or_else(
        || home.join("readiness").join("forge.json"),
        Path::to_path_buf,
    )
}

/// Returns whether a dev home looks like a real installation.
///
/// A home containing `installation.json` is refused so a debug build can
/// never mistake the real installation for a scratch home. Missing files
/// and I/O failures report "not installed" without failing closed: the
/// caller still validates every later stage with typed failures.
#[must_use]
pub fn dev_home_is_installed(home: &Path) -> bool {
    home.join("installation.json").is_file()
}

/// Mints one nonzero development instance identity.
///
/// The value fences reconnect capability custody for this process only; a
/// later process mints a fresh identity and the existing store rebinds to
/// it. The mixer is intentionally not cryptographic: the identity is a
/// nonce, never a credential.
#[must_use]
pub fn mint_dev_instance_id() -> [u8; 16] {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(u64::from(std::process::id()), |elapsed| {
            u64::try_from(elapsed.as_nanos() & u128::from(u64::MAX)).unwrap_or(u64::MAX)
        });
    let pid = u64::from(std::process::id());
    let mut state = nanos
        ^ pid.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ counter.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    if state == 0 {
        state = 0xD1B5_4A32_9974_7719;
    }
    let mut identity = [0_u8; 16];
    for chunk in identity.chunks_mut(8) {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
    }
    if identity.iter().all(|byte| *byte == 0) {
        identity[15] = 1;
    }
    identity
}

#[cfg(test)]
mod tests {
    use super::{
        DEV_ADMISSION_BUDGET, DEV_CONNECT_TIMEOUT_MS, DEV_HANDSHAKE_TIMEOUT_MS, DEV_HOME_ENV,
        DEV_READY_FILE_ENV, DEV_READY_MAX_BYTES, DEV_READY_POLL_MS, DEV_READY_WAIT_MS,
        DEV_REQUEST_TIMEOUT_MS, DEV_SHUTDOWN_TIMEOUT_MS, dev_home_from_value,
        dev_home_is_installed, dev_ready_path, mint_dev_instance_id,
    };
    use std::{ffi::OsStr, path::PathBuf};

    #[test]
    fn env_contract_names_are_exact() {
        assert_eq!(DEV_HOME_ENV, "ARTISAN_DEV_FORGE_HOME");
        assert_eq!(DEV_READY_FILE_ENV, "ARTISAN_DEV_FORGE_READY_FILE");
    }

    #[test]
    fn dev_limits_are_finite_and_nonzero() {
        for limit in [
            DEV_CONNECT_TIMEOUT_MS,
            DEV_HANDSHAKE_TIMEOUT_MS,
            DEV_REQUEST_TIMEOUT_MS,
            DEV_SHUTDOWN_TIMEOUT_MS,
            DEV_READY_WAIT_MS,
            DEV_READY_POLL_MS,
        ] {
            assert_ne!(limit, 0);
        }
        assert_ne!(DEV_ADMISSION_BUDGET, 0);
        assert_eq!(DEV_READY_MAX_BYTES, 4_096);
    }

    #[test]
    fn unset_or_empty_value_means_not_requested() {
        assert_eq!(dev_home_from_value(None), None);
        assert_eq!(dev_home_from_value(Some(OsStr::new(""))), None);
    }

    #[test]
    fn set_values_pass_through_verbatim_for_typed_validation() {
        let relative = PathBuf::from("relative/dev-home");
        assert_eq!(
            dev_home_from_value(Some(relative.as_os_str())),
            Some(relative)
        );
        if cfg!(windows) {
            let absolute = PathBuf::from(r"C:\scratch\forge-dev-home");
            assert_eq!(
                dev_home_from_value(Some(absolute.as_os_str())),
                Some(absolute)
            );
        } else {
            let absolute = PathBuf::from("/tmp/forge-dev-home");
            assert_eq!(
                dev_home_from_value(Some(absolute.as_os_str())),
                Some(absolute)
            );
        }
    }

    #[test]
    fn ready_path_defaults_under_home_and_honours_overrides() {
        let home = PathBuf::from(if cfg!(windows) {
            r"C:\scratch\forge-dev-home"
        } else {
            "/tmp/forge-dev-home"
        });
        assert_eq!(
            dev_ready_path(&home, None),
            home.join("readiness").join("forge.json")
        );
        let override_path = PathBuf::from(if cfg!(windows) {
            r"C:\scratch\custom.json"
        } else {
            "/tmp/custom.json"
        });
        assert_eq!(dev_ready_path(&home, Some(&override_path)), override_path);
    }

    #[test]
    fn fresh_scratch_home_is_not_an_installation() {
        let home = std::env::temp_dir().join(format!(
            "artisan-forge-dev-probe-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&home);
        assert!(!dev_home_is_installed(&home));
        std::fs::create_dir_all(&home).expect("scratch probe home");
        assert!(!dev_home_is_installed(&home));
        std::fs::write(home.join("installation.json"), b"{}").expect("probe manifest");
        assert!(dev_home_is_installed(&home));
        std::fs::remove_dir_all(&home).expect("probe cleanup");
    }

    #[test]
    fn minted_instance_identities_are_nonzero() {
        for _ in 0..8 {
            let identity = mint_dev_instance_id();
            assert!(identity.iter().any(|byte| *byte != 0));
        }
    }
}
