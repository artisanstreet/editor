//! Dependency-free validation and selection for bundled agent-name banks.
//!
//! The TypeScript orchestration service owns JSON loading, schema decoding,
//! and the durable defaults read. This leaf receives the two decoded bundled
//! banks and the completed defaults read as borrowed values. It validates both
//! banks before selecting one and returns the selected bank without copying,
//! sorting, deduplicating, normalizing, or randomly choosing a name.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// The canonical protocol identifier for the Norwegian feminine-name bank.
pub const NORWEGIAN_DATASET_ID: &str = "norwegian";

/// The canonical protocol identifier for the British feminine-name bank.
pub const BRITISH_DATASET_ID: &str = "british";

/// The protocol default used when durable defaults omit the dataset selection.
pub const DEFAULT_AGENT_NAME_DATASET_ID: &str = NORWEGIAN_DATASET_ID;

/// Short alias for the protocol's default agent-name dataset identifier.
pub const DEFAULT_DATASET_ID: &str = DEFAULT_AGENT_NAME_DATASET_ID;

/// A failure found while validating one caller-supplied canonical bank.
#[must_use = "an agent-name catalog validation failure must be handled"]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AgentNameCatalogError {
    /// A canonical bank contained no entries.
    EmptyBank {
        /// The canonical dataset identifier for the invalid bank.
        dataset_id: &'static str,
    },
    /// A canonical bank contained an empty name at the supplied position.
    EmptyName {
        /// The canonical dataset identifier for the invalid bank.
        dataset_id: &'static str,
        /// The zero-based position of the empty name.
        index: usize,
    },
}

impl AgentNameCatalogError {
    /// Returns the canonical dataset identifier whose bank failed validation.
    #[must_use = "read the invalid dataset identifier"]
    pub const fn dataset_id(self) -> &'static str {
        match self {
            Self::EmptyBank { dataset_id } | Self::EmptyName { dataset_id, .. } => dataset_id,
        }
    }

    /// Returns the zero-based invalid-name position, when an entry was empty.
    #[must_use = "read the invalid name index"]
    pub const fn name_index(self) -> Option<usize> {
        match self {
            Self::EmptyBank { .. } => None,
            Self::EmptyName { index, .. } => Some(index),
        }
    }
}

impl std::fmt::Display for AgentNameCatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyBank { dataset_id } => {
                write!(formatter, "{dataset_id} agent-name bank must not be empty")
            }
            Self::EmptyName { dataset_id, index } => write!(
                formatter,
                "{dataset_id} agent-name bank entry at index {index} must not be empty"
            ),
        }
    }
}

impl std::error::Error for AgentNameCatalogError {}

/// The successful value returned by the durable session-defaults read.
///
/// `None` means that the durable row omitted the selection and therefore uses
/// [`DEFAULT_AGENT_NAME_DATASET_ID`]. Any supplied text is retained exactly;
/// unknown or future identifiers are handled by the catalog's Norwegian
/// fallback after both banks have been validated.
#[must_use = "a completed defaults read must be supplied to catalog selection"]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AgentNameCatalogDefaultsRead<'a> {
    /// The durable agent-name dataset identifier, when one was read.
    pub agent_name_dataset: Option<&'a str>,
}

impl<'a> AgentNameCatalogDefaultsRead<'a> {
    /// Creates a completed defaults read while preserving its optional value.
    #[must_use = "the completed defaults read must be supplied to catalog selection"]
    pub const fn new(agent_name_dataset: Option<&'a str>) -> Self {
        Self { agent_name_dataset }
    }

    /// Creates the completed read for a durable row with no selection.
    #[must_use = "the completed defaults read must be supplied to catalog selection"]
    pub const fn missing() -> Self {
        Self::new(None)
    }
}

/// The two decoded canonical banks supplied by the bundled-data boundary.
///
/// The slice and its entries are borrowed so selection can return the exact
/// caller-owned ordered bank without allocating or reproducing the data
/// assets.
#[must_use = "the supplied banks must be validated and selected"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentNameCatalogBanks<'banks, 'name> {
    /// The canonical Norwegian bank, also used as the unknown-selection fallback.
    pub norwegian: &'banks [&'name str],
    /// The canonical British bank.
    pub british: &'banks [&'name str],
}

impl<'banks, 'name> AgentNameCatalogBanks<'banks, 'name> {
    /// Creates a bank pair without changing either supplied slice.
    #[must_use = "the supplied banks must be validated and selected"]
    pub const fn new(norwegian: &'banks [&'name str], british: &'banks [&'name str]) -> Self {
        Self { norwegian, british }
    }

    /// Validates both canonical banks in Norwegian-then-British order.
    ///
    /// A bank must contain at least one entry, and every entry must be a
    /// non-empty string. Whitespace is otherwise data: no trimming or other
    /// visible-name validation is performed. Duplicate names are valid.
    ///
    /// # Errors
    ///
    /// Returns the first empty bank or empty entry in canonical bank order.
    #[must_use = "handle invalid agent-name banks"]
    pub fn validate(
        self,
    ) -> Result<ValidatedAgentNameCatalogBanks<'banks, 'name>, AgentNameCatalogError> {
        validate_bank(NORWEGIAN_DATASET_ID, self.norwegian)?;
        validate_bank(BRITISH_DATASET_ID, self.british)?;

        Ok(ValidatedAgentNameCatalogBanks {
            norwegian: self.norwegian,
            british: self.british,
        })
    }
}

/// A bank pair that has passed validation for both canonical datasets.
///
/// This type is produced only by [`AgentNameCatalogBanks::validate`], making
/// the validation-before-selection ordering explicit at this boundary.
#[must_use = "a validated bank pair must be selected or retained"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedAgentNameCatalogBanks<'banks, 'name> {
    norwegian: &'banks [&'name str],
    british: &'banks [&'name str],
}

impl<'banks, 'name> ValidatedAgentNameCatalogBanks<'banks, 'name> {
    /// Borrows the validated Norwegian bank in its supplied order.
    #[must_use]
    pub const fn norwegian(self) -> &'banks [&'name str] {
        self.norwegian
    }

    /// Borrows the validated British bank in its supplied order.
    #[must_use]
    pub const fn british(self) -> &'banks [&'name str] {
        self.british
    }

    /// Selects a canonical bank from one completed durable defaults value.
    ///
    /// A missing selection first resolves to the protocol default. The two
    /// canonical identifiers return their exact corresponding slices. Every
    /// other identifier, including an empty, case-variant, or future value,
    /// returns the already-validated Norwegian slice.
    #[must_use]
    pub fn select(self, durable_selection: Option<&str>) -> &'banks [&'name str] {
        if durable_selection.unwrap_or(DEFAULT_AGENT_NAME_DATASET_ID) == BRITISH_DATASET_ID {
            self.british
        } else {
            self.norwegian
        }
    }
}

/// All decoded inputs needed for one catalog resolution.
///
/// The `durable_defaults` field is the explicit successful completion of the
/// source service's defaults read. It is intentionally not an Effect, a
/// persistence handle, or an asynchronous operation.
#[must_use = "the catalog input must be resolved"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentNameCatalogInput<'banks, 'name, 'selection> {
    /// Caller-supplied Norwegian and British bundled banks.
    pub banks: AgentNameCatalogBanks<'banks, 'name>,
    /// Completed durable defaults read used for selection.
    pub durable_defaults: AgentNameCatalogDefaultsRead<'selection>,
}

impl<'banks, 'name, 'selection> AgentNameCatalogInput<'banks, 'name, 'selection> {
    /// Creates a catalog input without copying or interpreting any value.
    #[must_use = "the catalog input must be resolved"]
    pub const fn new(
        norwegian: &'banks [&'name str],
        british: &'banks [&'name str],
        durable_defaults: AgentNameCatalogDefaultsRead<'selection>,
    ) -> Self {
        Self {
            banks: AgentNameCatalogBanks::new(norwegian, british),
            durable_defaults,
        }
    }
}

/// Stateless deterministic policy for validating and selecting agent-name banks.
#[must_use]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AgentNameCatalogPolicy;

impl AgentNameCatalogPolicy {
    /// Creates the stateless catalog policy.
    #[must_use = "the stateless catalog policy must be used for resolution"]
    pub const fn new() -> Self {
        Self
    }

    /// Validates both supplied banks and selects the result for the defaults read.
    ///
    /// Validation is completed before the durable selection is inspected. The
    /// returned slice is one of the exact caller-supplied slices, so its order,
    /// duplicate entries, Unicode spelling, and surrounding whitespace are
    /// preserved byte-for-byte.
    ///
    /// # Errors
    ///
    /// Returns [`AgentNameCatalogError`] when either canonical bank is empty or
    /// contains an empty entry. An invalid non-selected bank still fails.
    #[must_use = "handle invalid agent-name catalog input"]
    pub fn resolve<'banks, 'name>(
        input: AgentNameCatalogInput<'banks, 'name, '_>,
    ) -> Result<&'banks [&'name str], AgentNameCatalogError> {
        let validated = input.banks.validate()?;
        Ok(validated.select(input.durable_defaults.agent_name_dataset))
    }

    /// Alias for [`Self::resolve`] using the source catalog's selection verb.
    ///
    /// # Errors
    ///
    /// Returns the same validation error as [`Self::resolve`].
    #[must_use = "handle invalid agent-name catalog input"]
    pub fn select<'banks, 'name>(
        input: AgentNameCatalogInput<'banks, 'name, '_>,
    ) -> Result<&'banks [&'name str], AgentNameCatalogError> {
        Self::resolve(input)
    }
}

/// Resolves one caller-supplied catalog input without retaining a policy value.
///
/// # Errors
///
/// Returns [`AgentNameCatalogError`] when either canonical bank is invalid.
#[must_use = "handle invalid agent-name catalog input"]
pub fn resolve_agent_name_catalog<'banks, 'name>(
    input: AgentNameCatalogInput<'banks, 'name, '_>,
) -> Result<&'banks [&'name str], AgentNameCatalogError> {
    AgentNameCatalogPolicy::resolve(input)
}

fn validate_bank(dataset_id: &'static str, bank: &[&str]) -> Result<(), AgentNameCatalogError> {
    if bank.is_empty() {
        return Err(AgentNameCatalogError::EmptyBank { dataset_id });
    }

    if let Some((index, _)) = bank.iter().enumerate().find(|(_, name)| name.is_empty()) {
        return Err(AgentNameCatalogError::EmptyName { dataset_id, index });
    }

    Ok(())
}
