use super::*;
use crate::engine_core::ArchiveEntryTypes;
use flate2::{Compression, write::GzEncoder};
use std::io::Cursor;

const MEMBER: &str = "package/bin/opencode2.exe";
const BOUND: u64 = 1024 * 1024;
const POLICY: ArchivePolicy = ArchivePolicy {
    expanded_bound_bytes: BOUND,
    max_entries: 64,
    max_name_bytes: 255,
    entry_types: ArchiveEntryTypes {
        pax_headers: true,
        gnu_long_names: true,
    },
};

fn member(target: &Path) -> Extraction<'_> {
    Extraction::Member {
        member: MEMBER,
        target,
    }
}

fn tree(destination: &Path) -> Extraction<'_> {
    Extraction::Tree {
        strip: "package/",
        destination,
    }
}

fn extract_tree(archive: Vec<u8>, policy: &ArchivePolicy) -> Result<(), ArchiveError> {
    let directory = tempfile::tempdir().unwrap();
    extract_gzip_tar(Cursor::new(archive), tree(directory.path()), policy)
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
            extract_gzip_tar(Cursor::new(archive), member(&target), &POLICY),
            Err(ArchiveError::UnsafeName { path: name.into() })
        );
    }
    let archive = tar_gzip(&[(MEMBER, b"target executable".as_slice(), b'0')]);
    extract_gzip_tar(Cursor::new(archive), member(&target), &POLICY).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"target executable");
    assert_eq!(measure(&target).unwrap().1, 17);

    let directory_member = tar_gzip(&[(MEMBER, b"".as_slice(), b'5')]);
    let other = directory.path().join("other.exe");
    assert_eq!(
        extract_gzip_tar(Cursor::new(directory_member), member(&other), &POLICY),
        Err(ArchiveError::TargetInvalid)
    );
}

#[test]
fn links_devices_and_unknown_entries_are_unsupported_with_their_kind() {
    for (flag, kind) in [
        (b'1', "hardlink"),
        (b'2', "symlink"),
        (b'3', "character_device"),
        (b'4', "block_device"),
        (b'6', "fifo"),
        (b'K', "gnu_long_link"),
        (b'S', "unknown"),
    ] {
        let archive = tar_gzip(&[("package/link", b"".as_slice(), flag)]);
        assert_eq!(
            extract_tree(archive, &POLICY),
            Err(ArchiveError::UnsupportedEntry {
                kind,
                path: "package/link".into()
            }),
            "type flag {flag}"
        );
    }
}

#[test]
fn metadata_forms_outside_the_policy_are_unsupported() {
    let strict = ArchivePolicy {
        entry_types: ArchiveEntryTypes {
            pax_headers: false,
            gnu_long_names: false,
        },
        ..POLICY
    };
    for (flag, kind) in [
        (b'L', "gnu_long_name"),
        (b'x', "pax_header"),
        (b'g', "pax_global_header"),
    ] {
        let archive = tar_gzip(&[("package/meta", b"".as_slice(), flag)]);
        assert_eq!(
            extract_tree(archive, &strict),
            Err(ArchiveError::UnsupportedEntry {
                kind,
                path: "package/meta".into()
            })
        );
    }
}

#[test]
fn duplicates_case_collisions_and_file_ancestors_are_rejected() {
    for (entries, collided) in [
        (
            vec![
                (MEMBER, b"a".as_slice(), b'0'),
                (MEMBER, b"b".as_slice(), b'0'),
            ],
            MEMBER,
        ),
        (
            vec![
                (MEMBER, b"a".as_slice(), b'0'),
                ("PACKAGE/BIN/OPENCODE2.EXE", b"b".as_slice(), b'0'),
            ],
            "PACKAGE/BIN/OPENCODE2.EXE",
        ),
        (
            vec![
                ("package", b"a".as_slice(), b'0'),
                (MEMBER, b"b".as_slice(), b'0'),
            ],
            MEMBER,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        assert_eq!(
            extract_gzip_tar(Cursor::new(tar_gzip(&entries)), member(&target), &POLICY),
            Err(ArchiveError::NameCollision {
                path: collided.into()
            })
        );
    }
}

#[test]
fn entry_count_expanded_size_and_name_length_are_bounded_before_payloads() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("opencode2.exe");
    let many = tar_many_entries(65);
    assert_eq!(
        parse_tar(&mut Cursor::new(many), member(&target), &POLICY),
        Err(ArchiveError::TooManyEntries {
            entries: 65,
            limit: 64
        })
    );
    let oversized = tar_declared_size("oversized", BOUND + 1, b'0');
    assert_eq!(
        parse_tar(&mut Cursor::new(oversized), member(&target), &POLICY),
        Err(ArchiveError::ExpandedTooLarge {
            bytes: BOUND + 1,
            limit: BOUND
        })
    );
    let long = format!("package/{}", "n".repeat(300));
    let archive = tar_gzip(&[
        ("././@LongLink", format!("{long}\0").as_bytes(), b'L'),
        ("package/short", b"x".as_slice(), b'0'),
    ]);
    assert_eq!(
        extract_tree(archive, &POLICY),
        Err(ArchiveError::NameTooLong {
            length: 308,
            limit: 255
        })
    );
    let huge = tar_declared_size("././@LongLink", 128 * 1024, b'L');
    assert_eq!(
        parse_tar(&mut Cursor::new(huge), member(&target), &POLICY),
        Err(ArchiveError::MetadataTooLarge {
            bytes: 128 * 1024,
            limit: 64 * 1024
        })
    );
}

#[test]
fn gnu_long_names_and_pax_paths_name_the_next_member() {
    let long = format!("package/{}/tool", "deep/".repeat(30).trim_end_matches('/'));
    let pax = [
        pax_record("mtime", "1790000000.5"),
        pax_record("path", &format!("package/{}/pax", "p".repeat(120))),
    ]
    .concat();
    let global = pax_record("comment", "ignored");
    let archive = tar_gzip(&[
        ("pax_global_header", global.as_bytes(), b'g'),
        ("././@LongLink", format!("{long}\0").as_bytes(), b'L'),
        ("package/truncated", b"gnu".as_slice(), b'0'),
        ("PaxHeader/x", pax.as_bytes(), b'x'),
        ("package/short", b"pax".as_slice(), b'0'),
    ]);
    let directory = tempfile::tempdir().unwrap();
    extract_gzip_tar(Cursor::new(archive), tree(directory.path()), &POLICY).unwrap();
    let relative = long.strip_prefix("package/").unwrap();
    assert_eq!(fs::read(directory.path().join(relative)).unwrap(), b"gnu");
    let pax_file = directory.path().join(format!("{}/pax", "p".repeat(120)));
    assert_eq!(fs::read(pax_file).unwrap(), b"pax");
    assert!(!directory.path().join("truncated").exists());

    let escaping = tar_gzip(&[
        ("././@LongLink", b"package/../../escape\0".as_slice(), b'L'),
        ("package/innocent", b"x".as_slice(), b'0'),
    ]);
    assert_eq!(
        extract_tree(escaping, &POLICY),
        Err(ArchiveError::UnsafeName {
            path: "package/../../escape".into()
        })
    );
    let dangling = tar_gzip(&[("././@LongLink", b"package/x\0".as_slice(), b'L')]);
    assert!(matches!(
        extract_tree(dangling, &POLICY),
        Err(ArchiveError::Malformed { .. })
    ));
}

#[test]
fn gnu_headers_empty_owner_fields_and_record_padding_are_accepted() {
    let mut archive = Vec::new();
    let mut gnu = tar_header("package/gnu", 3, b'0', 0o755);
    gnu[257..265].copy_from_slice(b"ustar  \0");
    // GNU headers keep access times where ustar has its prefix.
    gnu[345..350].copy_from_slice(b"12345");
    reseal(&mut gnu);
    archive.extend_from_slice(&gnu);
    archive.extend_from_slice(b"gnu");
    archive.resize(archive.len() + 509, 0);
    let mut npm = tar_header("package/npm", 3, b'0', 0o644);
    npm[108..124].fill(0);
    reseal(&mut npm);
    archive.extend_from_slice(&npm);
    archive.extend_from_slice(b"npm");
    archive.resize(10_240, 0);
    let directory = tempfile::tempdir().unwrap();
    extract_gzip_tar(Cursor::new(gzip(&archive)), tree(directory.path()), &POLICY).unwrap();
    assert_eq!(fs::read(directory.path().join("gnu")).unwrap(), b"gnu");
    assert_eq!(fs::read(directory.path().join("npm")).unwrap(), b"npm");

    let mut v7 = tar_header("package/v7", 0, b'0', 0o644);
    v7[257..265].fill(0);
    reseal(&mut v7);
    let mut old = v7.to_vec();
    old.resize(old.len() + 1024, 0);
    assert_eq!(
        extract_tree(gzip(&old), &POLICY),
        Err(ArchiveError::UnsupportedFormat { entry: 1 })
    );
}

#[test]
fn missing_target_bad_checksum_truncation_and_trailing_data_fail() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    let archive = tar_gzip(&[("package/other", b"a".as_slice(), b'0')]);
    assert_eq!(
        extract_gzip_tar(Cursor::new(archive), member(&target), &POLICY),
        Err(ArchiveError::TargetMissing)
    );

    let mut corrupted = tar_bytes(&[(MEMBER, b"a".as_slice(), b'0')]);
    corrupted[0] ^= 1;
    let second = directory.path().join("second");
    assert_eq!(
        extract_gzip_tar(Cursor::new(gzip(&corrupted)), member(&second), &POLICY),
        Err(ArchiveError::Malformed {
            entry: 1,
            field: "checksum"
        })
    );

    let mut truncated = tar_bytes(&[(MEMBER, b"a".as_slice(), b'0')]);
    truncated.truncate(truncated.len() - 512);
    let third = directory.path().join("third");
    assert_eq!(
        extract_gzip_tar(Cursor::new(gzip(&truncated)), member(&third), &POLICY),
        Err(ArchiveError::Truncated)
    );

    let first = gzip(&tar_bytes(&[(MEMBER, b"a".as_slice(), b'0')]));
    let trailing = [first, gzip(b"trailing")].concat();
    let fourth = directory.path().join("fourth");
    assert_eq!(
        extract_gzip_tar(Cursor::new(trailing), member(&fourth), &POLICY),
        Err(ArchiveError::TrailingData)
    );

    let mut garbage = tar_bytes(&[(MEMBER, b"a".as_slice(), b'0')]);
    garbage.extend_from_slice(b"not padding");
    let fifth = directory.path().join("fifth");
    assert_eq!(
        extract_gzip_tar(Cursor::new(gzip(&garbage)), member(&fifth), &POLICY),
        Err(ArchiveError::TrailingData)
    );

    let mut bad_crc = gzip(&tar_bytes(&[(MEMBER, b"a".as_slice(), b'0')]));
    let crc_byte = bad_crc.len() - 8;
    bad_crc[crc_byte] ^= 1;
    let sixth = directory.path().join("sixth");
    assert_eq!(
        extract_gzip_tar(Cursor::new(bad_crc), member(&sixth), &POLICY),
        Err(ArchiveError::Compression)
    );
}

#[test]
fn errors_name_the_rule_and_limit() {
    let error = ArchiveError::TooManyEntries {
        entries: 632,
        limit: 512,
    };
    assert_eq!(error.code(), "too_many_entries");
    assert_eq!(
        error.to_string(),
        "too_many_entries: 632 entries, limit 512"
    );
    let error = ArchiveError::UnsupportedEntry {
        kind: "symlink",
        path: "package/bin/link".into(),
    };
    assert_eq!(
        error.to_string(),
        "unsupported_entry: symlink package/bin/link"
    );
    assert_eq!(reported(&"é".repeat(150)).len(), 200);
    assert_eq!(reported("a\nb"), "a?b");
}

#[test]
fn tree_extraction_strips_the_prefix_and_keeps_the_layout() {
    let directory = tempfile::tempdir().unwrap();
    let archive = tar_gzip_with_modes(&[
        ("package/", b"".as_slice(), b'5', 0o755),
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
    extract_gzip_tar(Cursor::new(archive), tree(directory.path()), &POLICY).unwrap();
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
    let archive = tar_gzip(&[("package/../escape", b"x".as_slice(), b'0')]);
    assert_eq!(
        extract_tree(archive, &POLICY),
        Err(ArchiveError::UnsafeName {
            path: "package/../escape".into()
        })
    );
}

fn pax_record(key: &str, value: &str) -> String {
    let body = format!(" {key}={value}\n");
    let mut length = body.len() + 1;
    while format!("{length}{body}").len() != length {
        length += 1;
    }
    format!("{length}{body}")
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
    reseal(&mut header);
    header
}

fn reseal(header: &mut [u8; 512]) {
    for byte in &mut header[148..156] {
        *byte = b' ';
    }
    let checksum = header.iter().map(|byte| u64::from(*byte)).sum();
    write_octal(&mut header[148..156], checksum);
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
