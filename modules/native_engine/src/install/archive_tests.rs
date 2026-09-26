use super::*;
use flate2::{Compression, write::GzEncoder};
use std::io::Cursor;

const MEMBER: &str = "package/bin/opencode2.exe";
const BOUND: u64 = 1024 * 1024;

fn member(target: &Path) -> Extraction<'_> {
    Extraction::Member {
        member: MEMBER,
        target,
    }
}

#[test]
fn member_names_reject_traversal_absolute_and_windows_forms() {
    for unsafe_name in [
        "",
        "/package/bin/opencode2.exe",
        "C:/package/bin/opencode2.exe",
        "package\\bin\\opencode2.exe",
        "package/../opencode2.exe",
        "package//opencode2.exe",
        "package/bin/./opencode2.exe",
    ] {
        assert!(!is_safe_member_name(unsafe_name), "{unsafe_name}");
    }
    assert!(is_safe_member_name(MEMBER));
}

#[test]
fn member_extraction_accepts_only_the_exact_target_and_rejects_hostile_entries() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("opencode2.exe");
    for name in [
        "package/bin/../opencode2.exe",
        "package\\bin\\opencode2.exe",
        "C:/package/bin/opencode2.exe",
    ] {
        let archive = tar_gzip(&[(name, b"bad".as_slice(), b'0')]);
        assert_eq!(
            extract_gzip_tar(Cursor::new(archive), member(&target), BOUND),
            Err(ArchiveError::Invalid)
        );
    }
    let archive = tar_gzip(&[(MEMBER, b"target executable".as_slice(), b'0')]);
    extract_gzip_tar(Cursor::new(archive), member(&target), BOUND).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"target executable");
    assert_eq!(measure(&target).unwrap().1, 17);

    let directory_member = tar_gzip(&[(MEMBER, b"".as_slice(), b'5')]);
    let other = directory.path().join("other.exe");
    assert_eq!(
        extract_gzip_tar(Cursor::new(directory_member), member(&other), BOUND),
        Err(ArchiveError::TargetInvalid)
    );
}

#[test]
fn unsupported_entry_kinds_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("opencode2.exe");
    for kind in *b"12xgL3" {
        let archive = tar_gzip(&[("unsupported", b"".as_slice(), kind)]);
        assert_eq!(
            extract_gzip_tar(Cursor::new(archive), member(&target), BOUND),
            Err(ArchiveError::Invalid),
            "type flag {kind}"
        );
    }
}

#[test]
fn duplicates_case_collisions_and_file_ancestors_are_rejected() {
    for entries in [
        vec![
            (MEMBER, b"a".as_slice(), b'0'),
            (MEMBER, b"b".as_slice(), b'0'),
        ],
        vec![
            (MEMBER, b"a".as_slice(), b'0'),
            ("PACKAGE/BIN/OPENCODE2.EXE", b"b".as_slice(), b'0'),
        ],
        vec![
            ("package", b"a".as_slice(), b'0'),
            (MEMBER, b"b".as_slice(), b'0'),
        ],
    ] {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        assert_eq!(
            extract_gzip_tar(Cursor::new(tar_gzip(&entries)), member(&target), BOUND),
            Err(ArchiveError::Invalid)
        );
    }
}

#[test]
fn entry_count_and_expanded_size_are_bounded_before_payloads() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("opencode2.exe");
    let many = tar_many_entries(MAX_ARCHIVE_ENTRIES + 1);
    assert_eq!(
        parse_tar(&mut Cursor::new(many), member(&target), BOUND),
        Err(ArchiveError::Invalid)
    );
    let oversized = tar_declared_size("oversized", BOUND + 1, b'0');
    assert_eq!(
        parse_tar(&mut Cursor::new(oversized), member(&target), BOUND),
        Err(ArchiveError::Invalid)
    );
}

#[test]
fn missing_target_bad_checksum_truncation_and_trailing_data_fail() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    let archive = tar_gzip(&[("package/other", b"a".as_slice(), b'0')]);
    assert_eq!(
        extract_gzip_tar(Cursor::new(archive), member(&target), BOUND),
        Err(ArchiveError::TargetMissing)
    );

    let mut corrupted = tar_bytes(&[(MEMBER, b"a".as_slice(), b'0')]);
    corrupted[0] ^= 1;
    let second = directory.path().join("second");
    assert!(extract_gzip_tar(Cursor::new(gzip(&corrupted)), member(&second), BOUND).is_err());

    let mut truncated = tar_bytes(&[(MEMBER, b"a".as_slice(), b'0')]);
    truncated.truncate(truncated.len() - 512);
    let third = directory.path().join("third");
    assert!(extract_gzip_tar(Cursor::new(gzip(&truncated)), member(&third), BOUND).is_err());

    let first = gzip(&tar_bytes(&[(MEMBER, b"a".as_slice(), b'0')]));
    let trailing = [first, gzip(b"trailing")].concat();
    let fourth = directory.path().join("fourth");
    assert_eq!(
        extract_gzip_tar(Cursor::new(trailing), member(&fourth), BOUND),
        Err(ArchiveError::Invalid)
    );

    let mut bad_crc = gzip(&tar_bytes(&[(MEMBER, b"a".as_slice(), b'0')]));
    let crc_byte = bad_crc.len() - 8;
    bad_crc[crc_byte] ^= 1;
    let fifth = directory.path().join("fifth");
    assert_eq!(
        extract_gzip_tar(Cursor::new(bad_crc), member(&fifth), BOUND),
        Err(ArchiveError::Invalid)
    );
}

#[test]
fn tree_extraction_strips_the_prefix_and_keeps_the_layout() {
    let directory = tempfile::tempdir().unwrap();
    let archive = tar_gzip_with_modes(&[
        ("package/package.json", b"{}".as_slice(), b'0', 0o644),
        (
            "package/vendor/x86_64-unknown-linux-musl/bin/codex",
            b"codex".as_slice(),
            b'0',
            0o755,
        ),
        (
            "package/vendor/x86_64-unknown-linux-musl/codex-path/rg",
            b"rg".as_slice(),
            b'0',
            0o755,
        ),
        ("other/ignored", b"x".as_slice(), b'0', 0o644),
    ]);
    extract_gzip_tar(
        Cursor::new(archive),
        Extraction::Tree {
            strip: "package/",
            destination: directory.path(),
        },
        BOUND,
    )
    .unwrap();
    let codex = directory
        .path()
        .join("vendor/x86_64-unknown-linux-musl/bin/codex");
    assert_eq!(fs::read(&codex).unwrap(), b"codex");
    assert!(
        directory
            .path()
            .join("vendor/x86_64-unknown-linux-musl/codex-path/rg")
            .is_file()
    );
    assert!(!directory.path().join("ignored").exists());
    assert!(!directory.path().join("other").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&codex), 0o755);
        assert_eq!(mode(&directory.path().join("package.json")), 0o644);
    }
}

#[test]
fn tree_extraction_rejects_traversal_below_the_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let archive = tar_gzip(&[("package/../escape", b"x".as_slice(), b'0')]);
    assert_eq!(
        extract_gzip_tar(
            Cursor::new(archive),
            Extraction::Tree {
                strip: "package/",
                destination: directory.path(),
            },
            BOUND,
        ),
        Err(ArchiveError::Invalid)
    );
}

pub(crate) fn tar_gzip(entries: &[(&str, &[u8], u8)]) -> Vec<u8> {
    gzip(&tar_bytes(entries))
}

pub(crate) fn tar_gzip_with_modes(entries: &[(&str, &[u8], u8, u64)]) -> Vec<u8> {
    let mut archive = Vec::new();
    for (name, body, kind, mode) in entries {
        archive.extend_from_slice(&tar_header(name, body.len() as u64, *kind, *mode));
        archive.extend_from_slice(body);
        archive.resize(archive.len() + ((512 - body.len() % 512) % 512), 0);
    }
    archive.resize(archive.len() + 1024, 0);
    gzip(&archive)
}

pub(crate) fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn tar_bytes(entries: &[(&str, &[u8], u8)]) -> Vec<u8> {
    let mut archive = Vec::new();
    for (name, body, kind) in entries {
        archive.extend_from_slice(&tar_header(name, body.len() as u64, *kind, 0o644));
        archive.extend_from_slice(body);
        archive.resize(archive.len() + ((512 - body.len() % 512) % 512), 0);
    }
    archive.resize(archive.len() + 1024, 0);
    archive
}

fn tar_header(name: &str, size: u64, kind: u8, mode: u64) -> [u8; 512] {
    let mut header = [0_u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    write_octal(&mut header[100..108], mode);
    write_octal(&mut header[108..116], 0);
    write_octal(&mut header[116..124], 0);
    write_octal(&mut header[124..136], size);
    write_octal(&mut header[136..148], 0);
    header[156] = kind;
    header[257..263].copy_from_slice(b"ustar\0");
    for byte in &mut header[148..156] {
        *byte = b' ';
    }
    let checksum = header.iter().map(|byte| u64::from(*byte)).sum();
    write_octal(&mut header[148..156], checksum);
    header
}

fn tar_many_entries(count: usize) -> Vec<u8> {
    let mut archive = Vec::with_capacity(count.saturating_mul(512).saturating_add(1024));
    for index in 0..count {
        archive.extend_from_slice(&tar_header(&format!("entry-{index}"), 0, b'0', 0o644));
    }
    archive.resize(archive.len() + 1024, 0);
    archive
}

fn tar_declared_size(name: &str, size: u64, kind: u8) -> Vec<u8> {
    let mut archive = tar_header(name, size, kind, 0o644).to_vec();
    archive.resize(archive.len() + 1024, 0);
    archive
}

fn write_octal(field: &mut [u8], value: u64) {
    let text = format!("{value:o}");
    let start = field.len() - text.len() - 1;
    field.fill(0);
    field[start..start + text.len()].copy_from_slice(text.as_bytes());
    field[start + text.len()] = 0;
}
