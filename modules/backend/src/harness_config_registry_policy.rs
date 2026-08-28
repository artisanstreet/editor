//! Dependency-free registry policy for Artisan-owned harness configuration.
//!
//! This is the native counterpart of `harness-config/keys.ts`. It contains
//! only the deterministic ownership boundary: which key identities Artisan
//! may write and which harness documents those identities resolve to. File
//! parsing, schema validation, effects, and the actual write service remain
//! outside this leaf.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

use std::fmt;

/// A structured document format understood by the harness-config service.
///
/// The format is kept local to this policy so callers do not need to depend
/// on a parser or on the protocol crate merely to construct a target.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConfigDocumentFormat {
    /// A JSON configuration document.
    Json,
    /// A TOML configuration document.
    Toml,
}

impl ConfigDocumentFormat {
    /// Returns the source spelling of the format.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Toml => "toml",
        }
    }
}

/// When a changed harness setting becomes observable.
///
/// This local enum mirrors the values accepted by the TypeScript
/// `ModelBehaviourActivationTiming` schema without importing that schema's
/// package into the dependency-free registry policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HarnessConfigActivationTiming {
    /// The changed value is observed immediately.
    Immediate,
    /// The changed value is observed on the next turn.
    NextTurn,
    /// The changed value is observed by newly created threads.
    NewThreads,
    /// The changed value is observed after a restart.
    RestartRequired,
}

impl HarnessConfigActivationTiming {
    /// Returns the source spelling of the activation timing.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::NextTurn => "next_turn",
            Self::NewThreads => "new_threads",
            Self::RestartRequired => "restart_required",
        }
    }
}

/// Short local name for callers that do not need the harness-specific prefix.
pub type ActivationTiming = HarnessConfigActivationTiming;

/// A dotted configuration path, stored in source order.
pub type ConfigKeyPath = Vec<String>;

/// Identity and product-facing custody for one Artisan-owned key.
///
/// The schema carried by the TypeScript key is intentionally absent here. The
/// registry only answers ownership and target questions; value encoding and
/// decoding belong to the service that consumes this policy.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HarnessConfigKeyIdentity {
    /// When a changed value becomes observable to the user.
    pub activation: HarnessConfigActivationTiming,
    /// Product-facing explanation, rather than the native harness key name.
    pub description: String,
    /// Harness that owns the native document.
    pub harness_id: String,
    /// Nested path inside that harness document.
    pub path: ConfigKeyPath,
}

impl HarnessConfigKeyIdentity {
    /// Creates an owned key identity while preserving every supplied value.
    #[must_use]
    pub fn new<I, S>(
        activation: HarnessConfigActivationTiming,
        description: impl Into<String>,
        harness_id: impl Into<String>,
        path: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            activation,
            description: description.into(),
            harness_id: harness_id.into(),
            path: path
                .into_iter()
                .map(|segment| segment.as_ref().to_owned())
                .collect(),
        }
    }

    /// Returns the stable identity used by declaration lookup.
    #[must_use]
    pub fn key_id(&self) -> String {
        harness_config_key_id(self)
    }
}

/// Locates one harness document and its recoverable backup directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessConfigTarget {
    /// Directory in which the service stores backups before replacement.
    pub backups_directory: String,
    /// Structured format of the target document.
    pub format: ConfigDocumentFormat,
    /// Harness identified by this target.
    pub harness_id: String,
    /// Absolute or otherwise caller-resolved document path.
    pub path: String,
}

impl HarnessConfigTarget {
    /// Creates an owned target while preserving the supplied path values.
    #[must_use]
    pub fn new(
        backups_directory: impl Into<String>,
        format: ConfigDocumentFormat,
        harness_id: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            backups_directory: backups_directory.into(),
            format,
            harness_id: harness_id.into(),
            path: path.into(),
        }
    }
}

/// The stable identity of one key, matching `${harness_id}:${path.join(".")}`.
#[must_use]
pub fn harness_config_key_id(key: &HarnessConfigKeyIdentity) -> String {
    format!("{}:{}", key.harness_id, key.path.join("."))
}

/// Harness ID of the declared Codex request-user-input key.
pub const CODEX_REQUEST_USER_INPUT_HARNESS_ID: &str = "codex";

/// Product description of the declared Codex request-user-input key.
pub const CODEX_REQUEST_USER_INPUT_DESCRIPTION: &str =
    "Let the agent pause and ask you a question outside plan mode instead of assuming an answer.";

/// Native path of the declared Codex request-user-input key.
pub const CODEX_REQUEST_USER_INPUT_PATH: &[&str] = &["features", "default_mode_request_user_input"];

/// Harness ID of the auto-compaction migration key.
pub const CODEX_AUTO_COMPACTION_TRIGGER_TOKENS_HARNESS_ID: &str = "codex";

/// Product description of the auto-compaction migration key.
pub const CODEX_AUTO_COMPACTION_TRIGGER_TOKENS_DESCRIPTION: &str = "Token threshold that triggers automatic history compaction; this does not change model context capacity.";

/// Native path of the auto-compaction migration key.
pub const CODEX_AUTO_COMPACTION_TRIGGER_TOKENS_PATH: &[&str] = &["model_auto_compact_token_limit"];

/// Returns the declared Codex request-user-input key.
///
/// This is the only key in the default writable declaration set. The returned
/// value is owned so a registry can retain it without borrowing static data.
#[must_use]
pub fn codex_request_user_input_key() -> HarnessConfigKeyIdentity {
    HarnessConfigKeyIdentity::new(
        HarnessConfigActivationTiming::NewThreads,
        CODEX_REQUEST_USER_INPUT_DESCRIPTION,
        CODEX_REQUEST_USER_INPUT_HARNESS_ID,
        CODEX_REQUEST_USER_INPUT_PATH,
    )
}

/// Returns the separately defined Codex auto-compaction migration key.
///
/// The key is intentionally not returned by [`declared_harness_config_keys`]
/// and therefore cannot be written through this registry until ownership is
/// moved from the Model Behaviour adapter.
#[must_use]
pub fn codex_auto_compaction_trigger_tokens_key() -> HarnessConfigKeyIdentity {
    HarnessConfigKeyIdentity::new(
        HarnessConfigActivationTiming::NewThreads,
        CODEX_AUTO_COMPACTION_TRIGGER_TOKENS_DESCRIPTION,
        CODEX_AUTO_COMPACTION_TRIGGER_TOKENS_HARNESS_ID,
        CODEX_AUTO_COMPACTION_TRIGGER_TOKENS_PATH,
    )
}

/// Returns the default writable declaration set in declaration order.
#[must_use]
pub fn declared_harness_config_keys() -> Vec<HarnessConfigKeyIdentity> {
    vec![codex_request_user_input_key()]
}

/// Alias naming the same set by its constructor role.
#[must_use]
pub fn default_harness_config_keys() -> Vec<HarnessConfigKeyIdentity> {
    declared_harness_config_keys()
}

/// Why registry construction was rejected.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HarnessConfigRegistryError {
    /// Two supplied keys resolve to one `harness:path` identity.
    DuplicateKeyIdentity,
    /// Two supplied targets claim the same harness ID.
    DuplicateHarnessTarget,
}

impl fmt::Display for HarnessConfigRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateKeyIdentity => formatter.write_str("Harness config keys must be unique"),
            Self::DuplicateHarnessTarget => {
                formatter.write_str("Each harness may declare only one config target")
            }
        }
    }
}

impl std::error::Error for HarnessConfigRegistryError {}

/// Ordered, validated ownership registry for one backend runtime.
///
/// Keys and targets remain in the exact order supplied by the caller. The
/// private identity vector is built only for deterministic declaration lookup;
/// target lookup scans the retained target order after construction has
/// rejected duplicate harness IDs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessConfigRegistry {
    keys: Vec<HarnessConfigKeyIdentity>,
    targets: Vec<HarnessConfigTarget>,
    key_ids: Vec<String>,
}

impl HarnessConfigRegistry {
    /// Builds a registry from optional keys and ordered targets.
    ///
    /// `None` selects [`default_harness_config_keys`]. `Some(Vec::new())` is
    /// an explicit empty declaration set. Key uniqueness is checked by stable
    /// `harness:path` identity, before target uniqueness, matching the source
    /// layer's failure order.
    ///
    /// # Errors
    ///
    /// Returns [`HarnessConfigRegistryError::DuplicateKeyIdentity`] when two
    /// keys have the same identity, or
    /// [`HarnessConfigRegistryError::DuplicateHarnessTarget`] when two targets
    /// claim one harness.
    pub fn new(
        keys: Option<Vec<HarnessConfigKeyIdentity>>,
        targets: Vec<HarnessConfigTarget>,
    ) -> Result<Self, HarnessConfigRegistryError> {
        let keys = keys.unwrap_or_else(default_harness_config_keys);
        let mut key_ids = Vec::with_capacity(keys.len());

        for key in &keys {
            let key_id = harness_config_key_id(key);
            if key_ids.contains(&key_id) {
                return Err(HarnessConfigRegistryError::DuplicateKeyIdentity);
            }
            key_ids.push(key_id);
        }

        let mut harness_ids = Vec::with_capacity(targets.len());
        for target in &targets {
            if harness_ids.contains(&target.harness_id) {
                return Err(HarnessConfigRegistryError::DuplicateHarnessTarget);
            }
            harness_ids.push(target.harness_id.clone());
        }

        Ok(Self {
            keys,
            targets,
            key_ids,
        })
    }

    /// Builds an explicitly empty fail-closed registry.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            keys: Vec::new(),
            targets: Vec::new(),
            key_ids: Vec::new(),
        }
    }

    /// Builds a registry using the default writable keys.
    ///
    /// # Errors
    ///
    /// Propagates duplicate-target rejection from [`Self::new`].
    pub fn with_default_keys(
        targets: Vec<HarnessConfigTarget>,
    ) -> Result<Self, HarnessConfigRegistryError> {
        Self::new(None, targets)
    }

    /// Returns writable key declarations in their original input order.
    #[must_use]
    pub fn keys(&self) -> &[HarnessConfigKeyIdentity] {
        &self.keys
    }

    /// Returns config targets in their original input order.
    #[must_use]
    pub fn targets(&self) -> &[HarnessConfigTarget] {
        &self.targets
    }

    /// Returns whether the registry declares the supplied key identity.
    ///
    /// Activation and description are custody metadata and do not change key
    /// identity, exactly as in the TypeScript `Declares` lookup.
    #[must_use]
    pub fn declares(&self, key: &HarnessConfigKeyIdentity) -> bool {
        self.key_ids.contains(&harness_config_key_id(key))
    }

    /// Finds the unique target for a harness, if one was supplied.
    #[must_use]
    pub fn find_target(&self, harness_id: &str) -> Option<&HarnessConfigTarget> {
        self.targets
            .iter()
            .find(|target| target.harness_id == harness_id)
    }
}

impl Default for HarnessConfigRegistry {
    /// Defaults to the empty fail-closed registry, not the writable default
    /// key set, because no target may be inferred by an unconfigured runtime.
    fn default() -> Self {
        Self::empty()
    }
}

/// Constructs a validated harness-config registry.
///
/// This free function mirrors the TypeScript layer factory while keeping the
/// Rust API independent of an effect runtime.
pub fn make_harness_config_registry(
    keys: Option<Vec<HarnessConfigKeyIdentity>>,
    targets: Vec<HarnessConfigTarget>,
) -> Result<HarnessConfigRegistry, HarnessConfigRegistryError> {
    HarnessConfigRegistry::new(keys, targets)
}

/// Returns the empty registry used when no writable target is configured.
#[must_use]
pub const fn empty_harness_config_registry() -> HarnessConfigRegistry {
    HarnessConfigRegistry::empty()
}
