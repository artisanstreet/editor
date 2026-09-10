//! Hermes version matrix: parsing, prerelease, and the minimum gate.
//!
//! Fixture `--version` outputs flow through [`parse_hermes_version`] and
//! [`HermesVersion::meets_minimum`], mirroring the TypeScript
//! `/Hermes Agent v(\d+)\.(\d+)\.(\d+)/i` match plus the `[0, 20, 0]` gate.

use artisan_native_engine::hermes::probe::check_minimum_version;
use artisan_native_engine::hermes::version::{MINIMUM_HERMES_VERSION, parse_hermes_version};

#[test]
fn minimum_gate_constant_matches_typescript() {
    assert_eq!(MINIMUM_HERMES_VERSION, [0, 20, 0]);
}

#[test]
fn exact_minimum_parses_and_passes() {
    let version = parse_hermes_version("Hermes Agent v0.20.0").expect("parses");
    assert_eq!(version.triple(), [0, 20, 0]);
    assert_eq!(version.prerelease(), None);
    assert!(version.meets_minimum());
    check_minimum_version(&version).expect("gate accepts");
    assert_eq!(version.to_string(), "0.20.0");
}

#[test]
fn match_is_case_insensitive_and_unanchored() {
    let version =
        parse_hermes_version("prefix lines\nhermes agent v1.2.3\ntrailer").expect("parses");
    assert_eq!(version.triple(), [1, 2, 3]);
    assert!(version.meets_minimum());
}

#[test]
fn observed_installed_output_parses() {
    let version = parse_hermes_version("Hermes Agent v0.20.0 (2026.8.3)").expect("parses");
    assert_eq!(version.triple(), [0, 20, 0]);
    assert!(version.meets_minimum());
}

#[test]
fn newer_minor_and_patch_pass() {
    for output in [
        "Hermes Agent v0.20.1",
        "Hermes Agent v0.21.0",
        "Hermes Agent v1.0.0",
    ] {
        let version = parse_hermes_version(output).expect("parses");
        assert!(version.meets_minimum(), "{output} passes");
        check_minimum_version(&version).expect("gate accepts");
    }
}

#[test]
fn older_major_or_minor_fails_despite_larger_tail() {
    // TypeScript compares lexicographically: 0.19.99 < 0.20.0 even though the
    // trailing component is larger.
    for output in ["Hermes Agent v0.19.9", "Hermes Agent v0.19.99"] {
        let version = parse_hermes_version(output).expect("parses");
        assert!(!version.meets_minimum(), "{output} fails");
        assert!(check_minimum_version(&version).is_err());
    }
}

#[test]
fn prerelease_suffix_is_captured_and_still_passes() {
    let version = parse_hermes_version("Hermes Agent v0.20.0-beta.1").expect("parses");
    assert_eq!(version.triple(), [0, 20, 0]);
    assert_eq!(version.prerelease(), Some("beta.1"));
    assert!(version.meets_minimum());
    assert_eq!(version.to_string(), "0.20.0-beta.1");
}

#[test]
fn prerelease_below_minimum_still_fails() {
    let version = parse_hermes_version("Hermes Agent v0.19.9-rc.2").expect("parses");
    assert_eq!(version.prerelease(), Some("rc.2"));
    assert!(!version.meets_minimum());
}

#[test]
fn malformed_outputs_are_rejected() {
    for output in [
        "",
        "not hermes at all",
        "Hermes 0.20.0",
        "Hermes Agent 0.20.0",
        "Hermes Agent v0.20",
        "Hermes Agent v0.20.",
        "Hermes Agent v.20.0",
        "Hermes Agent v",
    ] {
        assert!(parse_hermes_version(output).is_err(), "{output:?} rejected");
    }
}

#[test]
fn fourth_numeric_component_is_trailing_noise() {
    // The unanchored TypeScript match accepts the leading triple and ignores
    // the rest; the native parser mirrors that.
    let version = parse_hermes_version("Hermes Agent v0.20.0.0").expect("parses");
    assert_eq!(version.triple(), [0, 20, 0]);
}

#[test]
fn overflowing_component_is_rejected() {
    assert!(parse_hermes_version("Hermes Agent v99999999999999999999999.0.0").is_err());
}

#[test]
fn version_error_message_carries_no_output_text() {
    let error = parse_hermes_version("no version here").expect_err("rejected");
    let rendered = format!("{error}");
    assert!(!rendered.contains("no version here"));
    assert!(rendered.contains("unrecognized version string"));
}
