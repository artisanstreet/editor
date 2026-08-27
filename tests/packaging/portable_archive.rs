//! Deterministic portable archive proof.
//!
//! Verifies the stored ZIP produced by `//packaging/portable:portable_archive`
//! contains exactly the two Bazel-built Windows binaries under contract-derived
//! names, with deterministic metadata and byte-identical fixture.

use runfiles::{Runfiles, rlocation};
use std::collections::HashSet;
use std::path::PathBuf;

const LAYOUT_TEXT: &str = include_str!("../../packaging/portable/portable_layout.txt");

const EOCD_SIG: u32 = 0x0605_4b50;
const CDH_SIG: u32 = 0x0201_4b50;
const LFH_SIG: u32 = 0x0403_4b50;
const FIXED_DOS_TIMESTAMP: u32 = 0x3c21_0000;

fn contract_names() -> (String, String, String) {
    assert!(!LAYOUT_TEXT.contains('\r'), "contract contains CR");
    let mut package_root: Option<String> = None;
    let mut editor_sibling: Option<String> = None;
    let mut forge_sibling: Option<String> = None;
    for raw_line in LAYOUT_TEXT.lines() {
        if raw_line.is_empty() || raw_line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = raw_line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        match key {
            "package_root" => {
                assert!(package_root.is_none(), "duplicate package_root");
                package_root = Some(value.to_owned());
            }
            "editor_sibling" => {
                assert!(editor_sibling.is_none(), "duplicate editor_sibling");
                editor_sibling = Some(value.to_owned());
            }
            "forge_sibling" => {
                assert!(forge_sibling.is_none(), "duplicate forge_sibling");
                forge_sibling = Some(value.to_owned());
            }
            _ => {}
        }
    }
    let pr = package_root.expect("missing package_root");
    let es = editor_sibling.expect("missing editor_sibling");
    let fs = forge_sibling.expect("missing forge_sibling");
    for name in [&pr, &es, &fs] {
        assert!(
            !name.is_empty() && name.len() <= 255,
            "invalid name length {name:?}"
        );
        for &b in name.as_bytes() {
            let allowed = b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-');
            assert!(allowed, "disallowed byte {b:02X} in {name:?}");
        }
    }
    (pr, es, fs)
}

fn resolve_env(key: &str) -> PathBuf {
    let rloc = std::env::var(key).unwrap_or_else(|_| panic!("missing env {key}"));
    let runfiles = Runfiles::create().expect("runfiles create");
    rlocation!(runfiles, rloc.as_str()).unwrap_or_else(|| panic!("rlocation missing for {rloc}"))
}

fn read_bytes_from_env(key: &str) -> Vec<u8> {
    let path = resolve_env(key);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {} {e}", path.display()))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    assert!(
        offset.checked_add(2).expect("add") <= bytes.len(),
        "u16 out of bounds {offset}"
    );
    u16::from(bytes[offset]) | (u16::from(bytes[offset + 1]) << 8)
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    assert!(
        offset.checked_add(4).expect("add") <= bytes.len(),
        "u32 out of bounds {offset}"
    );
    u32::from(bytes[offset])
        | (u32::from(bytes[offset + 1]) << 8)
        | (u32::from(bytes[offset + 2]) << 16)
        | (u32::from(bytes[offset + 3]) << 24)
}

#[derive(Debug)]
struct Eocd {
    entries: u16,
    size: u32,
    offset: u32,
}

fn parse_eocd(bytes: &[u8]) -> Eocd {
    assert!(bytes.len() >= 22, "file too small for EOCD");
    let eocd_offset = bytes.len().checked_sub(22).expect("sub");
    assert!(
        get_u32(bytes, eocd_offset) == EOCD_SIG,
        "EOCD signature missing"
    );
    let disk_number = get_u16(bytes, eocd_offset + 4);
    let cd_disk = get_u16(bytes, eocd_offset + 6);
    let entries_this_disk = get_u16(bytes, eocd_offset + 8);
    let total_entries = get_u16(bytes, eocd_offset + 10);
    let central_size = get_u32(bytes, eocd_offset + 12);
    let central_offset = get_u32(bytes, eocd_offset + 16);
    let comment_len = get_u16(bytes, eocd_offset + 20);
    assert!(disk_number == 0 && cd_disk == 0, "multi-disk not allowed");
    assert!(
        entries_this_disk == total_entries,
        "entries_this_disk mismatch"
    );
    assert!(
        total_entries == 2,
        "expected exactly 2 entries, found {total_entries}"
    );
    assert!(comment_len == 0, "archive comment not allowed");
    assert!(
        central_size != 0xFFFF_FFFF && central_offset != 0xFFFF_FFFF,
        "ZIP64 not allowed"
    );
    assert!(entries_this_disk != 0xFFFF, "ZIP64 entries not allowed");
    let cd_end = central_offset
        .checked_add(central_size)
        .expect("central end add");
    assert!(
        cd_end as usize == eocd_offset,
        "central dir must end at EOCD"
    );
    assert!(
        central_offset as usize <= bytes.len(),
        "central offset out of bounds"
    );
    assert!(cd_end as usize <= bytes.len(), "central end out of bounds");
    assert!(
        eocd_offset.checked_add(22).expect("add") == bytes.len(),
        "trailing bytes after EOCD"
    );
    Eocd {
        entries: total_entries,
        size: central_size,
        offset: central_offset,
    }
}

#[derive(Debug, Clone)]
struct CentralEntry {
    method: u16,
    flags: u16,
    dos_timestamp: u32,
    crc: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    name: String,
    external_attr: u32,
    local_offset: u32,
}

fn parse_central(bytes: &[u8], eocd: &Eocd) -> Vec<CentralEntry> {
    let mut offset = eocd.offset as usize;
    let mut entries = Vec::new();
    for _ in 0..eocd.entries {
        assert!(
            offset.checked_add(46).expect("add") <= bytes.len(),
            "central header truncated"
        );
        assert!(
            get_u32(bytes, offset) == CDH_SIG,
            "central signature missing at {offset}"
        );
        let flags = get_u16(bytes, offset + 8);
        let method = get_u16(bytes, offset + 10);
        let dos_timestamp = get_u32(bytes, offset + 12);
        let crc = get_u32(bytes, offset + 16);
        let compressed_size = get_u32(bytes, offset + 20);
        let uncompressed_size = get_u32(bytes, offset + 24);
        let name_len = get_u16(bytes, offset + 28) as usize;
        let extra_len = get_u16(bytes, offset + 30) as usize;
        let comment_len = get_u16(bytes, offset + 32) as usize;
        let disk_start = get_u16(bytes, offset + 34);
        let internal_attr = get_u16(bytes, offset + 36);
        let external_attr = get_u32(bytes, offset + 38);
        let local_offset = get_u32(bytes, offset + 42);
        assert!(extra_len == 0, "extra field not allowed");
        assert!(comment_len == 0, "file comment not allowed");
        assert!(disk_start == 0, "disk start must be 0");
        assert!(internal_attr == 0, "internal attr must be 0");
        let name_start = offset.checked_add(46).expect("add");
        let name_end = name_start.checked_add(name_len).expect("add");
        assert!(name_end <= bytes.len(), "name out of bounds");
        let name_bytes = &bytes[name_start..name_end];
        let name = std::str::from_utf8(name_bytes)
            .expect("name utf8")
            .to_owned();
        assert!(!name_bytes.contains(&0), "name contains NUL");
        offset = name_end
            .checked_add(extra_len)
            .expect("add")
            .checked_add(comment_len)
            .expect("add");
        assert!(offset <= bytes.len(), "central entry overflow");
        entries.push(CentralEntry {
            method,
            flags,
            dos_timestamp,
            crc,
            compressed_size,
            uncompressed_size,
            name,
            external_attr,
            local_offset,
        });
    }
    assert!(
        offset == eocd.offset as usize + eocd.size as usize,
        "central size mismatch"
    );
    entries
}

#[derive(Debug)]
struct LocalEntry {
    method: u16,
    flags: u16,
    dos_timestamp: u32,
    crc: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    name: String,
    data_offset: usize,
    data_len: usize,
}

fn parse_locals(bytes: &[u8], centrals: &[CentralEntry]) -> Vec<LocalEntry> {
    let mut locals = Vec::new();
    for central in centrals {
        let offset = central.local_offset as usize;
        assert!(
            offset.checked_add(30).expect("add") <= bytes.len(),
            "local header truncated at {offset}"
        );
        assert!(
            get_u32(bytes, offset) == LFH_SIG,
            "local signature missing at {offset}"
        );
        let flags = get_u16(bytes, offset + 6);
        let method = get_u16(bytes, offset + 8);
        let dos_timestamp = get_u32(bytes, offset + 10);
        let crc = get_u32(bytes, offset + 14);
        let compressed_size = get_u32(bytes, offset + 18);
        let uncompressed_size = get_u32(bytes, offset + 22);
        let name_len = get_u16(bytes, offset + 26) as usize;
        let extra_len = get_u16(bytes, offset + 28) as usize;
        assert!(extra_len == 0, "local extra not allowed");
        let name_start = offset.checked_add(30).expect("add");
        let name_end = name_start.checked_add(name_len).expect("add");
        assert!(name_end <= bytes.len(), "local name out of bounds");
        let name_bytes = &bytes[name_start..name_end];
        let name = std::str::from_utf8(name_bytes)
            .expect("local name utf8")
            .to_owned();
        let data_offset = name_end.checked_add(extra_len).expect("add");
        let data_end = data_offset
            .checked_add(compressed_size as usize)
            .expect("add");
        assert!(data_end <= bytes.len(), "local data out of bounds");
        assert!(flags & 0x08 == 0, "data descriptor not allowed");
        assert!(flags & 0x01 == 0, "encryption not allowed");
        locals.push(LocalEntry {
            method,
            flags,
            dos_timestamp,
            crc,
            compressed_size,
            uncompressed_size,
            name,
            data_offset,
            data_len: compressed_size as usize,
        });
    }
    locals
}

fn metadata_ranges(
    bytes: &[u8],
    eocd: &Eocd,
    centrals: &[CentralEntry],
    locals: &[LocalEntry],
) -> Vec<(usize, usize)> {
    assert!(
        centrals.len() == locals.len(),
        "central/local entry count mismatch"
    );
    let mut ranges = Vec::new();
    for (central, local) in centrals.iter().zip(locals) {
        let start = central.local_offset as usize;
        let end = local.data_offset;
        assert!(
            start <= end && end <= bytes.len(),
            "local header range out of bounds {start}..{end}"
        );
        ranges.push((start, end));
    }
    let central_start = eocd.offset as usize;
    let central_end = central_start.checked_add(eocd.size as usize).expect("add");
    assert!(
        central_end <= bytes.len(),
        "central directory range out of bounds"
    );
    ranges.push((central_start, central_end));
    let eocd_start = bytes.len().checked_sub(22).expect("sub");
    ranges.push((eocd_start, bytes.len()));
    ranges
}

fn assert_names_valid(centrals: &[CentralEntry], locals: &[LocalEntry]) {
    let (package_root, editor_sibling, forge_sibling) = contract_names();
    let expected_first = format!("{package_root}/{editor_sibling}");
    let expected_second = format!("{package_root}/{forge_sibling}");
    let expected = [expected_first.as_str(), expected_second.as_str()];
    assert!(
        centrals.len() == 2 && locals.len() == 2,
        "expected 2 entries"
    );
    for (idx, central) in centrals.iter().enumerate() {
        assert!(
            central.name == expected[idx],
            "central name order mismatch {idx}: {:?}",
            central.name
        );
    }
    for (idx, local) in locals.iter().enumerate() {
        assert!(
            local.name == expected[idx],
            "local name mismatch {idx}: {:?}",
            local.name
        );
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut seen_lower: HashSet<String> = HashSet::new();
    for entry in centrals {
        let name = &entry.name;
        if name.is_empty()
            || name.starts_with('/')
            || name.contains('\\')
            || name.contains(':')
            || name.contains("//")
        {
            panic!("name contains illegal pattern {name:?}");
        }
        if name
            .split('/')
            .any(|s| s == "." || s == ".." || s.is_empty())
        {
            panic!("dot segment in {name:?}");
        }
        assert!(
            !name.chars().any(|c| c.is_control() || !c.is_ascii()),
            "non-ascii/control in {name:?}"
        );
        assert!(seen.insert(name.clone()), "duplicate name {name:?}");
        let lower = name.to_ascii_lowercase();
        assert!(
            seen_lower.insert(lower.clone()),
            "case-insensitive collision {name:?}"
        );
        assert!(
            !name_bytes_contain_host_path(name),
            "host path in name {name:?}"
        );
    }
}

fn name_bytes_contain_host_path(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains(":\\")
        || lower.contains("c:/")
        || lower.contains("execroot")
        || lower.contains("runfiles")
        || lower.contains("bazel-out")
        || lower.starts_with('/')
}

#[test]
fn portable_archive_byte_identical_fixture() {
    let archive = read_bytes_from_env("ARTISAN_PORTABLE_ARCHIVE");
    let fixture = read_bytes_from_env("ARTISAN_PORTABLE_ARCHIVE_REPRODUCIBILITY");
    assert_eq!(archive, fixture, "fixture must be byte-identical");
}

#[test]
fn portable_archive_structure_valid() {
    let archive = read_bytes_from_env("ARTISAN_PORTABLE_ARCHIVE");
    let eocd = parse_eocd(&archive);
    let centrals = parse_central(&archive, &eocd);
    let locals = parse_locals(&archive, &centrals);
    assert_eq!(centrals.len(), 2);
    assert_eq!(locals.len(), 2);
    for central in &centrals {
        if central.compressed_size == 0xFFFF_FFFF
            || central.uncompressed_size == 0xFFFF_FFFF
            || central.local_offset == 0xFFFF_FFFF
        {
            panic!("ZIP64 size/offset not allowed");
        }
    }
    for (start, end) in metadata_ranges(&archive, &eocd, &centrals, &locals) {
        let metadata_contains_zip64_sig = archive[start..end]
            .windows(4)
            .any(|w| w == [0x50, 0x4b, 0x06, 0x06] || w == [0x50, 0x4b, 0x06, 0x07]);
        assert!(!metadata_contains_zip64_sig, "ZIP64 EOCD not allowed");
    }
}

#[test]
fn portable_archive_names_contract_order() {
    let archive = read_bytes_from_env("ARTISAN_PORTABLE_ARCHIVE");
    let eocd = parse_eocd(&archive);
    let centrals = parse_central(&archive, &eocd);
    let locals = parse_locals(&archive, &centrals);
    assert_names_valid(&centrals, &locals);
}

#[test]
fn portable_archive_deterministic_metadata() {
    let archive = read_bytes_from_env("ARTISAN_PORTABLE_ARCHIVE");
    let eocd = parse_eocd(&archive);
    let centrals = parse_central(&archive, &eocd);
    let locals = parse_locals(&archive, &centrals);
    for central in &centrals {
        assert_eq!(central.method, 0, "method must be 0");
        assert_eq!(central.flags, 0, "flags must be 0");
        assert_eq!(
            central.dos_timestamp, FIXED_DOS_TIMESTAMP,
            "timestamp mismatch"
        );
        let perm = (central.external_attr >> 16) & 0o777;
        assert_eq!(perm, 0o777, "mode 0777 required, got {perm:o}");
    }
    for local in &locals {
        assert_eq!(local.method, 0);
        assert_eq!(local.flags, 0);
        assert_eq!(local.dos_timestamp, FIXED_DOS_TIMESTAMP);
    }
    let first_attr = centrals[0].external_attr;
    assert_eq!(centrals[1].external_attr, first_attr);
}

#[test]
fn portable_archive_crc_and_sizes_agree() {
    let archive = read_bytes_from_env("ARTISAN_PORTABLE_ARCHIVE");
    let editor = read_bytes_from_env("ARTISAN_PORTABLE_EDITOR_BINARY");
    let forge = read_bytes_from_env("ARTISAN_PORTABLE_FORGE_BINARY");
    let eocd = parse_eocd(&archive);
    let centrals = parse_central(&archive, &eocd);
    let locals = parse_locals(&archive, &centrals);
    let binaries = [&editor, &forge];
    for idx in 0..2 {
        let central = &centrals[idx];
        let local = &locals[idx];
        assert_eq!(central.method, local.method);
        assert_eq!(central.flags, local.flags);
        assert_eq!(central.dos_timestamp, local.dos_timestamp);
        assert_eq!(central.crc, local.crc);
        assert_eq!(central.compressed_size, local.compressed_size);
        assert_eq!(central.uncompressed_size, local.uncompressed_size);
        assert_eq!(central.compressed_size, central.uncompressed_size);
        assert_eq!(local.compressed_size, local.uncompressed_size);
        let data = &archive[local.data_offset..local.data_offset + local.data_len];
        let computed = crc32(data);
        assert_eq!(computed, central.crc, "crc mismatch entry {idx}");
        assert_eq!(
            u32::try_from(data.len()).expect("member length exceeds u32"),
            central.compressed_size,
            "size mismatch {idx}"
        );
        assert_eq!(central.name, local.name);
        assert_eq!(
            central.local_offset as usize + 30 + central.name.len(),
            local.data_offset,
            "offset agreement {idx}"
        );
        let binary = binaries[idx];
        assert_eq!(data.len(), binary.len());
    }
}

#[test]
fn portable_archive_member_bytes_match_binaries() {
    let archive = read_bytes_from_env("ARTISAN_PORTABLE_ARCHIVE");
    let editor = read_bytes_from_env("ARTISAN_PORTABLE_EDITOR_BINARY");
    let forge = read_bytes_from_env("ARTISAN_PORTABLE_FORGE_BINARY");
    assert!(editor.starts_with(b"MZ"), "editor must be PE MZ");
    assert!(forge.starts_with(b"MZ"), "forge must be PE MZ");
    let eocd = parse_eocd(&archive);
    let centrals = parse_central(&archive, &eocd);
    let locals = parse_locals(&archive, &centrals);
    let editor_data = &archive[locals[0].data_offset..locals[0].data_offset + locals[0].data_len];
    let forge_data = &archive[locals[1].data_offset..locals[1].data_offset + locals[1].data_len];
    assert_eq!(editor_data, editor.as_slice(), "editor bytes mismatch");
    assert_eq!(forge_data, forge.as_slice(), "forge bytes mismatch");
    assert_eq!(crc32(editor_data), crc32(&editor));
    assert_eq!(crc32(forge_data), crc32(&forge));
}

#[test]
fn portable_archive_excludes_prohibited_members() {
    let archive = read_bytes_from_env("ARTISAN_PORTABLE_ARCHIVE");
    let eocd = parse_eocd(&archive);
    let centrals = parse_central(&archive, &eocd);
    assert_eq!(centrals.len(), 2);
    let names_lower: Vec<String> = centrals
        .iter()
        .map(|c| c.name.to_ascii_lowercase())
        .collect();
    for name in &names_lower {
        assert!(
            !std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdb")),
            "PDB not allowed {name:?}"
        );
        assert!(!name.contains("node"), "Node runtime not allowed {name:?}");
        assert!(!name.contains("electron"), "Electron not allowed {name:?}");
        assert!(!name.contains("runfiles"), "runfiles not allowed {name:?}");
        assert!(!name.contains(".ts"), "TypeScript not allowed {name:?}");
        assert!(!name.contains("resources/"), "resources dir not allowed");
        assert!(!name.contains("licenses/"), "licenses dir not allowed");
        assert!(
            !std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dll")),
            "DLL not allowed {name:?}"
        );
        assert!(!name.contains("metadata"), "metadata sidecar not allowed");
        assert!(!name.contains("source"), "source not allowed");
    }
    let (package_root, editor_sibling, forge_sibling) = contract_names();
    let expected: HashSet<String> = HashSet::from([
        format!("{package_root}/{editor_sibling}"),
        format!("{package_root}/{forge_sibling}"),
    ]);
    let actual: HashSet<String> = centrals.iter().map(|c| c.name.clone()).collect();
    assert_eq!(actual, expected, "exact two-member set required");
}

#[test]
fn portable_archive_no_host_paths_in_metadata() {
    let archive = read_bytes_from_env("ARTISAN_PORTABLE_ARCHIVE");
    let eocd = parse_eocd(&archive);
    let centrals = parse_central(&archive, &eocd);
    for central in &centrals {
        let name_lower = central.name.to_ascii_lowercase();
        assert!(!name_lower.contains(":\\"), "host path drive in name");
        assert!(!name_lower.contains("execroot"), "execroot in name");
        assert!(!name_lower.contains("bazel-out"), "bazel-out in name");
        assert!(!name_lower.contains("runfiles"), "runfiles in name");
        assert!(
            !central.name.contains('\\'),
            "backslash in name {:?}",
            central.name
        );
        assert!(
            !central.name.contains(':'),
            "colon in name {:?}",
            central.name
        );
    }
    let locals = parse_locals(&archive, &centrals);
    for (start, end) in metadata_ranges(&archive, &eocd, &centrals, &locals) {
        let metadata_str = String::from_utf8_lossy(&archive[start..end]);
        let lower = metadata_str.to_ascii_lowercase();
        assert!(!lower.contains("execroot"), "execroot in metadata bytes");
        for pattern in ["c:\\", "d:\\", "bazel-out", "artisan_editor\\"] {
            assert!(
                !lower.contains(pattern),
                "host pattern {pattern:?} in metadata"
            );
        }
    }
}
