//! Tar header decoding: POSIX ustar and GNU headers, numeric fields in octal
//! or base-256, and the PAX and GNU metadata that carry long names.

use std::io::Read;

use super::{ArchiveError, read_error};

/// The type flag of one header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EntryType {
    Regular,
    Directory,
    HardLink,
    SymbolicLink,
    CharacterDevice,
    BlockDevice,
    Fifo,
    GnuLongName,
    GnuLongLink,
    PaxLocal,
    PaxGlobal,
    Other,
}

impl EntryType {
    const fn from_flag(flag: u8) -> Self {
        match flag {
            0 | b'0' | b'7' => Self::Regular,
            b'5' => Self::Directory,
            b'1' => Self::HardLink,
            b'2' => Self::SymbolicLink,
            b'3' => Self::CharacterDevice,
            b'4' => Self::BlockDevice,
            b'6' => Self::Fifo,
            b'L' => Self::GnuLongName,
            b'K' => Self::GnuLongLink,
            b'x' => Self::PaxLocal,
            b'g' => Self::PaxGlobal,
            _ => Self::Other,
        }
    }

    /// Returns the stable name used in [`ArchiveError::UnsupportedEntry`].
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Regular => "file",
            Self::Directory => "directory",
            Self::HardLink => "hardlink",
            Self::SymbolicLink => "symlink",
            Self::CharacterDevice => "character_device",
            Self::BlockDevice => "block_device",
            Self::Fifo => "fifo",
            Self::GnuLongName => "gnu_long_name",
            Self::GnuLongLink => "gnu_long_link",
            Self::PaxLocal => "pax_header",
            Self::PaxGlobal => "pax_global_header",
            Self::Other => "unknown",
        }
    }
}

/// One decoded, checksum-verified header.
#[derive(Debug)]
pub(super) struct Header {
    pub(super) name: String,
    pub(super) size: u64,
    pub(super) mode: u64,
    pub(super) entry_type: EntryType,
}

impl Header {
    /// Decodes a non-zero header block. `entry` numbers the header for
    /// diagnostics.
    pub(super) fn parse(block: &[u8; 512], entry: u64) -> Result<Self, ArchiveError> {
        let malformed = |field| ArchiveError::Malformed { entry, field };
        let posix = &block[257..263] == b"ustar\0";
        let gnu = &block[257..265] == b"ustar  \0";
        if !posix && !gnu {
            return Err(ArchiveError::UnsupportedFormat { entry });
        }
        let required = |range: std::ops::Range<usize>, field| match number(&block[range]) {
            Ok(Some(value)) => Ok(value),
            Ok(None) | Err(InvalidNumber) => Err(malformed(field)),
        };
        let stored = required(148..156, "checksum")?;
        if !checksum_matches(block, stored) {
            return Err(malformed("checksum"));
        }
        let size = required(124..136, "size")?;
        let mode = number(&block[100..108])
            .map_err(|InvalidNumber| malformed("mode"))?
            .unwrap_or(0o644);
        // Owner and time fields are never used, but must still be numbers
        // when present (npm leaves the owner fields empty).
        for (range, field) in [(108..116, "uid"), (116..124, "gid"), (136..148, "mtime")] {
            number(&block[range]).map_err(|InvalidNumber| malformed(field))?;
        }
        let name = text(&block[0..100]).ok_or_else(|| malformed("name"))?;
        // Only POSIX ustar has a name prefix; GNU headers keep other data
        // at that offset.
        let prefix = if posix {
            text(&block[345..500]).ok_or_else(|| malformed("prefix"))?
        } else {
            String::new()
        };
        let name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        Ok(Self {
            name,
            size,
            mode,
            entry_type: EntryType::from_flag(block[156]),
        })
    }
}

/// Reads one 512-byte block.
pub(super) fn read_block<R: Read>(reader: &mut R) -> Result<[u8; 512], ArchiveError> {
    let mut block = [0_u8; 512];
    reader
        .read_exact(&mut block)
        .map_err(|error| read_error(&error))?;
    Ok(block)
}

/// Accepts the POSIX unsigned sum and the historic signed sum.
fn checksum_matches(block: &[u8; 512], stored: u64) -> bool {
    let field = 148..156;
    let unsigned = block
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if field.contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum::<u64>();
    let signed = block
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if field.contains(&index) {
                i64::from(b' ')
            } else {
                i64::from(byte.cast_signed())
            }
        })
        .sum::<i64>();
    stored == unsigned || i64::try_from(stored).is_ok_and(|stored| stored == signed)
}

/// A numeric field that is neither octal nor non-negative base-256.
struct InvalidNumber;

/// Decodes a numeric field; an empty field (only spaces and NULs) is
/// `None`. Octal digits may be surrounded by spaces and NULs; a set high bit
/// marks GNU base-256.
fn number(field: &[u8]) -> Result<Option<u64>, InvalidNumber> {
    let push = |value: u64, radix: u64, digit: u64| {
        value
            .checked_mul(radix)
            .and_then(|value| value.checked_add(digit))
            .ok_or(InvalidNumber)
    };
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        // Base-256: only non-negative values that fit in 64 bits.
        if field[0] != 0x80 {
            return Err(InvalidNumber);
        }
        return field[1..]
            .iter()
            .try_fold(0, |value, byte| push(value, 256, u64::from(*byte)))
            .map(Some);
    }
    let blank = |byte: &u8| matches!(byte, b' ' | 0);
    let leading = field.iter().take_while(|byte| blank(byte)).count();
    let digits = field[leading..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if !field[leading + digits..].iter().all(blank) {
        return Err(InvalidNumber);
    }
    if digits == 0 {
        return Ok(None);
    }
    field[leading..leading + digits]
        .iter()
        .try_fold(0, |value, digit| match digit {
            b'0'..=b'7' => push(value, 8, u64::from(digit - b'0')),
            _ => Err(InvalidNumber),
        })
        .map(Some)
}

/// Decodes a NUL-terminated UTF-8 field; bytes after the terminator must be
/// zero.
fn text(field: &[u8]) -> Option<String> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    if field[end..].iter().any(|byte| *byte != 0) {
        return None;
    }
    String::from_utf8(field[..end].to_vec()).ok()
}

/// Decodes the payload of a GNU long-name entry.
pub(super) fn long_name(bytes: &[u8], entry: u64) -> Result<String, ArchiveError> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8(bytes[..end].to_vec()).map_err(|_| ArchiveError::Malformed {
        entry,
        field: "long name",
    })
}

/// The PAX records that apply to the next member. Every other key (times,
/// owners, vendor extensions) is ignored; `linkpath` only matters for link
/// entries, which are rejected.
#[derive(Debug, Default)]
pub(super) struct PaxRecords {
    pub(super) path: Option<String>,
    pub(super) size: Option<u64>,
}

impl PaxRecords {
    pub(super) const fn is_empty(&self) -> bool {
        self.path.is_none() && self.size.is_none()
    }

    /// Parses `"<length> <key>=<value>\n"` records.
    pub(super) fn parse(bytes: &[u8], entry: u64) -> Result<Self, ArchiveError> {
        let malformed = || ArchiveError::Malformed {
            entry,
            field: "pax record",
        };
        let mut records = Self::default();
        let mut rest = bytes;
        while !rest.is_empty() {
            let space = rest
                .iter()
                .position(|byte| *byte == b' ')
                .ok_or_else(malformed)?;
            let length = std::str::from_utf8(&rest[..space])
                .ok()
                .and_then(|length| length.parse::<usize>().ok())
                .filter(|length| *length > space + 1 && *length <= rest.len())
                .ok_or_else(malformed)?;
            let record = &rest[space + 1..length];
            let record = record.strip_suffix(b"\n").ok_or_else(malformed)?;
            let equals = record
                .iter()
                .position(|byte| *byte == b'=')
                .ok_or_else(malformed)?;
            let value = &record[equals + 1..];
            match &record[..equals] {
                b"path" => {
                    records.path =
                        Some(String::from_utf8(value.to_vec()).map_err(|_| malformed())?);
                }
                b"size" => {
                    records.size = Some(
                        std::str::from_utf8(value)
                            .ok()
                            .and_then(|size| size.parse::<u64>().ok())
                            .ok_or_else(malformed)?,
                    );
                }
                _ => {}
            }
            rest = &rest[length..];
        }
        Ok(records)
    }
}
