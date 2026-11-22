use super::{OpenBeneathError, Relative};
use std::ffi::OsStr;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
use std::path::{Component, Path, PathBuf};
use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT,
    FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NTCREATEFILE_CREATE_OPTIONS,
    NtCreateFile,
};
use windows::Win32::Foundation::{
    HANDLE, OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, UNICODE_STRING,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ACCESS_RIGHTS, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_NAME_NORMALIZED,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
    FileAttributeTagInfo, GETFINALPATHNAMEBYHANDLE_FLAGS, GetFileInformationByHandleEx,
    GetFinalPathNameByHandleW, ReOpenFile, SYNCHRONIZE, VOLUME_NAME_DOS,
};
use windows::Win32::System::IO::IO_STATUS_BLOCK;
use windows::core::PWSTR;

const NAME_SURROGATE: u32 = 0x2000_0000;

const DIRECTORY_ACCESS: FILE_ACCESS_RIGHTS =
    FILE_ACCESS_RIGHTS(FILE_TRAVERSE.0 | FILE_READ_ATTRIBUTES.0);

pub(super) fn directory(canonical: &Path) -> Result<std::fs::File, OpenBeneathError> {
    let mut at: PathBuf = canonical
        .components()
        .take_while(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
        .collect();
    let mut held = std::fs::File::options()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(&at)
        .map_err(|source| OpenBeneathError::Open {
            at: at.clone(),
            source,
        })?;
    for name in super::canonical_names(canonical) {
        at.push(name);
        held = open_directory_at(&held, name, &at)?;
    }
    Ok(held)
}

pub(super) fn held_at(directory: &std::fs::File, canonical: &Path) -> std::io::Result<bool> {
    let held = final_path(directory)?;
    Ok(super::same_path(
        &crate::resources::without_verbatim_prefix(&held),
        canonical,
    ))
}

fn open_directory_at(
    parent: &std::fs::File,
    name: &OsStr,
    at: &Path,
) -> Result<std::fs::File, OpenBeneathError> {
    let opened = open_at(
        parent,
        name,
        DIRECTORY_ACCESS,
        FILE_DIRECTORY_FILE | FILE_OPEN_FOR_BACKUP_INTENT,
    )
    .map_err(|source| OpenBeneathError::Open {
        at: at.to_path_buf(),
        source,
    })?;
    followed(opened, DIRECTORY_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, at)
}

fn status_error(status: windows::Win32::Foundation::NTSTATUS) -> std::io::Error {
    let code = unsafe { RtlNtStatusToDosError(status) };
    std::io::Error::from_raw_os_error(i32::from_ne_bytes(code.to_ne_bytes()))
}

fn open_at(
    parent: &std::fs::File,
    name: &OsStr,
    access: FILE_ACCESS_RIGHTS,
    options: NTCREATEFILE_CREATE_OPTIONS,
) -> std::io::Result<std::fs::File> {
    let mut wide: Vec<u16> = name.encode_wide().collect();
    let bytes = wide
        .len()
        .checked_mul(2)
        .and_then(|bytes| u16::try_from(bytes).ok())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidFilename))?;
    let object_name = UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes,
        Buffer: PWSTR(wide.as_mut_ptr()),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>())
            .expect("invariant: OBJECT_ATTRIBUTES is a few dozen bytes"),
        RootDirectory: HANDLE(parent.as_raw_handle()),
        ObjectName: &raw const object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        ..OBJECT_ATTRIBUTES::default()
    };
    let mut handle = HANDLE::default();
    let mut status_block = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtCreateFile(
            &raw mut handle,
            access | SYNCHRONIZE,
            &raw const attributes,
            &raw mut status_block,
            None,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            None,
            0,
        )
    };
    if status.is_err() {
        return Err(status_error(status));
    }
    Ok(unsafe { std::fs::File::from_raw_handle(handle.0) })
}

enum Reparse {
    None,
    Link,
    Other,
}

fn reparse(file: &std::fs::File) -> std::io::Result<Reparse> {
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileAttributeTagInfo,
            (&raw mut info).cast(),
            u32::try_from(size_of::<FILE_ATTRIBUTE_TAG_INFO>())
                .expect("invariant: FILE_ATTRIBUTE_TAG_INFO is eight bytes"),
        )
    }
    .map_err(std::io::Error::from)?;
    Ok(
        match (
            info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0,
            info.ReparseTag & NAME_SURROGATE != 0,
        ) {
            (false, _) => Reparse::None,
            (true, true) => Reparse::Link,
            (true, false) => Reparse::Other,
        },
    )
}

fn followed(
    file: std::fs::File,
    access: FILE_ACCESS_RIGHTS,
    flags: FILE_FLAGS_AND_ATTRIBUTES,
    at: &Path,
) -> Result<std::fs::File, OpenBeneathError> {
    let failed = |source: std::io::Error| OpenBeneathError::Open {
        at: at.to_path_buf(),
        source,
    };
    match reparse(&file).map_err(failed)? {
        Reparse::None => Ok(file),
        Reparse::Link => Err(OpenBeneathError::Linked {
            at: at.to_path_buf(),
        }),
        Reparse::Other => {
            let reopened = unsafe {
                ReOpenFile(
                    HANDLE(file.as_raw_handle()),
                    access.0 | SYNCHRONIZE.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    flags,
                )
            }
            .map_err(|error| failed(error.into()))?;
            Ok(unsafe { std::fs::File::from_raw_handle(reopened.0) })
        }
    }
}

pub(super) fn open_file(
    root: &std::fs::File,
    relative: &Relative,
) -> Result<std::fs::File, OpenBeneathError> {
    let mut at = PathBuf::new();
    let mut held: Option<std::fs::File> = None;
    for name in &relative.parents {
        at.push(name);
        let parent = held.as_ref().unwrap_or(root);
        held = Some(open_directory_at(parent, name, &at)?);
    }
    at.push(&relative.name);
    let parent = held.as_ref().unwrap_or(root);
    let opened = open_at(
        parent,
        &relative.name,
        FILE_GENERIC_READ,
        FILE_NON_DIRECTORY_FILE,
    )
    .map_err(|source| OpenBeneathError::Open {
        at: at.clone(),
        source,
    })?;
    followed(opened, FILE_GENERIC_READ, FILE_FLAGS_AND_ATTRIBUTES(0), &at)
}

pub(crate) fn final_path(file: &std::fs::File) -> std::io::Result<PathBuf> {
    let handle = HANDLE(file.as_raw_handle());
    let mut buffer = vec![0_u16; 512];
    loop {
        let written = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                &mut buffer,
                GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0),
            )
        };
        let written = usize::try_from(written).expect("invariant: a u32 fits in usize");
        match written {
            0 => return Err(std::io::Error::last_os_error()),
            fits if fits < buffer.len() => {
                buffer.truncate(fits);
                return Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer)));
            }
            needed => buffer.resize(needed, 0),
        }
    }
}
