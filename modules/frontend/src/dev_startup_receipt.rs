//! Opt-in development startup receipt for the native transport service.
//!
//! Without [`STARTUP_RECEIPT_ENV`] this module is a no-op and production
//! behavior is unchanged. When the variable names an absolute file, the
//! service writes one small JSON document there the moment startup reaches
//! authenticated initial-query completion (`ready`), or the moment startup
//! fails (`failed` with the secret-free [`ServiceFailure`] stage and
//! category). The `dev` launcher waits for this receipt instead of probing
//! the Forge itself, so no bootstrap credential is ever consumed outside
//! the owned session. Receipt content never carries secrets: stages,
//! categories, and paths are fixed or finite by construction.

#![forbid(unsafe_code)]

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use crate::native_transport_service::ServiceFailure;

/// Environment variable selecting the startup receipt file.
///
/// When unset or empty, no receipt is written. When set, it must name an
/// absolute file whose parent directory already exists; anything else is
/// refused silently so a misconfigured launcher can never redirect or
/// crash the shipping service.
pub const STARTUP_RECEIPT_ENV: &str = "ARTISAN_DEV_STARTUP_RECEIPT";

/// Schema marker written into every receipt document.
pub const STARTUP_RECEIPT_SCHEMA: &str = "artisan-dev-startup-v1";

/// Stage recorded when the initial authenticated queries complete.
pub const STARTUP_READY_STAGE: &str = "initial-catalog";

/// Selects the receipt file from one environment value.
///
/// Returns `None` when the variable is unset or empty. A set value passes
/// through verbatim; [`usable_receipt_path`] rejects it later when it is
/// not an absolute file with an existing parent.
#[must_use]
pub fn receipt_path_from_value(value: Option<&OsStr>) -> Option<PathBuf> {
    value.and_then(|value| {
        if value.is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    })
}

/// Reads [`STARTUP_RECEIPT_ENV`] from the process environment.
///
/// See [`receipt_path_from_value`] for the selection contract.
#[must_use]
pub fn receipt_path_from_env() -> Option<PathBuf> {
    receipt_path_from_value(std::env::var_os(STARTUP_RECEIPT_ENV).as_deref())
}

/// Returns whether a receipt path is usable.
///
/// Only absolute files with an existing parent directory qualify. Missing
/// parents, relative paths, and empty values report unusable without
/// failing: the caller simply skips the receipt.
#[must_use]
pub fn usable_receipt_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty() && parent.is_dir())
}

/// Renders one receipt document.
///
/// The `detail` is always a fixed call-site string or a [`ServiceFailure`]
/// rendering, both finite and secret-free.
#[must_use]
pub fn render_receipt(status: &str, stage: &str, detail: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema": STARTUP_RECEIPT_SCHEMA,
        "status": status,
        "stage": stage,
        "detail": detail,
    }))
    .unwrap_or_default()
}

/// Writes receipt bytes atomically through a sibling temporary plus rename.
///
/// Best-effort by design: every I/O failure is swallowed so reporting can
/// never disturb the service it observes.
pub fn write_receipt_bytes(path: &Path, bytes: &[u8]) {
    let Some(parent) = path.parent() else {
        return;
    };
    if !parent.is_dir() {
        return;
    }
    let temporary = path.with_extension("tmp-dev-receipt");
    if std::fs::write(&temporary, bytes).is_err() {
        return;
    }
    let _ = std::fs::rename(&temporary, path);
}

/// Reports authenticated initial-query completion.
///
/// No-op unless [`STARTUP_RECEIPT_ENV`] selects a usable file.
pub fn report_ready() {
    let Some(path) = receipt_path_from_env() else {
        return;
    };
    if !usable_receipt_path(&path) {
        return;
    }
    write_receipt_bytes(
        &path,
        &render_receipt(
            "ready",
            STARTUP_READY_STAGE,
            "authenticated and initial queries complete",
        ),
    );
}

/// Reports a startup failure with its secret-free stage and category.
///
/// No-op unless [`STARTUP_RECEIPT_ENV`] selects a usable file.
pub fn report_failed(failure: ServiceFailure) {
    let Some(path) = receipt_path_from_env() else {
        return;
    };
    if !usable_receipt_path(&path) {
        return;
    }
    write_receipt_bytes(
        &path,
        &render_receipt("failed", &failure.stage.to_string(), &failure.to_string()),
    );
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{
        STARTUP_READY_STAGE, STARTUP_RECEIPT_ENV, STARTUP_RECEIPT_SCHEMA, receipt_path_from_value,
        render_receipt, usable_receipt_path, write_receipt_bytes,
    };

    #[test]
    fn env_contract_name_is_exact() {
        assert_eq!(STARTUP_RECEIPT_ENV, "ARTISAN_DEV_STARTUP_RECEIPT");
    }

    #[test]
    fn unset_or_empty_value_means_disabled() {
        assert_eq!(receipt_path_from_value(None), None);
        assert_eq!(receipt_path_from_value(Some(OsStr::new(""))), None);
    }

    #[test]
    fn set_values_pass_through_for_validation() {
        let relative = std::path::PathBuf::from("relative/receipt.json");
        assert_eq!(
            receipt_path_from_value(Some(relative.as_os_str())),
            Some(relative)
        );
    }

    #[test]
    fn only_absolute_files_with_existing_parents_are_usable() {
        let directory =
            std::env::temp_dir().join(format!("artisan-startup-receipt-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("probe dir");
        assert!(usable_receipt_path(&directory.join("receipt.json")));
        assert!(!usable_receipt_path(std::path::Path::new(
            "relative/receipt.json"
        )));
        assert!(!usable_receipt_path(
            &directory.join("missing-parent").join("receipt.json")
        ));
        std::fs::remove_dir_all(&directory).expect("probe cleanup");
    }

    #[test]
    fn rendered_receipts_carry_schema_status_and_stage() {
        let bytes = render_receipt("ready", STARTUP_READY_STAGE, "detail");
        let document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(document["schema"].as_str(), Some(STARTUP_RECEIPT_SCHEMA));
        assert_eq!(document["status"].as_str(), Some("ready"));
        assert_eq!(document["stage"].as_str(), Some(STARTUP_READY_STAGE));
    }

    #[test]
    fn atomic_write_activates_the_receipt() {
        let directory = std::env::temp_dir().join(format!(
            "artisan-startup-receipt-write-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("probe dir");
        let path = directory.join("receipt.json");
        write_receipt_bytes(&path, b"{\"schema\":\"probe\"}");
        assert_eq!(
            std::fs::read(&path).expect("reads"),
            b"{\"schema\":\"probe\"}"
        );
        assert!(!path.with_extension("tmp-dev-receipt").exists());
        std::fs::remove_dir_all(&directory).expect("probe cleanup");
    }

    #[test]
    fn writes_without_existing_parents_are_dropped() {
        let path = std::env::temp_dir()
            .join("artisan-startup-receipt-absent")
            .join("receipt.json");
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
        write_receipt_bytes(&path, b"{}");
        assert!(!path.exists());
    }
}
