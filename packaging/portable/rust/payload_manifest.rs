//! Hermetic generator for the per-version payload integrity manifest.
//!
//! The action receives only explicitly declared Bazel files. It hashes those
//! bytes, validates their canonical archive names, and serializes the same
//! compact, lexically ordered JSON shape written by the installer staging
//! code. It does not inspect a directory, runfiles tree, environment, or
//! build metadata.

use std::{collections::BTreeMap, env, fmt::Write as _, fs, path::PathBuf, process};

use sha2::{Digest, Sha256};

const PAYLOAD_MANIFEST_NAME: &str = "payload-manifest.json";
const SAFE_MEMBER_CHARS: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-/";
const RESERVED_DEVICES: &[&str] = &["con", "prn", "aux", "nul"];
const REQUIRED_LAYOUT: &[(&str, &str)] = &[
    ("schema_version", "2"),
    ("proof_scope", "versioned-release-layout"),
    ("package_root", "Artisan Street"),
    ("bin_dir", "bin"),
    ("versions_dir", "versions"),
    ("resources_dir", "resources"),
    ("licenses_dir", "licenses"),
    ("installation_file", "installation.json"),
    ("payload_manifest_file", PAYLOAD_MANIFEST_NAME),
    ("ae_executable", "ae"),
    ("installer_executable", "installer"),
    ("editor_executable", "editor"),
    ("forge_executable", "forge"),
    ("stable_launcher_role", "ae"),
    ("bootstrap_role", "installer"),
    ("broker_role", "forbidden"),
    ("mutable_state", "outside-immutable-roots"),
    ("source_tree_fallback", "false"),
    ("runfiles_fallback", "false"),
];

#[derive(Debug)]
struct Config {
    output: PathBuf,
    layout: PathBuf,
    files: Vec<FileInput>,
}

#[derive(Debug)]
struct FileInput {
    member: String,
    path: PathBuf,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("payload manifest generation failed: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let config = parse_args(env::args().skip(1))?;
    let layout = fs::read_to_string(&config.layout)
        .map_err(|error| format!("read layout {}: {error}", config.layout.display()))?;
    validate_layout_text(&layout)?;

    let mut files = BTreeMap::new();
    let mut folded_names = BTreeMap::new();
    for input in config.files {
        let metadata = fs::metadata(&input.path)
            .map_err(|error| format!("stat input {}: {error}", input.path.display()))?;
        if !metadata.is_file() {
            return Err(format!(
                "declared payload input is not a regular file: {}",
                input.path.display()
            ));
        }
        let bytes = fs::read(&input.path)
            .map_err(|error| format!("read input {}: {error}", input.path.display()))?;
        add_digest(&mut files, &mut folded_names, &input.member, &bytes)?;
    }

    if files.is_empty() {
        return Err("at least one payload file must be declared".to_owned());
    }

    let document = serde_json::json!({
        "format_version": 1,
        "files": files,
    });
    let bytes = serde_json::to_vec(&document)
        .map_err(|error| format!("serialize payload manifest: {error}"))?;
    fs::write(&config.output, bytes)
        .map_err(|error| format!("write manifest {}: {error}", config.output.display()))
}

fn parse_args<I>(args: I) -> Result<Config, String>
where
    I: Iterator<Item = String>,
{
    let mut args = args;
    let mut output = None;
    let mut layout = None;
    let mut files = Vec::new();

    while let Some(option) = args.next() {
        match option.as_str() {
            "--output" => output = Some(next_argument(&mut args, "--output")?),
            "--layout" => layout = Some(next_argument(&mut args, "--layout")?),
            "--file" => {
                let member = next_argument(&mut args, "--file member")?;
                let path = next_argument(&mut args, "--file input")?;
                files.push(FileInput {
                    member,
                    path: PathBuf::from(path),
                });
            }
            _ => return Err(format!("unknown argument {option:?}")),
        }
    }

    Ok(Config {
        output: PathBuf::from(output.ok_or_else(|| "missing --output".to_owned())?),
        layout: PathBuf::from(layout.ok_or_else(|| "missing --layout".to_owned())?),
        files,
    })
}

fn next_argument<I>(args: &mut I, option: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("missing argument after {option}"))
}

fn validate_layout_text(text: &str) -> Result<(), String> {
    if !text.is_ascii() {
        return Err("versioned layout must be ASCII".to_owned());
    }
    if text.contains('\r') {
        return Err("versioned layout must use LF line endings".to_owned());
    }

    let mut actual = BTreeMap::new();
    for (line_index, line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut pieces = trimmed.split('=');
        let key = pieces
            .next()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| format!("versioned layout line {line_number} has no key"))?;
        let value = pieces
            .next()
            .map(str::trim)
            .ok_or_else(|| format!("versioned layout line {line_number} has no value"))?;
        if pieces.next().is_some() {
            return Err(format!(
                "versioned layout line {line_number} has multiple equals signs"
            ));
        }
        if actual.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(format!("duplicate versioned layout key {key:?}"));
        }
    }

    let expected = REQUIRED_LAYOUT
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<BTreeMap<_, _>>();
    if actual != expected {
        return Err("versioned layout does not match the closed v2 contract".to_owned());
    }
    Ok(())
}

fn add_digest(
    files: &mut BTreeMap<String, String>,
    folded_names: &mut BTreeMap<String, String>,
    member: &str,
    bytes: &[u8],
) -> Result<(), String> {
    validate_member_name(member)?;
    if member.eq_ignore_ascii_case(PAYLOAD_MANIFEST_NAME) {
        return Err("payload manifest cannot list itself as a payload file".to_owned());
    }
    if files.contains_key(member) {
        return Err(format!("duplicate archive member {member:?}"));
    }
    let folded = member.to_ascii_lowercase();
    if let Some(previous) = folded_names.get(&folded) {
        return Err(format!(
            "ASCII case-folded archive member collision between {previous:?} and {member:?}"
        ));
    }

    files.insert(member.to_owned(), sha256_hex(bytes));
    folded_names.insert(folded, member.to_owned());
    Ok(())
}

fn validate_member_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("archive member name must not be empty".to_owned());
    }
    if name.len() > usize::from(u16::MAX) {
        return Err(format!("archive member name is too long: {name:?}"));
    }
    if !name.is_ascii() {
        return Err(format!("archive member name must be ASCII: {name:?}"));
    }
    if name.starts_with('/') {
        return Err(format!("archive member name must be relative: {name:?}"));
    }
    if name.contains('\\') {
        return Err(format!(
            "archive member name contains a backslash: {name:?}"
        ));
    }
    if name.contains(':') {
        return Err(format!(
            "archive member name contains a colon or ADS: {name:?}"
        ));
    }
    if name
        .bytes()
        .any(|byte| !SAFE_MEMBER_CHARS.contains(char::from(byte)))
    {
        return Err(format!("archive member name is not canonical: {name:?}"));
    }
    for segment in name.split('/') {
        validate_segment(segment)?;
    }
    Ok(())
}

fn validate_segment(segment: &str) -> Result<(), String> {
    if segment.is_empty() || segment == "." || segment == ".." {
        return Err(format!(
            "archive member has an empty or dot segment: {segment:?}"
        ));
    }
    if segment.ends_with('.') {
        return Err(format!(
            "archive member segment has a trailing dot: {segment:?}"
        ));
    }
    if segment.len() > 255 {
        return Err(format!("archive member segment is too long: {segment:?}"));
    }
    let device = segment
        .split_once('.')
        .map_or(segment, |(stem, _)| stem)
        .to_ascii_lowercase();
    if RESERVED_DEVICES.contains(&device.as_str())
        || (device.len() == 4
            && (device.starts_with("com") || device.starts_with("lpt"))
            && device.as_bytes()[3].is_ascii_digit())
    {
        return Err(format!(
            "archive member names a reserved device: {segment:?}"
        ));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut result = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(result, "{byte:02x}");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        PAYLOAD_MANIFEST_NAME, add_digest, sha256_hex, validate_layout_text, validate_member_name,
    };
    use std::collections::BTreeMap;

    fn manifest_for(entries: &[(&str, &[u8])]) -> Result<Vec<u8>, String> {
        let mut files = BTreeMap::new();
        let mut folded_names = BTreeMap::new();
        for (member, bytes) in entries {
            add_digest(&mut files, &mut folded_names, member, bytes)?;
        }
        serde_json::to_vec(&serde_json::json!({
            "format_version": 1,
            "files": files,
        }))
        .map_err(|error| error.to_string())
    }

    #[test]
    fn manifest_is_compact_sorted_and_matches_installer_serialization() {
        let manifest = manifest_for(&[("bin/ae.exe", b"artisan")]).expect("manifest");
        assert_eq!(
            manifest,
            br#"{"format_version":1,"files":{"bin/ae.exe":"0b74ed7ff22b86fd0838fd29a78940a8d54377951e968867948a57b3e53646fc"}}"#
        );
        assert_eq!(sha256_hex(b"artisan").len(), 64);
    }

    #[test]
    fn manifest_names_are_unique_case_insensitively_and_never_include_the_manifest() {
        let duplicate = manifest_for(&[("bin/ae.exe", b"a"), ("bin/ae.exe", b"b")]);
        assert!(duplicate.is_err());

        let collision = manifest_for(&[("bin/ae.exe", b"a"), ("bin/AE.EXE", b"a")]);
        assert!(collision.is_err());

        let self_entry = manifest_for(&[(PAYLOAD_MANIFEST_NAME, b"manifest")]);
        assert!(self_entry.is_err());
    }

    #[test]
    fn invalid_member_names_fail_closed() {
        let invalid = [
            "",
            "/absolute",
            "bin//ae.exe",
            "bin/./ae.exe",
            "bin/../ae.exe",
            r"bin\ae.exe",
            "C:/ae.exe",
            "bin/ae.exe:stream",
            "bin/café.exe",
            "bin/ae.exe.",
            "bin/CON.exe",
            "bin/com0.txt",
            "bin/com1.txt",
        ];
        for name in invalid {
            assert!(
                validate_member_name(name).is_err(),
                "{name:?} must be rejected"
            );
        }
        assert!(validate_member_name("resources/legal/license-v2.txt").is_ok());
    }

    #[test]
    fn layout_is_consumed_as_the_closed_v2_contract() {
        let layout = include_str!("../versioned_layout.txt");
        validate_layout_text(layout).expect("accepted versioned layout");

        let drifted = layout.replace(
            "payload_manifest_file = payload-manifest.json",
            "payload_manifest_file = other.json",
        );
        assert!(validate_layout_text(&drifted).is_err());
    }

    #[test]
    fn sorted_manifest_keys_are_stable_independent_of_declaration_order() {
        let first =
            manifest_for(&[("bin/forge.exe", b"forge"), ("bin/ae.exe", b"ae")]).expect("first");
        let second =
            manifest_for(&[("bin/ae.exe", b"ae"), ("bin/forge.exe", b"forge")]).expect("second");
        assert_eq!(first, second);
    }
}
