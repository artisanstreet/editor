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
/// On Windows, stable Rust does not yet expose the volume serial number and
/// file index fields, so the same open handle is inherited as standard input
/// by a short PowerShell helper that calls `GetFileInformationByHandle`.
/// Neither path is passed to that helper.
///
/// # Errors
///
/// Returns the operating-system metadata error, or an error from the
/// descriptor-bound Windows helper and its exact `device:inode` response.
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
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            WINDOWS_FILE_IDENTITY_SCRIPT,
        ])
        .stdin(std::process::Stdio::from(file.try_clone()?))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()?;

    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let message = if detail.is_empty() {
            format!("file identity helper exited with {}", output.status)
        } else {
            format!(
                "file identity helper exited with {}: {detail}",
                output.status
            )
        };
        return Err(io::Error::other(message));
    }

    parse_windows_file_identity(&output.stdout)
}

#[cfg(windows)]
fn parse_windows_file_identity(output: &[u8]) -> io::Result<FileIdentity> {
    let text = std::str::from_utf8(output).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file identity helper returned non-UTF-8 output: {error}"),
        )
    })?;
    let (device, inode) = text.trim().split_once(':').ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "file identity helper returned an invalid device:inode pair",
        )
    })?;
    let device = device.parse::<u64>().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file identity helper returned an invalid device: {error}"),
        )
    })?;
    let inode = inode.parse::<u64>().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file identity helper returned an invalid inode: {error}"),
        )
    })?;

    Ok(FileIdentity::new(device, inode))
}

#[cfg(windows)]
const WINDOWS_FILE_IDENTITY_SCRIPT: &str = r#"
$ErrorActionPreference = "Stop"
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public static class ArtisanFileIdentity {
    [StructLayout(LayoutKind.Sequential)]
    private struct FileTime {
        public uint Low;
        public uint High;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ByHandleFileInformation {
        public uint FileAttributes;
        public FileTime CreationTime;
        public FileTime LastAccessTime;
        public FileTime LastWriteTime;
        public uint VolumeSerialNumber;
        public uint FileSizeHigh;
        public uint FileSizeLow;
        public uint NumberOfLinks;
        public uint FileIndexHigh;
        public uint FileIndexLow;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr GetStdHandle(int standardHandle);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandle(
        SafeFileHandle handle,
        out ByHandleFileInformation information
    );

    public static void WriteIdentity() {
        using (var handle = new SafeFileHandle(GetStdHandle(-10), false)) {
            ByHandleFileInformation information;
            if (!GetFileInformationByHandle(handle, out information)) {
                throw new Win32Exception(Marshal.GetLastWin32Error());
            }

            ulong inode = ((ulong)information.FileIndexHigh << 32) | information.FileIndexLow;
            Console.Write(
                information.VolumeSerialNumber.ToString(
                    System.Globalization.CultureInfo.InvariantCulture
                )
                + ":"
                + inode.ToString(System.Globalization.CultureInfo.InvariantCulture)
            );
        }
    }
}
'@
[ArtisanFileIdentity]::WriteIdentity()
"#;
