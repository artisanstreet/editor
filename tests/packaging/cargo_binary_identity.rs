//! Source-only proof of the four approved Cargo binary output identities.
//!
//! The registered target embeds these manifests through `compile_data`:
//! `//:modules/cli/Cargo.toml`, `//:modules/installer/Cargo.toml`,
//! `//modules/frontend:Cargo.toml`, and `//modules/backend:Cargo.toml`.
//! This proof reads only the closed `[[bin]]` name/path surface. It does not
//! build a Cargo or Bazel binary, touch the filesystem, launch a process, or
//! inspect the environment; those are separate packaging decisions.

const CARGO_MANIFESTS: &[(&str, &str, &str, &str)] = &[
    (
        "modules/cli/Cargo.toml",
        include_str!("../../modules/cli/Cargo.toml"),
        "ae",
        "rust/main.rs",
    ),
    (
        "modules/installer/Cargo.toml",
        include_str!("../../modules/installer/Cargo.toml"),
        "installer",
        "rust/main.rs",
    ),
    (
        "modules/frontend/Cargo.toml",
        include_str!("../../modules/frontend/Cargo.toml"),
        "editor",
        "src/main.rs",
    ),
    (
        "modules/backend/Cargo.toml",
        include_str!("../../modules/backend/Cargo.toml"),
        "forge",
        "src/main.rs",
    ),
];

#[derive(Debug, PartialEq, Eq)]
struct BinaryTarget {
    name: String,
    path: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    MissingBinaryBlock,
    DuplicateBinaryBlock,
    InvalidSectionHeader,
    TrailingGarbage,
    MalformedBinaryField,
    UnknownBinaryField,
    DuplicateName,
    DuplicatePath,
    MissingName,
    MissingPath,
    MalformedQuotedValue,
}

/// Parses only the explicit `name` and `path` keys in one `[[bin]]` block.
///
/// Other Cargo sections are skipped as opaque text. That is enough for this
/// closed proof and prevents package, dependency, example, or test metadata
/// from being mistaken for binary identity fields.
fn parse_binary_target(manifest: &str) -> Result<BinaryTarget, ParseError> {
    let mut saw_binary_block = false;
    let mut in_binary_block = false;
    let mut in_other_section = false;
    let mut name = None;
    let mut path = None;

    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') {
            let is_binary_block = parse_section_header(line)?;
            if is_binary_block {
                if saw_binary_block {
                    return Err(ParseError::DuplicateBinaryBlock);
                }
                saw_binary_block = true;
                in_binary_block = true;
                in_other_section = false;
            } else {
                in_binary_block = false;
                in_other_section = true;
            }
            continue;
        }

        if in_binary_block {
            parse_binary_field(line, &mut name, &mut path)?;
        } else if in_other_section {
            continue;
        } else {
            return Err(ParseError::TrailingGarbage);
        }
    }

    if !saw_binary_block {
        return Err(ParseError::MissingBinaryBlock);
    }

    Ok(BinaryTarget {
        name: name.ok_or(ParseError::MissingName)?,
        path: path.ok_or(ParseError::MissingPath)?,
    })
}

/// Returns whether a syntactically closed array-of-table header is `[[bin]]`.
fn parse_section_header(line: &str) -> Result<bool, ParseError> {
    let line = strip_comment(line)?.trim();

    if line.starts_with("[[") {
        if !line.ends_with("]]") || line.len() <= 4 {
            return Err(ParseError::InvalidSectionHeader);
        }
        let name = line[2..line.len() - 2].trim();
        if name.is_empty() || name.contains(['[', ']']) {
            return Err(ParseError::InvalidSectionHeader);
        }
        return Ok(name == "bin");
    }

    if !line.starts_with('[') || !line.ends_with(']') || line.len() <= 2 {
        return Err(ParseError::InvalidSectionHeader);
    }
    let name = line[1..line.len() - 1].trim();
    if name.is_empty() || name.contains(['[', ']']) {
        return Err(ParseError::InvalidSectionHeader);
    }
    Ok(false)
}

fn parse_binary_field(
    line: &str,
    name: &mut Option<String>,
    path: &mut Option<String>,
) -> Result<(), ParseError> {
    let line = strip_comment(line)?.trim();
    let (key, value) = line
        .split_once('=')
        .ok_or(ParseError::MalformedBinaryField)?;
    let key = key.trim();
    let value = parse_quoted_value(value.trim())?;

    match key {
        "name" => {
            if name.is_some() {
                return Err(ParseError::DuplicateName);
            }
            *name = Some(value);
        }
        "path" => {
            if path.is_some() {
                return Err(ParseError::DuplicatePath);
            }
            *path = Some(value);
        }
        _ => return Err(ParseError::UnknownBinaryField),
    }
    Ok(())
}

/// Reads one narrow basic string and rejects all unsupported TOML value forms.
fn parse_quoted_value(value: &str) -> Result<String, ParseError> {
    if !value.starts_with('"') {
        return Err(ParseError::MalformedQuotedValue);
    }

    let mut parsed = String::new();
    let mut offset = 1;
    while offset < value.len() {
        let byte = value.as_bytes()[offset];
        match byte {
            b'"' => {
                if value[offset + 1..].trim().is_empty() {
                    return Ok(parsed);
                }
                return Err(ParseError::MalformedQuotedValue);
            }
            b'\\' => {
                offset += 1;
                let escaped = value
                    .as_bytes()
                    .get(offset)
                    .copied()
                    .ok_or(ParseError::MalformedQuotedValue)?;
                match escaped {
                    b'"' => parsed.push('"'),
                    b'\\' => parsed.push('\\'),
                    _ => return Err(ParseError::MalformedQuotedValue),
                }
                offset += 1;
            }
            b'\n' | b'\r' => return Err(ParseError::MalformedQuotedValue),
            _ => {
                let character = value[offset..]
                    .chars()
                    .next()
                    .ok_or(ParseError::MalformedQuotedValue)?;
                if character.is_control() {
                    return Err(ParseError::MalformedQuotedValue);
                }
                parsed.push(character);
                offset += character.len_utf8();
            }
        }
    }

    Err(ParseError::MalformedQuotedValue)
}

/// Removes a comment while preserving `#` characters inside a quoted value.
fn strip_comment(line: &str) -> Result<&str, ParseError> {
    let mut quote = None;
    let mut escaped = false;

    for (offset, byte) in line.bytes().enumerate() {
        match quote {
            Some(b'"') => {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quote = None;
                }
            }
            Some(b'\'') => {
                if byte == b'\'' {
                    quote = None;
                }
            }
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'#' => return Ok(&line[..offset]),
                _ => {}
            },
            _ => return Err(ParseError::MalformedQuotedValue),
        }
    }

    if quote.is_some() || escaped {
        return Err(ParseError::MalformedQuotedValue);
    }
    Ok(line)
}

fn normalized_forward_slashes(path: &str) -> String {
    path.chars()
        .map(|character| if character == '\\' { '/' } else { character })
        .collect()
}

fn assert_manifest_target(label: &str, manifest: &str, expected_name: &str, expected_path: &str) {
    let target = parse_binary_target(manifest)
        .unwrap_or_else(|error| panic!("{label} did not parse: {error:?}"));
    assert_eq!(target.name, expected_name, "{label} binary name");
    assert_eq!(
        normalized_forward_slashes(&target.path),
        expected_path,
        "{label} binary path"
    );
}

#[test]
fn approved_manifests_declare_the_authoritative_binary_identities() {
    for &(label, manifest, expected_name, expected_path) in CARGO_MANIFESTS {
        assert_manifest_target(label, manifest, expected_name, expected_path);
    }
}

#[test]
fn comments_and_unrelated_sections_do_not_become_binary_fields() {
    let manifest = r#"
# A root comment and blank lines are not a binary block.
[package]
name = "package-name-is-not-an-output"
version = "0.0.0"

[[bin]]
# The only fields that matter are in this exact block.
name = "editor" # inline comments are harmless
path = "src/main.rs"

[dependencies]
name = "dependency-name"
path = "dependency/source.rs"

[[test]]
name = "test-name"
path = "tests/not-a-binary.rs"

[package.metadata]
name = "metadata-name"
path = "metadata/path"
"#;

    assert_manifest_target("synthetic manifest", manifest, "editor", "src/main.rs");
}

#[test]
fn parser_normalizes_only_path_separators() {
    let manifest = r#"
[[bin]]
name = "editor"
path = "src\\main.rs"
"#;

    let target = parse_binary_target(manifest).expect("the narrow Cargo block should parse");
    assert_eq!(target.name, "editor");
    assert_eq!(normalized_forward_slashes(&target.path), "src/main.rs");
}

#[test]
fn malformed_or_ambiguous_binary_blocks_are_rejected() {
    let rejected = [
        (
            "missing name",
            r#"[[bin]]
path = "rust/main.rs"
"#,
        ),
        (
            "missing path",
            r#"[[bin]]
name = "installer"
"#,
        ),
        (
            "duplicate name",
            r#"[[bin]]
name = "installer"
name = "other"
path = "rust/main.rs"
"#,
        ),
        (
            "duplicate path",
            r#"[[bin]]
name = "installer"
path = "rust/main.rs"
path = "src/main.rs"
"#,
        ),
        (
            "duplicate binary block",
            r#"[[bin]]
name = "installer"
path = "rust/main.rs"

[[bin]]
name = "other"
path = "src/main.rs"
"#,
        ),
        (
            "unterminated quoted value",
            r#"[[bin]]
name = "installer
path = "rust/main.rs"
"#,
        ),
        (
            "trailing value after quote",
            r#"[[bin]]
name = "installer" trailing
path = "rust/main.rs"
"#,
        ),
        (
            "trailing non-comment garbage",
            r#"[[bin]]
name = "installer"
path = "rust/main.rs"
not-a-cargo-field
"#,
        ),
    ];

    for (label, manifest) in rejected {
        assert!(
            parse_binary_target(manifest).is_err(),
            "{label} must be rejected"
        );
    }
}

#[test]
fn legacy_and_package_derived_names_cannot_satisfy_new_identities() {
    let legacy_installer = parse_binary_target(
        r#"
[package]
name = "ae-installer"

[[bin]]
name = "ae-installer"
path = "rust/main.rs"
"#,
    )
    .expect("the old explicit identity is syntactically valid");
    assert_eq!(legacy_installer.name, "ae-installer");
    assert_ne!(legacy_installer.name, "installer");

    let default_manifests = [
        ("ae-installer", "installer"),
        ("artisan-frontend", "editor"),
        ("artisan-backend", "forge"),
    ];
    for (package_name, expected_name) in default_manifests {
        assert_ne!(
            package_name, expected_name,
            "a Cargo package-derived output cannot be the approved identity"
        );
    }

    assert!(
        parse_binary_target(
            r#"
[package]
name = "ae-installer"
"#,
        )
        .is_err(),
        "a package name without an explicit [[bin]] block is not an accepted identity"
    );
    assert!(
        parse_binary_target(
            r#"
[package]
name = "artisan-frontend"
"#,
        )
        .is_err(),
        "Cargo's default frontend name must not replace [[bin]] name"
    );
    assert!(
        parse_binary_target(
            r#"
[package]
name = "artisan-backend"
"#,
        )
        .is_err(),
        "Cargo's default backend name must not replace [[bin]] name"
    );
}
