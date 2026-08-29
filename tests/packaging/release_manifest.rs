//! Focused packaging boundary proof for the canonical release manifest.
//!
//! Behavioral generation, ZIP validation, and signing vectors live with the
//! production tool's cfg(test) suite. This test freezes the Starlark action
//! boundary and the concrete development target without invoking Bazel or a
//! release key.

const RULE_SOURCE: &str = include_str!("../../packaging/release/release_manifest.bzl");
const TOOL_SOURCE: &str = include_str!("../../packaging/release/rust/release_tool.rs");
const PACKAGE_SOURCE: &str = include_str!("BUILD.bazel");

#[test]
fn development_target_freezes_the_windows_x64_metadata() {
    for expected in [
        "archive = \"//packaging/portable:versioned_payload_archive\"",
        "format_version = 1",
        "product_version = \"0.0.0\"",
        "editor_forge_compatibility_version = \"0.0.0\"",
        "channel = \"nightly\"",
        "signing_key_id = \"development\"",
        "algorithm = \"ed25519\"",
        "minimum_installer_version = \"0.0.0\"",
        "minimum_cli_version = \"0.0.0\"",
        "artifact_id = \"windows-x64\"",
        "platform = \"windows\"",
        "architecture = \"x64\"",
        "libc = \"\"",
        "archive_format = \"zip\"",
        "file_name = \"artisan-editor-versioned-payload.zip\"",
    ] {
        assert!(
            PACKAGE_SOURCE.contains(expected),
            "missing target contract: {expected}"
        );
    }
}

#[test]
fn rule_is_generate_only_and_has_no_secret_key_attribute_or_input() {
    assert_eq!(RULE_SOURCE.matches("args.add(\"generate\")").count(), 1);
    assert!(!RULE_SOURCE.contains("args.add(\"sign\")"));
    assert!(!RULE_SOURCE.contains("--key-file"));
    assert!(!RULE_SOURCE.contains("key_file"));
    assert!(!RULE_SOURCE.contains("private_key"));
    assert!(!RULE_SOURCE.contains("secret_key"));
    assert!(RULE_SOURCE.contains("inputs = depset([ctx.file.archive])"));
    assert!(!RULE_SOURCE.contains("ctx.file.key"));
}

#[test]
fn generate_parser_has_no_runtime_key_path() {
    let generate_start = TOOL_SOURCE
        .find("fn parse_generate_args")
        .expect("generate parser");
    let sign_start = TOOL_SOURCE.find("fn parse_sign_args").expect("sign parser");
    let generate_parser = &TOOL_SOURCE[generate_start..sign_start];
    assert!(!generate_parser.contains("key-file"));
    assert!(!generate_parser.contains("key_file"));
    assert!(!generate_parser.contains("private_key"));
    assert!(!generate_parser.contains("secret_key"));
}
