//! Script-free, bounded tar extraction for verified engine artifacts.
//!
//! Archives are only parsed after their bytes matched the vendor digest or
//! the trust-on-first-download record. Parsing is bounded by the engine's
//! [`ArchivePolicy`] (entry count, total expanded size, member name length,
//! and which metadata forms the vendor's tar writer uses). It accepts POSIX
//! ustar and GNU headers, PAX `path`/`size` records, and GNU long names, and
//! still rejects every entry that is not a regular file or a directory,
//! unsafe or colliding member names, bad header checksums, and non-zero
//! trailing data. It never follows or creates links, so a member can only
//! ever land inside the destination. Every rejection is a typed
//! [`ArchiveError`] naming the rule, the limit, and the archive member.

use std::{
    collections::HashSet,
    fmt,
    fs::{self, OpenOptions},
    io::{self, BufRead, Read, Write},
    path::Path,
};

use flate2::bufread::GzDecoder;
use sha2::{Digest, Sha256};

use super::catalog::ArchivePolicy;

#[path = "archive_header.rs"]
mod header;

use header::{EntryType, Header, PaxRecords, read_block};

/// Longest archive member name carried in an error, in bytes.
const MAX_REPORTED_NAME_BYTES: usize = 200;
/// Largest PAX or GNU long-name metadata entry.
const MAX_METADATA_BYTES: u64 = 64 * 1024;
/// Largest run of zero bytes accepted after the end-of-archive marker (tar
/// writers pad to their record size, 10 KiB for GNU tar).
const MAX_TRAILING_PADDING: u64 = 1024 * 1024;

/// Why an archive was refused. Member names are archive-relative and
/// bounded; no host path or URL is ever carried.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArchiveError {
    /// More headers than the engine's policy allows.
    TooManyEntries { entries: u64, limit: u64 },
    /// The members expand beyond the engine's policy.
    ExpandedTooLarge { bytes: u64, limit: u64 },
    /// An entry type the engine's policy does not allow (links, devices,
    /// or metadata forms its vendor does not use).
    UnsupportedEntry { kind: &'static str, path: String },
    /// An absolute, traversing, or otherwise unsafe member name.
    UnsafeName { path: String },
    /// A member name longer than the engine's policy allows.
    NameTooLong { length: usize, limit: usize },
    /// A duplicate member, a case-insensitive collision, or a member below a
    /// regular file.
    NameCollision { path: String },
    /// A PAX or GNU long-name entry larger than [`MAX_METADATA_BYTES`].
    MetadataTooLarge { bytes: u64, limit: u64 },
    /// A header that is neither POSIX ustar nor GNU tar.
    UnsupportedFormat { entry: u64 },
    /// A malformed header field or metadata record.
    Malformed { entry: u64, field: &'static str },
    /// The archive ended early.
    Truncated,
    /// The gzip stream is corrupt.
    Compression,
    /// Data after the end of the archive or the gzip stream.
    TrailingData,
    /// The requested member or entry executable is missing.
    TargetMissing,
    /// The target could not be written or is not a regular file.
    TargetInvalid,
}

impl ArchiveError {
    /// Returns the stable classification.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TooManyEntries { .. } => "too_many_entries",
            Self::ExpandedTooLarge { .. } => "expanded_too_large",
            Self::UnsupportedEntry { .. } => "unsupported_entry",
            Self::UnsafeName { .. } => "unsafe_name",
            Self::NameTooLong { .. } => "name_too_long",
            Self::NameCollision { .. } => "name_collision",
            Self::MetadataTooLarge { .. } => "metadata_too_large",
            Self::UnsupportedFormat { .. } => "archive_format_unsupported",
            Self::Malformed { .. } => "archive_malformed",
            Self::Truncated => "archive_truncated",
            Self::Compression => "archive_compression_invalid",
            Self::TrailingData => "archive_trailing_data",
            Self::TargetMissing => "archive_target_missing",
            Self::TargetInvalid => "archive_target_invalid",
        }
    }
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        match self {
            Self::TooManyEntries { entries, limit } => {
                write!(formatter, "{code}: {entries} entries, limit {limit}")
            }
            Self::ExpandedTooLarge { bytes, limit } | Self::MetadataTooLarge { bytes, limit } => {
                write!(formatter, "{code}: {bytes} bytes, limit {limit}")
            }
            Self::UnsupportedEntry { kind, path } => write!(formatter, "{code}: {kind} {path}"),
            Self::UnsafeName { path } | Self::NameCollision { path } => {
                write!(formatter, "{code}: {path}")
            }
            Self::NameTooLong { length, limit } => {
                write!(formatter, "{code}: {length} bytes, limit {limit}")
            }
            Self::UnsupportedFormat { entry } => write!(formatter, "{code}: entry {entry}"),
            Self::Malformed { entry, field } => {
                write!(formatter, "{code}: {field} of entry {entry}")
            }
            Self::Truncated
            | Self::Compression
            | Self::TrailingData
            | Self::TargetMissing
            | Self::TargetInvalid => formatter.write_str(code),
        }
    }
}

impl std::error::Error for ArchiveError {}

/// What to take out of the archive.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Extraction<'a> {
    /// Exactly one member, written to `target`.
    Member { member: &'a str, target: &'a Path },
    /// Every member below `strip`, written below `destination`.
    Tree {
        strip: &'a str,
        destination: &'a Path,
    },
}

/// Extracts from a gzip tar stream, then requires the gzip end.
pub(crate) fn extract_gzip_tar<R: BufRead>(
    reader: R,
    extraction: Extraction<'_>,
    policy: &ArchivePolicy,
) -> Result<(), ArchiveError> {
    let mut gzip = GzDecoder::new(reader);
    parse_tar(&mut gzip, extraction, policy)?;
    ensure_gzip_end(gzip)
}

fn ensure_gzip_end<R: BufRead>(gzip: GzDecoder<R>) -> Result<(), ArchiveError> {
    let mut buffered = gzip.into_inner();
    if !buffered
        .fill_buf()
        .map_err(|_| ArchiveError::Truncated)?
        .is_empty()
    {
        return Err(ArchiveError::TrailingData);
    }
    Ok(())
}

/// Maps a read failure: an early end is truncation, anything else a corrupt
/// compression stream.
fn read_error(error: &io::Error) -> ArchiveError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        ArchiveError::Truncated
    } else {
        ArchiveError::Compression
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MemberKind {
    Regular,
    Directory,
}

/// Parses a tar stream to its end, extracting as requested, then requires
/// that only zero padding follows (reading the source to its end, which
/// also checks a gzip trailer).
pub(crate) fn parse_tar<R: Read>(
    reader: &mut R,
    extraction: Extraction<'_>,
    policy: &ArchivePolicy,
) -> Result<(), ArchiveError> {
    let mut names = ArchiveNames::default();
    let mut expanded = 0_u64;
    let mut entries = 0_u64;
    let mut member_found = false;
    let mut long_name: Option<String> = None;
    let mut pax = PaxRecords::default();
    loop {
        let block = read_block(reader)?;
        if block.iter().all(|byte| *byte == 0) {
            if read_block(reader)?.iter().any(|byte| *byte != 0) {
                return Err(ArchiveError::Malformed {
                    entry: entries + 1,
                    field: "end-of-archive marker",
                });
            }
            break;
        }
        entries += 1;
        if entries > policy.max_entries {
            return Err(ArchiveError::TooManyEntries {
                entries,
                limit: policy.max_entries,
            });
        }
        let header = Header::parse(&block, entries)?;
        match header.entry_type {
            EntryType::GnuLongName if policy.entry_types.gnu_long_names => {
                let bytes = read_metadata(reader, header.size)?;
                long_name = Some(header::long_name(&bytes, entries)?);
                continue;
            }
            EntryType::PaxLocal if policy.entry_types.pax_headers => {
                let bytes = read_metadata(reader, header.size)?;
                pax = PaxRecords::parse(&bytes, entries)?;
                continue;
            }
            EntryType::PaxGlobal if policy.entry_types.pax_headers => {
                // Global records only carry defaults such as comments or
                // times; a global `path` is meaningless and is ignored.
                read_metadata(reader, header.size)?;
                continue;
            }
            _ => {}
        }
        let raw_name = pax
            .path
            .take()
            .or_else(|| long_name.take())
            .unwrap_or(header.name);
        let size = pax.size.take().unwrap_or(header.size);
        let name = member_name(&raw_name, policy)?;
        let kind = match header.entry_type {
            EntryType::Regular => MemberKind::Regular,
            EntryType::Directory if size == 0 => MemberKind::Directory,
            EntryType::Directory => {
                return Err(ArchiveError::Malformed {
                    entry: entries,
                    field: "directory size",
                });
            }
            other => {
                return Err(ArchiveError::UnsupportedEntry {
                    kind: other.name(),
                    path: reported(&name),
                });
            }
        };
        names.register(&name, kind == MemberKind::Regular)?;
        if kind == MemberKind::Regular {
            expanded = expanded
                .checked_add(size)
                .filter(|total| *total <= policy.expanded_bound_bytes)
                .ok_or(ArchiveError::ExpandedTooLarge {
                    bytes: expanded.saturating_add(size),
                    limit: policy.expanded_bound_bytes,
                })?;
        }
        let member = Member {
            name: &name,
            kind,
            size,
            mode: header.mode,
        };
        let written = extract_member(reader, extraction, &member, &mut member_found)?;
        if !written {
            discard_exact(reader, size)?;
        }
        discard_padding(reader, size, entries)?;
    }
    if long_name.is_some() || !pax.is_empty() {
        return Err(ArchiveError::Malformed {
            entry: entries,
            field: "metadata without a member",
        });
    }
    require_zero_padding(reader)?;
    match extraction {
        Extraction::Member { .. } if !member_found => Err(ArchiveError::TargetMissing),
        _ => Ok(()),
    }
}

/// One validated member about to be extracted or skipped.
struct Member<'a> {
    name: &'a str,
    kind: MemberKind,
    size: u64,
    mode: u64,
}

/// Writes `member` when the extraction wants it; returns whether its data
/// was consumed.
fn extract_member<R: Read>(
    reader: &mut R,
    extraction: Extraction<'_>,
    member: &Member<'_>,
    member_found: &mut bool,
) -> Result<bool, ArchiveError> {
    match extraction {
        Extraction::Member {
            member: wanted,
            target,
        } if member.name == wanted => {
            if member.kind != MemberKind::Regular || *member_found {
                return Err(ArchiveError::TargetInvalid);
            }
            write_member(reader, target, member.size, member.mode)?;
            *member_found = true;
            Ok(true)
        }
        Extraction::Member { .. } => Ok(false),
        Extraction::Tree { strip, destination } => {
            let Some(relative) = member
                .name
                .strip_prefix(strip)
                .filter(|rest| !rest.is_empty())
            else {
                return Ok(false);
            };
            let path = destination.join(relative);
            if member.kind == MemberKind::Directory {
                fs::create_dir_all(&path).map_err(|_| ArchiveError::TargetInvalid)?;
                return Ok(false);
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|_| ArchiveError::TargetInvalid)?;
            }
            write_member(reader, &path, member.size, member.mode)?;
            Ok(true)
        }
    }
}

fn member_name(raw: &str, policy: &ArchivePolicy) -> Result<String, ArchiveError> {
    let name = raw.strip_suffix('/').unwrap_or(raw);
    if name.len() > policy.max_name_bytes {
        return Err(ArchiveError::NameTooLong {
            length: name.len(),
            limit: policy.max_name_bytes,
        });
    }
    if !is_safe_member_name(name) {
        return Err(ArchiveError::UnsafeName {
            path: reported(name),
        });
    }
    Ok(name.to_owned())
}

/// Bounds a member name carried in an error.
fn reported(name: &str) -> String {
    let mut end = name.len().min(MAX_REPORTED_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end]
        .chars()
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect()
}

fn read_metadata<R: Read>(reader: &mut R, size: u64) -> Result<Vec<u8>, ArchiveError> {
    if size > MAX_METADATA_BYTES {
        return Err(ArchiveError::MetadataTooLarge {
            bytes: size,
            limit: MAX_METADATA_BYTES,
        });
    }
    let length = usize::try_from(size).map_err(|_| ArchiveError::MetadataTooLarge {
        bytes: size,
        limit: MAX_METADATA_BYTES,
    })?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| read_error(&error))?;
    discard_padding(reader, size, 0)?;
    Ok(bytes)
}

/// Accepts only zero bytes, bounded, up to the end of the stream.
fn require_zero_padding<R: Read>(reader: &mut R) -> Result<(), ArchiveError> {
    let mut buffer = [0_u8; 8 * 1024];
    let mut total = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| read_error(&error))?;
        if read == 0 {
            return Ok(());
        }
        total += read as u64;
        if total > MAX_TRAILING_PADDING || buffer[..read].iter().any(|byte| *byte != 0) {
            return Err(ArchiveError::TrailingData);
        }
    }
}

fn write_member<R: Read>(
    reader: &mut R,
    target: &Path,
    size: u64,
    mode: u64,
) -> Result<(), ArchiveError> {
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)
        .map_err(|_| ArchiveError::TargetInvalid)?;
    copy_exact(reader, &mut output, size)?;
    output
        .flush()
        .and_then(|()| output.sync_all())
        .map_err(|_| ArchiveError::TargetInvalid)?;
    set_mode(target, mode)
}

#[cfg(unix)]
fn set_mode(target: &Path, mode: u64) -> Result<(), ArchiveError> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = if mode & 0o111 == 0 { 0o644 } else { 0o755 };
    fs::set_permissions(target, fs::Permissions::from_mode(permissions))
        .map_err(|_| ArchiveError::TargetInvalid)
}

#[cfg(not(unix))]
fn set_mode(_target: &Path, _mode: u64) -> Result<(), ArchiveError> {
    Ok(())
}

/// Returns the SHA-256 and size of a file written during extraction.
pub(crate) fn measure(path: &Path) -> Result<([u8; 32], u64), ArchiveError> {
    let mut file = fs::File::open(path).map_err(|_| ArchiveError::TargetMissing)?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| ArchiveError::TargetInvalid)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((hasher.finalize().into(), total))
}

/// Returns whether `name` is a relative, normalized, portable member name.
pub(crate) fn is_safe_member_name(name: &str) -> bool {
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
    {
        return false;
    }
    name.split('/').all(|component| {
        !component.is_empty() && component != "." && component != ".." && !component.contains(':')
    })
}

#[derive(Default)]
struct ArchiveNames {
    entries: HashSet<String>,
    files: HashSet<String>,
}

impl ArchiveNames {
    fn register(&mut self, name: &str, is_file: bool) -> Result<(), ArchiveError> {
        let collision = || ArchiveError::NameCollision {
            path: reported(name),
        };
        let folded = fold_name(name);
        if !self.entries.insert(folded.clone()) {
            return Err(collision());
        }
        let mut prefix = String::new();
        let mut components = name.split('/').peekable();
        while let Some(component) = components.next() {
            if components.peek().is_some() {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(component);
                if self.files.contains(&fold_name(&prefix)) {
                    return Err(collision());
                }
            }
        }
        if is_file
            && self.entries.iter().any(|entry| {
                entry
                    .strip_prefix(folded.as_str())
                    .is_some_and(|suffix| suffix.starts_with('/'))
            })
        {
            return Err(collision());
        }
        if is_file {
            self.files.insert(folded);
        }
        Ok(())
    }
}

fn fold_name(name: &str) -> String {
    name.chars().flat_map(char::to_lowercase).collect()
}

fn copy_exact<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    mut remaining: u64,
) -> Result<(), ArchiveError> {
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let length = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| ArchiveError::Truncated)?;
        let read = reader
            .read(&mut buffer[..length])
            .map_err(|error| read_error(&error))?;
        if read == 0 {
            return Err(ArchiveError::Truncated);
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|_| ArchiveError::TargetInvalid)?;
        remaining -= read as u64;
    }
    Ok(())
}

fn discard_exact<R: Read>(reader: &mut R, remaining: u64) -> Result<(), ArchiveError> {
    let copied = io::copy(&mut reader.take(remaining), &mut io::sink())
        .map_err(|error| read_error(&error))?;
    if copied == remaining {
        Ok(())
    } else {
        Err(ArchiveError::Truncated)
    }
}

fn discard_padding<R: Read>(reader: &mut R, size: u64, entry: u64) -> Result<(), ArchiveError> {
    let padding = (512 - (size % 512)) % 512;
    let mut buffer = [0_u8; 512];
    let length = usize::try_from(padding).map_err(|_| ArchiveError::Truncated)?;
    reader
        .read_exact(&mut buffer[..length])
        .map_err(|error| read_error(&error))?;
    if buffer[..length].iter().any(|byte| *byte != 0) {
        return Err(ArchiveError::Malformed {
            entry,
            field: "block padding",
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "archive_tests.rs"]
pub(crate) mod tests;
