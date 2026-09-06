//! Codex version parsing and comparison fixtures.
//!
//! Registration (controller-owned, not part of this packet):
//! `tests/native_engine/BUILD.bazel` gains a `rust_test` target for
//! `codex_version.rs` depending on `//modules/native_engine:native_engine`.

use std::cmp::Ordering;

use artisan_native_engine::codex::{
    CODEX_CONTINUATION_CLI_VERSION, CODEX_MINIMUM_CLI_VERSION, compare_semantic_versions,
    is_continuation_verified_version, meets_minimum_version, parse_codex_continuation_version,
    parse_codex_version,
};

#[test]
fn transport_constants_match_typescript_protocol() {
    assert_eq!(CODEX_MINIMUM_CLI_VERSION, "0.142.5");
    assert_eq!(CODEX_CONTINUATION_CLI_VERSION, "0.145.0");
}

#[test]
fn version_parses_first_semver_with_word_boundaries() {
    assert_eq!(
        parse_codex_version(b"codex-cli 0.145.0"),
        Some("0.145.0".to_owned())
    );
    assert_eq!(
        parse_codex_version(b"Codex 0.142.5 (windows-x86_64)"),
        Some("0.142.5".to_owned())
    );
    assert_eq!(
        parse_codex_version(b"v10.2.3 extra"),
        Some("10.2.3".to_owned())
    );
    assert_eq!(parse_codex_version(b"no version here"), None);
    assert_eq!(parse_codex_version(b"1.2"), None);
    assert_eq!(parse_codex_version(b"1.2.3.4"), Some("1.2.3".to_owned()));
    // Embedded in a longer digit run: no word boundary, so no match.
    assert_eq!(parse_codex_version(b"x10.2.3"), None);
    assert_eq!(parse_codex_version(b"0.145.0beta"), None);
}

#[test]
fn malformed_versions_never_panic_and_stay_unparseable() {
    for output in [
        b"".as_slice(),
        b"0.145".as_slice(),
        b"0.145.".as_slice(),
        b"..".as_slice(),
        b"codex".as_slice(),
        b"\xff\xfe\x00".as_slice(),
    ] {
        assert_eq!(parse_codex_version(output), None);
    }
}

#[test]
fn continuation_version_keeps_prerelease_and_build_metadata() {
    assert_eq!(
        parse_codex_continuation_version(b"codex-cli 0.145.0-alpha.1+build.7"),
        Some("0.145.0-alpha.1+build.7".to_owned())
    );
    assert_eq!(
        parse_codex_continuation_version(b"0.145.0"),
        Some("0.145.0".to_owned())
    );
    assert!(is_continuation_verified_version("0.145.0"));
    assert!(!is_continuation_verified_version("0.145.0-alpha.1"));
    assert!(!is_continuation_verified_version("0.146.0"));
}

#[test]
fn semantic_comparison_is_numeric_not_lexicographic() {
    assert_eq!(
        compare_semantic_versions("0.9.10", "0.9.9"),
        Ordering::Greater
    );
    assert_eq!(
        compare_semantic_versions("0.142.5", "0.142.5"),
        Ordering::Equal
    );
    assert_eq!(
        compare_semantic_versions("0.141.9", "0.142.5"),
        Ordering::Less
    );
    assert_eq!(
        compare_semantic_versions("0.150.0", "0.142.5"),
        Ordering::Greater
    );
}

#[test]
fn minimum_gate_matches_typescript_probe() {
    assert!(meets_minimum_version("0.142.5"));
    assert!(meets_minimum_version("0.145.0"));
    assert!(!meets_minimum_version("0.141.9"));
    assert!(!meets_minimum_version("0.0.0"));
}
