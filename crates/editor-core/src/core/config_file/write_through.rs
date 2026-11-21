use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;
use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_NORMAL, MOVE_FILE_FLAGS, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    MoveFileExW, SetFileAttributesW,
};
use windows::core::PCWSTR;

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub(super) fn rename(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    let flags = if replace {
        MOVEFILE_WRITE_THROUGH | MOVEFILE_REPLACE_EXISTING
    } else {
        MOVEFILE_WRITE_THROUGH
    };
    let source = wide(from);
    unsafe { SetFileAttributesW(PCWSTR(source.as_ptr()), FILE_ATTRIBUTE_NORMAL) }
        .map_err(io_error)?;
    move_file(&source, &wide(to), flags)
}

pub(super) fn move_folder(from: &Path, to: &Path) -> io::Result<()> {
    move_file(
        &wide(from),
        &wide(to),
        MOVEFILE_WRITE_THROUGH | MOVEFILE_REPLACE_EXISTING,
    )
}

fn move_file(from: &[u16], to: &[u16], flags: MOVE_FILE_FLAGS) -> io::Result<()> {
    unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), flags) }.map_err(io_error)
}

fn io_error(error: windows::core::Error) -> io::Error {
    WIN32_ERROR::from_error(&error).map_or_else(
        || io::Error::other(error),
        |code| io::Error::from_raw_os_error(code.0.cast_signed()),
    )
}
