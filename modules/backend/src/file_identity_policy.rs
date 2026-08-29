#![forbid(unsafe_code)]

//! Exact identity for an already-open file descriptor.
//!
//! The filesystem consumers open a file before checking its identity and keep
//! that descriptor alive while they perform a conditional operation. This
//! module therefore accepts a [`File`] rather than a path: the metadata read is
//! bound to the object represented by the open descriptor and never resolves a
//! path again.

use std::fs::File;
use std::io;

/// Identifies one opened file by its unsigned 64-bit device and inode values.
///
/// The fields intentionally remain separate. A file is the same file only
/// when both values match; matching either value alone is insufficient across
/// devices or inode/file-index reuse.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileIdentity {
    /// The filesystem device or Windows volume serial number.
    pub device: u64,
    /// The filesystem inode or Windows file index.
    pub inode: u64,
}

impl FileIdentity {
    /// Creates an identity from its exact unsigned components.
    #[must_use]
    pub const fn new(device: u64, inode: u64) -> Self {
        Self { device, inode }
    }
}

const UINT64_MODULUS: i128 = 1_i128 << 64;

/// Normalizes an integer representation to its low 64 bits.
///
/// Native metadata APIs may expose a signed representation even though the
/// identity is an unsigned 64-bit value. Euclidean remainder gives the same
/// result as adding `2^64` to a negative signed value, while also making the
/// modulo boundary explicit. The bound accepts the standard signed and
/// unsigned integer types that fit losslessly into `i128`, including `i64`
/// native values and `u64` values returned by Rust's Unix metadata extension.
#[must_use]
pub fn normalize_uint64<T>(value: T) -> u64
where
    T: Into<i128>,
{
    let normalized = value.into().rem_euclid(UINT64_MODULUS);
    u64::try_from(normalized).unwrap_or_default()
}

/// Compares two identities by device and inode, requiring both to match.
#[must_use]
pub const fn same_file_identity(left: FileIdentity, right: FileIdentity) -> bool {
    left.device == right.device && left.inode == right.inode
}

/// Reads exact identity metadata from an already-open file.
///
/// On Unix, [`File::metadata`] performs the descriptor-bound `fstat` call and
/// its stable metadata extensions expose the exact device and inode values.
/// On Windows, the safe `winapi-util` wrapper calls
/// `GetFileInformationByHandle` for the same open handle. Neither path is
/// passed to that wrapper.
///
/// # Errors
///
/// Returns the operating-system metadata error from the descriptor-bound
/// metadata query.
pub fn read_file_identity(file: &File) -> io::Result<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = file.metadata()?;
        Ok(FileIdentity::new(
            normalize_uint64(metadata.dev()),
            normalize_uint64(metadata.ino()),
        ))
    }

    #[cfg(windows)]
    {
        read_windows_file_identity(file)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "exact file identity is unsupported on this platform",
        ))
    }
}

#[cfg(windows)]
fn read_windows_file_identity(file: &File) -> io::Result<FileIdentity> {
    let information = winapi_util::file::information(winapi_util::HandleRef::from_file(file))?;
    Ok(FileIdentity::new(
        information.volume_serial_number(),
        information.file_index(),
    ))
}
