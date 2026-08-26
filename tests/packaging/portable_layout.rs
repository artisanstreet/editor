//! Layout-only proof of the Phase 9 portable runtime metadata contract.
//!
//! The proof covers exactly what [`packaging/portable/portable_layout.txt`]
//! declares (see `docs/decisions/NATIVE_PRODUCT_SCOPE.md`, "Process and package
//! decisions"):
//!
//! - the machine-readable contract parses fail-closed: schema version 1,
//!   layout-only proof scope, the stable `artisan-editor` package root, the
//!   declared sibling executable leaves, the optional resource/license
//!   directory names, mutable state outside the package root, and literal-false
//!   production fallbacks. Unknown keys, duplicate keys, missing keys, CR
//!   carriers, and any drifted value reject the contract;
//! - every declared name is a canonical safe Windows archive/runtime relative
//!   name: absolute, drive-letter, UNC, backslash, colon, dot-segment, leading
//!   separator, trailing-dot, whitespace, control-byte, non-ASCII, oversized,
//!   and reserved-device forms are rejected, and no two declared names collide
//!   case-insensitively;
//! - the two Bazel-built binaries arrive through their declared `data` runfiles
//!   (manifest or directory layout, test-only), resolve to exactly the expected
//!   artifacts, prove regular, non-symlink, non-empty PE images (`MZ`), stage
//!   into a fresh temporary `artisan-editor` root under their declared sibling
//!   names with no extra files, survive relocation into a differently named
//!   parent, and the relocated editor parent derives its Forge sibling from
//!   layout alone;
//! - the harness cleans up only its own uniquely named temporary area.
//!
//! Deliberately out of scope, matching the deferred decisions: no installer,
//! archive producer, updater, signing system, release channel, distribution
//! service, process supervisor, authenticated handoff, or real-engine smoke is
//! implemented, invoked, or claimed here. Neither executable is launched, no
//! network is touched, and no installer or user state is written. Runfiles
//! resolution exists only inside this test; production fallback flags stay
//! literally false.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ------------------------------------------------------------ contract text

/// The exported layout contract, embedded at compile time from the declared
/// `compile_data` label `//packaging/portable:portable_layout.txt`.
const METADATA_TEXT: &str = include_str!("../../packaging/portable/portable_layout.txt");

const KEY_SCHEMA_VERSION: &str = "schema_version";
const KEY_PROOF_SCOPE: &str = "proof_scope";
const KEY_PACKAGE_ROOT: &str = "package_root";
const KEY_EDITOR_SIBLING: &str = "editor_sibling";
const KEY_FORGE_SIBLING: &str = "forge_sibling";
const KEY_RESOURCES_DIR: &str = "resources_dir";
const KEY_LICENSES_DIR: &str = "licenses_dir";
const KEY_MUTABLE_STATE: &str = "mutable_state";
const KEY_SOURCE_TREE_FALLBACK: &str = "source_tree_fallback";
const KEY_RUNFILES_FALLBACK: &str = "runfiles_fallback";

/// Exact key set in declaration order; anything outside it is unknown.
const CONTRACT_KEYS: [&str; 10] = [
    KEY_SCHEMA_VERSION,
    KEY_PROOF_SCOPE,
    KEY_PACKAGE_ROOT,
    KEY_EDITOR_SIBLING,
    KEY_FORGE_SIBLING,
    KEY_RESOURCES_DIR,
    KEY_LICENSES_DIR,
    KEY_MUTABLE_STATE,
    KEY_SOURCE_TREE_FALLBACK,
    KEY_RUNFILES_FALLBACK,
];

// ------------------------------------------------------------ typed values

/// What the currently reached packaging proof covers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProofScope {
    /// Package layout only; no workflow or clean-host claim.
    LayoutOnly,
}

impl ProofScope {
    const LAYOUT_ONLY: &'static str = "layout-only";

    fn parse(raw: &str) -> Result<Self, ValueError> {
        if raw == Self::LAYOUT_ONLY {
            Ok(Self::LayoutOnly)
        } else {
            Err(ValueError::ProofScope {
                found: raw.to_owned(),
            })
        }
    }
}

/// Where mutable runtime state lives relative to the package root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutableStateLocation {
    /// The package root stays immutable; writable state lives elsewhere.
    OutsidePackageRoot,
}

impl MutableStateLocation {
    const OUTSIDE_PACKAGE_ROOT: &'static str = "outside-package-root";

    fn parse(raw: &str) -> Result<Self, ValueError> {
        if raw == Self::OUTSIDE_PACKAGE_ROOT {
            Ok(Self::OutsidePackageRoot)
        } else {
            Err(ValueError::MutableState {
                found: raw.to_owned(),
            })
        }
    }
}

/// A production resolution policy that must stay disabled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProductionFallback {
    /// The policy is explicitly off; production never uses the fallback.
    Disabled,
}

impl ProductionFallback {
    const DISABLED: &'static str = "false";

    fn parse(key: &'static str, raw: &str) -> Result<Self, ValueError> {
        if raw == Self::DISABLED {
            Ok(Self::Disabled)
        } else {
            Err(ValueError::FallbackMustBeFalse {
                key,
                found: raw.to_owned(),
            })
        }
    }
}

/// Reserved optional member directories of a portable package. Presence is
/// optional; when shipped they must use exactly these names at the root.
#[derive(Clone, Debug, Eq, PartialEq)]
struct OptionalDirs {
    resources: SafeRelativeName,
    licenses: SafeRelativeName,
}

/// The parsed portable runtime layout contract.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PortableLayout {
    schema_version: u32,
    proof_scope: ProofScope,
    package_root: SafeRelativeName,
    editor_sibling: SafeRelativeName,
    forge_sibling: SafeRelativeName,
    optional_dirs: OptionalDirs,
    mutable_state: MutableStateLocation,
    source_tree_fallback: ProductionFallback,
    runfiles_fallback: ProductionFallback,
}

// ------------------------------------------------------- safe relative names

/// One canonical, single-component Windows-safe archive/runtime name.
///
/// Constructed only through [`SafeRelativeName::parse`], which rejects every
/// unsafe form outright. Because `/`, `\`, and `:` are disallowed bytes,
/// absolute paths, drive-letter forms, UNC paths, alternate data streams, and
/// multi-segment relatives all collapse into [`NameRejection::DisallowedByte`];
/// the remaining variants cover dot segments, trailing dots, and reserved
/// device names.
#[derive(Clone, Debug, Eq, PartialEq)]
struct SafeRelativeName(String);

/// Byte budget per name, matching the classic Windows component limit.
const MAX_NAME_BYTES: usize = 255;

/// Why a candidate name failed canonical validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NameRejection {
    Empty,
    TooLong {
        length: usize,
    },
    /// Includes `/`, `\`, `:`, whitespace, control bytes, and non-ASCII.
    DisallowedByte {
        byte: u8,
    },
    DotSegment,
    TrailingDot,
    ReservedDevice {
        device: &'static str,
    },
}

impl SafeRelativeName {
    /// Validates `raw` as one canonical safe Windows name component.
    fn parse(raw: &str) -> Result<Self, NameRejection> {
        if raw.is_empty() {
            return Err(NameRejection::Empty);
        }
        let bytes = raw.as_bytes();
        if bytes.len() > MAX_NAME_BYTES {
            return Err(NameRejection::TooLong {
                length: bytes.len(),
            });
        }
        for &byte in bytes {
            let allowed = byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-');
            if !allowed {
                return Err(NameRejection::DisallowedByte { byte });
            }
        }
        if raw == "." || raw == ".." {
            return Err(NameRejection::DotSegment);
        }
        if raw.ends_with('.') {
            return Err(NameRejection::TrailingDot);
        }
        // Trailing spaces cannot occur given the allowed charset above; the
        // explicit Windows strip-trailing-space hazard is therefore covered
        // by construction rather than a separate rejection branch.
        if let Some(device) = reserved_device_stem(raw) {
            return Err(NameRejection::ReservedDevice { device });
        }
        Ok(Self(raw.to_owned()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// Maps a name onto its reserved Windows device stem when it has one.
///
/// Matches the stem before the first dot, ASCII-case-insensitively, against
/// the classic DOS device vocabulary (including the `COM0`/`LPT0` additions).
fn reserved_device_stem(raw: &str) -> Option<&'static str> {
    let upper = raw.to_ascii_uppercase();
    let stem = upper.split('.').next().unwrap_or_default();
    let devices = [
        "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
        "LPT9",
    ];
    devices.iter().find(|device| **device == stem).copied()
}

// ------------------------------------------------------------ parse errors

/// Failure to parse or validate the layout contract.
#[derive(Debug)]
enum ContractError {
    Syntax(SyntaxError),
    Value(ValueError),
}

/// Line-level contract violations.
#[derive(Debug)]
enum SyntaxError {
    /// The text carries a carriage return; the contract is LF-only.
    CarriageReturn,
    MalformedLine {
        number: usize,
        text: String,
    },
    DuplicateKey {
        key: &'static str,
        first_line: usize,
        second_line: usize,
    },
    UnknownKey {
        key: String,
        number: usize,
    },
    MissingKey {
        key: &'static str,
    },
}

/// Value-level contract violations, each tied to its declaring key.
#[derive(Debug)]
enum ValueError {
    SchemaVersion {
        found: String,
    },
    ProofScope {
        found: String,
    },
    MutableState {
        found: String,
    },
    FallbackMustBeFalse {
        key: &'static str,
        found: String,
    },
    Name {
        key: &'static str,
        found: String,
        rejection: NameRejection,
    },
    SiblingNeedsExeSuffix {
        key: &'static str,
        found: String,
    },
    CaseInsensitiveCollision {
        first: &'static str,
        second: &'static str,
    },
}

impl std::fmt::Display for NameRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(formatter, "name is empty"),
            Self::TooLong { length } => {
                write!(
                    formatter,
                    "name is {length} bytes, limit is {MAX_NAME_BYTES}"
                )
            }
            Self::DisallowedByte { byte } => {
                write!(formatter, "disallowed byte 0x{byte:02X} in name")
            }
            Self::DotSegment => write!(formatter, "dot segment is not a valid name"),
            Self::TrailingDot => write!(formatter, "trailing dot would be stripped by Windows"),
            Self::ReservedDevice { device } => {
                write!(formatter, "reserved Windows device name {device}")
            }
        }
    }
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CarriageReturn => {
                write!(formatter, "contract contains a carriage return; LF only")
            }
            Self::MalformedLine { number, text } => {
                write!(formatter, "line {number} is malformed: {text:?}")
            }
            Self::DuplicateKey {
                key,
                first_line,
                second_line,
            } => write!(
                formatter,
                "key {key} duplicated on lines {first_line} and {second_line}"
            ),
            Self::UnknownKey { key, number } => {
                write!(formatter, "unknown key {key:?} on line {number}")
            }
            Self::MissingKey { key } => write!(formatter, "missing required key {key}"),
        }
    }
}

impl std::fmt::Display for ValueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SchemaVersion { found } => {
                write!(
                    formatter,
                    "{KEY_SCHEMA_VERSION} must be the literal \"1\", found {found:?}"
                )
            }
            Self::ProofScope { found } => {
                write!(
                    formatter,
                    "{KEY_PROOF_SCOPE} must be {:?}, found {found:?}",
                    ProofScope::LAYOUT_ONLY
                )
            }
            Self::MutableState { found } => {
                write!(
                    formatter,
                    "{KEY_MUTABLE_STATE} must be {:?}, found {found:?}",
                    MutableStateLocation::OUTSIDE_PACKAGE_ROOT
                )
            }
            Self::FallbackMustBeFalse { key, found } => {
                write!(
                    formatter,
                    "{key} must be the literal \"false\", found {found:?}"
                )
            }
            Self::Name {
                key,
                found,
                rejection,
            } => write!(
                formatter,
                "{key} value {found:?} is not a safe relative name: {rejection}"
            ),
            Self::SiblingNeedsExeSuffix { key, found } => {
                write!(formatter, "{key} value {found:?} must end in \".exe\"")
            }
            Self::CaseInsensitiveCollision { first, second } => {
                write!(
                    formatter,
                    "keys {first} and {second} collide case-insensitively on Windows"
                )
            }
        }
    }
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax(syntax) => std::fmt::Display::fmt(syntax, formatter),
            Self::Value(value) => std::fmt::Display::fmt(value, formatter),
        }
    }
}

impl Error for ContractError {}

impl From<ValueError> for ContractError {
    fn from(value: ValueError) -> Self {
        Self::Value(value)
    }
}

/// One accepted `key = value` line from the contract text.
struct FoundEntry {
    key: &'static str,
    number: usize,
    value: String,
}

/// Scans contract text into exactly-once entries, fail-closed on any
/// structural violation: CR carriers, malformed lines, unknown keys,
/// duplicate keys, or empty keys/values. Blank lines and `#` comments pass.
fn scan_entries(text: &str) -> Result<Vec<FoundEntry>, ContractError> {
    if text.contains('\r') {
        return Err(ContractError::Syntax(SyntaxError::CarriageReturn));
    }
    let mut found: Vec<FoundEntry> = Vec::new();
    for (offset, raw_line) in text.lines().enumerate() {
        let number = offset + 1;
        if raw_line.is_empty() || raw_line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = raw_line.split_once('=') else {
            return Err(ContractError::Syntax(SyntaxError::MalformedLine {
                number,
                text: raw_line.to_owned(),
            }));
        };
        let key = raw_key.trim_ascii();
        let value = raw_value.trim_ascii();
        if key.is_empty() || value.is_empty() {
            return Err(ContractError::Syntax(SyntaxError::MalformedLine {
                number,
                text: raw_line.to_owned(),
            }));
        }
        let Some(&known_key) = CONTRACT_KEYS.iter().find(|candidate| **candidate == key) else {
            return Err(ContractError::Syntax(SyntaxError::UnknownKey {
                key: key.to_owned(),
                number,
            }));
        };
        if let Some(previous) = found.iter().find(|entry| entry.key == known_key) {
            return Err(ContractError::Syntax(SyntaxError::DuplicateKey {
                key: known_key,
                first_line: previous.number,
                second_line: number,
            }));
        }
        found.push(FoundEntry {
            key: known_key,
            number,
            value: value.to_owned(),
        });
    }
    Ok(found)
}

/// Parses the contract fail-closed: exact key set, no duplicates, no missing
/// keys, and every value inside its closed vocabulary.
fn parse_contract(text: &str) -> Result<PortableLayout, ContractError> {
    let found = scan_entries(text)?;

    let schema = required_entry(&found, KEY_SCHEMA_VERSION)?;
    let scope = required_entry(&found, KEY_PROOF_SCOPE)?;
    let package_root = parse_name_for(
        KEY_PACKAGE_ROOT,
        &required_entry(&found, KEY_PACKAGE_ROOT)?.value,
    )?;
    let editor_sibling = parse_name_for(
        KEY_EDITOR_SIBLING,
        &required_entry(&found, KEY_EDITOR_SIBLING)?.value,
    )?;
    let forge_sibling = parse_name_for(
        KEY_FORGE_SIBLING,
        &required_entry(&found, KEY_FORGE_SIBLING)?.value,
    )?;
    let resources_dir = parse_name_for(
        KEY_RESOURCES_DIR,
        &required_entry(&found, KEY_RESOURCES_DIR)?.value,
    )?;
    let licenses_dir = parse_name_for(
        KEY_LICENSES_DIR,
        &required_entry(&found, KEY_LICENSES_DIR)?.value,
    )?;
    let mutable_state = required_entry(&found, KEY_MUTABLE_STATE)?;

    for (key, value) in [
        (KEY_EDITOR_SIBLING, editor_sibling.as_str()),
        (KEY_FORGE_SIBLING, forge_sibling.as_str()),
    ] {
        // Case-sensitive on purpose: the contract declares the literal
        // lowercase ".exe" suffix, and "ARTISAN-EDITOR.EXE" must fail it.
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        if !value.ends_with(".exe") {
            return Err(ContractError::Value(ValueError::SiblingNeedsExeSuffix {
                key,
                found: value.to_owned(),
            }));
        }
    }
    let named = [
        (KEY_PACKAGE_ROOT, package_root.as_str()),
        (KEY_EDITOR_SIBLING, editor_sibling.as_str()),
        (KEY_FORGE_SIBLING, forge_sibling.as_str()),
        (KEY_RESOURCES_DIR, resources_dir.as_str()),
        (KEY_LICENSES_DIR, licenses_dir.as_str()),
    ];
    for (index, (first_key, first)) in named.iter().enumerate() {
        for (second_key, second) in &named[index + 1..] {
            if first.eq_ignore_ascii_case(second) {
                return Err(ContractError::Value(ValueError::CaseInsensitiveCollision {
                    first: first_key,
                    second: second_key,
                }));
            }
        }
    }

    let source_tree_fallback = required_entry(&found, KEY_SOURCE_TREE_FALLBACK)?;
    let runfiles_fallback = required_entry(&found, KEY_RUNFILES_FALLBACK)?;

    Ok(PortableLayout {
        schema_version: parse_schema_version(&schema.value)?,
        proof_scope: ProofScope::parse(&scope.value)?,
        package_root,
        editor_sibling,
        forge_sibling,
        optional_dirs: OptionalDirs {
            resources: resources_dir,
            licenses: licenses_dir,
        },
        mutable_state: MutableStateLocation::parse(&mutable_state.value)?,
        source_tree_fallback: ProductionFallback::parse(
            KEY_SOURCE_TREE_FALLBACK,
            &source_tree_fallback.value,
        )?,
        runfiles_fallback: ProductionFallback::parse(
            KEY_RUNFILES_FALLBACK,
            &runfiles_fallback.value,
        )?,
    })
}

/// Looks up exactly one previously parsed entry for `key`.
fn required_entry<'a>(
    found: &'a [FoundEntry],
    key: &'static str,
) -> Result<&'a FoundEntry, ContractError> {
    found
        .iter()
        .find(|entry| entry.key == key)
        .ok_or(ContractError::Syntax(SyntaxError::MissingKey { key }))
}

/// Parses the schema version, accepting only the literal defined version.
fn parse_schema_version(raw: &str) -> Result<u32, ValueError> {
    if raw == "1" {
        Ok(1)
    } else {
        Err(ValueError::SchemaVersion {
            found: raw.to_owned(),
        })
    }
}

/// Parses one declared name, attributing any rejection to its key.
fn parse_name_for(key: &'static str, raw: &str) -> Result<SafeRelativeName, ContractError> {
    SafeRelativeName::parse(raw).map_err(|rejection| {
        ContractError::Value(ValueError::Name {
            key,
            found: raw.to_owned(),
            rejection,
        })
    })
}

// ------------------------------------------------- test-only runfiles lookup

/// Repository-relative runfile keys of the declared Bazel binaries. These mirror
/// the `data = [...]` wiring in `tests/packaging/BUILD.bazel`; if the labels or
/// produced artifact names drift, resolution fails and this test fails closed.
const EDITOR_RUNFILE_KEY: &str = "modules/frontend/editor.exe";
const FORGE_RUNFILE_KEY: &str = "modules/backend/forge.exe";

/// Produced artifact basenames expected behind the runfile keys.
const EDITOR_ARTIFACT_BASENAME: &str = "editor.exe";
const FORGE_ARTIFACT_BASENAME: &str = "forge.exe";

/// Workspace prefixes tried in priority order for unprefixed repository keys.
const FALLBACK_WORKSPACE_PREFIXES: [&str; 2] = ["artisan_editor", "_main"];

/// Why test-only runfile resolution failed.
#[derive(Debug)]
enum RunfilesError {
    Unavailable { detail: String },
    MissingEntry { key: String },
}

impl std::fmt::Display for RunfilesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable { detail } => write!(
                formatter,
                "no usable runfiles layout (manifest or directory); \
                 there is deliberately no source-tree fallback: {detail}"
            ),
            Self::MissingEntry { key } => {
                write!(formatter, "runfiles lack the declared entry {key:?}")
            }
        }
    }
}

impl Error for RunfilesError {}

/// Test-only runfiles resolution, restricted to the two layouts Bazel provides.
///
/// There is deliberately no source-tree or ambient-checkout fallback here: the
/// contract keeps both production fallback flags literally false, and this
/// resolver exists only so the Bazel test can inspect its declared `data`.
enum Runfiles {
    /// Windows-style `RUNFILES_MANIFEST_FILE` mappings, runfile key to real path.
    Manifest {
        entries: std::collections::BTreeMap<String, PathBuf>,
    },
    /// Directory-backed runfiles rooted at `root/<prefix>/...`.
    Directory { root: PathBuf, prefix: String },
}

impl Runfiles {
    /// Detects the layout from the process environment, accepting a layout only
    /// when it actually serves both declared binary keys.
    fn detect() -> Result<Self, RunfilesError> {
        let prefixes = workspace_prefixes();
        let mut attempted = Vec::new();

        if let Ok(manifest_path) = std::env::var("RUNFILES_MANIFEST_FILE")
            && let Ok(text) = fs::read_to_string(&manifest_path)
        {
            let entries = parse_runfiles_manifest(&text);
            if serves_manifest_probes(&entries, &prefixes) {
                return Ok(Self::Manifest { entries });
            }
            attempted.push(format!(
                "manifest {manifest_path} lacked the declared probes"
            ));
        } else {
            attempted.push("RUNFILES_MANIFEST_FILE unset or unreadable".to_owned());
        }

        if let Ok(dir) = std::env::var("RUNFILES_DIR") {
            let root = PathBuf::from(dir);
            if let Some(prefix) = tree_prefix(&root, &prefixes) {
                return Ok(Self::Directory { root, prefix });
            }
            attempted.push(format!(
                "directory {} lacked the declared probes",
                root.display()
            ));
        } else {
            attempted.push("RUNFILES_DIR unset".to_owned());
        }

        if let Ok(executable) = std::env::current_exe()
            && let Some(parent) = executable.parent()
            && let Some(stem) = executable.file_stem().and_then(std::ffi::OsStr::to_str)
        {
            let root = parent.join(format!("{stem}.runfiles"));
            if let Some(prefix) = tree_prefix(&root, &prefixes) {
                return Ok(Self::Directory { root, prefix });
            }
            attempted.push(format!(
                "sibling tree {} lacked the declared probes",
                root.display()
            ));
        }

        Err(RunfilesError::Unavailable {
            detail: attempted.join("; "),
        })
    }

    /// Resolves one declared repository-relative runfile key.
    fn resolve(&self, key: &str) -> Result<PathBuf, RunfilesError> {
        match self {
            Self::Manifest { entries } => {
                for prefix in workspace_prefixes() {
                    if let Some(path) = entries.get(&format!("{prefix}/{key}")) {
                        return Ok(path.clone());
                    }
                }
                entries
                    .get(key)
                    .cloned()
                    .ok_or_else(|| RunfilesError::MissingEntry {
                        key: key.to_owned(),
                    })
            }
            Self::Directory { root, prefix } => Ok(root.join(prefix).join(key)),
        }
    }
}

/// Workspace prefixes accepted in runfile keys: nonempty `TEST_WORKSPACE`,
/// then the module name, then Bazel's canonical `_main`.
fn workspace_prefixes() -> Vec<String> {
    let mut prefixes = Vec::new();
    if let Ok(workspace) = std::env::var("TEST_WORKSPACE")
        && !workspace.is_empty()
    {
        prefixes.push(workspace);
    }
    for fallback in FALLBACK_WORKSPACE_PREFIXES {
        if !prefixes.contains(&fallback.to_owned()) {
            prefixes.push(fallback.to_owned());
        }
    }
    prefixes
}

/// Parses `key path` manifest lines; paths may themselves contain spaces.
fn parse_runfiles_manifest(text: &str) -> std::collections::BTreeMap<String, PathBuf> {
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(key, path)| (key.to_owned(), PathBuf::from(path)))
        .collect()
}

/// True when the manifest maps both declared binary keys under some accepted
/// workspace prefix (or bare).
fn serves_manifest_probes(
    entries: &std::collections::BTreeMap<String, PathBuf>,
    prefixes: &[String],
) -> bool {
    [EDITOR_RUNFILE_KEY, FORGE_RUNFILE_KEY].iter().all(|key| {
        prefixes
            .iter()
            .any(|prefix| entries.contains_key(&format!("{prefix}/{key}")))
            || entries.contains_key(*key)
    })
}

/// Returns the first workspace prefix under which the directory-backed tree
/// actually serves both declared binary keys as files.
fn tree_prefix(root: &Path, prefixes: &[String]) -> Option<String> {
    prefixes.iter().find_map(|prefix| {
        let serves = [EDITOR_RUNFILE_KEY, FORGE_RUNFILE_KEY].iter().all(|key| {
            let candidate = root.join(prefix).join(key);
            candidate.is_file()
        });
        serves.then(|| prefix.clone())
    })
}

// ------------------------------------------------------------- PE proving

/// Failure to prove a path is a regular non-symlink PE image.
#[derive(Debug)]
enum BinaryProofError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    SymbolicLink {
        path: PathBuf,
    },
    NotRegularFile {
        path: PathBuf,
    },
    TooSmall {
        path: PathBuf,
        length: u64,
    },
    NotPeImage {
        path: PathBuf,
        magic: [u8; 2],
    },
}

impl std::fmt::Display for BinaryProofError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "cannot read {}: {source}", path.display())
            }
            Self::SymbolicLink { path } => {
                write!(
                    formatter,
                    "{} is a symbolic link, not a plain artifact",
                    path.display()
                )
            }
            Self::NotRegularFile { path } => {
                write!(formatter, "{} is not a regular file", path.display())
            }
            Self::TooSmall { path, length } => {
                write!(
                    formatter,
                    "{} is {length} bytes; smaller than a PE header",
                    path.display()
                )
            }
            Self::NotPeImage { path, magic } => {
                write!(
                    formatter,
                    "{} lacks the MZ PE magic (found {magic:02X?})",
                    path.display()
                )
            }
        }
    }
}

impl Error for BinaryProofError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Proves `path` is a regular, non-symlink, non-empty PE image and returns its
/// size in bytes.
fn prove_regular_pe_image(path: &Path) -> Result<u64, BinaryProofError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| BinaryProofError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(BinaryProofError::SymbolicLink {
            path: path.to_path_buf(),
        });
    }
    if !file_type.is_file() {
        return Err(BinaryProofError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    let length = metadata.len();
    if length < 2 {
        return Err(BinaryProofError::TooSmall {
            path: path.to_path_buf(),
            length,
        });
    }
    let mut magic = [0_u8; 2];
    let mut file = fs::File::open(path).map_err(|source| BinaryProofError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    std::io::Read::read_exact(&mut file, &mut magic).map_err(|source| BinaryProofError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if magic != *b"MZ" {
        return Err(BinaryProofError::NotPeImage {
            path: path.to_path_buf(),
            magic,
        });
    }
    Ok(length)
}

// --------------------------------------------------- staging and relocation

/// A uniquely named temporary area this test owns and cleans exclusively.
struct ProofArea {
    root: PathBuf,
}

impl ProofArea {
    /// Creates `<temp>/artisan-portable-proof-<label>-<pid>-<nonce>`.
    fn create(label: &str) -> Result<Self, Box<dyn Error>> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!(
            "artisan-portable-proof-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for ProofArea {
    fn drop(&mut self) {
        // Best-effort cleanup of exactly this area; nothing outside it is ever
        // touched, and the cleanup-scope test asserts the removal happened.
        let _cleanup_result = fs::remove_dir_all(&self.root);
    }
}

/// Copies both resolved Bazel binaries into a fresh `package_root` directory
/// under their declared sibling names.
fn stage_declared_leaves(
    parent: &Path,
    layout: &PortableLayout,
    editor_source: &Path,
    forge_source: &Path,
) -> std::io::Result<PathBuf> {
    let package_root = parent.join(layout.package_root.as_str());
    fs::create_dir_all(&package_root)?;
    fs::copy(
        editor_source,
        package_root.join(layout.editor_sibling.as_str()),
    )?;
    fs::copy(
        forge_source,
        package_root.join(layout.forge_sibling.as_str()),
    )?;
    Ok(package_root)
}

/// Package-root-relative file set a conforming layout-only package contains.
fn declared_leaf_set(layout: &PortableLayout) -> Vec<String> {
    let mut expected = vec![
        layout.editor_sibling.as_str().to_owned(),
        layout.forge_sibling.as_str().to_owned(),
    ];
    expected.sort();
    expected
}

/// Collects every file below `directory` as a `/`-separated relative path.
fn collect_relative_files(
    directory: &Path,
    prefix: &str,
    out: &mut Vec<String>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = format!("{prefix}{name}");
        if entry.file_type()?.is_dir() {
            collect_relative_files(&entry.path(), &format!("{relative}/"), out)?;
        } else {
            out.push(relative);
        }
    }
    Ok(())
}

/// Sorted relative file set below `directory`.
fn sorted_relative_files(directory: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut files = Vec::new();
    collect_relative_files(directory, "", &mut files)?;
    files.sort();
    Ok(files)
}

/// Sorted one-level directory entry names directly below `directory`.
fn sorted_entry_names(directory: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut names = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

// ------------------------------------------------------------------ tests

#[test]
fn contract_parses_to_declared_layout_values() {
    let layout = parse_contract(METADATA_TEXT).expect("contract metadata must parse");
    assert_eq!(layout.schema_version, 1);
    assert_eq!(layout.proof_scope, ProofScope::LayoutOnly);
    assert_eq!(layout.package_root.as_str(), "artisan-editor");
    assert_eq!(layout.editor_sibling.as_str(), "artisan-editor.exe");
    assert_eq!(layout.forge_sibling.as_str(), "artisan-forge.exe");
    assert_eq!(layout.optional_dirs.resources.as_str(), "resources");
    assert_eq!(layout.optional_dirs.licenses.as_str(), "licenses");
    assert_eq!(
        layout.mutable_state,
        MutableStateLocation::OutsidePackageRoot
    );
    assert_eq!(
        layout.source_tree_fallback,
        ProductionFallback::Disabled,
        "the production source-tree fallback must stay false"
    );
    assert_eq!(
        layout.runfiles_fallback,
        ProductionFallback::Disabled,
        "the production runfiles fallback must stay false"
    );
}

/// Normalized rejection identity asserted against real parse results.
///
/// Tags deliberately carry the discriminating payload (which key, which name
/// rejection) while ignoring line numbers, which depend on contract layout.
#[derive(Debug, Eq, PartialEq)]
enum Rejection {
    Syntax(SyntaxTag),
    Value(ValueTag),
}

/// Line-level rejection identities.
#[derive(Debug, Eq, PartialEq)]
enum SyntaxTag {
    CarriageReturn,
    MalformedLine,
    DuplicateKey,
    UnknownKey,
    MissingKey,
}

/// Value-level rejection identities.
#[derive(Debug, Eq, PartialEq)]
enum ValueTag {
    SchemaVersion,
    ProofScope,
    MutableState,
    FallbackFlag,
    /// A declared name was unsafe in exactly this keyed way.
    Name(&'static str, NameRejection),
    SiblingSuffix,
    CaseCollision,
}

fn syntax_tag(error: &SyntaxError) -> SyntaxTag {
    match error {
        SyntaxError::CarriageReturn => SyntaxTag::CarriageReturn,
        SyntaxError::MalformedLine { .. } => SyntaxTag::MalformedLine,
        SyntaxError::DuplicateKey { .. } => SyntaxTag::DuplicateKey,
        SyntaxError::UnknownKey { .. } => SyntaxTag::UnknownKey,
        SyntaxError::MissingKey { .. } => SyntaxTag::MissingKey,
    }
}

fn value_tag(error: &ValueError) -> ValueTag {
    match error {
        ValueError::SchemaVersion { .. } => ValueTag::SchemaVersion,
        ValueError::ProofScope { .. } => ValueTag::ProofScope,
        ValueError::MutableState { .. } => ValueTag::MutableState,
        ValueError::FallbackMustBeFalse { .. } => ValueTag::FallbackFlag,
        ValueError::Name { key, rejection, .. } => ValueTag::Name(key, *rejection),
        ValueError::SiblingNeedsExeSuffix { .. } => ValueTag::SiblingSuffix,
        ValueError::CaseInsensitiveCollision { .. } => ValueTag::CaseCollision,
    }
}

/// Replaces the value of `key` with `replacement`, preserving other lines.
fn replace_value(template: &str, key: &str, replacement: &str) -> String {
    template
        .lines()
        .map(|line| {
            let trimmed = line.trim_ascii_start();
            if trimmed.starts_with(key) && trimmed.contains('=') {
                format!("{key} = {replacement}")
            } else {
                line.to_owned()
            }
        })
        .chain(std::iter::once(String::new()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Removes the `key` line entirely.
fn remove_key(template: &str, key: &str) -> String {
    template
        .lines()
        .filter(|line| {
            let trimmed = line.trim_ascii_start();
            !(trimmed.starts_with(key) && trimmed.contains('='))
        })
        .map(str::to_owned)
        .chain(std::iter::once(String::new()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Appends one extra line to the contract.
fn append_line(template: &str, line: &str) -> String {
    format!("{template}{line}\n")
}

/// Builds every drifted-contract variant with its expected rejection identity.
///
/// Split across helpers purely for reviewability; the test asserts each
/// drifted text produces exactly the rejection identity listed beside it.
fn drift_cases() -> Vec<(&'static str, String, Rejection)> {
    syntax_drift_cases()
        .into_iter()
        .chain(value_name_drift_cases())
        .chain(value_policy_drift_cases())
        .collect()
}

/// Structural drift: unknown, duplicate, missing, malformed, and CR lines.
fn syntax_drift_cases() -> Vec<(&'static str, String, Rejection)> {
    vec![
        (
            "unknown key",
            append_line(METADATA_TEXT, "future_packaging_key = anything"),
            Rejection::Syntax(SyntaxTag::UnknownKey),
        ),
        (
            "duplicate sibling key",
            append_line(METADATA_TEXT, "editor_sibling = other.exe"),
            Rejection::Syntax(SyntaxTag::DuplicateKey),
        ),
        (
            "removed required key",
            remove_key(METADATA_TEXT, KEY_LICENSES_DIR),
            Rejection::Syntax(SyntaxTag::MissingKey),
        ),
        (
            "line without a pair",
            append_line(METADATA_TEXT, "this line has no assignment"),
            Rejection::Syntax(SyntaxTag::MalformedLine),
        ),
        (
            "carriage return carrier",
            METADATA_TEXT.replace('\n', "\r\n"),
            Rejection::Syntax(SyntaxTag::CarriageReturn),
        ),
    ]
}

/// Declared names that violate the safe Windows name grammar.
fn value_name_drift_cases() -> Vec<(&'static str, String, Rejection)> {
    vec![
        (
            "absolute root",
            replace_value(METADATA_TEXT, KEY_PACKAGE_ROOT, "/etc"),
            Rejection::Value(ValueTag::Name(
                KEY_PACKAGE_ROOT,
                NameRejection::DisallowedByte { byte: b'/' },
            )),
        ),
        (
            "drive-letter form",
            replace_value(METADATA_TEXT, KEY_PACKAGE_ROOT, r"C:\Users"),
            Rejection::Value(ValueTag::Name(
                KEY_PACKAGE_ROOT,
                NameRejection::DisallowedByte { byte: b':' },
            )),
        ),
        (
            "UNC form",
            replace_value(METADATA_TEXT, KEY_EDITOR_SIBLING, r"\\server\share\x.exe"),
            Rejection::Value(ValueTag::Name(
                KEY_EDITOR_SIBLING,
                NameRejection::DisallowedByte { byte: b'\\' },
            )),
        ),
        (
            "dot segment",
            replace_value(METADATA_TEXT, KEY_RESOURCES_DIR, ".."),
            Rejection::Value(ValueTag::Name(KEY_RESOURCES_DIR, NameRejection::DotSegment)),
        ),
        (
            "trailing dot stripped by Windows",
            replace_value(METADATA_TEXT, KEY_EDITOR_SIBLING, "artisan-editor.exe."),
            Rejection::Value(ValueTag::Name(
                KEY_EDITOR_SIBLING,
                NameRejection::TrailingDot,
            )),
        ),
        (
            "reserved device",
            replace_value(METADATA_TEXT, KEY_FORGE_SIBLING, "con.exe"),
            Rejection::Value(ValueTag::Name(
                KEY_FORGE_SIBLING,
                NameRejection::ReservedDevice { device: "CON" },
            )),
        ),
    ]
}

/// Closed-vocabulary and cross-name policy drift outside the name grammar.
fn value_policy_drift_cases() -> Vec<(&'static str, String, Rejection)> {
    vec![
        (
            "schema drift",
            replace_value(METADATA_TEXT, KEY_SCHEMA_VERSION, "2"),
            Rejection::Value(ValueTag::SchemaVersion),
        ),
        (
            "scope drift",
            replace_value(METADATA_TEXT, KEY_PROOF_SCOPE, "full-product"),
            Rejection::Value(ValueTag::ProofScope),
        ),
        (
            "mutable state drift",
            replace_value(METADATA_TEXT, KEY_MUTABLE_STATE, "inside-package-root"),
            Rejection::Value(ValueTag::MutableState),
        ),
        (
            "source-tree fallback enabled",
            replace_value(METADATA_TEXT, KEY_SOURCE_TREE_FALLBACK, "true"),
            Rejection::Value(ValueTag::FallbackFlag),
        ),
        (
            "runfiles fallback uppercase TRUE",
            replace_value(METADATA_TEXT, KEY_RUNFILES_FALLBACK, "TRUE"),
            Rejection::Value(ValueTag::FallbackFlag),
        ),
        (
            "sibling without exe suffix",
            replace_value(METADATA_TEXT, KEY_FORGE_SIBLING, "artisan-forge"),
            Rejection::Value(ValueTag::SiblingSuffix),
        ),
        (
            "case-insensitive collision",
            replace_value(METADATA_TEXT, KEY_EDITOR_SIBLING, "ARTISAN-FORGE.exe"),
            Rejection::Value(ValueTag::CaseCollision),
        ),
    ]
}

#[test]
fn contract_fails_closed_on_drift() {
    for (label, drifted, expected) in drift_cases() {
        let rejection = match parse_contract(&drifted) {
            Ok(_) => panic!("{label}: drift must reject the contract"),
            Err(error) => {
                assert!(
                    !error.to_string().is_empty(),
                    "{label}: rejection needs a readable cause"
                );
                match error {
                    ContractError::Syntax(syntax) => Rejection::Syntax(syntax_tag(&syntax)),
                    ContractError::Value(value) => Rejection::Value(value_tag(&value)),
                }
            }
        };
        assert_eq!(rejection, expected, "{label}: wrong rejection identity");
    }
}

#[test]
fn safe_relative_name_accepts_canonical_names() {
    for accepted in [
        "artisan-editor",
        "artisan-editor.exe",
        "resources",
        "_private-dir.v2",
        "Aa09._-",
    ] {
        let parsed = SafeRelativeName::parse(accepted)
            .unwrap_or_else(|rejection| panic!("{accepted:?} must parse: {rejection}"));
        assert_eq!(parsed.as_str(), accepted);
    }
}

#[test]
fn safe_relative_name_rejects_unsafe_windows_forms() {
    let cases: &[(&str, &str, NameRejection)] = &[
        ("empty", "", NameRejection::Empty),
        (
            "absolute posix path",
            "/etc",
            NameRejection::DisallowedByte { byte: b'/' },
        ),
        (
            "separator",
            "a/b",
            NameRejection::DisallowedByte { byte: b'/' },
        ),
        (
            "backslash",
            "a\\b",
            NameRejection::DisallowedByte { byte: b'\\' },
        ),
        (
            "drive letter",
            "C:",
            NameRejection::DisallowedByte { byte: b':' },
        ),
        (
            "alternate data stream",
            "x:y",
            NameRejection::DisallowedByte { byte: b':' },
        ),
        (
            "UNC host separator",
            "\\\\srv",
            NameRejection::DisallowedByte { byte: b'\\' },
        ),
        ("dot segment", ".", NameRejection::DotSegment),
        ("dot-dot segment", "..", NameRejection::DotSegment),
        (
            "space byte",
            "a b",
            NameRejection::DisallowedByte { byte: b' ' },
        ),
        (
            "control byte",
            "a\u{1}b",
            NameRejection::DisallowedByte { byte: 0x01 },
        ),
        (
            "non-ascii byte",
            "café",
            NameRejection::DisallowedByte { byte: 0xC3 },
        ),
        ("trailing dot", "x.exe.", NameRejection::TrailingDot),
        (
            "reserved CON",
            "CON",
            NameRejection::ReservedDevice { device: "CON" },
        ),
        (
            "reserved com1 with suffix",
            "com1.txt",
            NameRejection::ReservedDevice { device: "COM1" },
        ),
        (
            "reserved lpt9 mixed case",
            "Lpt9",
            NameRejection::ReservedDevice { device: "LPT9" },
        ),
    ];
    for (label, input, expected) in cases {
        assert_eq!(
            SafeRelativeName::parse(input).err(),
            Some(*expected),
            "{label}: {input:?}"
        );
    }
    let long = "a".repeat(MAX_NAME_BYTES + 1);
    assert_eq!(
        SafeRelativeName::parse(&long).err(),
        Some(NameRejection::TooLong {
            length: MAX_NAME_BYTES + 1
        })
    );
}

#[test]
fn bazel_binaries_resolve_exactly_through_declared_runfiles() -> Result<(), Box<dyn Error>> {
    let layout = parse_contract(METADATA_TEXT)?;
    let runfiles = Runfiles::detect()?;

    let editor_source = runfiles.resolve(EDITOR_RUNFILE_KEY)?;
    let forge_source = runfiles.resolve(FORGE_RUNFILE_KEY)?;
    assert_ne!(editor_source, forge_source);
    assert_eq!(
        editor_source.file_name(),
        Some(std::ffi::OsStr::new(EDITOR_ARTIFACT_BASENAME)),
        "editor runfile must resolve to the Bazel editor artifact"
    );
    assert_eq!(
        forge_source.file_name(),
        Some(std::ffi::OsStr::new(FORGE_ARTIFACT_BASENAME)),
        "forge runfile must resolve to the Bazel forge artifact"
    );

    let editor_length = prove_regular_pe_image(&editor_source)?;
    let forge_length = prove_regular_pe_image(&forge_source)?;
    assert!(
        editor_length > 2 && forge_length > 2,
        "both artifacts carry content beyond the PE magic"
    );

    // The declared sibling names must differ from the Bazel artifact names:
    // staging renames them, which is exactly the packaged-layout behavior.
    assert_ne!(layout.editor_sibling.as_str(), EDITOR_ARTIFACT_BASENAME);
    assert_ne!(layout.forge_sibling.as_str(), FORGE_ARTIFACT_BASENAME);
    Ok(())
}

#[test]
fn staged_package_root_holds_exactly_the_declared_leaves() -> Result<(), Box<dyn Error>> {
    let layout = parse_contract(METADATA_TEXT)?;
    let runfiles = Runfiles::detect()?;
    let editor_source = runfiles.resolve(EDITOR_RUNFILE_KEY)?;
    let forge_source = runfiles.resolve(FORGE_RUNFILE_KEY)?;

    let area = ProofArea::create("staged-leaves")?;
    let initial_parent = area.path().join("initial-parent");
    let package_root =
        stage_declared_leaves(&initial_parent, &layout, &editor_source, &forge_source)?;

    let staged = sorted_relative_files(&package_root)?;
    assert_eq!(staged, declared_leaf_set(&layout));

    // Layout-only proof: neither optional directory ships yet.
    assert!(
        !package_root
            .join(layout.optional_dirs.resources.as_str())
            .exists()
    );
    assert!(
        !package_root
            .join(layout.optional_dirs.licenses.as_str())
            .exists()
    );

    // Staged copies stay regular PE images under their declared names.
    let editor_length = prove_regular_pe_image(&package_root.join(layout.editor_sibling.as_str()))?;
    let forge_length = prove_regular_pe_image(&package_root.join(layout.forge_sibling.as_str()))?;
    assert!(editor_length > 2 && forge_length > 2);

    // The owned area holds exactly the staging parent, nothing else.
    assert_eq!(sorted_entry_names(area.path())?, vec!["initial-parent"]);
    Ok(())
}

#[test]
fn relocated_editor_parent_derives_forge_sibling() -> Result<(), Box<dyn Error>> {
    let layout = parse_contract(METADATA_TEXT)?;
    let runfiles = Runfiles::detect()?;
    let editor_source = runfiles.resolve(EDITOR_RUNFILE_KEY)?;
    let forge_source = runfiles.resolve(FORGE_RUNFILE_KEY)?;

    let area = ProofArea::create("relocated-derivation")?;
    let initial_parent = area.path().join("parent-a");
    let relocated_parent = area.path().join("parent-b-differently-named");
    fs::create_dir_all(&relocated_parent)?;

    let staged_root =
        stage_declared_leaves(&initial_parent, &layout, &editor_source, &forge_source)?;
    assert_eq!(
        sorted_relative_files(&staged_root)?,
        declared_leaf_set(&layout)
    );

    // Relocate the whole package root into the differently named parent.
    let relocated_root = relocated_parent.join(layout.package_root.as_str());
    fs::rename(&staged_root, &relocated_root)?;
    assert!(!staged_root.exists(), "staged root must move, not copy");
    assert!(
        fs::read_dir(&initial_parent)?.next().is_none(),
        "initial parent must be empty after relocation"
    );
    assert_eq!(
        sorted_relative_files(&relocated_root)?,
        declared_leaf_set(&layout),
        "relocated package keeps exactly the declared leaves"
    );

    // Derive Forge purely from the relocated editor's parent directory.
    let relocated_editor = relocated_root.join(layout.editor_sibling.as_str());
    let derived_forge = relocated_editor
        .parent()
        .expect("editor path always has a parent")
        .join(layout.forge_sibling.as_str());
    assert_eq!(
        derived_forge,
        relocated_root.join(layout.forge_sibling.as_str())
    );
    let forge_length = prove_regular_pe_image(&derived_forge)?;
    assert!(forge_length > 2);
    Ok(())
}

#[test]
fn cleanup_removes_only_the_owned_temporary_area() -> Result<(), Box<dyn Error>> {
    let area = ProofArea::create("cleanup-scope")?;
    let area_path = area.path().to_path_buf();
    assert!(area_path.is_dir());

    // A sentinel sibling outside the owned area must survive cleanup.
    let sentinel = area_path.with_file_name(format!(
        "{}-sentinel",
        area_path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("unique area name")
    ));
    fs::create_dir_all(&sentinel)?;
    fs::write(sentinel.join("marker.txt"), b"keep")?;

    drop(area);
    assert!(!area_path.exists(), "owned area must be removed on drop");
    assert!(
        sentinel.is_dir(),
        "cleanup must not touch neighboring paths"
    );
    assert_eq!(fs::read(sentinel.join("marker.txt"))?, b"keep");

    // Remove our own sentinel scratch, leaving the temp dir as we found it.
    fs::remove_dir_all(&sentinel)?;
    Ok(())
}
