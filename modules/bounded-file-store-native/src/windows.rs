use std::{
    mem::{offset_of, size_of, size_of_val},
    slice,
    sync::Mutex,
    thread,
    time::Duration,
};

mod receipt;
mod test_hook;

use napi::Result;
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_BASIC_INFORMATION, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_LINK_INFORMATION,
            FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
            FILE_SYNCHRONOUS_IO_NONALERT, FileBasicInformation, FileDispositionInformationEx,
            FileLinkInformationEx, FileRenameInformationEx, NtCreateFile, NtFlushBuffersFile,
            NtSetInformationFile, RtlDosPathNameToNtPathName_U_WithStatus,
        },
    },
    Win32::{
        Foundation::{
            CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE,
            LocalFree, NTSTATUS, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, STATUS_ACCESS_DENIED,
            STATUS_CANT_WAIT, STATUS_DELETE_PENDING, STATUS_FILE_DELETED, STATUS_LOCK_NOT_GRANTED,
            STATUS_NO_SUCH_FILE, STATUS_OBJECT_NAME_COLLISION, STATUS_OBJECT_NAME_EXISTS,
            STATUS_OBJECT_NAME_NOT_FOUND, STATUS_OBJECT_PATH_NOT_FOUND, STATUS_OPLOCK_NOT_GRANTED,
            STATUS_RETRY, STATUS_SHARING_VIOLATION, STATUS_USER_MAPPED_FILE, UNICODE_STRING,
        },
        Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, SDDL_REVISION_1,
                SE_FILE_OBJECT, SetSecurityInfo,
            },
            DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetKernelObjectSecurity,
            GetSecurityDescriptorDacl, OWNER_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED, SECURITY_DESCRIPTOR,
            UNPROTECTED_DACL_SECURITY_INFORMATION,
        },
        Storage::FileSystem::{
            DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_BASIC_INFO, FILE_BEGIN, FILE_DISPOSITION_FLAG_DELETE,
            FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
            FILE_DISPOSITION_INFO_EX, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_ID_INFO,
            FILE_NAME_NORMALIZED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_STANDARD_INFO, FILE_TYPE_DISK, FILE_WRITE_EA, FileBasicInfo, FileIdInfo,
            FileStandardInfo, FlushFileBuffers, GetDriveTypeW, GetFileInformationByHandleEx,
            GetFileType, GetFinalPathNameByHandleW, GetVolumeInformationByHandleW,
            GetVolumePathNameW, ReadFile, SetFilePointerEx, VOLUME_NAME_NONE, WRITE_DAC, WriteFile,
        },
        System::{
            IO::IO_STATUS_BLOCK,
            Threading::GetCurrentProcess,
            WindowsProgramming::{DRIVE_FIXED, RtlFreeUnicodeString},
        },
    },
};
use zeroize::{Zeroize, Zeroizing};

use super::{ReplaceRegularFileOptions, native_error};
use receipt::{Marker, ReceiptRole};

const MAXIMUM_NT_PATH_UNITS: usize = u16::MAX as usize / 2;
const MAXIMUM_SECURITY_DESCRIPTOR_BYTES: usize = 1024 * 1024;
const SECURITY_INFORMATION: u32 =
    OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
const ORDINARY_ATTRIBUTES_MASK: u32 = !(FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT);

pub(crate) struct RootHandle {
    handle: HANDLE,
    receipt_key: [u8; 32],
    mutation: Mutex<()>,
}

/** The owned root handle may move with its Arc lease between N-API task threads. */
unsafe impl Send for RootHandle {}

/** Root access is immutable, while mutation admission is protected by the mutex. */
unsafe impl Sync for RootHandle {}

impl Drop for RootHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
        self.receipt_key.zeroize();
    }
}

struct Handle(HANDLE);

/** An owned kernel handle may move between Rust threads without changing ownership. */
unsafe impl Send for Handle {}

/** Windows synchronizes handle operations, and this wrapper never mutates the raw value. */
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

struct OpenedFile {
    file: Handle,
    parents: Vec<Handle>,
}

#[derive(Clone, PartialEq)]
struct Metadata {
    volume: u64,
    id: [u8; 16],
    creation: i64,
    write: i64,
    change: i64,
    attributes: u32,
    size: u64,
    links: u32,
    directory: bool,
    reparse: bool,
}

#[derive(Clone)]
struct SecurityDescriptorBuffer {
    canonical: Vec<u16>,
    storage: Vec<usize>,
    length: usize,
}

struct LocalSecurityString(*mut u16);

impl Drop for LocalSecurityString {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0 as _) };
    }
}

impl SecurityDescriptorBuffer {
    fn as_bytes(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.storage.as_ptr() as *const u8, self.length) }
    }

    fn as_ptr(&self) -> *const SECURITY_DESCRIPTOR {
        self.storage.as_ptr() as _
    }

    fn control(&self) -> std::result::Result<u16, AttemptError> {
        let bytes = self.as_bytes();

        if bytes.len() < 4 {
            return Err(Failed);
        }

        Ok(u16::from_le_bytes([bytes[2], bytes[3]]))
    }
}

impl PartialEq for SecurityDescriptorBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.canonical == other.canonical
    }
}

#[derive(Clone, PartialEq)]
struct SnapshotData {
    metadata: Metadata,
    bytes: Vec<u8>,
    security: SecurityDescriptorBuffer,
}

struct Snapshot {
    opened: OpenedFile,
    data: SnapshotData,
}

struct ArtifactNames {
    stage_path: String,
    backup_path: String,
    stage_name: String,
    backup_name: String,
}

#[derive(Clone, Copy)]
pub(crate) enum ReplaceOutcome {
    Replaced,
    AlreadyReplaced,
    Changed,
}

impl ReplaceOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Replaced => "Replaced",
            Self::AlreadyReplaced => "AlreadyReplaced",
            Self::Changed => "Changed",
        }
    }
}

#[derive(Clone, Copy)]
enum AttemptError {
    Transient,
    Failed,
}

use AttemptError::{Failed, Transient};

#[derive(Clone, Copy)]
enum OpenPathError {
    Missing,
    Transient,
    Failed,
}

#[derive(Clone, Copy)]
enum MutationOpenMode {
    Exclusive,
    CompatibleRead,
}

#[derive(Clone, Copy)]
enum NameMutation {
    Applied,
    Collision,
}

#[derive(Clone, Copy)]
enum NtStatusKind {
    Missing,
    Collision,
    Transient,
    Permanent,
}

pub(crate) fn open_root(
    root_directory: &str,
    receipt_authentication_key: [u8; 32],
) -> Result<RootHandle> {
    let receipt_authentication_key = Zeroizing::new(receipt_authentication_key);

    if !is_absolute_drive_path(root_directory)
        || root_directory.contains('\0')
        || root_directory.encode_utf16().count() > MAXIMUM_NT_PATH_UNITS
    {
        return Err(native_error("native file store root is invalid"));
    }

    let mut root_path: Vec<u16> = root_directory.encode_utf16().chain(Some(0)).collect();

    if !is_fixed_volume(&root_path) {
        return Err(native_error("native file store root is unsupported"));
    }

    let mut nt_path = UNICODE_STRING::default();
    let status = unsafe {
        RtlDosPathNameToNtPathName_U_WithStatus(
            root_path.as_mut_ptr(),
            &mut nt_path,
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };

    if !nt_success(status) {
        return Err(native_error("native file store root is invalid"));
    }

    let opened = open_nt_file(
        &nt_path,
        std::ptr::null_mut(),
        FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        FILE_OPEN,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE,
        None,
        std::ptr::null(),
    );

    unsafe { RtlFreeUnicodeString(&mut nt_path) };

    let handle = opened.map_err(|_| native_error("native file store root is unavailable"))?;
    let mut root = RootHandle {
        handle,
        receipt_key: [0; 32],
        mutation: Mutex::new(()),
    };
    let metadata = file_metadata(root.handle)?;

    if metadata.reparse || !metadata.directory || !is_ntfs(root.handle) {
        return Err(native_error("native file store root is unsupported"));
    }

    root.receipt_key =
        receipt::derive_root_key(&receipt_authentication_key, metadata.volume, metadata.id);

    Ok(root)
}

pub(crate) fn read_regular_file(
    root: &RootHandle,
    relative_path: &str,
    maximum_bytes: u32,
) -> Result<Vec<u8>> {
    let opened = open_relative_file(
        root,
        relative_path,
        FILE_GENERIC_READ,
        FILE_SHARE_READ,
        false,
    )
    .map_err(|_| native_error("native file read failed"))?;
    let before = file_metadata(opened.file.0)?;

    if before.directory
        || before.reparse
        || before.links != 1
        || before.size > maximum_bytes as u64
        || opened_name_is_private(opened.file.0)?
    {
        return Err(native_error("native file read rejected"));
    }

    let bytes = read_exact_from_start(opened.file.0, before.size as usize)
        .map_err(|_| native_error("native file read failed"))?;
    let after = file_metadata(opened.file.0)?;

    if before != after {
        return Err(native_error("native file changed during read"));
    }

    Ok(bytes)
}

pub(crate) fn replace_regular_file(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
) -> Result<ReplaceOutcome> {
    test_hook::wait_for_replace_race()
        .map_err(|_| native_error("native test rendezvous failed"))?;
    test_hook::trace("start:replace");

    let _mutation = root
        .mutation
        .lock()
        .map_err(|_| native_error("native file store is unavailable"))?;
    let names = artifact_names(options);

    for attempt in 0..20 {
        match replace_once(root, options, &names) {
            Ok(outcome) => return Ok(outcome),
            Err(Transient) if attempt < 19 => thread::sleep(Duration::from_millis(5)),
            Err(Transient) => {
                return Err(native_error("native file replacement did not converge"));
            }
            Err(Failed) => return Err(native_error("native file replacement failed")),
        }
    }

    Err(native_error("native file replacement did not converge"))
}

pub(crate) fn finalize_regular_file_replacement(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
) -> Result<()> {
    test_hook::trace("start:finalize");

    let _mutation = root
        .mutation
        .lock()
        .map_err(|_| native_error("native file store is unavailable"))?;
    let names = artifact_names(options);

    for attempt in 0..20 {
        match finalize_once(root, options, &names) {
            Ok(()) => {
                test_hook::trace("done:finalize");

                return Ok(());
            }
            Err(Transient) if attempt < 19 => thread::sleep(Duration::from_millis(5)),
            Err(Transient) => {
                return Err(native_error("native file finalization did not converge"));
            }
            Err(Failed) => return Err(native_error("native file finalization failed")),
        }
    }

    Err(native_error("native file finalization did not converge"))
}

fn replace_once(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
    names: &ArtifactNames,
) -> std::result::Result<ReplaceOutcome, AttemptError> {
    let stage = open_mutation_snapshot_optional(
        root,
        &names.stage_path,
        options.maximum_bytes,
        MutationOpenMode::Exclusive,
        true,
    )?;
    let backup = open_mutation_snapshot_optional(
        root,
        &names.backup_path,
        options.maximum_bytes,
        MutationOpenMode::Exclusive,
        true,
    )?;

    match (stage, backup) {
        (None, None) => {
            test_hook::trace("state:receipt-none");
            replace_without_receipt(root, options, names)
        }
        (Some(stage), None) => {
            test_hook::trace("state:receipt-stage-only");
            replace_stage_only(root, options, names, stage)
        }
        (None, Some(backup)) => {
            test_hook::trace("state:receipt-backup-only");
            recover_backup_only(root, options, backup)
        }
        (Some(stage), Some(backup)) => {
            test_hook::trace("state:receipt-complete");
            replace_complete_receipt(root, options, stage, backup)
        }
    }
}

fn replace_without_receipt(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
    names: &ArtifactNames,
) -> std::result::Result<ReplaceOutcome, AttemptError> {
    let target = open_mutation_snapshot_optional(
        root,
        &options.path,
        options.maximum_bytes,
        MutationOpenMode::Exclusive,
        false,
    )?;
    let Some(mut target) = target else {
        return Ok(ReplaceOutcome::Changed);
    };
    test_hook::trace("done:target-opened");

    if let Some(marker) = read_valid_marker(root, &target, options)? {
        match marker.role {
            ReceiptRole::Finalizing
                if valid_target_only_marker(&target, &marker, options, true) =>
            {
                clear_marker_and_flush(target.opened.file.0, parent_handle(&target.opened, root))?;

                return Ok(ReplaceOutcome::AlreadyReplaced);
            }
            ReceiptRole::Restoring
                if valid_target_only_marker(&target, &marker, options, false) =>
            {
                clear_marker_and_flush(target.opened.file.0, parent_handle(&target.opened, root))?;

                return Ok(ReplaceOutcome::Changed);
            }
            _ => return Err(Failed),
        }
    }

    if target.data.bytes != options.expected {
        return Ok(ReplaceOutcome::Changed);
    }

    if options.expected == options.replacement {
        return Ok(ReplaceOutcome::AlreadyReplaced);
    }

    if target.data.metadata.links != 1 {
        return Ok(ReplaceOutcome::Changed);
    }
    test_hook::trace("done:target-verified");

    let original = target.data.clone();
    test_hook::trace("start:create-stage");
    let stage = create_stage(
        root,
        &options.path,
        &names.stage_path,
        &names.stage_name,
        &target,
        options,
    )?;
    test_hook::trace("done:create-stage");

    refresh_snapshot(&mut target, options.maximum_bytes, false)?;
    test_hook::trace("done:target-refreshed");

    if target.data != original {
        let stage_marker = read_valid_marker(root, &stage, options)?.ok_or(Failed)?;

        if stage_marker.role != ReceiptRole::Stage
            || !marker_peer_matches(&stage_marker, &original.metadata)
        {
            return Err(Failed);
        }

        delete_snapshot(root, stage)?;

        return Ok(ReplaceOutcome::Changed);
    }

    match rename_no_replace(
        target.opened.file.0,
        parent_handle(&target.opened, root),
        &names.backup_name,
    )? {
        NameMutation::Applied => {}
        NameMutation::Collision => return Err(Transient),
    }
    test_hook::trace("done:target-renamed");

    test_hook::crash_at("backup-renamed");

    write_marker(
        root,
        &target,
        ReceiptRole::Backup,
        stage.data.metadata.volume,
        stage.data.metadata.id,
        options,
    )?;
    test_hook::trace("done:backup-marker-written");

    flush_parent(parent_handle(&target.opened, root));
    refresh_snapshot(&mut target, options.maximum_bytes, true)?;
    test_hook::trace("done:backup-refreshed");
    test_hook::crash_at("backup-marked");

    if target.data.bytes != options.expected
        || target.data.metadata.links != 1
        || !same_identity(&target.data.metadata, &original.metadata)
        || !same_receipt_metadata(&stage, &target)
    {
        return Err(Failed);
    }

    publish(root, options, stage, target)
}

fn replace_stage_only(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
    names: &ArtifactNames,
    stage: Snapshot,
) -> std::result::Result<ReplaceOutcome, AttemptError> {
    let stage_marker = read_valid_marker(root, &stage, options)?.ok_or(Failed)?;

    if stage_marker.role == ReceiptRole::CreatingStage {
        if !valid_creating_stage(&stage, &stage_marker) {
            return Err(Failed);
        }

        delete_snapshot(root, stage)?;

        return replace_without_receipt(root, options, names);
    }

    if !matches!(
        stage_marker.role,
        ReceiptRole::Stage | ReceiptRole::Finalizing
    ) || stage.data.bytes != options.replacement
    {
        return Err(Failed);
    }

    let target = open_mutation_snapshot_optional(
        root,
        &options.path,
        options.maximum_bytes,
        MutationOpenMode::CompatibleRead,
        false,
    )?;
    let Some(target) = target else {
        return Err(Failed);
    };

    if same_identity(&stage.data.metadata, &target.data.metadata) {
        return match stage_marker.role {
            ReceiptRole::Finalizing => {
                complete_finalization(root, options, stage, None, target, stage_marker)?;

                Ok(ReplaceOutcome::AlreadyReplaced)
            }
            _ => Err(Failed),
        };
    }

    if stage_marker.role != ReceiptRole::Stage || stage.data.metadata.links != 1 {
        return Err(Failed);
    }

    if read_valid_marker(root, &target, options)?.is_some() {
        return Err(Failed);
    }

    if !marker_peer_matches(&stage_marker, &target.data.metadata) {
        delete_snapshot(root, stage)?;

        return Ok(ReplaceOutcome::Changed);
    }

    if target.data.bytes != options.expected {
        delete_snapshot(root, stage)?;

        return Ok(ReplaceOutcome::Changed);
    }

    if stage.data.metadata.links != 1
        || target.data.metadata.links != 1
        || !same_receipt_metadata(&stage, &target)
    {
        return Err(Failed);
    }

    let target_proof = target.data.clone();
    drop(target);

    let target = open_mutation_snapshot_optional(
        root,
        &options.path,
        options.maximum_bytes,
        MutationOpenMode::Exclusive,
        false,
    )?
    .ok_or(Transient)?;

    if target.data != target_proof {
        return Err(Transient);
    }

    if read_valid_marker(root, &target, options)?.is_some()
        || !marker_peer_matches(&stage_marker, &target.data.metadata)
    {
        return Err(Failed);
    }

    match rename_no_replace(
        target.opened.file.0,
        parent_handle(&target.opened, root),
        &names.backup_name,
    )? {
        NameMutation::Applied => {}
        NameMutation::Collision => return Err(Transient),
    }

    test_hook::crash_at("backup-renamed");

    let mut backup = target;
    write_marker(
        root,
        &backup,
        ReceiptRole::Backup,
        stage.data.metadata.volume,
        stage.data.metadata.id,
        options,
    )?;
    flush_parent(parent_handle(&backup.opened, root));
    refresh_snapshot(&mut backup, options.maximum_bytes, true)?;
    test_hook::crash_at("backup-marked");
    let backup_marker = read_valid_marker(root, &backup, options)?.ok_or(Failed)?;

    if !valid_unpublished_receipt(&stage, &stage_marker, &backup, &backup_marker, options) {
        return Err(Failed);
    }

    publish(root, options, stage, backup)
}

fn replace_complete_receipt(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
    stage: Snapshot,
    backup: Snapshot,
) -> std::result::Result<ReplaceOutcome, AttemptError> {
    let stage_marker = read_valid_marker(root, &stage, options)?.ok_or(Failed)?;
    let mut backup_marker = read_valid_marker(root, &backup, options)?;

    if stage_marker.role == ReceiptRole::Stage && backup_marker.is_none() {
        if !valid_unmarked_backup_adoption(&stage, &stage_marker, &backup, options) {
            return Err(Failed);
        }

        write_marker(
            root,
            &backup,
            ReceiptRole::Backup,
            stage.data.metadata.volume,
            stage.data.metadata.id,
            options,
        )?;
        backup_marker = read_valid_marker(root, &backup, options)?;
    }

    let backup_marker = backup_marker.ok_or(Failed)?;
    let target = open_mutation_snapshot_optional(
        root,
        &options.path,
        options.maximum_bytes,
        MutationOpenMode::CompatibleRead,
        false,
    )?;

    if stage_marker.role == ReceiptRole::Finalizing {
        let target = target.ok_or(Failed)?;

        complete_finalization(
            root,
            options,
            stage,
            Some((backup, backup_marker)),
            target,
            stage_marker,
        )?;

        return Ok(ReplaceOutcome::AlreadyReplaced);
    }

    match target {
        None if valid_unpublished_receipt(
            &stage,
            &stage_marker,
            &backup,
            &backup_marker,
            options,
        ) =>
        {
            publish(root, options, stage, backup)
        }
        Some(target)
            if valid_published(
                &stage,
                &stage_marker,
                &backup,
                &backup_marker,
                &target,
                options,
            ) =>
        {
            Ok(ReplaceOutcome::AlreadyReplaced)
        }
        _ => Err(Failed),
    }
}

fn recover_backup_only(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
    mut backup: Snapshot,
) -> std::result::Result<ReplaceOutcome, AttemptError> {
    let mut backup_marker = read_valid_marker(root, &backup, options)?.ok_or(Failed)?;

    if !matches!(
        backup_marker.role,
        ReceiptRole::Backup | ReceiptRole::Restoring
    ) || !valid_backup_only(&backup, &backup_marker, options, backup.data.metadata.links)
    {
        return Err(Failed);
    }

    let target = open_mutation_snapshot_optional(
        root,
        &options.path,
        options.maximum_bytes,
        MutationOpenMode::CompatibleRead,
        false,
    )?;

    match target {
        None => {
            if backup.data.metadata.links != 1 {
                return Err(Failed);
            }

            if backup_marker.role == ReceiptRole::Backup {
                write_marker_from(
                    root,
                    &backup,
                    ReceiptRole::Restoring,
                    &backup_marker,
                    options,
                )?;
                backup_marker = read_valid_marker(root, &backup, options)?.ok_or(Failed)?;
            }

            test_hook::crash_at("restoring-marked");

            if backup_marker.role != ReceiptRole::Restoring {
                return Err(Failed);
            }

            match link_no_replace(
                backup.opened.file.0,
                parent_handle(&backup.opened, root),
                leaf_name(&options.path),
            )? {
                NameMutation::Applied => {}
                NameMutation::Collision => return Err(Transient),
            }

            flush_parent(parent_handle(&backup.opened, root));
            refresh_snapshot(&mut backup, options.maximum_bytes, true)?;
            test_hook::crash_at("target-restored");

            let restored = open_mutation_snapshot_optional(
                root,
                &options.path,
                options.maximum_bytes,
                MutationOpenMode::CompatibleRead,
                false,
            )?
            .ok_or(Transient)?;

            if !valid_restored_backup(&backup, &backup_marker, &restored, options) {
                return Err(Failed);
            }

            finish_restoration(root, options, backup, restored, &backup_marker)?;

            Ok(ReplaceOutcome::Changed)
        }
        Some(target) if same_identity(&backup.data.metadata, &target.data.metadata) => {
            if backup_marker.role != ReceiptRole::Restoring
                || !valid_restored_backup(&backup, &backup_marker, &target, options)
            {
                return Err(Failed);
            }

            finish_restoration(root, options, backup, target, &backup_marker)?;

            Ok(ReplaceOutcome::Changed)
        }
        Some(_) => Err(Failed),
    }
}

fn publish(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
    mut stage: Snapshot,
    mut backup: Snapshot,
) -> std::result::Result<ReplaceOutcome, AttemptError> {
    test_hook::trace("start:publish");
    let stage_marker = read_valid_marker(root, &stage, options)?.ok_or(Failed)?;
    let backup_marker = read_valid_marker(root, &backup, options)?.ok_or(Failed)?;

    if !valid_unpublished_receipt(&stage, &stage_marker, &backup, &backup_marker, options) {
        return Err(Failed);
    }

    match link_no_replace(
        stage.opened.file.0,
        parent_handle(&stage.opened, root),
        leaf_name(&options.path),
    )? {
        NameMutation::Applied => {
            flush_parent(parent_handle(&stage.opened, root));
            test_hook::trace("done:target-linked");
            test_hook::crash_at("target-published");
            verify_published(root, options, &mut stage, &mut backup)?;
            test_hook::trace("done:publish-verified");

            Ok(ReplaceOutcome::Replaced)
        }
        NameMutation::Collision => {
            refresh_snapshot(&mut stage, options.maximum_bytes, true)?;
            refresh_snapshot(&mut backup, options.maximum_bytes, true)?;

            let target = open_mutation_snapshot_optional(
                root,
                &options.path,
                options.maximum_bytes,
                MutationOpenMode::CompatibleRead,
                false,
            )?
            .ok_or(Transient)?;

            let stage_marker = read_valid_marker(root, &stage, options)?.ok_or(Failed)?;
            let backup_marker = read_valid_marker(root, &backup, options)?.ok_or(Failed)?;

            if valid_published(
                &stage,
                &stage_marker,
                &backup,
                &backup_marker,
                &target,
                options,
            ) {
                return Ok(ReplaceOutcome::AlreadyReplaced);
            }

            Err(Failed)
        }
    }
}

fn verify_published(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
    stage: &mut Snapshot,
    backup: &mut Snapshot,
) -> std::result::Result<(), AttemptError> {
    refresh_snapshot(stage, options.maximum_bytes, true)?;
    refresh_snapshot(backup, options.maximum_bytes, true)?;

    let target = open_mutation_snapshot_optional(
        root,
        &options.path,
        options.maximum_bytes,
        MutationOpenMode::CompatibleRead,
        false,
    )?
    .ok_or(Transient)?;

    let stage_marker = read_valid_marker(root, stage, options)?.ok_or(Failed)?;
    let backup_marker = read_valid_marker(root, backup, options)?.ok_or(Failed)?;

    if !valid_published(
        stage,
        &stage_marker,
        backup,
        &backup_marker,
        &target,
        options,
    ) {
        return Err(Failed);
    }

    Ok(())
}

fn finalize_once(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
    names: &ArtifactNames,
) -> std::result::Result<(), AttemptError> {
    let stage = open_mutation_snapshot_optional(
        root,
        &names.stage_path,
        options.maximum_bytes,
        MutationOpenMode::Exclusive,
        true,
    )?;
    let backup = open_mutation_snapshot_optional(
        root,
        &names.backup_path,
        options.maximum_bytes,
        MutationOpenMode::Exclusive,
        true,
    )?;

    let Some(stage) = stage else {
        if backup.is_some() {
            return Err(Failed);
        }

        let target = open_mutation_snapshot_optional(
            root,
            &options.path,
            options.maximum_bytes,
            MutationOpenMode::Exclusive,
            false,
        )?;
        let Some(target) = target else {
            return Ok(());
        };

        let marker = read_valid_marker(root, &target, options)?;

        if marker.as_ref().is_some_and(|marker| {
            marker.role == ReceiptRole::Finalizing
                && valid_target_only_marker(&target, marker, options, true)
        }) {
            clear_marker_and_flush(target.opened.file.0, parent_handle(&target.opened, root))?;

            return Ok(());
        }

        return if marker.is_none() {
            Ok(())
        } else {
            Err(Failed)
        };
    };

    let target = open_mutation_snapshot_optional(
        root,
        &options.path,
        options.maximum_bytes,
        MutationOpenMode::CompatibleRead,
        false,
    )?
    .ok_or(Failed)?;

    let stage_marker = read_valid_marker(root, &stage, options)?.ok_or(Failed)?;
    let backup = match backup {
        Some(backup) => {
            let marker = read_valid_marker(root, &backup, options)?.ok_or(Failed)?;

            Some((backup, marker))
        }
        None => None,
    };

    complete_finalization(root, options, stage, backup, target, stage_marker)
}

fn valid_unpublished_receipt(
    stage: &Snapshot,
    stage_marker: &Marker,
    backup: &Snapshot,
    backup_marker: &Marker,
    options: &ReplaceRegularFileOptions,
) -> bool {
    stage.data.bytes == options.replacement
        && backup.data.bytes == options.expected
        && stage.data.metadata.links == 1
        && backup.data.metadata.links == 1
        && !same_identity(&stage.data.metadata, &backup.data.metadata)
        && same_receipt_metadata(stage, backup)
        && stage_marker.role == ReceiptRole::Stage
        && backup_marker.role == ReceiptRole::Backup
        && marker_peer_matches(stage_marker, &backup.data.metadata)
        && marker_peer_matches(backup_marker, &stage.data.metadata)
}

fn valid_published(
    stage: &Snapshot,
    stage_marker: &Marker,
    backup: &Snapshot,
    backup_marker: &Marker,
    target: &Snapshot,
    options: &ReplaceRegularFileOptions,
) -> bool {
    stage.data.bytes == options.replacement
        && target.data.bytes == options.replacement
        && backup.data.bytes == options.expected
        && stage.data.metadata.links == 2
        && target.data.metadata.links == 2
        && backup.data.metadata.links == 1
        && same_identity(&stage.data.metadata, &target.data.metadata)
        && !same_identity(&stage.data.metadata, &backup.data.metadata)
        && same_receipt_metadata(stage, target)
        && same_receipt_metadata(stage, backup)
        && stage_marker.role == ReceiptRole::Stage
        && backup_marker.role == ReceiptRole::Backup
        && marker_peer_matches(stage_marker, &backup.data.metadata)
        && marker_peer_matches(backup_marker, &stage.data.metadata)
}

fn valid_backup_only(
    backup: &Snapshot,
    marker: &Marker,
    options: &ReplaceRegularFileOptions,
    links: u32,
) -> bool {
    backup.data.bytes == options.expected
        && backup.data.metadata.links == links
        && matches!(marker.role, ReceiptRole::Backup | ReceiptRole::Restoring)
        && valid_absent_peer(marker, &backup.data.metadata)
}

fn valid_restored_backup(
    backup: &Snapshot,
    marker: &Marker,
    target: &Snapshot,
    options: &ReplaceRegularFileOptions,
) -> bool {
    marker.role == ReceiptRole::Restoring
        && valid_backup_only(backup, marker, options, 2)
        && target.data.bytes == options.expected
        && target.data.metadata.links == 2
        && same_identity(&backup.data.metadata, &target.data.metadata)
        && same_receipt_metadata(backup, target)
}

fn read_valid_marker(
    root: &RootHandle,
    snapshot: &Snapshot,
    options: &ReplaceRegularFileOptions,
) -> std::result::Result<Option<Marker>, AttemptError> {
    let Some(marker) =
        receipt::read(snapshot.opened.file.0, &root.receipt_key).map_err(|_| Failed)?
    else {
        return Ok(None);
    };

    let valid_self = if marker.role == ReceiptRole::CreatingStage {
        marker.self_volume == 0 && marker.self_id == [0; 16]
    } else {
        marker.self_volume == snapshot.data.metadata.volume
            && marker.self_id == snapshot.data.metadata.id
    };

    if marker.namespace != receipt::namespace(&options.operation_id, &options.path)
        || marker.expected != receipt::hash(&options.expected)
        || marker.replacement != receipt::hash(&options.replacement)
        || !valid_self
    {
        return Err(Failed);
    }

    Ok(Some(marker))
}

fn write_marker(
    root: &RootHandle,
    snapshot: &Snapshot,
    role: ReceiptRole,
    peer_volume: u64,
    peer_id: [u8; 16],
    options: &ReplaceRegularFileOptions,
) -> std::result::Result<(), AttemptError> {
    let marker = Marker {
        role,
        namespace: receipt::namespace(&options.operation_id, &options.path),
        expected: receipt::hash(&options.expected),
        replacement: receipt::hash(&options.replacement),
        self_volume: snapshot.data.metadata.volume,
        self_id: snapshot.data.metadata.id,
        peer_volume,
        peer_id,
    };

    receipt::write(snapshot.opened.file.0, &marker, &root.receipt_key).map_err(|_| Failed)?;
    flush_receipt_metadata(
        snapshot.opened.file.0,
        parent_handle(&snapshot.opened, root),
    )
}

fn write_marker_from(
    root: &RootHandle,
    snapshot: &Snapshot,
    role: ReceiptRole,
    source: &Marker,
    options: &ReplaceRegularFileOptions,
) -> std::result::Result<(), AttemptError> {
    write_marker(
        root,
        snapshot,
        role,
        source.peer_volume,
        source.peer_id,
        options,
    )
}

fn complete_finalization(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
    mut stage: Snapshot,
    mut backup: Option<(Snapshot, Marker)>,
    mut target: Snapshot,
    mut stage_marker: Marker,
) -> std::result::Result<(), AttemptError> {
    if !valid_published_file(&stage, &target, options) {
        return Err(Failed);
    }

    match (&backup, stage_marker.role) {
        (Some((backup, backup_marker)), ReceiptRole::Stage | ReceiptRole::Finalizing)
            if valid_finalization_receipt(
                &stage,
                &stage_marker,
                backup,
                backup_marker,
                options,
            ) => {}
        (None, ReceiptRole::Finalizing)
            if valid_absent_peer(&stage_marker, &stage.data.metadata) => {}
        _ => return Err(Failed),
    }

    if stage_marker.role == ReceiptRole::Stage {
        write_marker_from(
            root,
            &stage,
            ReceiptRole::Finalizing,
            &stage_marker,
            options,
        )?;
        stage_marker.role = ReceiptRole::Finalizing;
    }

    if stage_marker.role != ReceiptRole::Finalizing {
        return Err(Failed);
    }

    refresh_snapshot(&mut stage, options.maximum_bytes, true)?;
    refresh_snapshot(&mut target, options.maximum_bytes, false)?;
    stage_marker = read_valid_marker(root, &stage, options)?.ok_or(Failed)?;

    if stage_marker.role != ReceiptRole::Finalizing
        || !valid_published_file(&stage, &target, options)
    {
        return Err(Failed);
    }

    test_hook::crash_at("finalizing-marked");

    if let Some((backup, backup_marker)) = backup.as_mut() {
        refresh_snapshot(backup, options.maximum_bytes, true)?;
        *backup_marker = read_valid_marker(root, backup, options)?.ok_or(Failed)?;

        if !valid_finalization_receipt(&stage, &stage_marker, backup, backup_marker, options) {
            return Err(Failed);
        }
    } else if !valid_absent_peer(&stage_marker, &stage.data.metadata) {
        return Err(Failed);
    }

    drop(target);

    if let Some((backup, _)) = backup {
        delete_snapshot(root, backup)?;
    }

    test_hook::crash_at("backup-deleted");

    refresh_snapshot(&mut stage, options.maximum_bytes, true)?;
    let final_marker = read_valid_marker(root, &stage, options)?.ok_or(Failed)?;

    if final_marker.role != ReceiptRole::Finalizing
        || final_marker.peer_volume != stage_marker.peer_volume
        || final_marker.peer_id != stage_marker.peer_id
        || stage.data.bytes != options.replacement
        || stage.data.metadata.links != 2
    {
        return Err(Failed);
    }

    let surviving_target = duplicate_handle(stage.opened.file.0)?;
    let receipt_parent = duplicate_handle(parent_handle(&stage.opened, root))?;

    delete_snapshot(root, stage)?;
    test_hook::crash_at("stage-deleted");
    clear_marker_and_flush(surviving_target.0, receipt_parent.0)
}

fn finish_restoration(
    root: &RootHandle,
    options: &ReplaceRegularFileOptions,
    mut backup: Snapshot,
    mut target: Snapshot,
    expected_marker: &Marker,
) -> std::result::Result<(), AttemptError> {
    refresh_snapshot(&mut backup, options.maximum_bytes, true)?;
    refresh_snapshot(&mut target, options.maximum_bytes, false)?;
    let marker = read_valid_marker(root, &backup, options)?.ok_or(Failed)?;

    if marker.peer_volume != expected_marker.peer_volume
        || marker.peer_id != expected_marker.peer_id
        || !valid_restored_backup(&backup, &marker, &target, options)
    {
        return Err(Failed);
    }

    let surviving_target = duplicate_handle(backup.opened.file.0)?;
    let receipt_parent = duplicate_handle(parent_handle(&backup.opened, root))?;

    drop(target);
    delete_snapshot(root, backup)?;
    test_hook::crash_at("restoration-backup-deleted");
    clear_marker_and_flush(surviving_target.0, receipt_parent.0)
}

fn clear_marker_and_flush(handle: HANDLE, parent: HANDLE) -> std::result::Result<(), AttemptError> {
    receipt::clear(handle).map_err(|_| Failed)?;
    flush_receipt_metadata(handle, parent)
}

fn flush_receipt_metadata(handle: HANDLE, parent: HANDLE) -> std::result::Result<(), AttemptError> {
    match flush_exact_handle(handle) {
        Ok(()) => Ok(()),
        Err(status) if status == STATUS_ACCESS_DENIED => {
            flush_exact_handle(parent).map_err(attempt_error)
        }
        Err(status) => Err(attempt_error(status)),
    }
}

fn flush_exact_handle(handle: HANDLE) -> std::result::Result<(), NTSTATUS> {
    let mut io_status = IO_STATUS_BLOCK::default();
    let status = unsafe { NtFlushBuffersFile(handle, &mut io_status) };

    if !nt_success(status) {
        return Err(status);
    }

    Ok(())
}

fn valid_unmarked_backup_adoption(
    stage: &Snapshot,
    stage_marker: &Marker,
    backup: &Snapshot,
    options: &ReplaceRegularFileOptions,
) -> bool {
    stage_marker.role == ReceiptRole::Stage
        && stage.data.bytes == options.replacement
        && backup.data.bytes == options.expected
        && stage.data.metadata.links == 1
        && backup.data.metadata.links == 1
        && !same_identity(&stage.data.metadata, &backup.data.metadata)
        && marker_peer_matches(stage_marker, &backup.data.metadata)
        && same_receipt_metadata(stage, backup)
}

fn valid_finalization_receipt(
    stage: &Snapshot,
    stage_marker: &Marker,
    backup: &Snapshot,
    backup_marker: &Marker,
    options: &ReplaceRegularFileOptions,
) -> bool {
    backup.data.bytes == options.expected
        && backup.data.metadata.links == 1
        && !same_identity(&stage.data.metadata, &backup.data.metadata)
        && same_receipt_metadata(stage, backup)
        && matches!(
            stage_marker.role,
            ReceiptRole::Stage | ReceiptRole::Finalizing
        )
        && backup_marker.role == ReceiptRole::Backup
        && marker_peer_matches(stage_marker, &backup.data.metadata)
        && marker_peer_matches(backup_marker, &stage.data.metadata)
}

fn valid_published_file(
    stage: &Snapshot,
    target: &Snapshot,
    options: &ReplaceRegularFileOptions,
) -> bool {
    stage.data.bytes == options.replacement
        && target.data.bytes == options.replacement
        && stage.data.metadata.links == 2
        && target.data.metadata.links == 2
        && same_identity(&stage.data.metadata, &target.data.metadata)
        && same_receipt_metadata(stage, target)
}

fn valid_target_only_marker(
    target: &Snapshot,
    marker: &Marker,
    options: &ReplaceRegularFileOptions,
    replacement: bool,
) -> bool {
    let expected_bytes = if replacement {
        &options.replacement
    } else {
        &options.expected
    };

    target.data.bytes == *expected_bytes
        && target.data.metadata.links == 1
        && valid_absent_peer(marker, &target.data.metadata)
}

fn valid_creating_stage(stage: &Snapshot, marker: &Marker) -> bool {
    marker.role == ReceiptRole::CreatingStage
        && stage.data.metadata.links == 1
        && marker.peer_volume == stage.data.metadata.volume
        && marker.peer_id != [0; 16]
        && marker.peer_id != stage.data.metadata.id
}

fn valid_absent_peer(marker: &Marker, metadata: &Metadata) -> bool {
    marker.peer_volume == metadata.volume
        && marker.peer_id != [0; 16]
        && marker.peer_id != metadata.id
}

fn marker_peer_matches(marker: &Marker, metadata: &Metadata) -> bool {
    marker.peer_volume == metadata.volume && marker.peer_id == metadata.id
}

fn same_identity(left: &Metadata, right: &Metadata) -> bool {
    left.volume == right.volume && left.id == right.id
}

fn same_receipt_metadata(left: &Snapshot, right: &Snapshot) -> bool {
    left.data.metadata.attributes == right.data.metadata.attributes
        && left.data.security == right.data.security
}

fn delete_snapshot(root: &RootHandle, snapshot: Snapshot) -> std::result::Result<(), AttemptError> {
    let OpenedFile { file, parents } = snapshot.opened;
    let parent = parents.last().map_or(root.handle, |handle| handle.0);
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    let mut io_status = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtSetInformationFile(
            file.0,
            &mut io_status,
            &disposition as *const _ as _,
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            FileDispositionInformationEx,
        )
    };

    if !nt_success(status) {
        return Err(attempt_error(status));
    }

    drop(file);
    flush_parent(parent);
    drop(parents);

    Ok(())
}

fn duplicate_handle(handle: HANDLE) -> std::result::Result<Handle, AttemptError> {
    let process = unsafe { GetCurrentProcess() };
    let mut duplicate = std::ptr::null_mut();
    let succeeded = unsafe {
        DuplicateHandle(
            process,
            handle,
            process,
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };

    if succeeded == 0 || duplicate.is_null() || duplicate == INVALID_HANDLE_VALUE {
        return Err(Failed);
    }

    Ok(Handle(duplicate))
}

fn create_stage(
    root: &RootHandle,
    target_path: &str,
    stage_path: &str,
    stage_name: &str,
    target: &Snapshot,
    options: &ReplaceRegularFileOptions,
) -> std::result::Result<Snapshot, AttemptError> {
    let (parent, parents, _) = open_parent(root, target_path, true).map_err(open_attempt_error)?;
    let stage_utf16: Vec<u16> = stage_name.encode_utf16().collect();
    let stage_string = unicode_string(&stage_utf16);
    let creation_ea = receipt::creation_ea(
        &Marker {
            role: ReceiptRole::CreatingStage,
            namespace: receipt::namespace(&options.operation_id, &options.path),
            expected: receipt::hash(&options.expected),
            replacement: receipt::hash(&options.replacement),
            self_volume: 0,
            self_id: [0; 16],
            peer_volume: target.data.metadata.volume,
            peer_id: target.data.metadata.id,
        },
        &root.receipt_key,
    );
    let opened = open_nt_file(
        &stage_string,
        parent,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        FILE_SHARE_READ,
        FILE_CREATE,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE | WRITE_DAC | FILE_WRITE_EA,
        Some(creation_ea.as_bytes()),
        target.data.security.as_ptr(),
    )
    .map_err(create_attempt_error)?;
    let opened = OpenedFile {
        file: Handle(opened),
        parents,
    };
    test_hook::trace("done:stage-created");

    test_hook::crash_at("creating-stage");

    write_exact(opened.file.0, &options.replacement)?;
    test_hook::trace("done:stage-written");

    if unsafe { FlushFileBuffers(opened.file.0) } == 0 {
        return Err(Failed);
    }
    test_hook::trace("done:stage-content-flushed");

    set_attributes(
        opened.file.0,
        ordinary_attributes(target.data.metadata.attributes),
    )?;
    test_hook::trace("done:stage-attributes-set");

    if !apply_dacl(opened.file.0, &target.data.security) {
        return Err(Failed);
    }
    test_hook::trace("done:stage-dacl-set");

    if unsafe { FlushFileBuffers(opened.file.0) } == 0 {
        return Err(Failed);
    }
    test_hook::trace("done:stage-metadata-flushed");

    let mut stage = snapshot_from_opened(opened, options.maximum_bytes, true)?;
    test_hook::trace("done:stage-snapshot-read");
    test_hook::trace(if stage.data.bytes == options.replacement {
        "check:stage-bytes-match"
    } else {
        "check:stage-bytes-mismatch"
    });
    test_hook::trace(if stage.data.metadata.links == 1 {
        "check:stage-link-count-match"
    } else {
        "check:stage-link-count-mismatch"
    });
    test_hook::trace(
        if !same_identity(&stage.data.metadata, &target.data.metadata) {
            "check:stage-identity-distinct"
        } else {
            "check:stage-identity-reused"
        },
    );
    test_hook::trace(
        if stage.data.metadata.attributes == target.data.metadata.attributes {
            "check:stage-attributes-match"
        } else {
            "check:stage-attributes-mismatch"
        },
    );
    test_hook::trace(if stage.data.security == target.data.security {
        "check:stage-security-match"
    } else {
        "check:stage-security-mismatch"
    });

    if stage.data.bytes != options.replacement
        || stage.data.metadata.links != 1
        || same_identity(&stage.data.metadata, &target.data.metadata)
        || !same_receipt_metadata(&stage, target)
    {
        return Err(Failed);
    }
    test_hook::trace("done:stage-snapshot-verified");

    write_marker(
        root,
        &stage,
        ReceiptRole::Stage,
        target.data.metadata.volume,
        target.data.metadata.id,
        options,
    )?;
    test_hook::trace("done:stage-marker-written");
    refresh_snapshot(&mut stage, options.maximum_bytes, true)?;
    test_hook::trace("done:stage-marker-refreshed");
    test_hook::crash_at("stage-ready");

    let proof = stage.data.clone();
    drop(stage);

    let stage = open_mutation_snapshot_optional(
        root,
        stage_path,
        options.maximum_bytes,
        MutationOpenMode::Exclusive,
        true,
    )?
    .ok_or(Transient)?;
    test_hook::trace("done:stage-reopened");

    if stage.data != proof {
        return Err(Failed);
    }
    test_hook::trace("done:stage-reopen-verified");

    Ok(stage)
}

fn refresh_snapshot(
    snapshot: &mut Snapshot,
    maximum_bytes: u32,
    allow_artifact: bool,
) -> std::result::Result<(), AttemptError> {
    snapshot.data = read_snapshot_data(&snapshot.opened, maximum_bytes, allow_artifact)?;

    Ok(())
}

fn snapshot_from_opened(
    opened: OpenedFile,
    maximum_bytes: u32,
    allow_artifact: bool,
) -> std::result::Result<Snapshot, AttemptError> {
    let data = read_snapshot_data(&opened, maximum_bytes, allow_artifact)?;

    Ok(Snapshot { opened, data })
}

fn read_snapshot_data(
    opened: &OpenedFile,
    maximum_bytes: u32,
    allow_artifact: bool,
) -> std::result::Result<SnapshotData, AttemptError> {
    let before = file_metadata(opened.file.0).map_err(|_| Failed)?;

    if before.directory
        || before.reparse
        || before.size > maximum_bytes as u64
        || (!allow_artifact && opened_name_is_private(opened.file.0).map_err(|_| Failed)?)
    {
        return Err(Failed);
    }

    let bytes = read_exact_from_start(opened.file.0, before.size as usize)?;
    let security = security_descriptor(opened.file.0)?;
    let after = file_metadata(opened.file.0).map_err(|_| Failed)?;

    if before != after {
        return Err(Transient);
    }

    Ok(SnapshotData {
        metadata: before,
        bytes,
        security,
    })
}

fn open_mutation_snapshot_optional(
    root: &RootHandle,
    path: &str,
    maximum_bytes: u32,
    mode: MutationOpenMode,
    allow_artifact: bool,
) -> std::result::Result<Option<Snapshot>, AttemptError> {
    let (access, sharing) = match mode {
        MutationOpenMode::Exclusive => {
            (FILE_GENERIC_READ | DELETE | FILE_WRITE_EA, FILE_SHARE_READ)
        }
        MutationOpenMode::CompatibleRead => (
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        ),
    };
    let opened = match open_relative_file(root, path, access, sharing, true) {
        Ok(opened) => opened,
        Err(OpenPathError::Missing) => return Ok(None),
        Err(OpenPathError::Transient) => return Err(Transient),
        Err(OpenPathError::Failed) => return Err(Failed),
    };
    let snapshot = snapshot_from_opened(opened, maximum_bytes, allow_artifact)?;

    Ok(Some(snapshot))
}

fn open_relative_file(
    root: &RootHandle,
    path: &str,
    access: u32,
    sharing: u32,
    mutation: bool,
) -> std::result::Result<OpenedFile, OpenPathError> {
    let (parent, parents, leaf) = open_parent(root, path, mutation)?;
    let leaf_utf16: Vec<u16> = leaf.encode_utf16().collect();
    let leaf_string = unicode_string(&leaf_utf16);
    let file = open_nt_file(
        &leaf_string,
        parent,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        sharing,
        FILE_OPEN,
        access,
        None,
        std::ptr::null(),
    )
    .map_err(open_status_error)?;

    Ok(OpenedFile {
        file: Handle(file),
        parents,
    })
}

fn open_parent<'a>(
    root: &RootHandle,
    path: &'a str,
    mutation: bool,
) -> std::result::Result<(HANDLE, Vec<Handle>, &'a str), OpenPathError> {
    let segments: Vec<_> = path.split(['/', '\\']).collect();
    let (leaf, directories) = segments.split_last().ok_or(OpenPathError::Failed)?;
    let mut parents = Vec::with_capacity(directories.len());
    let mut current = root.handle;

    for segment in directories {
        let segment_utf16: Vec<u16> = segment.encode_utf16().collect();
        let segment_string = unicode_string(&segment_utf16);
        let access = if mutation {
            FILE_GENERIC_READ | FILE_GENERIC_WRITE
        } else {
            FILE_GENERIC_READ
        };
        let directory = open_nt_file(
            &segment_string,
            current,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            access,
            None,
            std::ptr::null(),
        )
        .map_err(open_status_error)?;
        let directory = Handle(directory);
        let metadata = file_metadata(directory.0).map_err(|_| OpenPathError::Failed)?;

        if !metadata.directory
            || metadata.reparse
            || opened_name_is_private(directory.0).map_err(|_| OpenPathError::Failed)?
        {
            return Err(OpenPathError::Failed);
        }

        current = directory.0;
        parents.push(directory);
    }

    Ok((current, parents, leaf))
}

fn read_exact_from_start(
    handle: HANDLE,
    size: usize,
) -> std::result::Result<Vec<u8>, AttemptError> {
    if unsafe { SetFilePointerEx(handle, 0, std::ptr::null_mut(), FILE_BEGIN) } == 0 {
        return Err(Failed);
    }

    let mut bytes = vec![0; size];
    let mut offset = 0;

    while offset < bytes.len() {
        let mut bytes_read = 0;
        let amount = (bytes.len() - offset).min(u32::MAX as usize) as u32;
        let succeeded = unsafe {
            ReadFile(
                handle,
                bytes[offset..].as_mut_ptr(),
                amount,
                &mut bytes_read,
                std::ptr::null_mut(),
            )
        };

        if succeeded == 0 || bytes_read == 0 || bytes_read > amount {
            return Err(Failed);
        }

        offset += bytes_read as usize;
    }

    Ok(bytes)
}

fn write_exact(handle: HANDLE, bytes: &[u8]) -> std::result::Result<(), AttemptError> {
    let mut offset = 0;

    while offset < bytes.len() {
        let mut bytes_written = 0;
        let amount = (bytes.len() - offset).min(u32::MAX as usize) as u32;
        let succeeded = unsafe {
            WriteFile(
                handle,
                bytes[offset..].as_ptr(),
                amount,
                &mut bytes_written,
                std::ptr::null_mut(),
            )
        };

        if succeeded == 0 || bytes_written == 0 || bytes_written > amount {
            return Err(Failed);
        }

        offset += bytes_written as usize;
    }

    Ok(())
}

fn set_attributes(handle: HANDLE, attributes: u32) -> std::result::Result<(), AttemptError> {
    let information = FILE_BASIC_INFORMATION {
        FileAttributes: attributes,
        ..Default::default()
    };
    let mut io_status = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtSetInformationFile(
            handle,
            &mut io_status,
            &information as *const _ as _,
            size_of::<FILE_BASIC_INFORMATION>() as u32,
            FileBasicInformation,
        )
    };

    if nt_success(status) {
        Ok(())
    } else {
        Err(attempt_error(status))
    }
}

/** Builds an explicitly aligned variable-length NT rename record. */
fn rename_no_replace(
    handle: HANDLE,
    parent: HANDLE,
    name: &str,
) -> std::result::Result<NameMutation, AttemptError> {
    let utf16: Vec<u16> = name.encode_utf16().collect();
    let offset = offset_of!(FILE_RENAME_INFORMATION, FileName);
    let filename_bytes = utf16.len().checked_mul(size_of::<u16>()).ok_or(Failed)?;
    let filename_length = u32::try_from(filename_bytes).map_err(|_| Failed)?;
    let length = size_of::<FILE_RENAME_INFORMATION>()
        .checked_add(filename_bytes)
        .ok_or(Failed)?;
    let mut storage = vec![0usize; length.div_ceil(size_of::<usize>())];
    let information = storage.as_mut_ptr() as *mut FILE_RENAME_INFORMATION;

    unsafe {
        (*information).Anonymous.Flags = 0;
        (*information).RootDirectory = parent;
        (*information).FileNameLength = filename_length;
        std::ptr::copy_nonoverlapping(
            utf16.as_ptr() as *const u8,
            (information as *mut u8).add(offset),
            filename_bytes,
        );
    }

    set_name_information(handle, information as _, length, FileRenameInformationEx)
}

/** Builds an explicitly aligned variable-length NT hard-link record. */
fn link_no_replace(
    handle: HANDLE,
    parent: HANDLE,
    name: &str,
) -> std::result::Result<NameMutation, AttemptError> {
    let utf16: Vec<u16> = name.encode_utf16().collect();
    let offset = offset_of!(FILE_LINK_INFORMATION, FileName);
    let filename_bytes = utf16.len().checked_mul(size_of::<u16>()).ok_or(Failed)?;
    let filename_length = u32::try_from(filename_bytes).map_err(|_| Failed)?;
    let length = size_of::<FILE_LINK_INFORMATION>()
        .checked_add(filename_bytes)
        .ok_or(Failed)?;
    let mut storage = vec![0usize; length.div_ceil(size_of::<usize>())];
    let information = storage.as_mut_ptr() as *mut FILE_LINK_INFORMATION;

    unsafe {
        (*information).Anonymous.Flags = 0;
        (*information).RootDirectory = parent;
        (*information).FileNameLength = filename_length;
        std::ptr::copy_nonoverlapping(
            utf16.as_ptr() as *const u8,
            (information as *mut u8).add(offset),
            filename_bytes,
        );
    }

    set_name_information(handle, information as _, length, FileLinkInformationEx)
}

fn set_name_information(
    handle: HANDLE,
    information: *const core::ffi::c_void,
    length: usize,
    information_class: i32,
) -> std::result::Result<NameMutation, AttemptError> {
    let mut io_status = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtSetInformationFile(
            handle,
            &mut io_status,
            information,
            length as u32,
            information_class,
        )
    };

    if nt_success(status) {
        return Ok(NameMutation::Applied);
    }

    match classify_ntstatus(status) {
        NtStatusKind::Collision => Ok(NameMutation::Collision),
        NtStatusKind::Missing | NtStatusKind::Transient => Err(Transient),
        NtStatusKind::Permanent => Err(Failed),
    }
}

fn security_descriptor(
    handle: HANDLE,
) -> std::result::Result<SecurityDescriptorBuffer, AttemptError> {
    let mut required = 0;

    unsafe {
        GetKernelObjectSecurity(
            handle,
            SECURITY_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut required,
        );
    }

    if required == 0 || required as usize > MAXIMUM_SECURITY_DESCRIPTOR_BYTES {
        return Err(Failed);
    }

    let capacity = required as usize;
    let mut storage = vec![0usize; capacity.div_ceil(size_of::<usize>())];
    let mut written = capacity as u32;
    let succeeded = unsafe {
        GetKernelObjectSecurity(
            handle,
            SECURITY_INFORMATION,
            storage.as_mut_ptr() as _,
            capacity as u32,
            &mut written,
        )
    };

    if succeeded == 0 || written == 0 || written as usize > capacity {
        return Err(Failed);
    }

    let canonical = canonical_security_descriptor(storage.as_ptr() as _)?;

    Ok(SecurityDescriptorBuffer {
        canonical,
        storage,
        length: written as usize,
    })
}

fn canonical_security_descriptor(
    descriptor: *const SECURITY_DESCRIPTOR,
) -> std::result::Result<Vec<u16>, AttemptError> {
    let mut canonical = std::ptr::null_mut();
    let mut length = 0;
    let succeeded = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor as _,
            SDDL_REVISION_1,
            SECURITY_INFORMATION,
            &mut canonical,
            &mut length,
        )
    };

    if succeeded == 0 || canonical.is_null() || length == 0 {
        return Err(Failed);
    }

    let canonical = LocalSecurityString(canonical);
    let normalized = unsafe { slice::from_raw_parts(canonical.0, length as usize) }.to_vec();

    Ok(normalized)
}

fn apply_dacl(handle: HANDLE, descriptor: &SecurityDescriptorBuffer) -> bool {
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl = std::ptr::null_mut();
    let queried = unsafe {
        GetSecurityDescriptorDacl(
            descriptor.as_ptr() as _,
            &mut present,
            &mut dacl,
            &mut defaulted,
        )
    };

    if queried == 0 || present == 0 {
        return false;
    }

    let Ok(control) = descriptor.control() else {
        return false;
    };
    let inheritance_information = if control & SE_DACL_PROTECTED != 0 {
        PROTECTED_DACL_SECURITY_INFORMATION
    } else {
        UNPROTECTED_DACL_SECURITY_INFORMATION
    };

    (unsafe {
        SetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | inheritance_information,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            dacl,
            std::ptr::null(),
        )
    }) == 0
}

fn file_metadata(handle: HANDLE) -> Result<Metadata> {
    let mut basic = FILE_BASIC_INFO::default();
    let mut standard = FILE_STANDARD_INFO::default();
    let mut id = FILE_ID_INFO::default();
    let basic_succeeded = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            &mut basic as *mut _ as _,
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    };
    let standard_succeeded = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            &mut standard as *mut _ as _,
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    let id_succeeded = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            &mut id as *mut _ as _,
            size_of::<FILE_ID_INFO>() as u32,
        )
    };

    if basic_succeeded == 0
        || standard_succeeded == 0
        || id_succeeded == 0
        || standard.EndOfFile < 0
    {
        return Err(native_error("native file metadata failed"));
    }

    Ok(Metadata {
        volume: id.VolumeSerialNumber,
        id: id.FileId.Identifier,
        creation: basic.CreationTime,
        write: basic.LastWriteTime,
        change: basic.ChangeTime,
        attributes: basic.FileAttributes,
        size: standard.EndOfFile as u64,
        links: standard.NumberOfLinks,
        directory: standard.Directory || basic.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        reparse: basic.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
    })
}

#[allow(clippy::too_many_arguments)]
fn open_nt_file(
    name: &UNICODE_STRING,
    root_directory: HANDLE,
    create_options: u32,
    share_access: u32,
    disposition: u32,
    desired_access: u32,
    ea_buffer: Option<&[u8]>,
    security_descriptor: *const SECURITY_DESCRIPTOR,
) -> std::result::Result<HANDLE, NTSTATUS> {
    let mut handle = std::ptr::null_mut();
    let mut io_status = IO_STATUS_BLOCK::default();
    let object_attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: root_directory,
        ObjectName: name as *const _ as _,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: security_descriptor,
        SecurityQualityOfService: std::ptr::null(),
    };
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            desired_access,
            &object_attributes,
            &mut io_status,
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            share_access,
            disposition,
            create_options,
            ea_buffer.map_or(std::ptr::null(), |buffer| buffer.as_ptr()) as _,
            ea_buffer.map_or(0, |buffer| buffer.len() as u32),
        )
    };

    if !nt_success(status) || handle.is_null() || handle == INVALID_HANDLE_VALUE {
        if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(handle) };
        }

        return Err(status);
    }

    Ok(handle)
}

fn classify_ntstatus(status: NTSTATUS) -> NtStatusKind {
    if matches!(
        status,
        STATUS_OBJECT_NAME_NOT_FOUND
            | STATUS_OBJECT_PATH_NOT_FOUND
            | STATUS_NO_SUCH_FILE
            | windows_sys::Win32::Foundation::STATUS_NOT_A_DIRECTORY
    ) {
        return NtStatusKind::Missing;
    }

    if matches!(
        status,
        STATUS_OBJECT_NAME_COLLISION | STATUS_OBJECT_NAME_EXISTS
    ) {
        return NtStatusKind::Collision;
    }

    if matches!(
        status,
        STATUS_SHARING_VIOLATION
            | STATUS_DELETE_PENDING
            | STATUS_FILE_DELETED
            | STATUS_RETRY
            | STATUS_CANT_WAIT
            | STATUS_LOCK_NOT_GRANTED
            | STATUS_OPLOCK_NOT_GRANTED
            | STATUS_USER_MAPPED_FILE
    ) {
        return NtStatusKind::Transient;
    }

    NtStatusKind::Permanent
}

fn open_status_error(status: NTSTATUS) -> OpenPathError {
    match classify_ntstatus(status) {
        NtStatusKind::Missing => OpenPathError::Missing,
        NtStatusKind::Transient => OpenPathError::Transient,
        NtStatusKind::Collision | NtStatusKind::Permanent => OpenPathError::Failed,
    }
}

fn open_attempt_error(error: OpenPathError) -> AttemptError {
    match error {
        OpenPathError::Transient => Transient,
        OpenPathError::Missing | OpenPathError::Failed => Failed,
    }
}

fn create_attempt_error(status: NTSTATUS) -> AttemptError {
    match classify_ntstatus(status) {
        NtStatusKind::Missing | NtStatusKind::Collision | NtStatusKind::Transient => Transient,
        NtStatusKind::Permanent => Failed,
    }
}

fn attempt_error(status: NTSTATUS) -> AttemptError {
    match classify_ntstatus(status) {
        NtStatusKind::Missing | NtStatusKind::Transient => Transient,
        NtStatusKind::Collision | NtStatusKind::Permanent => Failed,
    }
}

fn nt_success(status: NTSTATUS) -> bool {
    status >= 0
}

fn flush_parent(handle: HANDLE) {
    let mut io_status = IO_STATUS_BLOCK::default();

    unsafe {
        NtFlushBuffersFile(handle, &mut io_status);
    }
}

fn parent_handle(opened: &OpenedFile, root: &RootHandle) -> HANDLE {
    opened.parents.last().map_or(root.handle, |handle| handle.0)
}

fn ordinary_attributes(attributes: u32) -> u32 {
    let attributes = attributes & ORDINARY_ATTRIBUTES_MASK;

    if attributes == 0 {
        FILE_ATTRIBUTE_NORMAL
    } else {
        attributes
    }
}

fn artifact_names(options: &ReplaceRegularFileOptions) -> ArtifactNames {
    let namespace = receipt::namespace(&options.operation_id, &options.path)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let compact = &namespace[..32];
    let formatted = format!(
        "{}-{}-{}-{}-{}",
        &compact[..8],
        &compact[8..12],
        &compact[12..16],
        &compact[16..20],
        &compact[20..],
    );
    let parent = options
        .path
        .rsplit_once(['/', '\\'])
        .map_or("", |(parent, _)| parent);
    let prefix = if parent.is_empty() {
        String::new()
    } else {
        format!("{parent}/")
    };
    let stage_name = format!(".artisan-conditional-{namespace}.stage");
    let backup_name = format!(".artisan-conditional-{namespace}.backup-{formatted}");

    ArtifactNames {
        stage_path: format!("{prefix}{stage_name}"),
        backup_path: format!("{prefix}{backup_name}"),
        stage_name,
        backup_name,
    }
}

fn leaf_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn unicode_string(value: &[u16]) -> UNICODE_STRING {
    UNICODE_STRING {
        Length: size_of_val(value) as u16,
        MaximumLength: size_of_val(value) as u16,
        Buffer: value.as_ptr() as _,
    }
}

fn is_ntfs(handle: HANDLE) -> bool {
    let mut filesystem_name = [0u16; 16];
    let succeeded = unsafe {
        GetVolumeInformationByHandleW(
            handle,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            filesystem_name.as_mut_ptr(),
            filesystem_name.len() as u32,
        )
    };
    let name_length = filesystem_name
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(filesystem_name.len());
    let filesystem = String::from_utf16_lossy(&filesystem_name[..name_length]);

    succeeded != 0
        && unsafe { GetFileType(handle) } == FILE_TYPE_DISK
        && filesystem.eq_ignore_ascii_case("NTFS")
}

fn is_fixed_volume(root_path: &[u16]) -> bool {
    let mut volume_path = vec![0u16; MAXIMUM_NT_PATH_UNITS + 1];
    let succeeded = unsafe {
        GetVolumePathNameW(
            root_path.as_ptr(),
            volume_path.as_mut_ptr(),
            volume_path.len() as u32,
        )
    };

    succeeded != 0 && unsafe { GetDriveTypeW(volume_path.as_ptr()) } == DRIVE_FIXED
}

fn opened_name_is_private(handle: HANDLE) -> Result<bool> {
    let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_NONE;
    let required = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, flags) };

    if required == 0 || required as usize > MAXIMUM_NT_PATH_UNITS + 1 {
        return Err(native_error("native file name query failed"));
    }

    let mut path = vec![0u16; required as usize];
    let written =
        unsafe { GetFinalPathNameByHandleW(handle, path.as_mut_ptr(), path.len() as u32, flags) };

    if written == 0 || written as usize >= path.len() {
        return Err(native_error("native file name query failed"));
    }

    let name = String::from_utf16(&path[..written as usize])
        .map_err(|_| native_error("native file name query failed"))?;

    Ok(name
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(is_private_segment))
}

pub(crate) fn validate_relative_path(relative_path: &str) -> Result<()> {
    if relative_path.is_empty()
        || relative_path.starts_with(['/', '\\'])
        || relative_path.contains('\0')
        || relative_path.contains(':')
        || relative_path.encode_utf16().count() > MAXIMUM_NT_PATH_UNITS
    {
        return Err(native_error("relative path is invalid"));
    }

    for segment in relative_path.split(['/', '\\']) {
        if segment.is_empty()
            || matches!(segment, "." | "..")
            || segment.ends_with(['.', ' '])
            || segment.chars().any(|character| {
                character < ' ' || matches!(character, '<' | '>' | '"' | '|' | '?' | '*')
            })
            || segment.encode_utf16().count() > u16::MAX as usize / size_of::<u16>()
            || is_private_segment(segment)
        {
            return Err(native_error("relative path is invalid"));
        }
    }

    Ok(())
}

fn is_absolute_drive_path(path: &str) -> bool {
    let path = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let bytes = path.as_bytes();

    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn is_private_segment(segment: &str) -> bool {
    const PRIVATE_CONDITIONAL_PREFIX: &str = ".artisan-conditional-";

    segment.eq_ignore_ascii_case(".artisan-trash")
        || segment
            .get(..PRIVATE_CONDITIONAL_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PRIVATE_CONDITIONAL_PREFIX))
}
