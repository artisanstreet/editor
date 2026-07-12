use std::{
    mem::size_of,
    sync::{Arc, Mutex},
};

use napi::{
    Error, Result, Task,
    bindgen_prelude::{AsyncTask, Buffer},
};
use napi_derive::napi;
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
            FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile, RtlDosPathNameToNtPathName_U_WithStatus,
        },
    },
    Win32::{
        Foundation::{
            CloseHandle, HANDLE, INVALID_HANDLE_VALUE, NTSTATUS, OBJ_CASE_INSENSITIVE,
            OBJ_DONT_REPARSE, UNICODE_STRING,
        },
        Storage::FileSystem::{
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
            FILE_GENERIC_READ, FILE_ID_INFO, FILE_NAME_NORMALIZED, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FILE_TYPE_DISK, FileBasicInfo,
            FileIdInfo, FileStandardInfo, GetDriveTypeW, GetFileInformationByHandleEx, GetFileType,
            GetFinalPathNameByHandleW, GetVolumeInformationByHandleW, GetVolumePathNameW, ReadFile,
            VOLUME_NAME_NONE,
        },
        System::{
            IO::IO_STATUS_BLOCK,
            WindowsProgramming::{DRIVE_FIXED, RtlFreeUnicodeString},
        },
    },
};

const MAXIMUM_NT_PATH_UNITS: usize = u16::MAX as usize / 2;

#[napi(object)]
pub struct NativeBuildDescriptor {
    pub architecture: String,
    pub operating_system: String,
    pub target: String,
}

#[napi]
pub fn get_native_build_descriptor() -> NativeBuildDescriptor {
    NativeBuildDescriptor {
        architecture: std::env::consts::ARCH.to_owned(),
        operating_system: std::env::consts::OS.to_owned(),
        target: env!("ARTISAN_NATIVE_TARGET").to_owned(),
    }
}

struct OwnedHandle(HANDLE);

unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

impl OwnedHandle {
    fn into_raw(self) -> HANDLE {
        let handle = self.0;

        std::mem::forget(self);

        handle
    }
}

struct RootState {
    handle: Mutex<Option<Arc<OwnedHandle>>>,
}

#[napi]
pub struct NativeBoundedRegularFileStore {
    root: Arc<RootState>,
}

#[napi]
impl NativeBoundedRegularFileStore {
    #[napi(constructor)]
    pub fn new(root_directory: String) -> Result<Self> {
        let root_handle = open_root(&root_directory)?;

        Ok(Self {
            root: Arc::new(RootState {
                handle: Mutex::new(Some(Arc::new(OwnedHandle(root_handle)))),
            }),
        })
    }

    #[napi(ts_return_type = "Promise<Uint8Array>")]
    pub fn read_regular_file(
        &self,
        relative_path: String,
        maximum_bytes: f64,
    ) -> Result<AsyncTask<ReadRegularFileTask>> {
        validate_relative_path(&relative_path)?;
        let maximum_bytes = validate_maximum_bytes(maximum_bytes)?;

        let root_handle = self
            .root
            .handle
            .lock()
            .map_err(|_| native_error("native file store is unavailable"))?
            .as_ref()
            .cloned()
            .ok_or_else(|| native_error("native file store is closed"))?;

        Ok(AsyncTask::new(ReadRegularFileTask {
            root_handle,
            relative_path,
            maximum_bytes,
        }))
    }

    #[napi]
    pub fn close(&self) {
        if let Ok(mut root_handle) = self.root.handle.lock() {
            root_handle.take();
        }
    }
}

pub struct ReadRegularFileTask {
    root_handle: Arc<OwnedHandle>,
    relative_path: String,
    maximum_bytes: u32,
}

impl Task for ReadRegularFileTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        read_regular_file(&self.root_handle, &self.relative_path, self.maximum_bytes)
    }

    fn resolve(&mut self, _: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

fn native_error(reason: &'static str) -> Error {
    Error::from_reason(reason)
}

fn nt_success(status: NTSTATUS) -> bool {
    status >= 0
}

fn open_root(root_directory: &str) -> Result<HANDLE> {
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

    let result = open_nt_file(
        &nt_path,
        std::ptr::null_mut(),
        FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    );

    unsafe {
        RtlFreeUnicodeString(&mut nt_path);
    }

    let handle =
        OwnedHandle(result.map_err(|_| native_error("native file store root is unavailable"))?);
    let metadata = file_metadata(handle.0)?;

    if metadata.is_reparse || !metadata.is_directory || !is_ntfs(handle.0) {
        return Err(native_error("native file store root is unsupported"));
    }

    Ok(handle.into_raw())
}

fn open_nt_file(
    name: &UNICODE_STRING,
    root_directory: HANDLE,
    create_options: u32,
    share_access: u32,
) -> std::result::Result<HANDLE, ()> {
    let mut handle = std::ptr::null_mut();
    let mut io_status = IO_STATUS_BLOCK::default();
    let object_attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: root_directory,
        ObjectName: name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            FILE_GENERIC_READ,
            &object_attributes,
            &mut io_status,
            std::ptr::null(),
            0,
            share_access,
            FILE_OPEN,
            create_options,
            std::ptr::null(),
            0,
        )
    };

    if !nt_success(status) || handle.is_null() || handle == INVALID_HANDLE_VALUE {
        if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(handle);
            }
        }

        return Err(());
    }

    Ok(handle)
}

fn read_regular_file(
    root_handle: &OwnedHandle,
    relative_path: &str,
    maximum_bytes: u32,
) -> Result<Vec<u8>> {
    let segments: Vec<_> = relative_path.split(['/', '\\']).collect();
    let mut directories = Vec::with_capacity(segments.len().saturating_sub(1));
    let mut file = None;
    let mut parent_handle = root_handle.0;

    for (index, segment) in segments.iter().enumerate() {
        let segment_utf16: Vec<u16> = segment.encode_utf16().collect();
        let segment_name = unicode_string(&segment_utf16);
        let is_leaf = index + 1 == segments.len();
        let options = if is_leaf {
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT
        } else {
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT
        };
        let share_access = if is_leaf {
            FILE_SHARE_READ
        } else {
            FILE_SHARE_READ | FILE_SHARE_WRITE
        };
        let handle = OwnedHandle(
            open_nt_file(&segment_name, parent_handle, options, share_access)
                .map_err(|_| native_error("native file read failed"))?,
        );
        let metadata = file_metadata(handle.0)?;

        if metadata.is_reparse || metadata.is_directory == is_leaf {
            return Err(native_error("native file read rejected"));
        }

        if opened_name_is_private(handle.0)? || (is_leaf && metadata.number_of_links != 1) {
            return Err(native_error("native file read rejected"));
        }

        if is_leaf {
            file = Some(handle);
        } else {
            directories.push(handle);
            parent_handle = directories.last().expect("opened directory handle").0;
        }
    }

    let file = file.ok_or_else(|| native_error("native file read rejected"))?;
    let before = file_metadata(file.0)?;

    if before.size > maximum_bytes as u64 {
        return Err(native_error("native file exceeds maximum bytes"));
    }

    let mut bytes = vec![0; before.size as usize];

    if !bytes.is_empty() {
        let mut bytes_read = 0;
        let read_succeeded = unsafe {
            ReadFile(
                file.0,
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                &mut bytes_read,
                std::ptr::null_mut(),
            )
        };

        if read_succeeded == 0 || bytes_read as usize != bytes.len() {
            return Err(native_error("native file read failed"));
        }
    }

    let after = file_metadata(file.0)?;

    if before != after {
        return Err(native_error("native file changed during read"));
    }

    Ok(bytes)
}

struct FileMetadata {
    volume_serial_number: u64,
    file_id: [u8; 16],
    creation_time: i64,
    last_write_time: i64,
    change_time: i64,
    attributes: u32,
    size: u64,
    number_of_links: u32,
    is_directory: bool,
    is_reparse: bool,
}

fn file_metadata(handle: HANDLE) -> Result<FileMetadata> {
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

    Ok(FileMetadata {
        volume_serial_number: id.VolumeSerialNumber,
        file_id: id.FileId.Identifier,
        creation_time: basic.CreationTime,
        last_write_time: basic.LastWriteTime,
        change_time: basic.ChangeTime,
        attributes: basic.FileAttributes,
        size: standard.EndOfFile as u64,
        number_of_links: standard.NumberOfLinks,
        is_directory: standard.Directory || basic.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_reparse: basic.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
    })
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

    path.truncate(written as usize);

    let name = path
        .rsplit(|character| *character == b'/' as u16 || *character == b'\\' as u16)
        .next()
        .ok_or_else(|| native_error("native file name query failed"))?;
    let name =
        String::from_utf16(name).map_err(|_| native_error("native file name query failed"))?;

    Ok(is_private_segment(&name))
}

fn unicode_string(value: &[u16]) -> UNICODE_STRING {
    UNICODE_STRING {
        Length: (value.len() * 2) as u16,
        MaximumLength: (value.len() * 2) as u16,
        Buffer: value.as_ptr() as _,
    }
}

impl PartialEq for FileMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.volume_serial_number == other.volume_serial_number
            && self.file_id == other.file_id
            && self.creation_time == other.creation_time
            && self.last_write_time == other.last_write_time
            && self.change_time == other.change_time
            && self.attributes == other.attributes
            && self.size == other.size
            && self.number_of_links == other.number_of_links
            && self.is_directory == other.is_directory
            && self.is_reparse == other.is_reparse
    }
}

fn validate_relative_path(relative_path: &str) -> Result<()> {
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
            || segment.encode_utf16().count() > u16::MAX as usize / 2
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

fn validate_maximum_bytes(maximum_bytes: f64) -> Result<u32> {
    if !maximum_bytes.is_finite()
        || maximum_bytes <= 0.0
        || maximum_bytes.fract() != 0.0
        || maximum_bytes > u32::MAX as f64
    {
        return Err(native_error("maximum bytes is invalid"));
    }

    Ok(maximum_bytes as u32)
}
