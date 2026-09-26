//! Script-free, bounded tar extraction for verified engine artifacts.
//!
//! Archives are only parsed after their bytes matched the vendor digest.
//! Parsing still rejects anything but regular files and directories, unsafe
//! or colliding member names, bad header checksums, oversized expansion, and
//! trailing data, and it never follows or creates links.

use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{BufRead, Read, Write},
    path::Path,
};

use flate2::bufread::GzDecoder;
use sha2::{Digest, Sha256};

pub(crate) const MAX_ARCHIVE_ENTRIES: usize = 16_384;
const MAX_MEMBER_NAME_BYTES: usize = 255;

/// Bounded, path-free archive failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveError {
    Invalid,
    TargetMissing,
    TargetInvalid,
}

impl ArchiveError {
    /// Returns the stable classification.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Invalid => "archive_invalid",
            Self::TargetMissing => "archive_target_missing",
            Self::TargetInvalid => "archive_target_invalid",
        }
    }
}

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
    expanded_bound: u64,
) -> Result<(), ArchiveError> {
    let mut gzip = GzDecoder::new(reader);
    parse_tar(&mut gzip, extraction, expanded_bound)?;
    ensure_gzip_end(gzip)
}

pub(crate) fn ensure_gzip_end<R: BufRead>(mut gzip: GzDecoder<R>) -> Result<(), ArchiveError> {
    let mut byte = [0_u8; 1];
    match gzip.read(&mut byte) {
        Ok(0) => {}
        Ok(_) | Err(_) => return Err(ArchiveError::Invalid),
    }
    let mut buffered = gzip.into_inner();
    if !buffered
        .fill_buf()
        .map_err(|_| ArchiveError::Invalid)?
        .is_empty()
    {
        return Err(ArchiveError::Invalid);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    Regular,
    Directory,
}

pub(crate) fn parse_tar<R: Read>(
    reader: &mut R,
    extraction: Extraction<'_>,
    expanded_bound: u64,
) -> Result<(), ArchiveError> {
    let mut names = ArchiveNames::default();
    let mut expanded = 0_u64;
    let mut entries = 0_usize;
    let mut member_found = false;
    let mut header = [0_u8; 512];
    loop {
        reader
            .read_exact(&mut header)
            .map_err(|_| ArchiveError::Invalid)?;
        if header.iter().all(|byte| *byte == 0) {
            let mut terminal = [0_u8; 512];
            reader
                .read_exact(&mut terminal)
                .map_err(|_| ArchiveError::Invalid)?;
            if terminal.iter().any(|byte| *byte != 0) {
                return Err(ArchiveError::Invalid);
            }
            break;
        }
        entries = entries.checked_add(1).ok_or(ArchiveError::Invalid)?;
        if entries > MAX_ARCHIVE_ENTRIES {
            return Err(ArchiveError::Invalid);
        }
        validate_header(&header)?;
        let kind = match header[156] {
            0 | b'0' => EntryKind::Regular,
            b'5' => EntryKind::Directory,
            _ => return Err(ArchiveError::Invalid),
        };
        let size = parse_octal(&header[124..136])?;
        let mode = parse_octal(&header[100..108])?;
        let name = member_name(&header)?;
        names.register(&name, kind == EntryKind::Regular)?;
        expanded = expanded
            .checked_add(size)
            .filter(|total| *total <= expanded_bound)
            .ok_or(ArchiveError::Invalid)?;
        let written = match extraction {
            Extraction::Member { member, target } if name == member => {
                if kind != EntryKind::Regular || member_found {
                    return Err(ArchiveError::TargetInvalid);
                }
                write_member(reader, target, size, mode)?;
                member_found = true;
                true
            }
            Extraction::Tree { strip, destination } => {
                match name.strip_prefix(strip).filter(|rest| !rest.is_empty()) {
                    Some(relative) => {
                        let path = destination.join(relative);
                        if kind == EntryKind::Directory {
                            fs::create_dir_all(&path).map_err(|_| ArchiveError::TargetInvalid)?;
                            false
                        } else {
                            if let Some(parent) = path.parent() {
                                fs::create_dir_all(parent)
                                    .map_err(|_| ArchiveError::TargetInvalid)?;
                            }
                            write_member(reader, &path, size, mode)?;
                            true
                        }
                    }
                    None => false,
                }
            }
            Extraction::Member { .. } => false,
        };
        if !written {
            discard_exact(reader, size)?;
        }
        discard_padding(reader, size)?;
    }
    match extraction {
        Extraction::Member { .. } if !member_found => Err(ArchiveError::TargetMissing),
        _ => Ok(()),
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

fn validate_header(header: &[u8; 512]) -> Result<(), ArchiveError> {
    let stored = parse_octal(&header[148..156])?;
    let calculated = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum::<u64>();
    if stored != calculated {
        return Err(ArchiveError::Invalid);
    }
    for field in [
        &header[100..108],
        &header[108..116],
        &header[116..124],
        &header[124..136],
        &header[136..148],
    ] {
        parse_octal(field)?;
    }
    Ok(())
}

fn parse_octal(field: &[u8]) -> Result<u64, ArchiveError> {
    let mut value = 0_u64;
    let mut saw_digit = false;
    let mut ended = false;
    for byte in field {
        match *byte {
            b'0'..=b'7' if !ended => {
                saw_digit = true;
                value = value
                    .checked_mul(8)
                    .and_then(|value| value.checked_add(u64::from(*byte - b'0')))
                    .ok_or(ArchiveError::Invalid)?;
            }
            b' ' | 0 => {
                if saw_digit {
                    ended = true;
                }
            }
            _ => return Err(ArchiveError::Invalid),
        }
    }
    if saw_digit {
        Ok(value)
    } else {
        Err(ArchiveError::Invalid)
    }
}

fn member_name(header: &[u8; 512]) -> Result<String, ArchiveError> {
    let prefix = text_field(&header[345..500])?;
    let name = text_field(&header[0..100])?;
    let combined = if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    };
    let combined = combined.strip_suffix('/').unwrap_or(&combined).to_owned();
    if !is_safe_member_name(&combined) {
        return Err(ArchiveError::Invalid);
    }
    Ok(combined)
}

fn text_field(field: &[u8]) -> Result<String, ArchiveError> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    if field[end..].iter().any(|byte| *byte != 0) {
        return Err(ArchiveError::Invalid);
    }
    String::from_utf8(field[..end].to_vec()).map_err(|_| ArchiveError::Invalid)
}

pub(crate) fn is_safe_member_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > MAX_MEMBER_NAME_BYTES
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
        let folded = fold_name(name);
        if !self.entries.insert(folded.clone()) {
            return Err(ArchiveError::Invalid);
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
                    return Err(ArchiveError::Invalid);
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
            return Err(ArchiveError::Invalid);
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
            .map_err(|_| ArchiveError::Invalid)?;
        let read = reader
            .read(&mut buffer[..length])
            .map_err(|_| ArchiveError::Invalid)?;
        if read == 0 {
            return Err(ArchiveError::Invalid);
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|_| ArchiveError::TargetInvalid)?;
        remaining -= read as u64;
    }
    Ok(())
}

fn discard_exact<R: Read>(reader: &mut R, mut remaining: u64) -> Result<(), ArchiveError> {
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let length = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| ArchiveError::Invalid)?;
        let read = reader
            .read(&mut buffer[..length])
            .map_err(|_| ArchiveError::Invalid)?;
        if read == 0 {
            return Err(ArchiveError::Invalid);
        }
        remaining -= read as u64;
    }
    Ok(())
}

fn discard_padding<R: Read>(reader: &mut R, size: u64) -> Result<(), ArchiveError> {
    let padding = (512 - (size % 512)) % 512;
    let mut buffer = [0_u8; 512];
    let length = usize::try_from(padding).map_err(|_| ArchiveError::Invalid)?;
    reader
        .read_exact(&mut buffer[..length])
        .map_err(|_| ArchiveError::Invalid)?;
    if buffer[..length].iter().any(|byte| *byte != 0) {
        return Err(ArchiveError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
#[path = "archive_tests.rs"]
pub(crate) mod tests;
