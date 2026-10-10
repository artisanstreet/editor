//! Agent extensions the Forge keeps in every managed engine.
//!
//! The Forge is the authority over which skills an engine has. Artisan's own
//! skills ship with the Forge, skills the user adopts are copied into the
//! Forge's content store, and the Forge links each enabled skill into every
//! engine's home. Skills an engine already had on its own are reported as
//! discovered, never touched until the user adopts them. The Editor renders
//! the catalog and sends enable and adopt requests; it never decides what an
//! engine holds.

use thiserror::Error;

use crate::identifiers::RequestId;

/// Maximum extensions in one catalog.
pub const EXTENSION_CATALOG_MAX: usize = 512;
/// Maximum bytes of one extension identifier.
pub const EXTENSION_ID_MAX_BYTES: usize = 160;
/// Maximum bytes of one skill name (the Agent Skills specification limit).
pub const EXTENSION_NAME_MAX_BYTES: usize = 64;
/// Maximum bytes of one skill description (the Agent Skills specification
/// limit).
pub const EXTENSION_DESCRIPTION_MAX_BYTES: usize = 1024;
/// Maximum bytes of one presentation-ready reason.
pub const EXTENSION_REASON_MAX_BYTES: usize = 512;
/// Maximum engines reported for one extension.
pub const EXTENSION_ENGINES_MAX: usize = 16;
/// Bytes of a lowercase hexadecimal SHA-256 content digest.
pub const EXTENSION_DIGEST_BYTES: usize = 64;

/// What kind of extension an entry is.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExtensionKind {
    /// An Agent Skills folder with a `SKILL.md`.
    Skill,
}

impl ExtensionKind {
    /// Returns the stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Skill => "skill",
        }
    }
}

/// Where an extension comes from.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExtensionOrigin {
    /// Ships with Artisan.
    FirstParty,
    /// Adopted by the user into the Forge's content store.
    Adopted,
    /// Found in an engine's home but not managed by Artisan.
    Discovered,
}

impl ExtensionOrigin {
    /// Returns the stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FirstParty => "first_party",
            Self::Adopted => "adopted",
            Self::Discovered => "discovered",
        }
    }
}

/// Where one extension stands in one engine.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExtensionEngineStatus {
    /// Artisan's copy is in the engine's home.
    Synced,
    /// The extension is turned off, so Artisan keeps it out of the engine.
    Off,
    /// The engine is not installed yet; the extension follows once it is.
    EngineAbsent,
    /// The engine cannot run on this Forge yet.
    Unavailable,
    /// The engine has its own, different skill under the same name; Artisan
    /// leaves it in place.
    Conflict,
    /// A discovered skill the engine has on its own.
    Native,
    /// A discovered skill this engine does not have.
    NotPresent,
    /// The operating system refuses the links Artisan places skills with.
    LinksUnavailable,
    /// Placing the extension failed; `reason` says why.
    Failed,
}

impl ExtensionEngineStatus {
    /// Returns the stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Synced => "synced",
            Self::Off => "off",
            Self::EngineAbsent => "engine_absent",
            Self::Unavailable => "unavailable",
            Self::Conflict => "conflict",
            Self::Native => "native",
            Self::NotPresent => "not_present",
            Self::LinksUnavailable => "links_unavailable",
            Self::Failed => "failed",
        }
    }
}

/// One extension's state in one engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ExtensionEngineState {
    /// Engine identifier (`claude`, `codex`, …).
    pub engine_id: String,
    /// Where the extension stands there.
    pub status: ExtensionEngineStatus,
    /// Presentation-ready reason for `Unavailable`, `Conflict`, and `Failed`.
    pub reason: Option<String>,
}

/// One extension and its state in every engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Extension {
    /// Stable identifier (`artisan.<name>`, `local.<name>`, or
    /// `discovered.<name>.<digest prefix>`).
    pub id: String,
    /// Kind.
    pub kind: ExtensionKind,
    /// Origin.
    pub origin: ExtensionOrigin,
    /// The skill's name as engines know it.
    pub name: String,
    /// The skill's description, which engines use to decide when to load it.
    pub description: String,
    /// Lowercase hexadecimal SHA-256 of the skill's files.
    pub digest: String,
    /// Number of files in the skill.
    pub file_count: u32,
    /// Total bytes of the skill's files.
    pub byte_count: u64,
    /// Whether Artisan keeps it in the engines; always `false` for a
    /// discovered skill.
    pub enabled: bool,
    /// Its state per engine, in Forge engine order.
    pub engines: Vec<ExtensionEngineState>,
}

impl Extension {
    /// Validates bounds and field consistency.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError`] for an invalid identifier, name,
    /// description, digest, or engine state.
    pub fn validate(&self) -> Result<(), ExtensionError> {
        validate_extension_id(&self.id)?;
        if !valid_skill_name(&self.name) {
            return Err(ExtensionError::Name);
        }
        if self.description.len() > EXTENSION_DESCRIPTION_MAX_BYTES
            || self
                .description
                .chars()
                .any(|character| character.is_control() && character != '\n')
        {
            return Err(ExtensionError::Description);
        }
        if !valid_digest(&self.digest) {
            return Err(ExtensionError::Digest);
        }
        if self.origin == ExtensionOrigin::Discovered && self.enabled {
            return Err(ExtensionError::Inconsistent);
        }
        if self.engines.len() > EXTENSION_ENGINES_MAX {
            return Err(ExtensionError::TooMany);
        }
        let mut seen = std::collections::HashSet::with_capacity(self.engines.len());
        for state in &self.engines {
            crate::engine_install::validate_engine_id(&state.engine_id)
                .map_err(|_| ExtensionError::EngineId)?;
            if !seen.insert(state.engine_id.as_str()) {
                return Err(ExtensionError::Duplicate);
            }
            if let Some(reason) = &state.reason {
                validate_reason(reason)?;
            }
        }
        Ok(())
    }

    /// Returns this extension's state in one engine.
    #[must_use]
    pub fn engine(&self, engine_id: &str) -> Option<&ExtensionEngineState> {
        self.engines
            .iter()
            .find(|state| state.engine_id == engine_id)
    }
}

/// Whether the Forge's operating system lets Artisan place skills with
/// symbolic links.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ExtensionLinkSupport {
    /// Links work.
    #[default]
    Available,
    /// Windows refuses unprivileged symbolic links: Developer Mode is off.
    DeveloperModeRequired,
}

/// Every extension the Forge manages or discovered, and what each engine
/// last reported it can use.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct ExtensionCatalog {
    extensions: Vec<Extension>,
    links: ExtensionLinkSupport,
    inventories: Vec<EngineInventory>,
}

impl ExtensionCatalog {
    /// Creates a validated catalog.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError`] for too many or repeated extensions, or an
    /// invalid extension.
    pub fn new(
        extensions: Vec<Extension>,
        links: ExtensionLinkSupport,
    ) -> Result<Self, ExtensionError> {
        if extensions.len() > EXTENSION_CATALOG_MAX {
            return Err(ExtensionError::TooMany);
        }
        let mut seen = std::collections::HashSet::with_capacity(extensions.len());
        for extension in &extensions {
            extension.validate()?;
            if !seen.insert(extension.id.as_str()) {
                return Err(ExtensionError::Duplicate);
            }
        }
        Ok(Self {
            extensions,
            links,
            inventories: Vec::new(),
        })
    }

    /// Adds what the engines reported they can use.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError`] for too many inventories, an engine and
    /// project reported twice, or an invalid inventory.
    pub fn with_inventories(
        mut self,
        inventories: Vec<EngineInventory>,
    ) -> Result<Self, ExtensionError> {
        if inventories.len() > INVENTORIES_MAX {
            return Err(ExtensionError::TooMany);
        }
        let mut seen = std::collections::HashSet::with_capacity(inventories.len());
        for inventory in &inventories {
            inventory.validate()?;
            if !seen.insert((
                inventory.engine_id.as_str(),
                inventory.project_id.as_deref(),
            )) {
                return Err(ExtensionError::Duplicate);
            }
        }
        self.inventories = inventories;
        Ok(self)
    }

    /// What each engine last reported it can use, per project.
    #[must_use]
    pub fn inventories(&self) -> &[EngineInventory] {
        &self.inventories
    }

    /// What `engine_id` last reported for `project_id`, else for no
    /// project.
    #[must_use]
    pub fn inventory(&self, engine_id: &str, project_id: Option<&str>) -> Option<&EngineInventory> {
        let find = |project: Option<&str>| {
            self.inventories.iter().find(|inventory| {
                inventory.engine_id == engine_id && inventory.project_id.as_deref() == project
            })
        };
        project_id
            .and_then(|project| find(Some(project)))
            .or_else(|| find(None))
    }

    /// Returns the extensions: managed first, then discovered, each by name.
    #[must_use]
    pub fn extensions(&self) -> &[Extension] {
        &self.extensions
    }

    /// Returns one extension.
    #[must_use]
    pub fn extension(&self, id: &str) -> Option<&Extension> {
        self.extensions.iter().find(|extension| extension.id == id)
    }

    /// Returns whether skills can be linked into engines.
    #[must_use]
    pub const fn links(&self) -> ExtensionLinkSupport {
        self.links
    }
}

/// Maximum engine inventories in one catalog.
pub const INVENTORIES_MAX: usize = 64;
/// Maximum skills or connectors in one engine inventory.
pub const INVENTORY_ITEMS_MAX: usize = 512;
/// Maximum bytes of one reported skill or connector name.
pub const INVENTORY_NAME_MAX_BYTES: usize = 128;

/// Where an engine found a skill it reported.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InventorySkillSource {
    /// The user's own skills folder in the engine's home.
    User,
    /// The project's skills folder.
    Project,
    /// A plugin the engine has installed.
    Plugin,
    /// Shipped with the engine.
    System,
}

/// One skill an engine reported it can load.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InventorySkill {
    /// The name the engine invokes it by (`plugin:skill` for plugin skills).
    pub name: String,
    /// What it does; empty when unknown.
    pub description: String,
    /// Where the engine found it.
    pub source: InventorySkillSource,
    /// The plugin that provides it, for [`InventorySkillSource::Plugin`].
    pub plugin: Option<String>,
}

/// How a connector stood when its engine last reported it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConnectorStatus {
    /// Connected; its tools are available.
    Connected,
    /// Starting or not yet started.
    Pending,
    /// The user must sign in before it can connect.
    NeedsAuth,
    /// It failed to start or connect.
    Failed,
    /// Turned off in the engine.
    Disabled,
}

/// One MCP server an engine reported.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InventoryConnector {
    /// The server's name as the engine knows it.
    pub name: String,
    /// How it stood.
    pub status: ConnectorStatus,
    /// The plugin that provides it, if any.
    pub plugin: Option<String>,
}

/// What one engine reported it can use while running in one project.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineInventory {
    /// Engine identifier.
    pub engine_id: String,
    /// The project the engine ran in; `None` when it ran in none.
    pub project_id: Option<String>,
    /// Skills, by name.
    pub skills: Vec<InventorySkill>,
    /// Connectors, by name.
    pub connectors: Vec<InventoryConnector>,
}

impl EngineInventory {
    /// Validates bounds and names.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError`] for an invalid engine, too many items, or
    /// an invalid name or description.
    pub fn validate(&self) -> Result<(), ExtensionError> {
        crate::engine_install::validate_engine_id(&self.engine_id)
            .map_err(|_| ExtensionError::EngineId)?;
        if self
            .project_id
            .as_ref()
            .is_some_and(|project| project.is_empty() || project.len() > EXTENSION_ID_MAX_BYTES)
        {
            return Err(ExtensionError::Id);
        }
        if self.skills.len() > INVENTORY_ITEMS_MAX || self.connectors.len() > INVENTORY_ITEMS_MAX {
            return Err(ExtensionError::TooMany);
        }
        for skill in &self.skills {
            if !valid_inventory_name(&skill.name)
                || skill
                    .plugin
                    .as_deref()
                    .is_some_and(|plugin| !valid_inventory_name(plugin))
            {
                return Err(ExtensionError::Name);
            }
            if skill.description.len() > EXTENSION_DESCRIPTION_MAX_BYTES
                || skill
                    .description
                    .chars()
                    .any(|character| character.is_control() && character != '\n')
            {
                return Err(ExtensionError::Description);
            }
        }
        for connector in &self.connectors {
            if !valid_inventory_name(&connector.name)
                || connector
                    .plugin
                    .as_deref()
                    .is_some_and(|plugin| !valid_inventory_name(plugin))
            {
                return Err(ExtensionError::Name);
            }
        }
        Ok(())
    }
}

/// Whether `name` can name a reported skill, connector, or plugin: one
/// line, at most [`INVENTORY_NAME_MAX_BYTES`], without surrounding
/// whitespace.
#[must_use]
pub fn valid_inventory_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= INVENTORY_NAME_MAX_BYTES
        && name.trim() == name
        && !name.chars().any(char::is_control)
}

/// Reads the extension catalog. The connection that read it receives later
/// changes pushed as [`Event::Extensions`](crate::Event::Extensions).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ReadExtensions;

/// Turns a managed extension on or off in every engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SetExtensionEnabled {
    /// Client-minted request identity.
    pub request_id: RequestId,
    /// The managed extension.
    pub extension_id: String,
    /// Whether engines get it.
    pub enabled: bool,
}

/// Adopts a discovered skill: the Forge copies it into its content store,
/// backs up and replaces each engine's own copy with Artisan's, and places
/// it in every engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AdoptExtension {
    /// Client-minted request identity.
    pub request_id: RequestId,
    /// The discovered extension.
    pub extension_id: String,
}

/// Validates an extension identifier from the wire.
///
/// # Errors
///
/// Returns [`ExtensionError::Id`] for an invalid identifier.
pub fn validate_extension_id(id: &str) -> Result<(), ExtensionError> {
    (!id.is_empty()
        && id.len() <= EXTENSION_ID_MAX_BYTES
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_".contains(&byte)
        }))
    .then_some(())
    .ok_or(ExtensionError::Id)
}

/// Whether `name` is a valid Agent Skills name: 1–64 lowercase letters,
/// digits, and single hyphens, neither leading nor trailing.
#[must_use]
pub fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= EXTENSION_NAME_MAX_BYTES
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == EXTENSION_DIGEST_BYTES
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_reason(reason: &str) -> Result<(), ExtensionError> {
    if reason.trim().is_empty()
        || reason.len() > EXTENSION_REASON_MAX_BYTES
        || reason.chars().any(char::is_control)
    {
        return Err(ExtensionError::Reason);
    }
    Ok(())
}

/// Invalid extension data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ExtensionError {
    #[error("extension identifier is invalid")]
    Id,
    #[error("skill name is invalid")]
    Name,
    #[error("skill description is invalid")]
    Description,
    #[error("extension digest is invalid")]
    Digest,
    #[error("engine identifier is invalid")]
    EngineId,
    #[error("extension reason is invalid")]
    Reason,
    #[error("a discovered extension cannot be enabled")]
    Inconsistent,
    #[error("too many extension entries")]
    TooMany,
    #[error("an extension or engine appears twice")]
    Duplicate,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extension(id: &str, origin: ExtensionOrigin, enabled: bool) -> Extension {
        Extension {
            id: id.to_owned(),
            kind: ExtensionKind::Skill,
            origin,
            name: "artisan-editor".to_owned(),
            description: "Write replies that render richly.".to_owned(),
            digest: "a".repeat(EXTENSION_DIGEST_BYTES),
            file_count: 2,
            byte_count: 4096,
            enabled,
            engines: vec![
                ExtensionEngineState {
                    engine_id: "claude".to_owned(),
                    status: ExtensionEngineStatus::Synced,
                    reason: None,
                },
                ExtensionEngineState {
                    engine_id: "cursor".to_owned(),
                    status: ExtensionEngineStatus::Unavailable,
                    reason: Some("Cursor runs are not available yet.".to_owned()),
                },
            ],
        }
    }

    #[test]
    fn catalogs_validate_entries_and_reject_duplicates() {
        let catalog = ExtensionCatalog::new(
            vec![
                extension("artisan.artisan-editor", ExtensionOrigin::FirstParty, true),
                extension(
                    "discovered.artisan-editor.aaaaaaaa",
                    ExtensionOrigin::Discovered,
                    false,
                ),
            ],
            ExtensionLinkSupport::Available,
        )
        .unwrap();
        assert_eq!(
            catalog
                .extension("artisan.artisan-editor")
                .and_then(|entry| entry.engine("claude"))
                .map(|state| state.status),
            Some(ExtensionEngineStatus::Synced)
        );
        let repeated = extension("artisan.artisan-editor", ExtensionOrigin::FirstParty, true);
        assert_eq!(
            ExtensionCatalog::new(
                vec![repeated.clone(), repeated],
                ExtensionLinkSupport::Available
            ),
            Err(ExtensionError::Duplicate)
        );
    }

    #[test]
    fn entries_reject_invalid_fields() {
        let mut bad = extension("artisan.artisan-editor", ExtensionOrigin::FirstParty, true);
        bad.name = "Artisan Editor".to_owned();
        assert_eq!(bad.validate(), Err(ExtensionError::Name));
        let mut bad = extension("artisan.artisan-editor", ExtensionOrigin::FirstParty, true);
        bad.digest = "xyz".to_owned();
        assert_eq!(bad.validate(), Err(ExtensionError::Digest));
        let bad = extension("discovered.x", ExtensionOrigin::Discovered, true);
        assert_eq!(bad.validate(), Err(ExtensionError::Inconsistent));
        let mut bad = extension("artisan.artisan-editor", ExtensionOrigin::FirstParty, true);
        bad.engines[1].reason = Some(" ".to_owned());
        assert_eq!(bad.validate(), Err(ExtensionError::Reason));
        assert_eq!(
            validate_extension_id("Artisan.Editor"),
            Err(ExtensionError::Id)
        );
    }

    #[test]
    fn skill_names_follow_the_agent_skills_rules() {
        assert!(valid_skill_name("artisan-editor"));
        assert!(valid_skill_name("pdf2"));
        for invalid in [
            "",
            "-lead",
            "trail-",
            "double--hyphen",
            "Upper",
            "under_score",
        ] {
            assert!(!valid_skill_name(invalid), "{invalid}");
        }
        assert!(!valid_skill_name(&"a".repeat(EXTENSION_NAME_MAX_BYTES + 1)));
    }

    #[test]
    fn inventories_are_bounded_and_keyed_by_engine_and_project() {
        let inventory = |project: Option<&str>, name: &str| EngineInventory {
            engine_id: "claude".to_owned(),
            project_id: project.map(str::to_owned),
            skills: vec![InventorySkill {
                name: name.to_owned(),
                description: String::new(),
                source: InventorySkillSource::Plugin,
                plugin: Some("cloudflare".to_owned()),
            }],
            connectors: vec![InventoryConnector {
                name: "claude.ai Notion".to_owned(),
                status: ConnectorStatus::NeedsAuth,
                plugin: None,
            }],
        };
        let catalog = ExtensionCatalog::default()
            .with_inventories(vec![
                inventory(Some("p1"), "cloudflare:wrangler"),
                inventory(None, "cloudflare:wrangler"),
            ])
            .expect("valid inventories");
        assert_eq!(
            catalog
                .inventory("claude", Some("p1"))
                .and_then(|found| found.project_id.as_deref()),
            Some("p1")
        );
        assert_eq!(
            catalog
                .inventory("claude", Some("p2"))
                .map(|found| found.project_id.clone()),
            Some(None),
            "an unknown project falls back to the engine's projectless report"
        );
        assert_eq!(
            ExtensionCatalog::default()
                .with_inventories(vec![inventory(None, "a"), inventory(None, "b")]),
            Err(ExtensionError::Duplicate)
        );
        assert_eq!(
            ExtensionCatalog::default().with_inventories(vec![inventory(None, " padded")]),
            Err(ExtensionError::Name)
        );
    }
}
