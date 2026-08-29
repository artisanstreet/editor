//! Pure proof of the v2 versioned-release installation layout.
//!
//! This file is deliberately directly compilable with `rustc --test`.  It
//! parses the closed contract, validates caller-supplied version components,
//! materializes canonical member names for Windows and non-Windows targets,
//! and checks an already-materialized member set.  It never builds, stages,
//! archives, installs, signs, launches, or registers a binary.

use std::error::Error;
use std::fmt;

const CONTRACT_TEXT: &str = include_str!("../../packaging/portable/versioned_layout.txt");

const COMPANY_ROOT: &str = "Artisan Street";
const VERSION_CHECK: &str = "v1";
const MAX_NAME_BYTES: usize = 255;

const KEY_SCHEMA_VERSION: &str = "schema_version";
const KEY_PROOF_SCOPE: &str = "proof_scope";
const KEY_PACKAGE_ROOT: &str = "package_root";
const KEY_BIN_DIR: &str = "bin_dir";
const KEY_VERSIONS_DIR: &str = "versions_dir";
const KEY_RESOURCES_DIR: &str = "resources_dir";
const KEY_LICENSES_DIR: &str = "licenses_dir";
const KEY_INSTALLATION_FILE: &str = "installation_file";
const KEY_PAYLOAD_MANIFEST_FILE: &str = "payload_manifest_file";
const KEY_AE_EXECUTABLE: &str = "ae_executable";
const KEY_INSTALLER_EXECUTABLE: &str = "installer_executable";
const KEY_EDITOR_EXECUTABLE: &str = "editor_executable";
const KEY_FORGE_EXECUTABLE: &str = "forge_executable";
const KEY_STABLE_LAUNCHER_ROLE: &str = "stable_launcher_role";
const KEY_BOOTSTRAP_ROLE: &str = "bootstrap_role";
const KEY_BROKER_ROLE: &str = "broker_role";
const KEY_MUTABLE_STATE: &str = "mutable_state";
const KEY_SOURCE_TREE_FALLBACK: &str = "source_tree_fallback";
const KEY_RUNFILES_FALLBACK: &str = "runfiles_fallback";

/// The closed schema, in the order used by the contract.
const CONTRACT_KEYS: [&str; 19] = [
    KEY_SCHEMA_VERSION,
    KEY_PROOF_SCOPE,
    KEY_PACKAGE_ROOT,
    KEY_BIN_DIR,
    KEY_VERSIONS_DIR,
    KEY_RESOURCES_DIR,
    KEY_LICENSES_DIR,
    KEY_INSTALLATION_FILE,
    KEY_PAYLOAD_MANIFEST_FILE,
    KEY_AE_EXECUTABLE,
    KEY_INSTALLER_EXECUTABLE,
    KEY_EDITOR_EXECUTABLE,
    KEY_FORGE_EXECUTABLE,
    KEY_STABLE_LAUNCHER_ROLE,
    KEY_BOOTSTRAP_ROLE,
    KEY_BROKER_ROLE,
    KEY_MUTABLE_STATE,
    KEY_SOURCE_TREE_FALLBACK,
    KEY_RUNFILES_FALLBACK,
];

// --------------------------------------------------------------- typed values

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProofScope {
    VersionedReleaseLayout,
}

impl ProofScope {
    const LITERAL: &str = "versioned-release-layout";

    fn parse(raw: &str) -> Result<Self, ValueError> {
        if raw == Self::LITERAL {
            Ok(Self::VersionedReleaseLayout)
        } else {
            Err(ValueError::WrongLiteral {
                key: KEY_PROOF_SCOPE,
                expected: Self::LITERAL,
                found: raw.to_owned(),
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutableStateLocation {
    OutsideImmutableRoots,
}

impl MutableStateLocation {
    const LITERAL: &str = "outside-immutable-roots";

    fn parse(raw: &str) -> Result<Self, ValueError> {
        if raw == Self::LITERAL {
            Ok(Self::OutsideImmutableRoots)
        } else {
            Err(ValueError::WrongLiteral {
                key: KEY_MUTABLE_STATE,
                expected: Self::LITERAL,
                found: raw.to_owned(),
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProductionFallback {
    Disabled,
}

impl ProductionFallback {
    const LITERAL: &str = "false";

    fn parse(key: &'static str, raw: &str) -> Result<Self, ValueError> {
        if raw == Self::LITERAL {
            Ok(Self::Disabled)
        } else {
            Err(ValueError::WrongLiteral {
                key,
                expected: Self::LITERAL,
                found: raw.to_owned(),
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExecutableRole {
    Ae,
    Installer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BrokerPolicy {
    Forbidden,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SafeLeaf(String);

/// Why a path component is not safe under the existing strict ASCII policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NameRejection {
    Empty,
    TooLong { length: usize },
    DisallowedByte { byte: u8 },
    DotSegment,
    TrailingDot,
    TrailingSpace,
    ReservedDevice { device: &'static str },
}

impl SafeLeaf {
    /// The existing portable grammar: one non-empty ASCII component made of
    /// alphanumerics, `.`, `_`, and `-`, with Windows hazards rejected.
    fn parse(raw: &str) -> Result<Self, NameRejection> {
        if raw.is_empty() {
            return Err(NameRejection::Empty);
        }
        if raw.len() > MAX_NAME_BYTES {
            return Err(NameRejection::TooLong { length: raw.len() });
        }
        if raw == "." || raw == ".." {
            return Err(NameRejection::DotSegment);
        }
        if raw.ends_with('.') {
            return Err(NameRejection::TrailingDot);
        }
        if raw.ends_with(' ') {
            return Err(NameRejection::TrailingSpace);
        }
        for &byte in raw.as_bytes() {
            if !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')) {
                return Err(NameRejection::DisallowedByte { byte });
            }
        }
        if let Some(device) = reserved_device_stem(raw) {
            return Err(NameRejection::ReservedDevice { device });
        }
        Ok(Self(raw.to_owned()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

fn reserved_device_stem(raw: &str) -> Option<&'static str> {
    const DEVICES: [&str; 24] = [
        "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
        "LPT9",
    ];
    let stem = raw.split('.').next().unwrap_or_default();
    DEVICES
        .iter()
        .find(|device| device.eq_ignore_ascii_case(stem))
        .copied()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VersionId(SafeLeaf);

#[derive(Clone, Debug, Eq, PartialEq)]
enum VersionRejection {
    Placeholder,
    UnsafeName(NameRejection),
}

impl VersionId {
    fn parse(raw: &str) -> Result<Self, VersionRejection> {
        if raw == "<version>" {
            return Err(VersionRejection::Placeholder);
        }
        SafeLeaf::parse(raw)
            .map(Self)
            .map_err(VersionRejection::UnsafeName)
    }

    fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

// -------------------------------------------------------------- path values

#[derive(Clone, Debug, Eq, PartialEq)]
struct CompanyRoot(String);

#[derive(Clone, Debug, Eq, PartialEq)]
struct CanonicalPath(String);

#[derive(Clone, Debug, Eq, PartialEq)]
enum PathRejection {
    Empty,
    Absolute,
    ForbiddenByte {
        byte: u8,
    },
    EmptyComponent,
    WrongCompanyRoot {
        found: String,
    },
    UnsafeComponent {
        component: String,
        rejection: NameRejection,
    },
    ForbiddenBroker,
}

impl CompanyRoot {
    fn parse(raw: &str) -> Result<Self, String> {
        if raw == COMPANY_ROOT {
            Ok(Self(raw.to_owned()))
        } else {
            Err(raw.to_owned())
        }
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl CanonicalPath {
    /// Parses the slash-separated, archive-relative representation used by
    /// this helper.  A slash is structural only here; backslash and colon are
    /// always forbidden, including for drives, UNC paths, and ADS forms.
    fn parse(raw: &str) -> Result<Self, PathRejection> {
        if raw.is_empty() {
            return Err(PathRejection::Empty);
        }
        if raw.starts_with('/') {
            return Err(PathRejection::Absolute);
        }
        if raw.contains('\\') {
            return Err(PathRejection::ForbiddenByte { byte: b'\\' });
        }
        if raw.contains(':') {
            return Err(PathRejection::ForbiddenByte { byte: b':' });
        }

        let components: Vec<&str> = raw.split('/').collect();
        if components.iter().any(|component| component.is_empty()) {
            return Err(PathRejection::EmptyComponent);
        }
        if components.first().copied() != Some(COMPANY_ROOT) {
            return Err(PathRejection::WrongCompanyRoot {
                found: components.first().copied().unwrap_or_default().to_owned(),
            });
        }
        for component in components.iter().skip(1) {
            let safe =
                SafeLeaf::parse(component).map_err(|rejection| PathRejection::UnsafeComponent {
                    component: (*component).to_owned(),
                    rejection,
                })?;
            if safe
                .as_str()
                .split('.')
                .next()
                .is_some_and(|stem| stem.eq_ignore_ascii_case("broker"))
            {
                return Err(PathRejection::ForbiddenBroker);
            }
        }
        Ok(Self(raw.to_owned()))
    }

    fn from_components(components: &[&str]) -> Result<Self, PathRejection> {
        Self::parse(&components.join("/"))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetPlatform {
    Windows,
    NonWindows,
}

impl TargetPlatform {
    fn executable_suffix(self) -> &'static str {
        match self {
            Self::Windows => ".exe",
            Self::NonWindows => "",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MemberKind {
    File,
    Directory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LayoutMember {
    path: CanonicalPath,
    kind: MemberKind,
}

impl LayoutMember {
    fn new(path: &str, kind: MemberKind) -> Result<Self, PathRejection> {
        Ok(Self {
            path: CanonicalPath::parse(path)?,
            kind,
        })
    }

    fn path(&self) -> &str {
        self.path.as_str()
    }

    fn kind(&self) -> MemberKind {
        self.kind
    }
}

// ------------------------------------------------------------- contract model

#[derive(Clone, Debug, Eq, PartialEq)]
struct VersionedLayout {
    schema_version: u8,
    proof_scope: ProofScope,
    package_root: CompanyRoot,
    bin_dir: SafeLeaf,
    versions_dir: SafeLeaf,
    resources_dir: SafeLeaf,
    licenses_dir: SafeLeaf,
    installation_file: SafeLeaf,
    payload_manifest_file: SafeLeaf,
    ae_executable: SafeLeaf,
    installer_executable: SafeLeaf,
    editor_executable: SafeLeaf,
    forge_executable: SafeLeaf,
    stable_launcher_role: ExecutableRole,
    bootstrap_role: ExecutableRole,
    broker_role: BrokerPolicy,
    mutable_state: MutableStateLocation,
    source_tree_fallback: ProductionFallback,
    runfiles_fallback: ProductionFallback,
}

impl VersionedLayout {
    /// Enumerates every required directory and file in canonical member order.
    fn required_members(
        &self,
        target: TargetPlatform,
        raw_version: &str,
    ) -> Result<Vec<LayoutMember>, LayoutError> {
        let version = VersionId::parse(raw_version).map_err(LayoutError::Version)?;
        let root = self.package_root.as_str();
        let bin = self.bin_dir.as_str();
        let versions = self.versions_dir.as_str();
        let version_name = version.as_str();

        let ae = materialized_executable(&self.ae_executable, target);
        let installer = materialized_executable(&self.installer_executable, target);
        let editor = materialized_executable(&self.editor_executable, target);
        let forge = materialized_executable(&self.forge_executable, target);

        let mut members = Vec::with_capacity(14);
        push_member(&mut members, MemberKind::Directory, &[root])?;
        push_member(&mut members, MemberKind::Directory, &[root, bin])?;
        push_member(&mut members, MemberKind::File, &[root, bin, &ae])?;
        push_member(
            &mut members,
            MemberKind::File,
            &[root, self.installation_file.as_str()],
        )?;
        push_member(&mut members, MemberKind::Directory, &[root, versions])?;
        push_member(
            &mut members,
            MemberKind::Directory,
            &[root, versions, version_name],
        )?;
        push_member(
            &mut members,
            MemberKind::Directory,
            &[root, versions, version_name, bin],
        )?;
        push_member(
            &mut members,
            MemberKind::File,
            &[root, versions, version_name, bin, &ae],
        )?;
        push_member(
            &mut members,
            MemberKind::File,
            &[root, versions, version_name, bin, &installer],
        )?;
        push_member(
            &mut members,
            MemberKind::File,
            &[root, versions, version_name, bin, &editor],
        )?;
        push_member(
            &mut members,
            MemberKind::File,
            &[root, versions, version_name, bin, &forge],
        )?;
        push_member(
            &mut members,
            MemberKind::Directory,
            &[root, versions, version_name, self.resources_dir.as_str()],
        )?;
        push_member(
            &mut members,
            MemberKind::Directory,
            &[root, versions, version_name, self.licenses_dir.as_str()],
        )?;
        push_member(
            &mut members,
            MemberKind::File,
            &[
                root,
                versions,
                version_name,
                self.payload_manifest_file.as_str(),
            ],
        )?;
        ensure_unique_materialized_paths(&members)?;
        Ok(members)
    }

    /// Checks that a caller-provided member set is exactly the declaration:
    /// no duplicates, case-fold collisions, missing members, wrong kinds, or
    /// undeclared names are accepted.
    fn validate_members(
        &self,
        target: TargetPlatform,
        raw_version: &str,
        actual: &[LayoutMember],
    ) -> Result<(), LayoutError> {
        let required = self.required_members(target, raw_version)?;
        ensure_unique_materialized_paths(actual)?;

        for member in actual {
            if let Some(expected) = required
                .iter()
                .find(|expected| expected.path == member.path)
            {
                if expected.kind != member.kind {
                    return Err(LayoutError::WrongMemberKind {
                        path: member.path().to_owned(),
                        expected: expected.kind,
                        found: member.kind,
                    });
                }
                continue;
            }
            if let Some(expected) = required
                .iter()
                .find(|expected| expected.path().eq_ignore_ascii_case(member.path()))
            {
                return Err(LayoutError::CaseFoldCollision {
                    expected: expected.path().to_owned(),
                    found: member.path().to_owned(),
                });
            }
            return Err(LayoutError::UndeclaredMember {
                path: member.path().to_owned(),
            });
        }

        for expected in &required {
            if !actual.iter().any(|member| member == expected) {
                return Err(LayoutError::MissingMember {
                    path: expected.path().to_owned(),
                });
            }
        }
        Ok(())
    }

    fn validate_invariants(&self) -> Result<(), ContractError> {
        if self.stable_launcher_role != ExecutableRole::Ae
            || self.bootstrap_role != ExecutableRole::Installer
            || self.broker_role != BrokerPolicy::Forbidden
        {
            return Err(ContractError::Value(ValueError::Invariant(
                "role policy is not the closed v2 policy",
            )));
        }
        if self.stable_launcher_leaf().as_str() != self.ae_executable.as_str()
            || self.bootstrap_leaf().as_str() != self.installer_executable.as_str()
        {
            return Err(ContractError::Value(ValueError::Invariant(
                "role does not name its declared executable leaf",
            )));
        }
        let executable_leaves = [
            self.ae_executable.as_str(),
            self.installer_executable.as_str(),
            self.editor_executable.as_str(),
            self.forge_executable.as_str(),
        ];
        for (index, first) in executable_leaves.iter().enumerate() {
            if executable_leaves[index + 1..]
                .iter()
                .any(|second| first.eq_ignore_ascii_case(second))
            {
                return Err(ContractError::Value(ValueError::Invariant(
                    "executable leaves collide case-insensitively",
                )));
            }
        }
        for target in [TargetPlatform::Windows, TargetPlatform::NonWindows] {
            let members = self
                .required_members(target, VERSION_CHECK)
                .map_err(ContractError::Layout)?;
            if members
                .iter()
                .any(|member| member.path().to_ascii_lowercase().contains("broker"))
            {
                return Err(ContractError::Value(ValueError::Invariant(
                    "materialized members contain the forbidden Broker role",
                )));
            }
        }
        Ok(())
    }

    fn stable_launcher_leaf(&self) -> &SafeLeaf {
        &self.ae_executable
    }

    fn bootstrap_leaf(&self) -> &SafeLeaf {
        &self.installer_executable
    }
}

fn materialized_executable(leaf: &SafeLeaf, target: TargetPlatform) -> String {
    format!("{}{}", leaf.as_str(), target.executable_suffix())
}

fn push_member(
    members: &mut Vec<LayoutMember>,
    kind: MemberKind,
    components: &[&str],
) -> Result<(), LayoutError> {
    let path = CanonicalPath::from_components(components).map_err(LayoutError::Path)?;
    members.push(LayoutMember { path, kind });
    Ok(())
}

fn ensure_unique_materialized_paths(members: &[LayoutMember]) -> Result<(), LayoutError> {
    for (index, first) in members.iter().enumerate() {
        for second in &members[index + 1..] {
            if first.path().eq_ignore_ascii_case(second.path()) {
                return Err(LayoutError::DuplicateMaterializedPath {
                    first: first.path().to_owned(),
                    second: second.path().to_owned(),
                });
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------ parser errors

#[derive(Debug)]
enum ContractError {
    Syntax(SyntaxError),
    Value(ValueError),
    Layout(LayoutError),
}

#[derive(Debug)]
enum SyntaxError {
    CarriageReturn,
    MalformedLine {
        number: usize,
        text: String,
    },
    UnknownKey {
        key: String,
        number: usize,
    },
    DuplicateKey {
        key: &'static str,
        first_line: usize,
        second_line: usize,
    },
    MissingKey {
        key: &'static str,
    },
}

#[derive(Debug)]
enum ValueError {
    WrongLiteral {
        key: &'static str,
        expected: &'static str,
        found: String,
    },
    Name {
        key: &'static str,
        found: String,
        rejection: NameRejection,
    },
    Invariant(&'static str),
}

#[derive(Debug)]
enum LayoutError {
    Version(VersionRejection),
    Path(PathRejection),
    DuplicateMaterializedPath {
        first: String,
        second: String,
    },
    CaseFoldCollision {
        expected: String,
        found: String,
    },
    UndeclaredMember {
        path: String,
    },
    MissingMember {
        path: String,
    },
    WrongMemberKind {
        path: String,
        expected: MemberKind,
        found: MemberKind,
    },
}

impl fmt::Display for NameRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "name is empty"),
            Self::TooLong { length } => {
                write!(
                    formatter,
                    "name is {length} bytes; limit is {MAX_NAME_BYTES}"
                )
            }
            Self::DisallowedByte { byte } => {
                write!(formatter, "disallowed byte 0x{byte:02X}")
            }
            Self::DotSegment => write!(formatter, "dot segment is not allowed"),
            Self::TrailingDot => write!(formatter, "trailing dot is not allowed"),
            Self::TrailingSpace => write!(formatter, "trailing space is not allowed"),
            Self::ReservedDevice { device } => {
                write!(formatter, "reserved Windows device {device}")
            }
        }
    }
}

impl fmt::Display for VersionRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Placeholder => write!(formatter, "<version> is documentation only"),
            Self::UnsafeName(rejection) => rejection.fmt(formatter),
        }
    }
}

impl fmt::Display for PathRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "path is empty"),
            Self::Absolute => write!(formatter, "absolute path is not allowed"),
            Self::ForbiddenByte { byte } => {
                write!(formatter, "forbidden path byte 0x{byte:02X}")
            }
            Self::EmptyComponent => write!(formatter, "path has an empty component"),
            Self::WrongCompanyRoot { found } => {
                write!(
                    formatter,
                    "path must start with {COMPANY_ROOT:?}, found {found:?}"
                )
            }
            Self::UnsafeComponent {
                component,
                rejection,
            } => write!(formatter, "component {component:?} is unsafe: {rejection}"),
            Self::ForbiddenBroker => write!(formatter, "Broker members are forbidden"),
        }
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CarriageReturn => write!(formatter, "contract is LF-only"),
            Self::MalformedLine { number, text } => {
                write!(formatter, "malformed line {number}: {text:?}")
            }
            Self::UnknownKey { key, number } => {
                write!(formatter, "unknown key {key:?} on line {number}")
            }
            Self::DuplicateKey {
                key,
                first_line,
                second_line,
            } => write!(
                formatter,
                "key {key} duplicated on lines {first_line} and {second_line}"
            ),
            Self::MissingKey { key } => write!(formatter, "missing required key {key}"),
        }
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLiteral {
                key,
                expected,
                found,
            } => write!(formatter, "{key} must be {expected:?}, found {found:?}"),
            Self::Name {
                key,
                found,
                rejection,
            } => write!(formatter, "{key} value {found:?} is unsafe: {rejection}"),
            Self::Invariant(message) => write!(formatter, "invalid v2 invariant: {message}"),
        }
    }
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(rejection) => write!(formatter, "invalid version: {rejection}"),
            Self::Path(rejection) => write!(formatter, "invalid member path: {rejection}"),
            Self::DuplicateMaterializedPath { first, second } => {
                write!(
                    formatter,
                    "materialized paths collide: {first:?} and {second:?}"
                )
            }
            Self::CaseFoldCollision { expected, found } => write!(
                formatter,
                "member path {found:?} case-fold-collides with {expected:?}"
            ),
            Self::UndeclaredMember { path } => write!(formatter, "undeclared member {path:?}"),
            Self::MissingMember { path } => write!(formatter, "missing member {path:?}"),
            Self::WrongMemberKind {
                path,
                expected,
                found,
            } => write!(
                formatter,
                "member {path:?} has kind {found:?}; expected {expected:?}"
            ),
        }
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => error.fmt(formatter),
            Self::Value(error) => error.fmt(formatter),
            Self::Layout(error) => error.fmt(formatter),
        }
    }
}

impl Error for NameRejection {}
impl Error for VersionRejection {}
impl Error for PathRejection {}
impl Error for ContractError {}
impl Error for LayoutError {}

impl From<ValueError> for ContractError {
    fn from(error: ValueError) -> Self {
        Self::Value(error)
    }
}

struct FoundEntry {
    key: &'static str,
    number: usize,
    value: String,
}

struct ParsedPaths {
    package_root: CompanyRoot,
    bin_dir: SafeLeaf,
    versions_dir: SafeLeaf,
    resources_dir: SafeLeaf,
    licenses_dir: SafeLeaf,
    installation_file: SafeLeaf,
    payload_manifest_file: SafeLeaf,
    ae_executable: SafeLeaf,
    installer_executable: SafeLeaf,
    editor_executable: SafeLeaf,
    forge_executable: SafeLeaf,
}

fn scan_entries(text: &str) -> Result<Vec<FoundEntry>, ContractError> {
    if text.contains('\r') {
        return Err(ContractError::Syntax(SyntaxError::CarriageReturn));
    }
    let mut found: Vec<FoundEntry> = Vec::new();
    for (offset, line) in text.split('\n').enumerate() {
        let number = offset + 1;
        if line.is_empty() {
            continue;
        }
        if line.bytes().any(|byte| byte < 0x20 || byte == 0x7F) {
            return Err(ContractError::Syntax(SyntaxError::MalformedLine {
                number,
                text: line.to_owned(),
            }));
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            return Err(ContractError::Syntax(SyntaxError::MalformedLine {
                number,
                text: line.to_owned(),
            }));
        };
        if raw_value.contains('=') {
            return Err(ContractError::Syntax(SyntaxError::MalformedLine {
                number,
                text: line.to_owned(),
            }));
        }
        let key = raw_key.trim_ascii();
        let value = raw_value.trim_ascii_start();
        if key.is_empty() || value.is_empty() || value.ends_with(' ') {
            return Err(ContractError::Syntax(SyntaxError::MalformedLine {
                number,
                text: line.to_owned(),
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

fn required_entry<'a>(
    entries: &'a [FoundEntry],
    key: &'static str,
) -> Result<&'a str, ContractError> {
    entries
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| entry.value.as_str())
        .ok_or(ContractError::Syntax(SyntaxError::MissingKey { key }))
}

fn parse_exact_leaf(
    key: &'static str,
    raw: &str,
    expected: &'static str,
) -> Result<SafeLeaf, ContractError> {
    let leaf = SafeLeaf::parse(raw).map_err(|rejection| {
        ContractError::Value(ValueError::Name {
            key,
            found: raw.to_owned(),
            rejection,
        })
    })?;
    if leaf.as_str() != expected {
        return Err(ContractError::Value(ValueError::WrongLiteral {
            key,
            expected,
            found: raw.to_owned(),
        }));
    }
    Ok(leaf)
}

fn parse_exact_root(raw: &str) -> Result<CompanyRoot, ContractError> {
    CompanyRoot::parse(raw).map_err(|found| {
        ContractError::Value(ValueError::WrongLiteral {
            key: KEY_PACKAGE_ROOT,
            expected: COMPANY_ROOT,
            found,
        })
    })
}

fn parse_exact_role(
    key: &'static str,
    raw: &str,
    expected: &'static str,
    role: ExecutableRole,
) -> Result<ExecutableRole, ContractError> {
    if raw == expected {
        Ok(role)
    } else {
        Err(ContractError::Value(ValueError::WrongLiteral {
            key,
            expected,
            found: raw.to_owned(),
        }))
    }
}

fn parse_schema_version(entries: &[FoundEntry]) -> Result<u8, ContractError> {
    let found = required_entry(entries, KEY_SCHEMA_VERSION)?;
    if found == "2" {
        Ok(2)
    } else {
        Err(ContractError::Value(ValueError::WrongLiteral {
            key: KEY_SCHEMA_VERSION,
            expected: "2",
            found: found.to_owned(),
        }))
    }
}

fn parse_paths(entries: &[FoundEntry]) -> Result<ParsedPaths, ContractError> {
    Ok(ParsedPaths {
        package_root: parse_exact_root(required_entry(entries, KEY_PACKAGE_ROOT)?)?,
        bin_dir: parse_exact_leaf(KEY_BIN_DIR, required_entry(entries, KEY_BIN_DIR)?, "bin")?,
        versions_dir: parse_exact_leaf(
            KEY_VERSIONS_DIR,
            required_entry(entries, KEY_VERSIONS_DIR)?,
            "versions",
        )?,
        resources_dir: parse_exact_leaf(
            KEY_RESOURCES_DIR,
            required_entry(entries, KEY_RESOURCES_DIR)?,
            "resources",
        )?,
        licenses_dir: parse_exact_leaf(
            KEY_LICENSES_DIR,
            required_entry(entries, KEY_LICENSES_DIR)?,
            "licenses",
        )?,
        installation_file: parse_exact_leaf(
            KEY_INSTALLATION_FILE,
            required_entry(entries, KEY_INSTALLATION_FILE)?,
            "installation.json",
        )?,
        payload_manifest_file: parse_exact_leaf(
            KEY_PAYLOAD_MANIFEST_FILE,
            required_entry(entries, KEY_PAYLOAD_MANIFEST_FILE)?,
            "payload-manifest.json",
        )?,
        ae_executable: parse_exact_leaf(
            KEY_AE_EXECUTABLE,
            required_entry(entries, KEY_AE_EXECUTABLE)?,
            "ae",
        )?,
        installer_executable: parse_exact_leaf(
            KEY_INSTALLER_EXECUTABLE,
            required_entry(entries, KEY_INSTALLER_EXECUTABLE)?,
            "installer",
        )?,
        editor_executable: parse_exact_leaf(
            KEY_EDITOR_EXECUTABLE,
            required_entry(entries, KEY_EDITOR_EXECUTABLE)?,
            "editor",
        )?,
        forge_executable: parse_exact_leaf(
            KEY_FORGE_EXECUTABLE,
            required_entry(entries, KEY_FORGE_EXECUTABLE)?,
            "forge",
        )?,
    })
}

fn parse_roles(
    entries: &[FoundEntry],
) -> Result<(ExecutableRole, ExecutableRole, BrokerPolicy), ContractError> {
    let stable_launcher = parse_exact_role(
        KEY_STABLE_LAUNCHER_ROLE,
        required_entry(entries, KEY_STABLE_LAUNCHER_ROLE)?,
        "ae",
        ExecutableRole::Ae,
    )?;
    let bootstrap = parse_exact_role(
        KEY_BOOTSTRAP_ROLE,
        required_entry(entries, KEY_BOOTSTRAP_ROLE)?,
        "installer",
        ExecutableRole::Installer,
    )?;
    let broker = required_entry(entries, KEY_BROKER_ROLE)?;
    let broker_role = if broker == "forbidden" {
        BrokerPolicy::Forbidden
    } else {
        return Err(ContractError::Value(ValueError::WrongLiteral {
            key: KEY_BROKER_ROLE,
            expected: "forbidden",
            found: broker.to_owned(),
        }));
    };
    Ok((stable_launcher, bootstrap, broker_role))
}

fn parse_policy(
    entries: &[FoundEntry],
) -> Result<(MutableStateLocation, ProductionFallback, ProductionFallback), ContractError> {
    Ok((
        MutableStateLocation::parse(required_entry(entries, KEY_MUTABLE_STATE)?)?,
        ProductionFallback::parse(
            KEY_SOURCE_TREE_FALLBACK,
            required_entry(entries, KEY_SOURCE_TREE_FALLBACK)?,
        )?,
        ProductionFallback::parse(
            KEY_RUNFILES_FALLBACK,
            required_entry(entries, KEY_RUNFILES_FALLBACK)?,
        )?,
    ))
}

fn parse_contract(text: &str) -> Result<VersionedLayout, ContractError> {
    let entries = scan_entries(text)?;
    let schema_version = parse_schema_version(&entries)?;
    let proof_scope = ProofScope::parse(required_entry(&entries, KEY_PROOF_SCOPE)?)?;
    let paths = parse_paths(&entries)?;
    let (stable_launcher_role, bootstrap_role, broker_role) = parse_roles(&entries)?;
    let (mutable_state, source_tree_fallback, runfiles_fallback) = parse_policy(&entries)?;
    let layout = VersionedLayout {
        schema_version,
        proof_scope,
        package_root: paths.package_root,
        bin_dir: paths.bin_dir,
        versions_dir: paths.versions_dir,
        resources_dir: paths.resources_dir,
        licenses_dir: paths.licenses_dir,
        installation_file: paths.installation_file,
        payload_manifest_file: paths.payload_manifest_file,
        ae_executable: paths.ae_executable,
        installer_executable: paths.installer_executable,
        editor_executable: paths.editor_executable,
        forge_executable: paths.forge_executable,
        stable_launcher_role,
        bootstrap_role,
        broker_role,
        mutable_state,
        source_tree_fallback,
        runfiles_fallback,
    };
    layout.validate_invariants()?;
    Ok(layout)
}

// ------------------------------------------------------------------ fixtures

fn replace_value(template: &str, key: &str, replacement: &str) -> String {
    template
        .split('\n')
        .map(|line| {
            let Some((raw_key, _)) = line.split_once('=') else {
                return line.to_owned();
            };
            if raw_key.trim_ascii() == key {
                format!("{key} = {replacement}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn remove_key(template: &str, key: &str) -> String {
    template
        .split('\n')
        .filter(|line| {
            line.split_once('=')
                .map_or(true, |(raw_key, _)| raw_key.trim_ascii() != key)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn append_line(template: &str, line: &str) -> String {
    format!("{template}{line}\n")
}

fn member_specs(members: &[LayoutMember]) -> Vec<(&str, MemberKind)> {
    members
        .iter()
        .map(|member| (member.path(), member.kind()))
        .collect()
}

fn executable_extension_matches(member: &LayoutMember, target: TargetPlatform) -> bool {
    if member.kind() == MemberKind::Directory || !member.path().contains("/bin/") {
        return true;
    }
    let has_exe_suffix = member.path().to_ascii_lowercase().ends_with(".exe");
    has_exe_suffix == matches!(target, TargetPlatform::Windows)
}

fn expected_specs(target: TargetPlatform, version: &str) -> Vec<(String, MemberKind)> {
    let suffix = target.executable_suffix();
    let executable = |name: &str| format!("{name}{suffix}");
    let ae = executable("ae");
    let installer = executable("installer");
    let editor = executable("editor");
    let forge = executable("forge");
    vec![
        ("Artisan Street".to_owned(), MemberKind::Directory),
        ("Artisan Street/bin".to_owned(), MemberKind::Directory),
        (format!("Artisan Street/bin/{ae}"), MemberKind::File),
        (
            "Artisan Street/installation.json".to_owned(),
            MemberKind::File,
        ),
        ("Artisan Street/versions".to_owned(), MemberKind::Directory),
        (
            format!("Artisan Street/versions/{version}"),
            MemberKind::Directory,
        ),
        (
            format!("Artisan Street/versions/{version}/bin"),
            MemberKind::Directory,
        ),
        (
            format!("Artisan Street/versions/{version}/bin/{ae}"),
            MemberKind::File,
        ),
        (
            format!("Artisan Street/versions/{version}/bin/{installer}"),
            MemberKind::File,
        ),
        (
            format!("Artisan Street/versions/{version}/bin/{editor}"),
            MemberKind::File,
        ),
        (
            format!("Artisan Street/versions/{version}/bin/{forge}"),
            MemberKind::File,
        ),
        (
            format!("Artisan Street/versions/{version}/resources"),
            MemberKind::Directory,
        ),
        (
            format!("Artisan Street/versions/{version}/licenses"),
            MemberKind::Directory,
        ),
        (
            format!("Artisan Street/versions/{version}/payload-manifest.json"),
            MemberKind::File,
        ),
    ]
}

// --------------------------------------------------------------------- tests

#[test]
fn parses_the_closed_v2_contract_and_disables_production_fallbacks() {
    let layout = parse_contract(CONTRACT_TEXT).expect("v2 contract must parse");
    assert_eq!(layout.schema_version, 2);
    assert_eq!(layout.proof_scope, ProofScope::VersionedReleaseLayout);
    assert_eq!(layout.package_root.as_str(), COMPANY_ROOT);
    assert_eq!(layout.bin_dir.as_str(), "bin");
    assert_eq!(layout.versions_dir.as_str(), "versions");
    assert_eq!(layout.resources_dir.as_str(), "resources");
    assert_eq!(layout.licenses_dir.as_str(), "licenses");
    assert_eq!(layout.installation_file.as_str(), "installation.json");
    assert_eq!(
        layout.payload_manifest_file.as_str(),
        "payload-manifest.json"
    );
    assert_eq!(layout.ae_executable.as_str(), "ae");
    assert_eq!(layout.installer_executable.as_str(), "installer");
    assert_eq!(layout.editor_executable.as_str(), "editor");
    assert_eq!(layout.forge_executable.as_str(), "forge");
    assert_eq!(layout.stable_launcher_role, ExecutableRole::Ae);
    assert_eq!(layout.bootstrap_role, ExecutableRole::Installer);
    assert_eq!(layout.broker_role, BrokerPolicy::Forbidden);
    assert_eq!(
        layout.mutable_state,
        MutableStateLocation::OutsideImmutableRoots
    );
    assert_eq!(layout.source_tree_fallback, ProductionFallback::Disabled);
    assert_eq!(layout.runfiles_fallback, ProductionFallback::Disabled);
    assert!(!CONTRACT_TEXT.contains('\r'));
}

#[test]
fn windows_materialization_has_exact_members_and_exe_suffix() -> Result<(), Box<dyn Error>> {
    let layout = parse_contract(CONTRACT_TEXT)?;
    let version = "1.2.3";
    let members = layout.required_members(TargetPlatform::Windows, version)?;
    let actual = member_specs(&members)
        .into_iter()
        .map(|(path, kind)| (path.to_owned(), kind))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected_specs(TargetPlatform::Windows, version));
    assert_eq!(members.len(), 14);
    assert!(
        members
            .iter()
            .all(|member| !member.path().to_ascii_lowercase().contains("broker"))
    );
    assert!(members.iter().all(|member| {
        !member.path().contains("<version>")
            && executable_extension_matches(member, TargetPlatform::Windows)
    }));
    layout.validate_members(TargetPlatform::Windows, version, &members)?;
    Ok(())
}

#[test]
fn non_windows_materialization_has_exact_members_without_exe_suffix() -> Result<(), Box<dyn Error>>
{
    let layout = parse_contract(CONTRACT_TEXT)?;
    let version = "2026.08";
    let members = layout.required_members(TargetPlatform::NonWindows, version)?;
    let actual = member_specs(&members)
        .into_iter()
        .map(|(path, kind)| (path.to_owned(), kind))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected_specs(TargetPlatform::NonWindows, version));
    assert!(
        members
            .iter()
            .all(|member| executable_extension_matches(member, TargetPlatform::NonWindows))
    );
    assert!(
        members
            .iter()
            .all(|member| !member.path().to_ascii_lowercase().contains("broker"))
    );
    layout.validate_members(TargetPlatform::NonWindows, version, &members)?;
    Ok(())
}

#[test]
fn parser_fails_closed_on_table_driven_contract_drift() {
    let cases = [
        (
            "unknown key",
            append_line(CONTRACT_TEXT, "future_key = value"),
        ),
        (
            "duplicate key",
            append_line(CONTRACT_TEXT, "ae_executable = ae"),
        ),
        ("missing key", remove_key(CONTRACT_TEXT, KEY_LICENSES_DIR)),
        (
            "line without assignment",
            append_line(CONTRACT_TEXT, "not a pair"),
        ),
        (
            "multiple equals",
            append_line(CONTRACT_TEXT, "schema_version = 2 = extra"),
        ),
        (
            "carriage return carrier",
            CONTRACT_TEXT.replace('\n', "\r\n"),
        ),
        (
            "schema drift",
            replace_value(CONTRACT_TEXT, KEY_SCHEMA_VERSION, "1"),
        ),
        (
            "scope drift",
            replace_value(CONTRACT_TEXT, KEY_PROOF_SCOPE, "layout-only"),
        ),
        (
            "company root drift",
            replace_value(CONTRACT_TEXT, KEY_PACKAGE_ROOT, "artisan-street"),
        ),
        (
            "directory drift",
            replace_value(CONTRACT_TEXT, KEY_BIN_DIR, "bin/other"),
        ),
        (
            "installation leaf drift",
            replace_value(CONTRACT_TEXT, KEY_INSTALLATION_FILE, "../installation.json"),
        ),
        (
            "logical executable drift",
            replace_value(CONTRACT_TEXT, KEY_FORGE_EXECUTABLE, "forge.exe"),
        ),
        (
            "stable role drift",
            replace_value(CONTRACT_TEXT, KEY_STABLE_LAUNCHER_ROLE, "installer"),
        ),
        (
            "bootstrap role drift",
            replace_value(CONTRACT_TEXT, KEY_BOOTSTRAP_ROLE, "ae"),
        ),
        (
            "Broker role enabled",
            replace_value(CONTRACT_TEXT, KEY_BROKER_ROLE, "broker"),
        ),
        (
            "mutable state drift",
            replace_value(CONTRACT_TEXT, KEY_MUTABLE_STATE, "inside-package-root"),
        ),
        (
            "source fallback enabled",
            replace_value(CONTRACT_TEXT, KEY_SOURCE_TREE_FALLBACK, "true"),
        ),
        (
            "runfiles fallback enabled",
            replace_value(CONTRACT_TEXT, KEY_RUNFILES_FALLBACK, "TRUE"),
        ),
    ];
    for (label, drifted) in cases {
        let error = parse_contract(&drifted)
            .expect_err("every drifted contract must be rejected")
            .to_string();
        assert!(!error.is_empty(), "{label}: rejection should be readable");
    }
}

#[test]
fn strict_leaf_grammar_rejects_table_driven_unsafe_components() {
    let cases: &[(&str, &str)] = &[
        ("empty", ""),
        ("dot", "."),
        ("dot-dot", ".."),
        ("slash", "a/b"),
        ("backslash", r"a\b"),
        ("drive", "C:"),
        ("ADS", "name:stream"),
        ("trailing dot", "name."),
        ("trailing space", "name "),
        ("control", "name\u{1}"),
        ("non-ASCII", "café"),
        ("reserved CON", "CON"),
        ("reserved COM1 extension", "com1.txt"),
        ("reserved LPT9 mixed case", "Lpt9"),
    ];
    for (label, input) in cases {
        assert!(
            SafeLeaf::parse(input).is_err(),
            "{label}: {input:?} must be rejected"
        );
    }
    let long = "a".repeat(MAX_NAME_BYTES + 1);
    assert!(SafeLeaf::parse(&long).is_err());
}

#[test]
fn version_identifiers_are_separately_validated_and_never_use_the_placeholder() {
    let accepted = ["1.0.0", "2026.08", "v2-preview_1"];
    for version in accepted {
        assert!(
            VersionId::parse(version).is_ok(),
            "{version:?} should parse"
        );
        assert!(
            parse_contract(CONTRACT_TEXT)
                .unwrap()
                .required_members(TargetPlatform::Windows, version)
                .is_ok()
        );
    }

    let rejected = [
        "",
        ".",
        "..",
        "<version>",
        "/absolute",
        r"C:\version",
        r"\\server\share",
        "version:stream",
        "version/name",
        r"version\name",
        "CON",
        "com1.txt",
        "version.",
        "version ",
        "café",
    ];
    for version in rejected {
        assert!(
            VersionId::parse(version).is_err(),
            "{version:?} must not be a version identifier"
        );
        assert!(
            parse_contract(CONTRACT_TEXT)
                .unwrap()
                .required_members(TargetPlatform::Windows, version)
                .is_err(),
            "{version:?} must not materialize into a path"
        );
    }
}

#[test]
fn canonical_paths_reject_absolute_traversal_ads_and_non_ascii_forms() {
    let cases = [
        ("empty", ""),
        ("POSIX absolute", "/Artisan Street/bin"),
        ("UNC", "//server/share"),
        ("UNC backslash", r"\\server\share"),
        ("drive", "C:/Artisan Street/bin"),
        ("ADS", "Artisan Street/bin/ae.exe:stream"),
        ("traversal", "Artisan Street/versions/../bin"),
        ("dot segment", "Artisan Street/./bin"),
        ("backslash separator", r"Artisan Street/bin\ae.exe"),
        ("empty component", "Artisan Street//bin"),
        ("wrong root", "artisan street/bin"),
        ("root trailing space", "Artisan Street /bin"),
        ("leaf trailing dot", "Artisan Street/bin/ae.exe."),
        ("leaf trailing space", "Artisan Street/bin/ae.exe "),
        ("reserved device", "Artisan Street/bin/CON.exe"),
        ("control", "Artisan Street/bin/a\u{1}"),
        ("non-ASCII", "Artisan Street/bin/café"),
        ("Broker", "Artisan Street/bin/BROKER.exe"),
    ];
    for (label, path) in cases {
        assert!(
            CanonicalPath::parse(path).is_err(),
            "{label}: {path:?} must be rejected"
        );
    }
    assert!(CanonicalPath::parse("Artisan Street/bin/ae.exe").is_ok());
}

#[test]
fn member_validation_rejects_duplicates_collisions_missing_wrong_kind_and_extras()
-> Result<(), Box<dyn Error>> {
    let layout = parse_contract(CONTRACT_TEXT)?;
    let target = TargetPlatform::Windows;
    let version = "1.2.3";
    let required = layout.required_members(target, version)?;

    let mut duplicate = required.clone();
    duplicate.push(required[0].clone());
    assert!(matches!(
        layout.validate_members(target, version, &duplicate),
        Err(LayoutError::DuplicateMaterializedPath { .. })
    ));

    let mut case_collision = required.clone();
    case_collision[1] = LayoutMember::new("Artisan Street/BIN", MemberKind::Directory)?;
    assert!(matches!(
        layout.validate_members(target, version, &case_collision),
        Err(LayoutError::CaseFoldCollision { .. })
    ));

    let mut missing = required.clone();
    missing.pop();
    assert!(matches!(
        layout.validate_members(target, version, &missing),
        Err(LayoutError::MissingMember { .. })
    ));

    let mut wrong_kind = required.clone();
    wrong_kind[2] = LayoutMember::new(required[2].path(), MemberKind::Directory)?;
    assert!(matches!(
        layout.validate_members(target, version, &wrong_kind),
        Err(LayoutError::WrongMemberKind { .. })
    ));

    let mut undeclared = required.clone();
    undeclared[2] = LayoutMember::new("Artisan Street/bin/extra", MemberKind::File)?;
    assert!(matches!(
        layout.validate_members(target, version, &undeclared),
        Err(LayoutError::UndeclaredMember { .. })
    ));

    assert!(matches!(
        LayoutMember::new("Artisan Street/bin/BROKER.exe", MemberKind::File),
        Err(PathRejection::ForbiddenBroker)
    ));
    Ok(())
}

#[test]
fn member_path_uniqueness_is_case_insensitive() -> Result<(), Box<dyn Error>> {
    let first = LayoutMember::new("Artisan Street/bin/ae.exe", MemberKind::File)?;
    let second = LayoutMember::new("Artisan Street/BIN/AE.EXE", MemberKind::File)?;
    assert!(matches!(
        ensure_unique_materialized_paths(&[first, second]),
        Err(LayoutError::DuplicateMaterializedPath { .. })
    ));
    Ok(())
}
