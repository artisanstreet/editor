//! Deterministic version-payload archive proof.
//!
//! The archive is extracted directly into `versions/<version>`. This proof
//! therefore validates the five payload-root members, their exact Bazel input
//! bytes, and the manifest generated from those bytes. It also exercises the
//! pure rejection helpers used for negative producer fixtures.

use runfiles::{Runfiles, rlocation};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

const LAYOUT_TEXT: &str = include_str!("../../packaging/portable/versioned_layout.txt");

const EOCD_SIG: u32 = 0x0605_4b50;
const CDH_SIG: u32 = 0x0201_4b50;
const LFH_SIG: u32 = 0x0403_4b50;
const FIXED_DOS_TIMESTAMP: u32 = 0x3c21_0000;
const ZIPPER_VERSION_MADE_BY: u16 = 0x0300;
const ZIPPER_VERSION_NEEDED: u16 = 10;
const REQUIRED_MEMBER_COUNT: u16 = 5;

#[derive(Debug)]
struct Eocd {
    entries: u16,
    size: u32,
    offset: u32,
}

#[derive(Debug, Clone)]
struct CentralEntry {
    version_made_by: u16,
    version_needed: u16,
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

#[derive(Debug)]
struct LocalEntry {
    version_needed: u16,
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

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    assert!(
        offset.checked_add(2).expect("u16 offset") <= bytes.len(),
        "u16 out of bounds at {offset}"
    );
    u16::from(bytes[offset]) | (u16::from(bytes[offset + 1]) << 8)
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    assert!(
        offset.checked_add(4).expect("u32 offset") <= bytes.len(),
        "u32 out of bounds at {offset}"
    );
    u32::from(bytes[offset])
        | (u32::from(bytes[offset + 1]) << 8)
        | (u32::from(bytes[offset + 2]) << 16)
        | (u32::from(bytes[offset + 3]) << 24)
}

fn parse_eocd(bytes: &[u8]) -> Eocd {
    assert!(bytes.len() >= 22, "file too small for EOCD");
    let eocd_offset = bytes.len().checked_sub(22).expect("EOCD offset");
    assert_eq!(get_u32(bytes, eocd_offset), EOCD_SIG, "EOCD signature");

    let disk_number = get_u16(bytes, eocd_offset + 4);
    let cd_disk = get_u16(bytes, eocd_offset + 6);
    let entries_this_disk = get_u16(bytes, eocd_offset + 8);
    let total_entries = get_u16(bytes, eocd_offset + 10);
    let central_size = get_u32(bytes, eocd_offset + 12);
    let central_offset = get_u32(bytes, eocd_offset + 16);
    let comment_len = get_u16(bytes, eocd_offset + 20);

    assert_eq!(disk_number, 0, "multi-disk ZIP is not allowed");
    assert_eq!(cd_disk, 0, "multi-disk central directory is not allowed");
    assert_eq!(entries_this_disk, total_entries, "entry count mismatch");
    assert_eq!(total_entries, REQUIRED_MEMBER_COUNT, "exact payload count");
    assert_eq!(comment_len, 0, "archive comment is not allowed");
    assert_ne!(
        entries_this_disk,
        u16::MAX,
        "ZIP64 entry count is not allowed"
    );
    assert_ne!(central_size, u32::MAX, "ZIP64 central size is not allowed");
    assert_ne!(
        central_offset,
        u32::MAX,
        "ZIP64 central offset is not allowed"
    );

    let central_end = central_offset
        .checked_add(central_size)
        .expect("central directory end");
    assert_eq!(central_end as usize, eocd_offset, "central directory end");
    assert!(
        (central_offset as usize) <= bytes.len(),
        "central offset bounds"
    );
    assert!((central_end as usize) <= bytes.len(), "central end bounds");

    Eocd {
        entries: total_entries,
        size: central_size,
        offset: central_offset,
    }
}

fn parse_central(bytes: &[u8], eocd: &Eocd) -> Vec<CentralEntry> {
    let mut offset = eocd.offset as usize;
    let mut entries = Vec::with_capacity(usize::from(eocd.entries));
    for _ in 0..eocd.entries {
        assert!(
            offset.checked_add(46).expect("central header size") <= bytes.len(),
            "central header truncated"
        );
        assert_eq!(get_u32(bytes, offset), CDH_SIG, "central signature");

        let version_made_by = get_u16(bytes, offset + 4);
        let version_needed = get_u16(bytes, offset + 6);
        let flags = get_u16(bytes, offset + 8);
        let method = get_u16(bytes, offset + 10);
        let dos_timestamp = get_u32(bytes, offset + 12);
        let crc = get_u32(bytes, offset + 16);
        let compressed_size = get_u32(bytes, offset + 20);
        let uncompressed_size = get_u32(bytes, offset + 24);
        let name_len = usize::from(get_u16(bytes, offset + 28));
        let extra_len = usize::from(get_u16(bytes, offset + 30));
        let comment_len = usize::from(get_u16(bytes, offset + 32));
        let disk_start = get_u16(bytes, offset + 34);
        let internal_attr = get_u16(bytes, offset + 36);
        let external_attr = get_u32(bytes, offset + 38);
        let local_offset = get_u32(bytes, offset + 42);

        assert_eq!(extra_len, 0, "central extra field");
        assert_eq!(comment_len, 0, "central comment");
        assert_eq!(disk_start, 0, "central disk start");
        assert_eq!(internal_attr, 0, "central internal attributes");

        let name_start = offset.checked_add(46).expect("central name start");
        let name_end = name_start.checked_add(name_len).expect("central name end");
        assert!(name_end <= bytes.len(), "central name bounds");
        let name_bytes = &bytes[name_start..name_end];
        assert!(!name_bytes.contains(&0), "central name NUL");
        let name = std::str::from_utf8(name_bytes)
            .expect("central name UTF-8")
            .to_owned();
        offset = name_end
            .checked_add(extra_len)
            .expect("central extra end")
            .checked_add(comment_len)
            .expect("central comment end");
        assert!(offset <= bytes.len(), "central entry overflow");

        entries.push(CentralEntry {
            version_made_by,
            version_needed,
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

    assert_eq!(
        offset,
        eocd.offset as usize + eocd.size as usize,
        "central directory size"
    );
    entries
}

fn parse_locals(bytes: &[u8], eocd: &Eocd, centrals: &[CentralEntry]) -> Vec<LocalEntry> {
    let mut expected_offset = 0usize;
    let mut locals = Vec::with_capacity(centrals.len());
    for central in centrals {
        let offset = central.local_offset as usize;
        assert_eq!(
            offset, expected_offset,
            "local entries must be contiguous and ordered"
        );
        assert!(
            offset.checked_add(30).expect("local header size") <= bytes.len(),
            "local header truncated"
        );
        assert_eq!(get_u32(bytes, offset), LFH_SIG, "local signature");

        let version_needed = get_u16(bytes, offset + 4);
        let flags = get_u16(bytes, offset + 6);
        let method = get_u16(bytes, offset + 8);
        let dos_timestamp = get_u32(bytes, offset + 10);
        let crc = get_u32(bytes, offset + 14);
        let compressed_size = get_u32(bytes, offset + 18);
        let uncompressed_size = get_u32(bytes, offset + 22);
        let name_len = usize::from(get_u16(bytes, offset + 26));
        let extra_len = usize::from(get_u16(bytes, offset + 28));

        assert_eq!(extra_len, 0, "local extra field");
        let name_start = offset.checked_add(30).expect("local name start");
        let name_end = name_start.checked_add(name_len).expect("local name end");
        assert!(name_end <= bytes.len(), "local name bounds");
        let name = std::str::from_utf8(&bytes[name_start..name_end])
            .expect("local name UTF-8")
            .to_owned();
        let data_offset = name_end.checked_add(extra_len).expect("local data start");
        let data_end = data_offset
            .checked_add(compressed_size as usize)
            .expect("local data end");
        assert!(data_end <= bytes.len(), "local data bounds");
        assert!(
            data_end <= eocd.offset as usize,
            "local data overlaps central directory"
        );
        assert_eq!(flags & 0x08, 0, "data descriptor is not allowed");
        assert_eq!(flags & 0x01, 0, "encryption is not allowed");

        expected_offset = data_end;
        locals.push(LocalEntry {
            version_needed,
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
    assert_eq!(
        expected_offset, eocd.offset as usize,
        "unreferenced ZIP bytes"
    );
    locals
}

fn parse_archive(bytes: &[u8]) -> (Eocd, Vec<CentralEntry>, Vec<LocalEntry>) {
    let eocd = parse_eocd(bytes);
    let centrals = parse_central(bytes, &eocd);
    let locals = parse_locals(bytes, &eocd, &centrals);
    assert_eq!(centrals.len(), locals.len(), "central/local count");
    (eocd, centrals, locals)
}

fn metadata_ranges(
    bytes: &[u8],
    eocd: &Eocd,
    centrals: &[CentralEntry],
    locals: &[LocalEntry],
) -> Vec<(usize, usize)> {
    let mut ranges = Vec::with_capacity(centrals.len() + 2);
    for (central, local) in centrals.iter().zip(locals) {
        let start = central.local_offset as usize;
        let end = local.data_offset;
        assert!(start <= end && end <= bytes.len(), "local metadata bounds");
        ranges.push((start, end));
    }
    let central_start = eocd.offset as usize;
    let central_end = central_start
        .checked_add(eocd.size as usize)
        .expect("central metadata end");
    assert!(central_end <= bytes.len(), "central metadata bounds");
    ranges.push((central_start, central_end));
    ranges.push((bytes.len() - 22, bytes.len()));
    ranges
}

fn layout_entries() -> BTreeMap<String, String> {
    assert!(!LAYOUT_TEXT.contains('\r'), "versioned layout contains CR");
    let mut entries = BTreeMap::new();
    for raw_line in LAYOUT_TEXT.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut pieces = line.split('=');
        let key = pieces.next().expect("layout key").trim();
        let value = pieces.next().expect("layout value").trim();
        assert!(pieces.next().is_none(), "layout has multiple equals");
        assert!(!key.is_empty(), "layout key is empty");
        assert!(entries.insert(key.to_owned(), value.to_owned()).is_none());
    }
    entries
}

fn layout_value<'a>(entries: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    entries.get(key).map_or_else(
        || panic!("missing versioned layout key {key}"),
        String::as_str,
    )
}

fn expected_member_names() -> Vec<String> {
    let layout = layout_entries();
    assert_eq!(layout_value(&layout, "bin_dir"), "bin");
    assert_eq!(layout_value(&layout, "package_root"), "Artisan Street");
    assert_eq!(layout_value(&layout, "versions_dir"), "versions");
    assert_eq!(layout_value(&layout, "resources_dir"), "resources");
    assert_eq!(layout_value(&layout, "licenses_dir"), "licenses");
    assert_eq!(
        layout_value(&layout, "payload_manifest_file"),
        "payload-manifest.json"
    );

    let suffix = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    let mut expected = [
        format!("bin/{}{suffix}", layout_value(&layout, "ae_executable")),
        format!("bin/{}{suffix}", layout_value(&layout, "editor_executable")),
        format!("bin/{}{suffix}", layout_value(&layout, "forge_executable")),
        format!(
            "bin/{}{suffix}",
            layout_value(&layout, "installer_executable")
        ),
        layout_value(&layout, "payload_manifest_file").to_owned(),
    ];
    expected.sort();
    expected.to_vec()
}

fn validate_member_name(name: &str) -> Result<(), String> {
    const SAFE_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-/";
    if name.is_empty() {
        return Err("empty member name".to_owned());
    }
    if name.len() > usize::from(u16::MAX) {
        return Err(format!("member name is too long {name:?}"));
    }
    if !name.is_ascii() {
        return Err(format!("non-ASCII member name {name:?}"));
    }
    if name.starts_with('/') {
        return Err(format!("absolute member name {name:?}"));
    }
    if name.contains('\\') {
        return Err(format!("backslash in member name {name:?}"));
    }
    if name.contains(':') {
        return Err(format!("colon or ADS in member name {name:?}"));
    }
    if name
        .bytes()
        .any(|byte| !SAFE_CHARS.contains(char::from(byte)))
    {
        return Err(format!("non-canonical member name {name:?}"));
    }
    for segment in name.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!("empty or dot segment in {name:?}"));
        }
        if segment.ends_with('.') {
            return Err(format!("trailing dot in member name {name:?}"));
        }
        if segment.len() > 255 {
            return Err(format!("segment is too long in member name {name:?}"));
        }
        let device = segment
            .split_once('.')
            .map_or(segment, |(stem, _)| stem)
            .to_ascii_lowercase();
        if matches!(device.as_str(), "con" | "prn" | "aux" | "nul")
            || (device.len() == 4
                && (device.starts_with("com") || device.starts_with("lpt"))
                && device.as_bytes()[3].is_ascii_digit())
        {
            return Err(format!("reserved device in member name {name:?}"));
        }
    }
    Ok(())
}

fn validate_member_set(actual: &[String], expected: &[String]) -> Result<(), String> {
    let mut exact = BTreeSet::new();
    let mut folded = BTreeSet::new();
    for name in actual {
        validate_member_name(name)?;
        if !exact.insert(name.clone()) {
            return Err(format!("duplicate member {name:?}"));
        }
        let lower = name.to_ascii_lowercase();
        if !folded.insert(lower) {
            return Err(format!("case-folded member collision {name:?}"));
        }
    }

    let mut actual_sorted = actual.to_vec();
    let mut expected_sorted = expected.to_vec();
    actual_sorted.sort();
    expected_sorted.sort();
    if actual_sorted != expected_sorted {
        return Err(format!(
            "member set mismatch: actual {actual_sorted:?}, expected {expected_sorted:?}"
        ));
    }
    Ok(())
}

fn assert_member_contract(centrals: &[CentralEntry], locals: &[LocalEntry]) {
    let expected = expected_member_names();
    let actual = centrals
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    validate_member_set(&actual, &expected).expect("exact sorted member set");
    assert_eq!(actual, expected, "central members must be bytewise sorted");

    let local_names = locals
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(local_names, actual, "local and central names must agree");
    for name in actual {
        let lower = name.to_ascii_lowercase();
        assert!(!lower.contains("broker"), "Broker payload is forbidden");
        assert!(!lower.contains("electron"), "Electron payload is forbidden");
        assert!(!lower.contains("node"), "Node payload is forbidden");
        assert!(
            !ends_with_case_insensitive(&name, ".pdb"),
            "PDB payload is forbidden"
        );
        assert!(
            !ends_with_case_insensitive(&name, ".dll"),
            "undeclared DLL payload is forbidden"
        );
        assert!(!lower.contains(".ts"), "TypeScript payload is forbidden");
        assert!(!lower.contains("source"), "source payload is forbidden");
        assert!(!lower.contains("runfiles"), "runfiles payload is forbidden");
        assert!(!lower.contains("execroot"), "host path is forbidden");
        assert!(!lower.contains("bazel-out"), "host path is forbidden");
    }
}

fn ends_with_case_insensitive(value: &str, suffix: &str) -> bool {
    value
        .get(value.len().saturating_sub(suffix.len())..)
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(suffix))
}

fn assert_no_zip64_or_host_metadata(
    archive: &[u8],
    eocd: &Eocd,
    centrals: &[CentralEntry],
    locals: &[LocalEntry],
) {
    for central in centrals {
        assert_ne!(central.compressed_size, u32::MAX, "ZIP64 compressed size");
        assert_ne!(
            central.uncompressed_size,
            u32::MAX,
            "ZIP64 uncompressed size"
        );
        assert_ne!(central.local_offset, u32::MAX, "ZIP64 local offset");
    }
    for (start, end) in metadata_ranges(archive, eocd, centrals, locals) {
        let metadata = &archive[start..end];
        assert!(
            !metadata
                .windows(4)
                .any(|window| window == [0x50, 0x4b, 0x06, 0x06]
                    || window == [0x50, 0x4b, 0x06, 0x07]),
            "ZIP64 record in archive metadata"
        );
        let lower = String::from_utf8_lossy(metadata).to_ascii_lowercase();
        for pattern in ["c:\\", "d:\\", "execroot", "bazel-out", "runfiles"] {
            assert!(!lower.contains(pattern), "host path pattern {pattern:?}");
        }
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest
        .iter()
        .fold(String::with_capacity(64), |mut result, byte| {
            use std::fmt::Write;
            let _ = write!(result, "{byte:02x}");
            result
        })
}

fn member_bytes<'a>(archive: &'a [u8], locals: &[LocalEntry], name: &str) -> &'a [u8] {
    let local = locals
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("missing archive member {name:?}"));
    &archive[local.data_offset..local.data_offset + local.data_len]
}

fn payload_bytes(archive: &[u8], locals: &[LocalEntry]) -> BTreeMap<String, Vec<u8>> {
    locals
        .iter()
        .filter(|entry| entry.name != "payload-manifest.json")
        .map(|entry| {
            (
                entry.name.clone(),
                archive[entry.data_offset..entry.data_offset + entry.data_len].to_vec(),
            )
        })
        .collect()
}

fn canonical_manifest(payloads: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let files = payloads
        .iter()
        .map(|(name, bytes)| (name.clone(), sha256_hex(bytes)))
        .collect::<BTreeMap<_, _>>();
    serde_json::to_vec(&serde_json::json!({
        "format_version": 1,
        "files": files,
    }))
    .expect("manifest serialization")
}

fn validate_manifest(manifest: &[u8], payloads: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let document = serde_json::from_slice::<Value>(manifest)
        .map_err(|error| format!("manifest JSON: {error}"))?;
    let object = document
        .as_object()
        .ok_or_else(|| "manifest must be an object".to_owned())?;
    if object.len() != 2 {
        return Err("manifest has additions or omissions".to_owned());
    }
    if object.get("format_version").and_then(Value::as_u64) != Some(1) {
        return Err("manifest format version is not 1".to_owned());
    }
    let files = object
        .get("files")
        .and_then(Value::as_object)
        .ok_or_else(|| "manifest files is not an object".to_owned())?;
    let mut actual = BTreeMap::new();
    for (name, digest) in files {
        let digest = digest
            .as_str()
            .ok_or_else(|| format!("manifest digest for {name:?} is not a string"))?;
        if actual.insert(name.clone(), digest.to_owned()).is_some() {
            return Err(format!("duplicate manifest key {name:?}"));
        }
    }
    let expected = payloads
        .iter()
        .map(|(name, bytes)| (name.clone(), sha256_hex(bytes)))
        .collect::<BTreeMap<_, _>>();
    if actual != expected {
        return Err("manifest hashes or member set do not match payload bytes".to_owned());
    }
    if manifest != canonical_manifest(payloads) {
        return Err("manifest is not the canonical compact serialization".to_owned());
    }
    Ok(())
}

fn resolve_env(key: &str) -> PathBuf {
    let location = std::env::var(key).unwrap_or_else(|_| panic!("missing environment {key}"));
    let runfiles = Runfiles::create().expect("runfiles create");
    rlocation!(runfiles, location.as_str()).unwrap_or_else(|| panic!("missing runfile {location}"))
}

fn read_bytes_from_env(key: &str) -> Vec<u8> {
    let path = resolve_env(key);
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn parse_test_archive() -> (Vec<u8>, Eocd, Vec<CentralEntry>, Vec<LocalEntry>) {
    let archive = read_bytes_from_env("ARTISAN_VERSIONED_PAYLOAD_ARCHIVE");
    let (eocd, centrals, locals) = parse_archive(&archive);
    (archive, eocd, centrals, locals)
}

#[test]
fn independently_named_bazel_archives_are_byte_identical() {
    let archive = read_bytes_from_env("ARTISAN_VERSIONED_PAYLOAD_ARCHIVE");
    let fixture = read_bytes_from_env("ARTISAN_VERSIONED_PAYLOAD_ARCHIVE_REPRODUCIBILITY");
    assert_eq!(
        archive, fixture,
        "independent archive actions must match byte-for-byte"
    );
}

#[test]
fn archive_has_exact_sorted_version_payload_members() {
    let (archive, eocd, centrals, locals) = parse_test_archive();
    assert_member_contract(&centrals, &locals);
    assert_eq!(centrals.len(), usize::from(REQUIRED_MEMBER_COUNT));
    assert_eq!(locals.len(), usize::from(REQUIRED_MEMBER_COUNT));
    assert_no_zip64_or_host_metadata(&archive, &eocd, &centrals, &locals);
}

#[test]
fn archive_metadata_is_fixed_stored_and_unencrypted() {
    let (archive, eocd, centrals, locals) = parse_test_archive();
    for central in &centrals {
        assert_eq!(central.version_made_by, ZIPPER_VERSION_MADE_BY);
        assert_eq!(central.version_needed, ZIPPER_VERSION_NEEDED);
        assert_eq!(central.method, 0, "central method must be stored");
        assert_eq!(central.flags, 0, "central flags must be empty");
        assert_eq!(central.dos_timestamp, FIXED_DOS_TIMESTAMP);
        assert_eq!((central.external_attr >> 16) & 0o777, 0o777);
    }
    for local in &locals {
        assert_eq!(local.version_needed, ZIPPER_VERSION_NEEDED);
        assert_eq!(local.method, 0, "local method must be stored");
        assert_eq!(local.flags, 0, "local flags must be empty");
        assert_eq!(local.dos_timestamp, FIXED_DOS_TIMESTAMP);
    }
    assert_no_zip64_or_host_metadata(&archive, &eocd, &centrals, &locals);
}

#[test]
fn archive_headers_have_matching_stored_crc_sizes_and_names() {
    let (archive, _eocd, centrals, locals) = parse_test_archive();
    for (central, local) in centrals.iter().zip(&locals) {
        assert_eq!(central.name, local.name);
        assert_eq!(central.method, local.method);
        assert_eq!(central.flags, local.flags);
        assert_eq!(central.dos_timestamp, local.dos_timestamp);
        assert_eq!(central.crc, local.crc);
        assert_eq!(central.compressed_size, local.compressed_size);
        assert_eq!(central.uncompressed_size, local.uncompressed_size);
        assert_eq!(central.compressed_size, central.uncompressed_size);
        let bytes = &archive[local.data_offset..local.data_offset + local.data_len];
        assert_eq!(crc32(bytes), central.crc);
        assert_eq!(
            bytes.len(),
            usize::try_from(central.compressed_size).expect("size")
        );
    }
}

#[test]
fn every_executable_member_matches_its_authoritative_bazel_input() {
    let (archive, _eocd, centrals, locals) = parse_test_archive();
    assert_member_contract(&centrals, &locals);
    let suffix = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    let sources = [
        (
            format!("bin/ae{suffix}"),
            read_bytes_from_env("ARTISAN_VERSIONED_PAYLOAD_AE_BINARY"),
        ),
        (
            format!("bin/editor{suffix}"),
            read_bytes_from_env("ARTISAN_VERSIONED_PAYLOAD_EDITOR_BINARY"),
        ),
        (
            format!("bin/forge{suffix}"),
            read_bytes_from_env("ARTISAN_VERSIONED_PAYLOAD_FORGE_BINARY"),
        ),
        (
            format!("bin/installer{suffix}"),
            read_bytes_from_env("ARTISAN_VERSIONED_PAYLOAD_INSTALLER_BINARY"),
        ),
    ];
    for (name, source) in sources {
        assert_eq!(
            member_bytes(&archive, &locals, &name),
            source.as_slice(),
            "{name}"
        );
    }
}

#[test]
fn payload_manifest_has_exact_schema_sorted_hashes_and_no_self_entry() {
    let (archive, _eocd, centrals, locals) = parse_test_archive();
    assert_member_contract(&centrals, &locals);
    let payloads = payload_bytes(&archive, &locals);
    let manifest = member_bytes(&archive, &locals, "payload-manifest.json");
    validate_manifest(manifest, &payloads).expect("canonical payload manifest");
    assert_eq!(manifest, canonical_manifest(&payloads));
    assert!(!payloads.contains_key("payload-manifest.json"));

    let document = serde_json::from_slice::<Value>(manifest).expect("manifest JSON");
    let files = document
        .get("files")
        .and_then(Value::as_object)
        .expect("manifest files");
    let keys = files.keys().cloned().collect::<Vec<_>>();
    let mut sorted_keys = keys.clone();
    sorted_keys.sort();
    assert_eq!(keys, sorted_keys, "manifest keys must be lexical");
    assert_eq!(
        document.get("format_version").and_then(Value::as_u64),
        Some(1)
    );
}

#[test]
fn archive_contains_no_development_runtime_or_host_payload() {
    let (archive, eocd, centrals, locals) = parse_test_archive();
    assert_member_contract(&centrals, &locals);
    assert_no_zip64_or_host_metadata(&archive, &eocd, &centrals, &locals);
    let names = centrals
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        names
            .iter()
            .all(|name| name.starts_with("bin/") || *name == "payload-manifest.json")
    );
    assert!(names.iter().all(|name| !name.contains("resources/")));
    assert!(names.iter().all(|name| !name.contains("licenses/")));
}

#[test]
fn negative_member_and_suffix_fixtures_fail_closed() {
    let expected = expected_member_names();
    let mut addition = expected.clone();
    addition.push("bin/extra.exe".to_owned());
    assert!(
        validate_member_set(&addition, &expected).is_err(),
        "addition accepted"
    );

    let omission = expected[..expected.len() - 1].to_vec();
    assert!(
        validate_member_set(&omission, &expected).is_err(),
        "omission accepted"
    );

    let duplicate = vec!["bin/ae.exe".to_owned(), "bin/ae.exe".to_owned()];
    assert!(
        validate_member_set(&duplicate, &duplicate).is_err(),
        "duplicate accepted"
    );

    let collision = vec!["bin/ae.exe".to_owned(), "bin/AE.EXE".to_owned()];
    assert!(
        validate_member_set(&collision, &collision).is_err(),
        "case collision accepted"
    );

    for directory in ["resources", "licenses"] {
        for suffix in [
            "",
            ".",
            "..",
            "/absolute.txt",
            "nested//empty.txt",
            r"nested\name.txt",
            "C:/drive.txt",
            "name:stream.txt",
            "café.txt",
            "trailing.",
            "COM0.txt",
        ] {
            let member = format!("{directory}/{suffix}");
            assert!(
                validate_member_name(&member).is_err(),
                "invalid {directory} suffix accepted: {suffix:?}"
            );
        }
    }
    assert!(validate_member_name("licenses/third-party/license-v2.txt").is_ok());
}

#[test]
fn tampered_payload_and_manifest_bytes_are_rejected() {
    let (archive, _eocd, centrals, locals) = parse_test_archive();
    let payloads = payload_bytes(&archive, &locals);
    let manifest = member_bytes(&archive, &locals, "payload-manifest.json");
    validate_manifest(manifest, &payloads).expect("baseline manifest");

    let ae_name = if cfg!(target_os = "windows") {
        "bin/ae.exe"
    } else {
        "bin/ae"
    };
    let mut tampered_payloads = payloads.clone();
    tampered_payloads
        .get_mut(ae_name)
        .expect("ae payload")
        .push(0);
    assert!(
        validate_manifest(manifest, &tampered_payloads).is_err(),
        "tampered payload accepted"
    );

    let mut tampered_manifest = manifest.to_vec();
    let marker = b"\":\"";
    let position = tampered_manifest
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("manifest digest separator")
        + marker.len();
    tampered_manifest[position] = if tampered_manifest[position] == b'0' {
        b'1'
    } else {
        b'0'
    };
    assert!(
        validate_manifest(&tampered_manifest, &payloads).is_err(),
        "tampered manifest accepted"
    );

    assert_member_contract(&centrals, &locals);
}
