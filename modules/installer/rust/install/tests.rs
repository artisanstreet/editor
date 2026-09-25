//! Behavior tests for the installer authority: activation-pointer recovery,
//! the root lock and pending markers, owned removal, and stage leases.

use std::{fs, path::Path};

use sha2::{Digest, Sha256};
use tempfile::tempdir;

use super::{
    InstallerError, InstallerLock, PendingMarker, PendingMarkerKind, RootMode, StageLease,
    complete_install, inspect_activation_pointer, pending_marker_path,
    recover_activation_pointer_swap, remove_path_in_root, remove_validated_activation_pointer,
};

#[cfg(windows)]
use super::prepend_windows_path_entry;

#[cfg(unix)]
fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> bool {
    std::os::windows::fs::symlink_dir(target, link).is_ok()
}

#[cfg(unix)]
fn remove_directory_link(link: &std::path::Path) {
    fs::remove_file(link).expect("directory link");
}

#[cfg(windows)]
fn remove_directory_link(link: &std::path::Path) {
    fs::remove_dir(link).expect("directory link");
}

#[cfg(unix)]
fn create_file_link(target: &std::path::Path, link: &std::path::Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn create_file_link(target: &std::path::Path, link: &std::path::Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

fn activation_document(root: &Path, active_version: &str) -> serde_json::Value {
    serde_json::json!({
        "format_version": 1,
        "install_root": root,
        "activation_state": "active",
        "finalization_state": "complete",
        "active_version": active_version,
        "permanent_ae_path": root.join("bin").join(if cfg!(windows) { "ae.exe" } else { "ae" }),
    })
}

fn activation_bytes(root: &Path, active_version: &str) -> Vec<u8> {
    serde_json::to_vec(&activation_document(root, active_version)).expect("activation JSON")
}

fn assert_ambiguous(error: &InstallerError) {
    assert!(matches!(
        error,
        InstallerError::InstallationActivationTransactionAmbiguous
    ));
    assert_eq!(
        error.to_string(),
        "installation activation transaction is ambiguous; no files were changed"
    );
}

#[test]
fn activation_recovery_without_residue_is_a_no_op() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");

    recover_activation_pointer_swap(&lock).expect("no-residue recovery");

    for path in super::activation_pointer_paths(&root) {
        assert!(path.symlink_metadata().is_err());
    }
}

#[test]
fn activation_recovery_after_crash_before_pointer_swap_preserves_current() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let [current, temporary, previous] = super::activation_pointer_paths(&root);
    let current_bytes = activation_bytes(&root, "1.2.3");
    fs::write(&current, &current_bytes).expect("current pointer");
    fs::write(&temporary, activation_bytes(&root, "2.0.0")).expect("temporary pointer");

    recover_activation_pointer_swap(&lock).expect("pre-swap recovery");

    assert_eq!(fs::read(&current).expect("current bytes"), current_bytes);
    assert!(temporary.symlink_metadata().is_err());
    assert!(previous.symlink_metadata().is_err());
}

#[test]
fn activation_recovery_after_current_to_previous_restores_exact_bytes() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let [current, temporary, previous] = super::activation_pointer_paths(&root);
    let previous_bytes = activation_bytes(&root, "1.2.3");
    fs::write(&previous, &previous_bytes).expect("previous pointer");
    fs::write(&temporary, activation_bytes(&root, "2.0.0")).expect("temporary pointer");

    recover_activation_pointer_swap(&lock).expect("previous recovery");

    assert_eq!(fs::read(&current).expect("restored bytes"), previous_bytes);
    assert!(temporary.symlink_metadata().is_err());
    assert!(previous.symlink_metadata().is_err());
}

#[test]
fn activation_recovery_removes_uncommitted_temporary_without_pointer() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let [current, temporary, previous] = super::activation_pointer_paths(&root);
    fs::write(&temporary, activation_bytes(&root, "2.0.0")).expect("temporary pointer");

    recover_activation_pointer_swap(&lock).expect("temporary-only recovery");

    assert!(current.symlink_metadata().is_err());
    assert!(temporary.symlink_metadata().is_err());
    assert!(previous.symlink_metadata().is_err());
}

#[test]
fn activation_recovery_after_new_current_installation_keeps_new_bytes() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let [current, temporary, previous] = super::activation_pointer_paths(&root);
    let current_bytes = activation_bytes(&root, "2.0.0");
    fs::write(&current, &current_bytes).expect("new current pointer");
    fs::write(&previous, activation_bytes(&root, "1.2.3")).expect("previous pointer");

    recover_activation_pointer_swap(&lock).expect("post-swap recovery");

    assert_eq!(fs::read(&current).expect("current bytes"), current_bytes);
    assert!(temporary.symlink_metadata().is_err());
    assert!(previous.symlink_metadata().is_err());
}

#[test]
fn activation_recovery_is_idempotent_after_first_recovery() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let [current, temporary, previous] = super::activation_pointer_paths(&root);
    let current_bytes = activation_bytes(&root, "1.2.3");
    fs::write(&current, &current_bytes).expect("current pointer");
    fs::write(&temporary, activation_bytes(&root, "2.0.0")).expect("temporary pointer");
    fs::write(&previous, activation_bytes(&root, "0.9.0")).expect("previous pointer");

    recover_activation_pointer_swap(&lock).expect("first recovery");
    recover_activation_pointer_swap(&lock).expect("second recovery");

    assert_eq!(fs::read(&current).expect("current bytes"), current_bytes);
    assert!(temporary.symlink_metadata().is_err());
    assert!(previous.symlink_metadata().is_err());
}

#[test]
fn activation_recovery_rejects_invalid_documents_without_mutation() {
    let directory = tempdir().expect("temp");
    let invalid_root = directory.path().join("different-root");
    for name in [
        "malformed",
        "root-mismatch",
        "pending",
        "non-complete",
        "unsafe-version",
        "unsafe-path",
    ] {
        let root = directory.path().join(format!("case-{name}"));
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let current = root.join("installation.json");
        let bytes = if name == "malformed" {
            b"{".to_vec()
        } else {
            let mut document = activation_document(&root, "1.2.3");
            match name {
                "root-mismatch" => document["install_root"] = serde_json::json!(invalid_root),
                "pending" => document["activation_state"] = serde_json::json!("pending"),
                "non-complete" => document["finalization_state"] = serde_json::json!("pending"),
                "unsafe-version" => document["active_version"] = serde_json::json!("../escape"),
                "unsafe-path" => {
                    document["permanent_ae_path"] = serde_json::json!(invalid_root.join("ae"));
                }
                _ => unreachable!(),
            }
            serde_json::to_vec(&document).expect("invalid state document")
        };
        fs::write(&current, &bytes).expect("invalid current pointer");

        let error = recover_activation_pointer_swap(&lock).expect_err("invalid pointer");
        assert_ambiguous(&error);
        assert_eq!(fs::read(&current).expect("current remains"), bytes);
        assert!(
            root.join(".installation.json.tmp")
                .symlink_metadata()
                .is_err()
        );
        assert!(
            root.join(".installation.json.previous")
                .symlink_metadata()
                .is_err()
        );
    }
}

#[test]
fn activation_recovery_validates_all_residue_before_mutating() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let [current, temporary, previous] = super::activation_pointer_paths(&root);
    let current_bytes = activation_bytes(&root, "1.2.3");
    let previous_bytes = activation_bytes(&root, "0.9.0");
    fs::write(&current, &current_bytes).expect("current pointer");
    fs::write(&temporary, b"malformed").expect("malformed temporary pointer");
    fs::write(&previous, &previous_bytes).expect("previous pointer");

    let error = recover_activation_pointer_swap(&lock).expect_err("ambiguous residue");
    assert_ambiguous(&error);
    assert_eq!(fs::read(&current).expect("current remains"), current_bytes);
    assert_eq!(
        fs::read(&temporary).expect("temporary remains"),
        b"malformed"
    );
    assert_eq!(
        fs::read(&previous).expect("previous remains"),
        previous_bytes
    );
}

#[test]
fn activation_recovery_rejects_links_and_directories_without_mutation() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let target = root.join("foreign-pointer");
    let temporary = root.join(".installation.json.tmp");
    fs::write(&target, activation_bytes(&root, "1.2.3")).expect("foreign pointer");
    if !create_file_link(&target, &temporary) {
        eprintln!("SKIP: file links are not supported on this host");
        return;
    }

    let error = recover_activation_pointer_swap(&lock).expect_err("symlink residue");
    assert_ambiguous(&error);
    assert!(temporary.symlink_metadata().is_ok());
    assert_eq!(
        fs::read(&target).expect("foreign pointer remains"),
        activation_bytes(&root, "1.2.3")
    );

    fs::remove_file(&temporary).expect("remove test link");
    let previous = root.join(".installation.json.previous");
    fs::create_dir(&previous).expect("directory residue");
    let error = recover_activation_pointer_swap(&lock).expect_err("directory residue");
    assert_ambiguous(&error);
    assert!(previous.is_dir());
}

#[cfg(windows)]
#[test]
fn activation_recovery_rejects_windows_reparse_residue_without_mutation() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let target = root.join("foreign-directory");
    let temporary = root.join(".installation.json.tmp");
    fs::create_dir(&target).expect("foreign directory");
    if !create_directory_link(&target, &temporary) {
        eprintln!("SKIP: directory links are not supported on this host");
        return;
    }

    let error = recover_activation_pointer_swap(&lock).expect_err("reparse residue");
    assert_ambiguous(&error);
    assert!(temporary.symlink_metadata().is_ok());
    assert!(target.is_dir());
}

#[test]
fn activation_recovery_rejects_identity_substituted_residue() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let _lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let temporary = root.join(".installation.json.tmp");
    let replacement = root.join("replacement");
    fs::write(&temporary, activation_bytes(&root, "1.2.3")).expect("temporary pointer");
    let validated = inspect_activation_pointer(&root, &temporary)
        .expect("inspect temporary pointer")
        .expect("temporary pointer");
    fs::write(&replacement, activation_bytes(&root, "2.0.0")).expect("replacement");
    fs::remove_file(&temporary).expect("remove original pointer");
    fs::rename(&replacement, &temporary).expect("substitute pointer");

    let error = remove_validated_activation_pointer(&validated).expect_err("identity substitution");
    assert_ambiguous(&error);
    assert!(temporary.is_file());
}

#[test]
fn activation_recovery_preserves_versions_credentials_data_and_spaces() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan install root with spaces");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let versions = root.join("versions").join("1.2.3").join("payload");
    let credentials = root.join("credentials").join("credentials.json");
    let data = root.join("data").join("state.db");
    for path in [&versions, &credentials, &data] {
        fs::create_dir_all(path.parent().expect("parent")).expect("preserved directory");
        fs::write(path, path.to_string_lossy().as_bytes()).expect("preserved file");
    }
    let [current, temporary, previous] = super::activation_pointer_paths(&root);
    let current_bytes = activation_bytes(&root, "1.2.3");
    fs::write(&current, &current_bytes).expect("current pointer");
    fs::write(&temporary, activation_bytes(&root, "2.0.0")).expect("temporary pointer");

    recover_activation_pointer_swap(&lock).expect("space-path recovery");

    assert_eq!(fs::read(&current).expect("current bytes"), current_bytes);
    for path in [&versions, &credentials, &data] {
        assert_eq!(
            fs::read(path).expect("preserved file bytes"),
            path.to_string_lossy().as_bytes()
        );
    }
    assert!(temporary.symlink_metadata().is_err());
    assert!(previous.symlink_metadata().is_err());
}

#[test]
fn installer_lock_serializes_releases_and_preserves_its_sentinel() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("nested").join("Artisan");
    let sentinel = root.join(super::INSTALLER_LOCK_NAME);
    fs::create_dir_all(&root).expect("root");
    fs::write(&sentinel, b"sentinel").expect("sentinel contents");
    let first = InstallerLock::acquire(&root, RootMode::Existing).expect("first lock");

    let contention = InstallerLock::acquire(&root, RootMode::Existing)
        .expect_err("the root lock must be exclusive");
    assert!(matches!(contention, InstallerError::InstallationRootBusy));
    assert_eq!(contention.to_string(), "installation root is busy");
    assert_eq!(format!("{contention:?}"), "InstallationRootBusy");
    assert_eq!(format!("{first:?}"), "InstallerLock");

    drop(first);
    let second = InstallerLock::acquire(&root, RootMode::Existing).expect("released lock");
    drop(second);
    assert_eq!(fs::read(sentinel).expect("sentinel remains"), b"sentinel");
}

#[test]
fn root_creation_rejects_unsafe_shapes_without_following_them() {
    let directory = tempdir().expect("temp");
    let missing_root = directory.path().join("missing-root");
    assert!(matches!(
        InstallerLock::acquire(&missing_root, RootMode::Existing),
        Err(InstallerError::UnsafeInstallationRoot)
    ));
    assert!(!missing_root.exists());

    let file_root = directory.path().join("file-root");
    fs::write(&file_root, b"foreign").expect("file root");
    assert!(matches!(
        InstallerLock::acquire(&file_root, RootMode::Create),
        Err(InstallerError::UnsafeInstallationRoot)
    ));

    let real_parent = directory.path().join("real-parent");
    fs::create_dir(&real_parent).expect("real parent");
    let linked_root = directory.path().join("linked-root");
    if create_directory_link(&real_parent, &linked_root) {
        assert!(matches!(
            InstallerLock::acquire(&linked_root, RootMode::Existing),
            Err(InstallerError::UnsafeInstallationRoot)
        ));
        remove_directory_link(&linked_root);
    }
    let linked_parent = directory.path().join("linked-parent");
    if !create_directory_link(&real_parent, &linked_parent) {
        eprintln!("SKIP: directory links are not supported on this host");
        return;
    }
    let unsafe_root = linked_parent.join("root");
    assert!(matches!(
        InstallerLock::acquire(&unsafe_root, RootMode::Create),
        Err(InstallerError::UnsafeInstallationRoot)
    ));
    assert!(!unsafe_root.exists());
}

#[test]
fn lock_and_pending_marker_shapes_fail_closed_and_stay_untouched() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    fs::create_dir(&root).expect("root");
    let lock_path = root.join(super::INSTALLER_LOCK_NAME);
    fs::create_dir(&lock_path).expect("lock directory");
    assert!(matches!(
        InstallerLock::acquire(&root, RootMode::Existing),
        Err(InstallerError::InvalidInstallerLock)
    ));
    fs::remove_dir(&lock_path).expect("lock directory");
    let lock_target = directory.path().join("foreign-lock");
    fs::write(&lock_target, b"foreign").expect("foreign lock target");
    if create_file_link(&lock_target, &lock_path) {
        assert!(matches!(
            InstallerLock::acquire(&root, RootMode::Existing),
            Err(InstallerError::InvalidInstallerLock)
        ));
        fs::remove_file(&lock_path).expect("lock link");
    }

    let marker = pending_marker_path(&root, PendingMarkerKind::Cleanup).expect("marker path");
    fs::write(&marker, b"foreign marker").expect("foreign marker file");
    assert!(matches!(
        InstallerLock::acquire(&root, RootMode::Existing),
        Err(InstallerError::InvalidInstallerMarker)
    ));
    fs::remove_file(&marker).expect("foreign marker file");
    fs::create_dir(&marker).expect("foreign marker");
    let error = InstallerLock::acquire(&root, RootMode::Existing)
        .expect_err("pending marker must fence the root");
    assert!(matches!(error, InstallerError::InstallationRootPending));
    assert!(marker.is_dir());
    assert_eq!(
        error.to_string(),
        "installation root has a pending operation"
    );
}

#[test]
fn pending_markers_collide_atomically_and_clear_only_explicitly() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let marker = PendingMarker::create(&lock, PendingMarkerKind::AeReplacement).expect("marker");
    assert!(marker.path.is_dir());
    assert_eq!(format!("{marker:?}"), "PendingMarker");
    assert!(matches!(
        PendingMarker::create(&lock, PendingMarkerKind::Cleanup),
        Err(InstallerError::InstallationRootPending)
    ));
    let marker_path = marker.path.clone();
    drop(marker);
    assert!(marker_path.is_dir());
    let collision = PendingMarker::create(&lock, PendingMarkerKind::AeReplacement)
        .expect_err("the existing marker remains a collision");
    assert!(matches!(collision, InstallerError::InstallationRootPending));
    let marker = PendingMarker {
        identity: super::ordinary_path_identity(&marker_path, super::EntryKind::Directory)
            .expect("marker identity"),
        path: marker_path,
    };
    marker
        .clear_after_success()
        .expect("helper success cleanup");
    assert!(
        !pending_marker_path(&root, PendingMarkerKind::AeReplacement)
            .expect("marker path")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn lock_rejects_path_identity_substitution() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
    let sentinel = root.join(super::INSTALLER_LOCK_NAME);
    let moved = root.join("moved-lock");
    fs::rename(&sentinel, &moved).expect("move sentinel");
    fs::write(&sentinel, b"replacement").expect("replacement sentinel");

    assert!(matches!(
        lock.re_fence(),
        Err(InstallerError::InstallationRootChanged)
    ));
    assert_eq!(
        fs::read(&sentinel).expect("replacement remains"),
        b"replacement"
    );
    assert_eq!(fs::read(moved).expect("original remains open"), b"sentinel");
}

#[test]
fn recursive_owned_removal_preserves_a_foreign_link_and_target() {
    let directory = tempdir().expect("temp");
    let root = directory.path().join("Artisan");
    let foreign = directory.path().join("foreign");
    fs::create_dir(&root).expect("root");
    fs::create_dir(&foreign).expect("foreign");
    fs::write(foreign.join("keep"), b"keep").expect("foreign contents");
    let link = root.join("data");
    if !create_directory_link(&foreign, &link) {
        eprintln!("SKIP: directory links are not supported on this host");
        return;
    }

    let error = remove_path_in_root(&root, &link).expect_err("foreign link is unsafe");
    assert!(matches!(error, InstallerError::UnsafeOwnedPath));
    assert!(link.symlink_metadata().is_ok());
    assert_eq!(
        fs::read(foreign.join("keep")).expect("foreign target"),
        b"keep"
    );
}

#[test]
fn sha256_representation_matches_release_contract() {
    let mut hasher = Sha256::new();
    hasher.update(b"artisan");
    assert_eq!(
        hex::encode(hasher.finalize()),
        "0b74ed7ff22b86fd0838fd29a78940a8d54377951e968867948a57b3e53646fc"
    );
}

#[test]
fn permanent_lifecycle_binary_has_a_stable_archive_location() {
    let root = tempdir().expect("temp");
    let release = root.path().join("versions").join("1.2.3");
    let expected = root
        .path()
        .join("versions")
        .join("1.2.3")
        .join("bin")
        .join(if cfg!(windows) {
            "installer.exe"
        } else {
            "installer"
        });
    assert_eq!(super::versioned_installer_path(&release), expected);
    assert!(!expected.ends_with(if cfg!(windows) {
        "ae-installer.exe"
    } else {
        "ae-installer"
    }));
}

#[test]
fn first_install_leaves_forge_launch_to_the_editor_handoff() {
    assert_eq!(super::FIRST_RUN_CONFIGURATION_COMMANDS[0], ["setup"]);
    assert!(
        super::FIRST_RUN_CONFIGURATION_COMMANDS
            .iter()
            .flat_map(|arguments| arguments.iter())
            .all(|argument| *argument != "start")
    );
    assert!(!super::should_restore_retired_forge(
        true,
        super::Retirement {
            editors_closed: 1,
            forges_stopped: 1,
        },
    ));
}

#[test]
fn maintenance_update_restores_a_previously_running_forge() {
    assert!(super::should_restore_retired_forge(
        false,
        super::Retirement {
            editors_closed: 0,
            forges_stopped: 1,
        },
    ));
    assert!(!super::should_restore_retired_forge(
        false,
        super::Retirement::default(),
    ));
}

#[test]
fn installation_manifest_components_are_always_enabled() {
    assert_eq!(
        serde_json::to_value(super::installed_components()).expect("component projection"),
        serde_json::json!({"editor": true, "forge": true})
    );
}

#[test]
fn failed_install_removes_only_its_owned_stage() {
    let root = tempdir().expect("temp");
    let stage = root.path().join(".stage-1.2.3-owned");
    let sibling = root.path().join(".stage-1.2.3-sibling");
    let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
    fs::create_dir(&sibling).expect("sibling stage");
    fs::write(stage.join("partial"), b"partial payload").expect("partial payload");

    let result = complete_install(
        &mut lease,
        Err(InstallerError::Archive(
            "post-acquisition failure".to_owned(),
        )),
    );

    assert!(matches!(
        result,
        Err(InstallerError::Archive(message)) if message == "post-acquisition failure"
    ));
    assert!(!stage.exists());
    assert!(sibling.is_dir());
}

#[test]
fn pre_existing_stage_collision_is_rejected_and_untouched() {
    let root = tempdir().expect("temp");
    let stage = root.path().join(".stage-1.2.3-owned");
    fs::create_dir(&stage).expect("pre-existing stage");
    let marker = stage.join("marker");
    fs::write(&marker, b"keep").expect("collision marker");

    let result = StageLease::acquire(stage.clone(), "1.2.3");

    assert!(matches!(
        result,
        Err(InstallerError::ExistingRelease(version)) if version == "1.2.3"
    ));
    assert!(stage.is_dir());
    assert_eq!(fs::read(marker).expect("collision marker"), b"keep");
}

#[test]
fn missing_owned_stage_cleanup_is_idempotent() {
    let root = tempdir().expect("temp");
    let stage = root.path().join(".stage-1.2.3-owned");
    let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
    fs::remove_dir(&stage).expect("remove stage before cleanup");

    assert!(lease.cleanup().is_ok());
    assert!(!lease.armed);
    assert!(lease.cleanup().is_ok());
}

#[test]
fn cleanup_refuses_a_regular_file_target() {
    let root = tempdir().expect("temp");
    let stage = root.path().join(".stage-1.2.3-owned");
    let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
    fs::remove_dir(&stage).expect("remove stage before replacement");
    fs::write(&stage, b"do not remove").expect("file replacement");

    let result = lease.cleanup();

    assert!(matches!(
        result,
        Err(InstallerError::StageCleanupIncomplete)
    ));
    assert_eq!(fs::read(stage).expect("file target"), b"do not remove");
}

#[test]
fn cleanup_refuses_a_link_or_reparse_target() {
    let root = tempdir().expect("temp");
    let target = root.path().join("target");
    fs::create_dir(&target).expect("link target");
    let stage = root.path().join(".stage-1.2.3-owned");
    let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
    fs::remove_dir(&stage).expect("remove stage before replacement");
    if !create_directory_link(&target, &stage) {
        eprintln!("SKIP: directory links are not supported on this host");
        return;
    }

    let result = lease.cleanup();

    assert!(matches!(
        result,
        Err(InstallerError::StageCleanupIncomplete)
    ));
    assert!(stage.symlink_metadata().is_ok());
    assert!(target.is_dir());
}

#[test]
fn cleanup_failure_takes_precedence_and_is_path_free() {
    let root = tempdir().expect("temp");
    let stage = root.path().join(".stage-1.2.3-owned");
    let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
    fs::remove_dir(&stage).expect("remove stage before replacement");
    fs::write(&stage, b"preserve").expect("file replacement");
    let original = format!("original failure at {}", stage.display());

    let error = complete_install(&mut lease, Err(InstallerError::Archive(original)))
        .expect_err("cleanup failure");

    assert!(matches!(error, InstallerError::StageCleanupIncomplete));
    assert_eq!(error.to_string(), "staging cleanup could not be completed");
    assert!(!error.to_string().contains(&stage.display().to_string()));
    assert_eq!(fs::read(stage).expect("file target"), b"preserve");
}

#[test]
fn successful_transfer_disarms_lease_before_later_failure() {
    let root = tempdir().expect("temp");
    let stage = root.path().join(".stage-1.2.3-owned");
    let release_parent = root.path().join("versions");
    let release = release_parent.join("1.2.3");
    fs::create_dir(&release_parent).expect("release parent");
    let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
    fs::write(stage.join("payload"), b"release payload").expect("payload");

    lease.transfer_to(&release).expect("stage transfer");
    let result = complete_install(
        &mut lease,
        Err(InstallerError::Archive("activation failure".to_owned())),
    );

    assert!(matches!(
        result,
        Err(InstallerError::Archive(message)) if message == "activation failure"
    ));
    assert!(!lease.armed);
    assert!(!stage.exists());
    assert_eq!(
        fs::read(release.join("payload")).expect("release payload"),
        b"release payload"
    );
}

#[cfg(windows)]
#[test]
fn stable_cli_precedes_stale_path_entries() {
    let stable = r"C:\Users\test\AppData\Local\Artisan\bin";
    let legacy = r"C:\Users\test\AppData\Local\Programs\artisan-editor\resources\artisan-forge";

    assert_eq!(
        prepend_windows_path_entry(&format!("{legacy};{stable};C:\\Windows"), stable),
        format!("{stable};{legacy};C:\\Windows")
    );
}
