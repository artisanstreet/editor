//! Turns Forge's catalog and MCP servers into Marketplace rows, and ranks
//! them against the search.

use artisan_api::{
    CatalogSkill, EngineId, GlobalPrompt, McpServer, PromptEngineStatus, SkillEngineState,
    SkillEngineStatus, SkillOrigin,
};

/// One skill row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillRow {
    pub id: String,
    pub name: String,
    pub description: String,
    /// `"2 files · 41 KB"`.
    pub size: String,
    pub enabled: bool,
    pub origin: SkillOrigin,
    pub marks: Vec<EngineMark>,
    /// The first reason that needs the user's attention, under the row.
    pub attention: Option<String>,
    /// For a quarantined skill: `"Quarantined from Claude Code and Codex"`.
    pub found_in: Option<String>,
}

/// One engine's logo on a row: lit when the engine has the item, its
/// tooltip `"Claude Code · Synced"`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineMark {
    pub engine: EngineId,
    pub lit: bool,
    pub label: String,
}

/// One MCP server row. Every engine gets every server, so it has no marks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerRow {
    pub name: String,
    /// The url, or the command and its arguments.
    pub target: String,
}

/// A skill skills.sh lists that the host doesn't have.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListingRow {
    pub id: String,
    pub name: String,
    /// `"anthropics/skills · 1.0M installs · 8 other publishers"`.
    pub meta: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Item {
    Skill(SkillRow),
    Server(ServerRow),
    Listing(ListingRow),
}

impl Item {
    pub fn name(&self) -> &str {
        match self {
            Self::Skill(row) => &row.name,
            Self::Server(row) => &row.name,
            Self::Listing(row) => &row.name,
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Skill(row) => &row.description,
            Self::Server(row) => &row.target,
            Self::Listing(row) => &row.meta,
        }
    }

    pub const fn kind(&self) -> Kind {
        match self {
            Self::Skill(_) | Self::Listing(_) => Kind::Skills,
            Self::Server(_) => Kind::Servers,
        }
    }

    /// Discovered skills aren't Artisan's yet.
    pub fn managed(&self) -> bool {
        !matches!(self, Self::Skill(row) if row.origin == SkillOrigin::Discovered)
    }

    /// `marketplace-skill-<id>` or `marketplace-server-<name>`.
    pub fn selector(&self) -> String {
        match self {
            Self::Skill(row) => format!("marketplace-skill-{}", row.id),
            Self::Server(row) => format!("marketplace-server-{}", row.name),
            Self::Listing(row) => format!("marketplace-listing-{}", row.id),
        }
    }
}

/// The kind filter under the search.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Kind {
    #[default]
    All,
    Skills,
    Servers,
}

impl Kind {
    pub fn admits(self, item: &Item) -> bool {
        self == Self::All || item.kind() == self
    }
}

/// Every row the host has: Artisan's skills, the user's, MCP servers, then
/// skills the engines have on their own.
pub fn items(catalog: &[CatalogSkill], servers: &[McpServer]) -> Vec<Item> {
    let rank = |origin: SkillOrigin| match origin {
        SkillOrigin::FirstParty => 0,
        SkillOrigin::User => 1,
        SkillOrigin::Discovered => 3,
    };
    let mut items: Vec<(u8, Item)> = catalog
        .iter()
        .map(|skill| (rank(skill.origin), Item::Skill(skill_row(skill))))
        .collect();
    items.extend(servers.iter().map(|server| {
        let target = server.url.clone().unwrap_or_else(|| {
            std::iter::once(server.command.as_str())
                .chain(server.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        });
        (
            2,
            Item::Server(ServerRow {
                name: server.name.clone(),
                target,
            }),
        )
    }));
    items.sort_by_key(|(rank, _)| *rank);
    items.into_iter().map(|(_, item)| item).collect()
}

/// One listing per skill name, in the order skills.sh ranked the names:
/// the most-installed publisher's, with how many others publish that name.
pub fn by_name(listings: &[artisan_api::Listing]) -> Vec<(&artisan_api::Listing, usize)> {
    let mut named: Vec<(&artisan_api::Listing, usize)> = Vec::new();
    for listing in listings {
        match named.iter_mut().find(|(kept, _)| kept.name == listing.name) {
            Some((kept, others)) => {
                *others += 1;
                if listing.installs > kept.installs {
                    *kept = listing;
                }
            }
            None => named.push((listing, 0)),
        }
    }
    named
}

/// A skills.sh listing as a row.
pub fn listing_item(listing: &artisan_api::Listing, others: usize) -> Item {
    let others = match others {
        0 => String::new(),
        1 => " \u{b7} 1 other publisher".to_owned(),
        others => format!(" \u{b7} {others} other publishers"),
    };
    let meta = format!(
        "{} \u{b7} {}{others}",
        listing.publisher,
        installs_label(listing.installs)
    );
    Item::Listing(ListingRow {
        id: listing.id.clone(),
        name: listing.name.clone(),
        meta,
    })
}

/// `"812 installs"`, `"41K installs"`, `"1.0M installs"`.
#[expect(
    clippy::cast_precision_loss,
    reason = "the count is shown rounded to a tenth of a million"
)]
fn installs_label(installs: u64) -> String {
    let count = match installs {
        0..1_000 => installs.to_string(),
        1_000..1_000_000 => format!("{}K", installs / 1_000),
        _ => format!("{:.1}M", installs as f64 / 1_000_000.0),
    };
    if installs == 1 {
        "1 install".to_owned()
    } else {
        format!("{count} installs")
    }
}

/// The items matching `query`, best first: names that start with it, then
/// names that contain it, then descriptions that do.
pub fn search(items: &[Item], query: &str) -> Vec<Item> {
    let query = query.trim().to_lowercase();
    let mut hits: Vec<(u8, &Item)> = items
        .iter()
        .filter_map(|item| {
            let name = item.name().to_lowercase();
            let score = if name.starts_with(&query) {
                0
            } else if name.contains(&query) {
                1
            } else if item.detail().to_lowercase().contains(&query) {
                2
            } else {
                return None;
            };
            Some((score, item))
        })
        .collect();
    hits.sort_by_key(|(score, _)| *score);
    hits.into_iter().map(|(_, item)| item.clone()).collect()
}

fn skill_row(skill: &CatalogSkill) -> SkillRow {
    let attention = skill
        .engines
        .iter()
        .filter(|state| {
            matches!(
                state.status,
                SkillEngineStatus::Conflict | SkillEngineStatus::Failed
            )
        })
        .find_map(|state| state.reason.clone());
    let found_in = (skill.origin == SkillOrigin::Discovered).then(|| {
        let engines: Vec<&str> = skill
            .engines
            .iter()
            .filter(|state| state.status == SkillEngineStatus::Native)
            .map(|state| engine_name(state.engine))
            .collect();
        format!("Quarantined from {}", join_names(&engines))
    });
    SkillRow {
        id: skill.id.clone(),
        name: skill.name.clone(),
        description: skill.description.clone(),
        size: size_label(skill.file_count, skill.byte_count),
        enabled: skill.enabled,
        origin: skill.origin,
        marks: skill.engines.iter().map(engine_mark).collect(),
        attention,
        found_in,
    }
}

fn engine_mark(state: &SkillEngineState) -> EngineMark {
    let (lit, status) = match state.status {
        SkillEngineStatus::Synced => (true, "Synced"),
        SkillEngineStatus::Native => (true, "Quarantined from it"),
        SkillEngineStatus::Off => (false, "Off"),
        SkillEngineStatus::EngineAbsent => (false, "Not installed"),
        SkillEngineStatus::Unavailable => (false, "Unavailable"),
        SkillEngineStatus::Conflict => (false, "Conflict"),
        SkillEngineStatus::NotPresent => (false, "Didn't have it"),
        SkillEngineStatus::Failed => (false, "Failed"),
    };
    let mut label = format!("{} \u{b7} {status}", engine_name(state.engine));
    if let Some(reason) = &state.reason {
        label = format!("{label}\n{reason}");
    }
    EngineMark {
        engine: state.engine,
        lit,
        label,
    }
}

/// What needs the user about the global prompt: a setup choice, or engines
/// that edited it outside Artisan.
pub fn prompt_attention(prompt: &GlobalPrompt) -> Option<String> {
    let drifted: Vec<&str> = prompt
        .engines
        .iter()
        .filter(|state| state.status == PromptEngineStatus::Drifted)
        .map(|state| engine_name(state.engine))
        .collect();
    if !prompt.candidates.is_empty() {
        Some("Your engines have different global prompts. Choose the one Artisan keeps.".to_owned())
    } else if !drifted.is_empty() {
        Some(format!(
            "The global prompt was edited outside Artisan in {}.",
            join_names(&drifted)
        ))
    } else {
        None
    }
}

/// `"Quarantined 2 skills from Claude Code and Codex that Artisan didn't
/// install."`
pub fn found_summary(items: &[Item]) -> Option<String> {
    let mut engines: Vec<EngineId> = Vec::new();
    let mut count = 0;
    for item in items {
        if let Item::Skill(row) = item
            && row.origin == SkillOrigin::Discovered
        {
            count += 1;
            for mark in row.marks.iter().filter(|mark| mark.lit) {
                if !engines.contains(&mark.engine) {
                    engines.push(mark.engine);
                }
            }
        }
    }
    if count == 0 {
        return None;
    }
    engines.sort_by_key(|engine| engine.as_str());
    let names: Vec<&str> = engines.into_iter().map(engine_name).collect();
    Some(format!(
        "Quarantined {count} {} from {} that Artisan didn't install.",
        if count == 1 { "skill" } else { "skills" },
        join_names(&names),
    ))
}

/// An engine's product name, as the previous Editor wrote it.
pub const fn engine_name(engine: EngineId) -> &'static str {
    engine.display_name()
}

fn join_names(names: &[&str]) -> String {
    match names {
        [] => "no engine".to_owned(),
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `"1 file · 812 B"`, `"2 files · 41 KB"`.
fn size_label(files: u32, bytes: u64) -> String {
    let files = if files == 1 {
        "1 file".to_owned()
    } else {
        format!("{files} files")
    };
    let size = if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{} MB", bytes.div_ceil(1024 * 1024))
    };
    format!("{files} \u{b7} {size}")
}
