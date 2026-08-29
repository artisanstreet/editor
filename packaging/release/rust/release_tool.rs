//! Hermetic release-manifest generation and detached Ed25519 signing.
//!
//! The generate mode has one declared archive input and public metadata. It
//! never reads a key or any ambient process state. The sign mode is the only
//! mode that opens a key file, and it accepts that path only at runtime.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    env,
    fmt::Write as _,
    fs,
    io::{self, Cursor},
    path::{Path, PathBuf},
    process,
};
use zeroize::{Zeroize, Zeroizing};
use zip::ZipArchive;

const FORMAT_VERSION: u8 = 1;
const ED25519_ALGORITHM: &str = "ed25519";
const MAX_ARTIFACT_BYTES: u64 = 768 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 16_384;
const MAX_ENTRY_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MEMBER_NAME_BYTES: usize = 1_024;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_ARTIFACTS: usize = 64;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_PATH_SEGMENT_BYTES: usize = 255;
const SAFE_MEMBER_CHARS: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-/";
const RESERVED_DEVICES: &[&str] = &["con", "prn", "aux", "nul"];

#[derive(Debug)]
struct GenerateOptions {
    archive: PathBuf,
    output: PathBuf,
    metadata: ManifestMetadata,
}

#[derive(Debug)]
struct SignOptions {
    manifest: PathBuf,
    key_file: PathBuf,
    key_id: String,
    output: PathBuf,
}

#[derive(Debug)]
struct ManifestMetadata {
    format_version: u8,
    product_version: String,
    editor_forge_compatibility_version: String,
    channel: String,
    signing_key_id: String,
    algorithm: String,
    minimum_installer_version: String,
    minimum_cli_version: String,
    artifact_id: String,
    platform: String,
    architecture: String,
    libc: Option<String>,
    archive_format: String,
    file_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseManifest {
    format_version: u8,
    product_version: String,
    editor_forge_compatibility_version: String,
    channel: String,
    signing_identity: SigningIdentity,
    minimum_installer_version: String,
    minimum_cli_version: String,
    artifacts: Vec<ReleaseArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SigningIdentity {
    key_id: String,
    algorithm: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseArtifact {
    artifact_id: String,
    platform: String,
    architecture: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    libc: Option<String>,
    archive_format: String,
    file_name: String,
    byte_size: u64,
    sha256: String,
    archive_entries: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DetachedSignature {
    algorithm: String,
    key_id: String,
    signature: String,
}

fn main() {
    if let Err(error) = run(env::args().skip(1)) {
        eprintln!("release tool failed: {error}");
        process::exit(1);
    }
}

fn run<I>(args: I) -> Result<(), String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mode = args
        .next()
        .ok_or_else(|| "expected generate or sign mode".to_owned())?;
    match mode.as_str() {
        "generate" => generate(parse_generate_args(args)?),
        "sign" => sign(parse_sign_args(args)?),
        _ => Err("expected generate or sign mode".to_owned()),
    }
}

#[allow(clippy::similar_names, clippy::too_many_lines)]
fn parse_generate_args<I>(args: I) -> Result<GenerateOptions, String>
where
    I: Iterator<Item = String>,
{
    let mut args = args;
    let mut archive = None;
    let mut output = None;
    let mut format_version = None;
    let mut product_version = None;
    let mut editor_forge_compatibility_version = None;
    let mut channel = None;
    let mut signing_key_id = None;
    let mut algorithm = None;
    let mut minimum_installer_version = None;
    let mut minimum_cli_version = None;
    let mut artifact_id = None;
    let mut platform = None;
    let mut architecture = None;
    let mut libc = None;
    let mut archive_format = None;
    let mut file_name = None;

    while let Some(option) = args.next() {
        match option.as_str() {
            "--archive" => set_option(
                &mut archive,
                next_value(&mut args, "--archive")?,
                "--archive",
            )?,
            "--output" => set_option(&mut output, next_value(&mut args, "--output")?, "--output")?,
            "--format-version" => set_option(
                &mut format_version,
                next_value(&mut args, "--format-version")?,
                "--format-version",
            )?,
            "--product-version" => set_option(
                &mut product_version,
                next_value(&mut args, "--product-version")?,
                "--product-version",
            )?,
            "--editor-forge-compatibility-version" => set_option(
                &mut editor_forge_compatibility_version,
                next_value(&mut args, "--editor-forge-compatibility-version")?,
                "--editor-forge-compatibility-version",
            )?,
            "--channel" => set_option(
                &mut channel,
                next_value(&mut args, "--channel")?,
                "--channel",
            )?,
            "--signing-key-id" => set_option(
                &mut signing_key_id,
                next_value(&mut args, "--signing-key-id")?,
                "--signing-key-id",
            )?,
            "--algorithm" => set_option(
                &mut algorithm,
                next_value(&mut args, "--algorithm")?,
                "--algorithm",
            )?,
            "--minimum-installer-version" => set_option(
                &mut minimum_installer_version,
                next_value(&mut args, "--minimum-installer-version")?,
                "--minimum-installer-version",
            )?,
            "--minimum-cli-version" => set_option(
                &mut minimum_cli_version,
                next_value(&mut args, "--minimum-cli-version")?,
                "--minimum-cli-version",
            )?,
            "--artifact-id" => set_option(
                &mut artifact_id,
                next_value(&mut args, "--artifact-id")?,
                "--artifact-id",
            )?,
            "--platform" => set_option(
                &mut platform,
                next_value(&mut args, "--platform")?,
                "--platform",
            )?,
            "--architecture" => set_option(
                &mut architecture,
                next_value(&mut args, "--architecture")?,
                "--architecture",
            )?,
            "--libc" => set_option(&mut libc, next_value(&mut args, "--libc")?, "--libc")?,
            "--archive-format" => set_option(
                &mut archive_format,
                next_value(&mut args, "--archive-format")?,
                "--archive-format",
            )?,
            "--file-name" => set_option(
                &mut file_name,
                next_value(&mut args, "--file-name")?,
                "--file-name",
            )?,
            _ => return Err("unknown generate option".to_owned()),
        }
    }

    let metadata = ManifestMetadata {
        format_version: required_format_version(format_version)?,
        product_version: required_option(product_version, "--product-version")?,
        editor_forge_compatibility_version: required_option(
            editor_forge_compatibility_version,
            "--editor-forge-compatibility-version",
        )?,
        channel: required_option(channel, "--channel")?,
        signing_key_id: required_option(signing_key_id, "--signing-key-id")?,
        algorithm: required_option(algorithm, "--algorithm")?,
        minimum_installer_version: required_option(
            minimum_installer_version,
            "--minimum-installer-version",
        )?,
        minimum_cli_version: required_option(minimum_cli_version, "--minimum-cli-version")?,
        artifact_id: required_option(artifact_id, "--artifact-id")?,
        platform: required_option(platform, "--platform")?,
        architecture: required_option(architecture, "--architecture")?,
        libc,
        archive_format: required_option(archive_format, "--archive-format")?,
        file_name: required_option(file_name, "--file-name")?,
    };
    validate_metadata(&metadata, true)?;

    Ok(GenerateOptions {
        archive: PathBuf::from(required_option(archive, "--archive")?),
        output: PathBuf::from(required_option(output, "--output")?),
        metadata,
    })
}

fn parse_sign_args<I>(args: I) -> Result<SignOptions, String>
where
    I: Iterator<Item = String>,
{
    let mut args = args;
    let mut manifest = None;
    let mut key_file = None;
    let mut key_id = None;
    let mut output = None;

    while let Some(option) = args.next() {
        match option.as_str() {
            "--manifest" => set_option(
                &mut manifest,
                next_value(&mut args, "--manifest")?,
                "--manifest",
            )?,
            "--key-file" => set_option(
                &mut key_file,
                next_value(&mut args, "--key-file")?,
                "--key-file",
            )?,
            "--key-id" => set_option(&mut key_id, next_value(&mut args, "--key-id")?, "--key-id")?,
            "--output" => set_option(&mut output, next_value(&mut args, "--output")?, "--output")?,
            _ => return Err("unknown sign option".to_owned()),
        }
    }

    Ok(SignOptions {
        manifest: PathBuf::from(required_option(manifest, "--manifest")?),
        key_file: PathBuf::from(required_option(key_file, "--key-file")?),
        key_id: required_option(key_id, "--key-id")?,
        output: PathBuf::from(required_option(output, "--output")?),
    })
}

fn set_option(slot: &mut Option<String>, value: String, option: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("duplicate {option}"));
    }
    *slot = Some(value);
    Ok(())
}

fn next_value<I>(args: &mut I, option: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("missing value for {option}"))
}

fn required_option(value: Option<String>, option: &str) -> Result<String, String> {
    value.ok_or_else(|| format!("missing {option}"))
}

fn required_format_version(value: Option<String>) -> Result<u8, String> {
    let value = required_option(value, "--format-version")?;
    value
        .parse::<u8>()
        .map_err(|_| "format version must be an integer".to_owned())
}

fn generate(options: GenerateOptions) -> Result<(), String> {
    validate_metadata(&options.metadata, true)?;
    let declared = fs::metadata(&options.archive)
        .map_err(|_| "could not inspect declared archive".to_owned())?;
    if !declared.is_file() {
        return Err("declared archive is not a regular file".to_owned());
    }
    validate_artifact_size(declared.len())?;

    let archive_bytes =
        fs::read(&options.archive).map_err(|_| "could not read declared archive".to_owned())?;
    let actual_size = u64::try_from(archive_bytes.len())
        .map_err(|_| "archive byte length cannot be represented".to_owned())?;
    validate_artifact_size(actual_size)?;
    if actual_size != declared.len() {
        return Err("declared archive changed while it was being read".to_owned());
    }

    let members = read_archive_entries(&archive_bytes)?;
    let manifest = manifest_from_archive(options.metadata, &archive_bytes, members)?;
    let bytes = serialize_manifest(&manifest)?;
    fs::write(&options.output, bytes).map_err(|_| "could not write release manifest".to_owned())
}

fn sign(options: SignOptions) -> Result<(), String> {
    validate_identifier(&options.key_id, "requested key id")?;
    let manifest_bytes =
        fs::read(&options.manifest).map_err(|_| "could not read release manifest".to_owned())?;
    let manifest = parse_manifest(&manifest_bytes)?;
    validate_requested_identity(&manifest, &options.key_id)?;

    let signing_key = read_signing_key(&options.key_file)?;
    let envelope = detached_signature(&manifest_bytes, &options.key_id, &signing_key)?;
    fs::write(&options.output, envelope)
        .map_err(|_| "could not write detached signature".to_owned())
}

fn read_signing_key(path: &Path) -> Result<SigningKey, String> {
    let metadata =
        fs::metadata(path).map_err(|_| "could not inspect signing key file".to_owned())?;
    if !metadata.is_file() || metadata.len() != 32 {
        return Err("signing key file must contain exactly 32 bytes".to_owned());
    }
    let key_bytes =
        Zeroizing::new(fs::read(path).map_err(|_| "could not read signing key file".to_owned())?);
    if key_bytes.len() != 32 {
        return Err("signing key file must contain exactly 32 bytes".to_owned());
    }

    let mut seed = [0_u8; 32];
    seed.copy_from_slice(key_bytes.as_slice());
    let signing_key = SigningKey::from_bytes(&seed);
    seed.zeroize();
    Ok(signing_key)
}

fn detached_signature(
    manifest_bytes: &[u8],
    key_id: &str,
    signing_key: &SigningKey,
) -> Result<Vec<u8>, String> {
    let signature = signing_key.sign(manifest_bytes);
    serde_json::to_vec(&DetachedSignature {
        algorithm: ED25519_ALGORITHM.to_owned(),
        key_id: key_id.to_owned(),
        signature: STANDARD.encode(signature.to_bytes()),
    })
    .map_err(|_| "could not serialize detached signature".to_owned())
}

fn parse_manifest(bytes: &[u8]) -> Result<ReleaseManifest, String> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err("release manifest exceeds its size bound".to_owned());
    }
    let manifest = serde_json::from_slice(bytes)
        .map_err(|_| "release manifest has invalid JSON or fields".to_owned())?;
    validate_manifest(&manifest, false)?;
    Ok(manifest)
}

fn serialize_manifest(manifest: &ReleaseManifest) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(manifest)
        .map_err(|_| "could not serialize release manifest".to_owned())?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err("release manifest exceeds its size bound".to_owned());
    }
    Ok(bytes)
}

fn manifest_from_archive(
    metadata: ManifestMetadata,
    archive_bytes: &[u8],
    members: Vec<String>,
) -> Result<ReleaseManifest, String> {
    let byte_size = u64::try_from(archive_bytes.len())
        .map_err(|_| "archive byte length cannot be represented".to_owned())?;
    let manifest = ReleaseManifest {
        format_version: metadata.format_version,
        product_version: metadata.product_version,
        editor_forge_compatibility_version: metadata.editor_forge_compatibility_version,
        channel: metadata.channel,
        signing_identity: SigningIdentity {
            key_id: metadata.signing_key_id,
            algorithm: metadata.algorithm,
        },
        minimum_installer_version: metadata.minimum_installer_version,
        minimum_cli_version: metadata.minimum_cli_version,
        artifacts: vec![ReleaseArtifact {
            artifact_id: metadata.artifact_id,
            platform: metadata.platform,
            architecture: metadata.architecture,
            libc: metadata.libc,
            archive_format: metadata.archive_format,
            file_name: metadata.file_name,
            byte_size,
            sha256: sha256_hex(archive_bytes),
            archive_entries: members,
        }],
    };
    validate_manifest(&manifest, true)?;
    Ok(manifest)
}

fn read_archive_entries(bytes: &[u8]) -> Result<Vec<String>, String> {
    let mut zip_archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| "declared archive is not a valid ZIP".to_owned())?;
    if zip_archive.len() == 0 {
        return Err("ZIP must contain at least one regular file".to_owned());
    }
    if zip_archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("ZIP contains too many entries".to_owned());
    }

    let mut expanded = 0_u64;
    let mut names = Vec::with_capacity(zip_archive.len());
    for index in 0..zip_archive.len() {
        let mut entry = zip_archive
            .by_index(index)
            .map_err(|_| "ZIP member is malformed".to_owned())?;
        if entry.is_dir() {
            return Err("ZIP directories are not release members".to_owned());
        }
        if entry.is_symlink() || !entry.is_file() {
            return Err("ZIP member is not a regular file".to_owned());
        }
        if entry.unix_mode().is_some_and(|mode| {
            let file_type = mode & 0o170_000;
            file_type != 0 && file_type != 0o100_000
        }) {
            return Err("ZIP member has a non-regular file type".to_owned());
        }
        if entry.encrypted() {
            return Err("encrypted ZIP members are not release members".to_owned());
        }
        if entry.size() > MAX_ENTRY_BYTES {
            return Err("ZIP member exceeds its size bound".to_owned());
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or_else(|| "ZIP expanded size cannot be represented".to_owned())?;
        if expanded > MAX_EXPANDED_BYTES {
            return Err("ZIP expanded size exceeds its bound".to_owned());
        }

        let name = validate_raw_member_name(entry.name_raw())?;
        io::copy(&mut entry, &mut io::sink())
            .map_err(|_| "ZIP member data is malformed".to_owned())?;
        names.push(name);
    }
    validate_archive_entries(&names)?;
    Ok(names)
}

fn validate_raw_member_name(raw: &[u8]) -> Result<String, String> {
    if raw.len() > MAX_MEMBER_NAME_BYTES {
        return Err("ZIP member name exceeds its byte bound".to_owned());
    }
    if !raw.is_ascii() {
        return Err("ZIP member name must be ASCII".to_owned());
    }
    let name =
        std::str::from_utf8(raw).map_err(|_| "ZIP member name is not valid UTF-8".to_owned())?;
    validate_archive_member_name(name)?;
    Ok(name.to_owned())
}

fn validate_archive_entries(entries: &[String]) -> Result<(), String> {
    if entries.is_empty() {
        return Err("archive member list must not be empty".to_owned());
    }
    if entries.len() > MAX_ARCHIVE_ENTRIES {
        return Err("archive member list exceeds its entry bound".to_owned());
    }

    let mut exact = BTreeSet::new();
    let mut folded = BTreeSet::new();
    let mut previous = None;
    for name in entries {
        validate_archive_member_name(name)?;
        if let Some(previous) = previous {
            if name.as_str() < previous {
                return Err("archive members must be bytewise sorted".to_owned());
            }
        }
        if !exact.insert(name.clone()) {
            return Err("archive members must be unique".to_owned());
        }
        if !folded.insert(name.to_ascii_lowercase()) {
            return Err("archive members must not collide case-insensitively".to_owned());
        }
        previous = Some(name.as_str());
    }
    Ok(())
}

fn validate_archive_member_name(name: &str) -> Result<(), String> {
    validate_safe_path(name, MAX_MEMBER_NAME_BYTES, "archive member")
}

fn validate_safe_path(path: &str, max_bytes: usize, description: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err(format!("{description} must not be empty"));
    }
    if path.len() > max_bytes {
        return Err(format!("{description} exceeds its byte bound"));
    }
    if !path.is_ascii() {
        return Err(format!("{description} must be ASCII"));
    }
    if path.starts_with('/') {
        return Err(format!("{description} must be relative"));
    }
    if path.contains('\\') {
        return Err(format!("{description} must not contain backslashes"));
    }
    if path.contains(':') {
        return Err(format!("{description} must not contain drives or ADS"));
    }
    if path
        .bytes()
        .any(|byte| !SAFE_MEMBER_CHARS.contains(char::from(byte)))
    {
        return Err(format!("{description} contains a non-canonical character"));
    }
    for segment in path.split('/') {
        validate_path_segment(segment, description)?;
    }
    Ok(())
}

fn validate_path_segment(segment: &str, description: &str) -> Result<(), String> {
    if segment.is_empty() || segment == "." || segment == ".." {
        return Err(format!("{description} has an empty or traversal segment"));
    }
    if segment.ends_with('.') {
        return Err(format!("{description} has a trailing-dot segment"));
    }
    if segment.len() > MAX_PATH_SEGMENT_BYTES {
        return Err(format!("{description} has an overlong segment"));
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
        return Err(format!("{description} names a reserved device"));
    }
    Ok(())
}

fn validate_metadata(metadata: &ManifestMetadata, require_zip: bool) -> Result<(), String> {
    validate_manifest_header(
        metadata.format_version,
        &metadata.product_version,
        &metadata.editor_forge_compatibility_version,
        &metadata.channel,
        &metadata.signing_key_id,
        &metadata.algorithm,
        &metadata.minimum_installer_version,
        &metadata.minimum_cli_version,
    )?;
    validate_identifier(&metadata.artifact_id, "artifact id")?;
    validate_platform(
        &metadata.platform,
        &metadata.architecture,
        metadata.libc.as_deref(),
    )?;
    validate_archive_format(&metadata.archive_format, require_zip)?;
    validate_safe_path(
        &metadata.file_name,
        MAX_MEMBER_NAME_BYTES,
        "artifact file name",
    )
}

fn validate_manifest(manifest: &ReleaseManifest, require_zip: bool) -> Result<(), String> {
    validate_manifest_header(
        manifest.format_version,
        &manifest.product_version,
        &manifest.editor_forge_compatibility_version,
        &manifest.channel,
        &manifest.signing_identity.key_id,
        &manifest.signing_identity.algorithm,
        &manifest.minimum_installer_version,
        &manifest.minimum_cli_version,
    )?;
    if manifest.artifacts.is_empty() {
        return Err("release manifest must contain an artifact".to_owned());
    }
    if manifest.artifacts.len() > MAX_MANIFEST_ARTIFACTS {
        return Err("release manifest contains too many artifacts".to_owned());
    }

    let mut artifact_ids = BTreeSet::new();
    let mut casefolded = BTreeSet::new();
    for artifact in &manifest.artifacts {
        validate_artifact(artifact, require_zip)?;
        if !artifact_ids.insert(artifact.artifact_id.clone()) {
            return Err("artifact ids must be unique".to_owned());
        }
        if !casefolded.insert(artifact.artifact_id.to_ascii_lowercase()) {
            return Err("artifact ids must not collide case-insensitively".to_owned());
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_manifest_header(
    format_version: u8,
    product_version: &str,
    editor_forge_compatibility_version: &str,
    channel: &str,
    signing_key_id: &str,
    algorithm: &str,
    minimum_installer_version: &str,
    minimum_cli_version: &str,
) -> Result<(), String> {
    if format_version != FORMAT_VERSION {
        return Err("release manifest format version is unsupported".to_owned());
    }
    validate_semantic_version(product_version, "product version")?;
    validate_semantic_version(
        editor_forge_compatibility_version,
        "editor/Forge compatibility version",
    )?;
    validate_semantic_version(minimum_installer_version, "minimum installer version")?;
    validate_semantic_version(minimum_cli_version, "minimum CLI version")?;
    if !matches!(channel, "stable" | "beta" | "nightly") {
        return Err("release channel is unsupported".to_owned());
    }
    validate_identifier(signing_key_id, "signing key id")?;
    if algorithm != ED25519_ALGORITHM {
        return Err("release signing algorithm is unsupported".to_owned());
    }
    Ok(())
}

fn validate_artifact(artifact: &ReleaseArtifact, require_zip: bool) -> Result<(), String> {
    validate_identifier(&artifact.artifact_id, "artifact id")?;
    validate_platform(
        &artifact.platform,
        &artifact.architecture,
        artifact.libc.as_deref(),
    )?;
    validate_archive_format(&artifact.archive_format, require_zip)?;
    validate_safe_path(
        &artifact.file_name,
        MAX_MEMBER_NAME_BYTES,
        "artifact file name",
    )?;
    validate_artifact_size(artifact.byte_size)?;
    validate_sha256(&artifact.sha256)?;
    validate_archive_entries(&artifact.archive_entries)
}

fn validate_semantic_version(value: &str, description: &str) -> Result<(), String> {
    Version::parse(value)
        .map(|_| ())
        .map_err(|_| format!("{description} is not a valid semantic version"))
}

fn validate_identifier(value: &str, description: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || !value.is_ascii() {
        return Err(format!("{description} is empty or unsafe"));
    }
    if value == "." || value == ".." {
        return Err(format!("{description} is empty or unsafe"));
    }
    if value
        .bytes()
        .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(format!("{description} is empty or unsafe"));
    }
    Ok(())
}

fn validate_platform(platform: &str, architecture: &str, libc: Option<&str>) -> Result<(), String> {
    if !matches!(platform, "windows" | "macos" | "linux") {
        return Err("artifact platform is unsupported".to_owned());
    }
    if !matches!(architecture, "x64" | "arm64") {
        return Err("artifact architecture is unsupported".to_owned());
    }
    match (platform, libc) {
        ("windows" | "macos", None) | ("linux", None | Some("glibc")) => Ok(()),
        ("windows" | "macos", Some(_)) => {
            Err("libc is unsupported for this artifact platform".to_owned())
        }
        ("linux", Some(_)) => Err("artifact libc is unsupported".to_owned()),
        _ => Err("artifact platform is unsupported".to_owned()),
    }
}

fn validate_archive_format(format: &str, require_zip: bool) -> Result<(), String> {
    if !matches!(format, "zip" | "tar.zst") {
        return Err("archive format is unsupported".to_owned());
    }
    if require_zip && format != "zip" {
        return Err("generate mode accepts only ZIP artifacts".to_owned());
    }
    Ok(())
}

fn validate_artifact_size(size: u64) -> Result<(), String> {
    if size == 0 {
        return Err("artifact must not be empty".to_owned());
    }
    if size > MAX_ARTIFACT_BYTES {
        return Err("artifact exceeds its byte bound".to_owned());
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("artifact SHA-256 must be 64 lowercase hexadecimal characters".to_owned());
    }
    Ok(())
}

fn validate_requested_identity(
    manifest: &ReleaseManifest,
    requested_key_id: &str,
) -> Result<(), String> {
    if manifest.signing_identity.algorithm != ED25519_ALGORITHM {
        return Err("manifest signing algorithm must be ed25519".to_owned());
    }
    if manifest.signing_identity.key_id != requested_key_id {
        return Err("manifest signing identity does not match requested key id".to_owned());
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
    use super::*;
    use ed25519_dalek::{Signature, Verifier};
    use std::io::Write;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    fn concrete_metadata() -> ManifestMetadata {
        ManifestMetadata {
            format_version: 1,
            product_version: "0.0.0".to_owned(),
            editor_forge_compatibility_version: "0.0.0".to_owned(),
            channel: "nightly".to_owned(),
            signing_key_id: "development".to_owned(),
            algorithm: "ed25519".to_owned(),
            minimum_installer_version: "0.0.0".to_owned(),
            minimum_cli_version: "0.0.0".to_owned(),
            artifact_id: "windows-x64".to_owned(),
            platform: "windows".to_owned(),
            architecture: "x64".to_owned(),
            libc: None,
            archive_format: "zip".to_owned(),
            file_name: "artisan-editor-versioned-payload.zip".to_owned(),
        }
    }

    fn concrete_entries() -> Vec<String> {
        [
            "bin/ae.exe",
            "bin/editor.exe",
            "bin/forge.exe",
            "bin/installer.exe",
            "payload-manifest.json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn zip_with_members(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, bytes) in members {
            writer.start_file(*name, options).expect("start ZIP file");
            writer.write_all(bytes).expect("write ZIP file");
        }
        writer.finish().expect("finish ZIP").into_inner()
    }

    fn zip_with_directory() -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_directory("bin/", SimpleFileOptions::default())
            .expect("add ZIP directory");
        writer.finish().expect("finish ZIP").into_inner()
    }

    fn concrete_manifest(archive: &[u8], entries: Vec<String>) -> Vec<u8> {
        serialize_manifest(
            &manifest_from_archive(concrete_metadata(), archive, entries).expect("manifest"),
        )
        .expect("serialize manifest")
    }

    fn verify_for_test(payload: &[u8], envelope_bytes: &[u8], key: &SigningKey) -> bool {
        let Ok(envelope) = serde_json::from_slice::<DetachedSignature>(envelope_bytes) else {
            return false;
        };
        let Ok(manifest) = parse_manifest(payload) else {
            return false;
        };
        if manifest.signing_identity.algorithm != envelope.algorithm
            || manifest.signing_identity.key_id != envelope.key_id
        {
            return false;
        }
        let Ok(encoded_signature) = STANDARD.decode(envelope.signature) else {
            return false;
        };
        let Ok(signature) = Signature::from_slice(&encoded_signature) else {
            return false;
        };
        key.verifying_key().verify(payload, &signature).is_ok()
    }

    #[test]
    fn manifest_is_compact_and_has_the_exact_v1_field_order() {
        let archive = b"archive bytes";
        let bytes = concrete_manifest(archive, concrete_entries());
        let byte_size = archive.len();
        let digest = sha256_hex(archive);
        let expected = format!(
            r#"{{"format_version":1,"product_version":"0.0.0","editor_forge_compatibility_version":"0.0.0","channel":"nightly","signing_identity":{{"key_id":"development","algorithm":"ed25519"}},"minimum_installer_version":"0.0.0","minimum_cli_version":"0.0.0","artifacts":[{{"artifact_id":"windows-x64","platform":"windows","architecture":"x64","archive_format":"zip","file_name":"artisan-editor-versioned-payload.zip","byte_size":{byte_size},"sha256":"{digest}","archive_entries":["bin/ae.exe","bin/editor.exe","bin/forge.exe","bin/installer.exe","payload-manifest.json"]}}]}}"#,
        );
        assert_eq!(bytes, expected.into_bytes());
        assert!(bytes.iter().all(|byte| !byte.is_ascii_whitespace()));
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert!(value["artifacts"][0].get("libc").is_none());
    }

    #[test]
    fn metadata_argument_order_does_not_change_manifest_bytes() {
        let ordered = vec![
            "--archive",
            "archive.zip",
            "--output",
            "manifest.json",
            "--format-version",
            "1",
            "--product-version",
            "0.0.0",
            "--editor-forge-compatibility-version",
            "0.0.0",
            "--channel",
            "nightly",
            "--signing-key-id",
            "development",
            "--algorithm",
            "ed25519",
            "--minimum-installer-version",
            "0.0.0",
            "--minimum-cli-version",
            "0.0.0",
            "--artifact-id",
            "windows-x64",
            "--platform",
            "windows",
            "--architecture",
            "x64",
            "--archive-format",
            "zip",
            "--file-name",
            "artisan-editor-versioned-payload.zip",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let permuted = vec![
            "--file-name",
            "artisan-editor-versioned-payload.zip",
            "--architecture",
            "x64",
            "--archive-format",
            "zip",
            "--platform",
            "windows",
            "--artifact-id",
            "windows-x64",
            "--minimum-cli-version",
            "0.0.0",
            "--minimum-installer-version",
            "0.0.0",
            "--algorithm",
            "ed25519",
            "--signing-key-id",
            "development",
            "--channel",
            "nightly",
            "--editor-forge-compatibility-version",
            "0.0.0",
            "--product-version",
            "0.0.0",
            "--format-version",
            "1",
            "--output",
            "manifest.json",
            "--archive",
            "archive.zip",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let first = parse_generate_args(ordered.into_iter()).expect("ordered arguments");
        let second = parse_generate_args(permuted.into_iter()).expect("permuted arguments");
        assert_eq!(
            serialize_manifest(
                &manifest_from_archive(first.metadata, b"same archive", concrete_entries())
                    .expect("first manifest")
            )
            .expect("first bytes"),
            serialize_manifest(
                &manifest_from_archive(second.metadata, b"same archive", concrete_entries())
                    .expect("second manifest")
            )
            .expect("second bytes")
        );
    }

    #[test]
    fn archive_hash_size_and_sorted_regular_member_list_are_exact() {
        let archive = zip_with_members(&[
            ("bin/ae.exe", b"ae"),
            ("bin/editor.exe", b"editor"),
            ("payload-manifest.json", b"manifest"),
        ]);
        let entries = read_archive_entries(&archive).expect("archive entries");
        assert_eq!(
            entries,
            ["bin/ae.exe", "bin/editor.exe", "payload-manifest.json"]
        );
        let manifest = manifest_from_archive(concrete_metadata(), &archive, entries)
            .expect("archive manifest");
        assert_eq!(
            manifest.artifacts[0].byte_size,
            u64::try_from(archive.len()).expect("archive length")
        );
        assert_eq!(manifest.artifacts[0].sha256, sha256_hex(&archive));
    }

    #[test]
    fn metadata_and_size_bounds_fail_closed() {
        let mut metadata = concrete_metadata();
        metadata.product_version = "1.0".to_owned();
        assert!(validate_metadata(&metadata, true).is_err());

        let mut metadata = concrete_metadata();
        metadata.format_version = 2;
        assert!(validate_metadata(&metadata, true).is_err());

        let mut metadata = concrete_metadata();
        metadata.algorithm = "rsa".to_owned();
        assert!(validate_metadata(&metadata, true).is_err());

        let mut metadata = concrete_metadata();
        metadata.channel = "canary".to_owned();
        assert!(validate_metadata(&metadata, true).is_err());

        let mut metadata = concrete_metadata();
        metadata.platform = "android".to_owned();
        assert!(validate_metadata(&metadata, true).is_err());

        let mut metadata = concrete_metadata();
        metadata.architecture = "x86".to_owned();
        assert!(validate_metadata(&metadata, true).is_err());

        let mut metadata = concrete_metadata();
        metadata.libc = Some("musl".to_owned());
        assert!(validate_metadata(&metadata, true).is_err());

        let mut metadata = concrete_metadata();
        metadata.archive_format = "tar.zst".to_owned();
        assert!(validate_metadata(&metadata, true).is_err());

        let mut metadata = concrete_metadata();
        metadata.artifact_id = "../artifact".to_owned();
        assert!(validate_metadata(&metadata, true).is_err());

        let mut metadata = concrete_metadata();
        metadata.signing_key_id.clear();
        assert!(validate_metadata(&metadata, true).is_err());

        let mut metadata = concrete_metadata();
        metadata.file_name = "../payload.zip".to_owned();
        assert!(validate_metadata(&metadata, true).is_err());

        assert!(validate_artifact_size(0).is_err());
        assert!(validate_artifact_size(MAX_ARTIFACT_BYTES + 1).is_err());
    }

    #[test]
    fn unsafe_duplicate_collision_and_unsorted_members_fail_closed() {
        let overlong_segment = "a".repeat(1_020);
        let invalid = [
            vec![String::new()],
            vec!["bin/ae.exe".to_owned(), "bin/ae.exe".to_owned()],
            vec!["bin/AE.exe".to_owned(), "bin/ae.exe".to_owned()],
            vec!["z.txt".to_owned(), "a.txt".to_owned()],
            vec!["/absolute".to_owned()],
            vec![r"bin\ae.exe".to_owned()],
            vec!["C:/ae.exe".to_owned()],
            vec!["bin/ae.exe:stream".to_owned()],
            vec!["bin/../ae.exe".to_owned()],
            vec!["bin/./ae.exe".to_owned()],
            vec!["bin//ae.exe".to_owned()],
            vec!["bin/café.exe".to_owned()],
            vec!["bin/ae.exe.".to_owned()],
            vec!["bin/CON.exe".to_owned()],
            vec!["bin/com1.txt".to_owned()],
            vec![format!("bin/{overlong_segment}.exe")],
        ];
        for entries in invalid {
            assert!(
                validate_archive_entries(&entries).is_err(),
                "unsafe entries must be rejected: {entries:?}"
            );
        }

        let too_many = (0..=MAX_ARCHIVE_ENTRIES)
            .map(|index| format!("member-{index:05}"))
            .collect::<Vec<_>>();
        assert!(validate_archive_entries(&too_many).is_err());
        assert!(read_archive_entries(b"not a ZIP").is_err());
        assert!(read_archive_entries(&zip_with_directory()).is_err());
    }

    #[test]
    fn generate_mode_rejects_key_file_options_without_echoing_values() {
        let error = parse_generate_args(
            ["--key-file", "test-only-key-path"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect_err("generate must not accept a key file");
        assert_eq!(error, "unknown generate option");
        assert!(!error.contains("test-only-key-path"));
    }

    #[test]
    fn detached_signature_shape_binding_and_tamper_rejection_are_exact() {
        // Fixed test-only seed; production signing accepts bytes only from its runtime key file.
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let manifest = concrete_manifest(b"archive bytes", concrete_entries());
        let envelope = {
            let parsed = parse_manifest(&manifest).expect("manifest");
            validate_requested_identity(&parsed, "development").expect("identity");
            detached_signature(&manifest, "development", &signing_key).expect("signature")
        };
        let expected_signature = signing_key.sign(&manifest);
        let signature_text = STANDARD.encode(expected_signature.to_bytes());
        let expected = format!(
            r#"{{"algorithm":"ed25519","key_id":"development","signature":"{signature_text}"}}"#,
        );
        assert_eq!(envelope, expected.into_bytes());
        assert!(verify_for_test(&manifest, &envelope, &signing_key));

        let mut altered_manifest = manifest.clone();
        let marker = b"\"product_version\":\"0.0.0\"";
        let offset = altered_manifest
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("product version marker");
        altered_manifest[offset + marker.len() - 5] = b'1';
        assert!(!verify_for_test(&altered_manifest, &envelope, &signing_key));

        let mut altered_envelope =
            serde_json::from_slice::<DetachedSignature>(&envelope).expect("envelope");
        let mut tampered_bytes = STANDARD
            .decode(&altered_envelope.signature)
            .expect("signature bytes");
        tampered_bytes[0] ^= 1;
        altered_envelope.signature = STANDARD.encode(tampered_bytes);
        let invalid_signature = serde_json::to_vec(&altered_envelope).expect("envelope");
        assert!(!verify_for_test(
            &manifest,
            &invalid_signature,
            &signing_key
        ));

        let mut altered_key_id =
            serde_json::from_slice::<DetachedSignature>(&envelope).expect("envelope");
        altered_key_id.key_id = "other".to_owned();
        let invalid_key_id = serde_json::to_vec(&altered_key_id).expect("envelope");
        assert!(!verify_for_test(&manifest, &invalid_key_id, &signing_key));

        assert!(
            detached_signature(&manifest, "other", &signing_key).is_ok(),
            "the low-level serializer does not own identity policy"
        );
        let mut changed_identity = parse_manifest(&manifest).expect("manifest");
        changed_identity.signing_identity.key_id = "other".to_owned();
        let changed_identity = serialize_manifest(&changed_identity).expect("manifest");
        let parsed = parse_manifest(&changed_identity).expect("changed manifest");
        assert!(validate_requested_identity(&parsed, "development").is_err());
    }

    #[test]
    fn runtime_signer_uses_only_the_named_test_key_file() {
        // Fixed test-only seed; this file exists only for the duration of this test.
        let signing_key_bytes = [7_u8; 32];
        let process_id = process::id();
        let root = env::temp_dir().join(format!("artisan-release-tool-{process_id}"));
        fs::create_dir(&root).expect("create test directory");
        let manifest_path = root.join("manifest.json");
        let key_path = root.join("test-key.bin");
        let output_path = root.join("manifest.sig.json");
        let manifest = concrete_manifest(b"archive bytes", concrete_entries());
        fs::write(&manifest_path, &manifest).expect("write test manifest");
        fs::write(&key_path, signing_key_bytes).expect("write test key");

        sign(SignOptions {
            manifest: manifest_path,
            key_file: key_path,
            key_id: "development".to_owned(),
            output: output_path.clone(),
        })
        .expect("sign test manifest");
        let envelope = fs::read(&output_path).expect("read envelope");
        assert!(verify_for_test(
            &manifest,
            &envelope,
            &SigningKey::from_bytes(&signing_key_bytes)
        ));
        fs::remove_dir_all(root).expect("remove test directory");
    }
}
