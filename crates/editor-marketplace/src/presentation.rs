//! What the Marketplace shows, derived from Forge data (master's
//! `marketplace_presentation`): the catalog grouped into sections, each
//! engine named, and each per-engine status turned into a chip.

use artisan_api::{
    CatalogSkill, EngineId, GlobalPrompt, PromptEngineState, PromptEngineStatus, SkillEngineState,
    SkillEngineStatus, SkillOrigin,
};
use editor_ui::AssetId;
use editor_ui::status_chip::{ChipTone, EngineChip};

/// One skill row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillRow {
    pub id: String,
    pub name: String,
    pub description: String,
    /// `"2 files · 41 KB"`.
    pub size: String,
    pub enabled: bool,
    pub origin: SkillOrigin,
    pub chips: Vec<EngineChip>,
    /// The first reason that needs the user's attention, under the row.
    pub attention: Option<String>,
    /// For a discovered skill: `"Found in Claude Code and Codex"`.
    pub found_in: Option<String>,
}

/// The Marketplace sections in order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Sections {
    /// Artisan's own skills.
    pub artisan: Vec<SkillRow>,
    /// The user's skills.
    pub adopted: Vec<SkillRow>,
    /// Skills engines have on their own.
    pub discovered: Vec<SkillRow>,
}

pub(crate) fn sections(catalog: &[CatalogSkill]) -> Sections {
    let mut sections = Sections::default();
    for skill in catalog {
        let row = skill_row(skill);
        match skill.origin {
            SkillOrigin::FirstParty => sections.artisan.push(row),
            SkillOrigin::User => sections.adopted.push(row),
            SkillOrigin::Discovered => sections.discovered.push(row),
        }
    }
    sections
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
        format!("Found in {}", join_names(&engines))
    });
    SkillRow {
        id: skill.id.clone(),
        name: skill.name.clone(),
        description: skill.description.clone(),
        size: size_label(skill.file_count, skill.byte_count),
        enabled: skill.enabled,
        origin: skill.origin,
        chips: skill.engines.iter().map(skill_chip).collect(),
        attention,
        found_in,
    }
}

fn skill_chip(state: &SkillEngineState) -> EngineChip {
    let (label, tone, icon) = match state.status {
        SkillEngineStatus::Synced => ("Synced", ChipTone::Success, AssetId::TABLER_CHECK),
        SkillEngineStatus::Off => ("Off", ChipTone::Muted, AssetId::TABLER_MINUS),
        SkillEngineStatus::EngineAbsent => {
            ("Not installed", ChipTone::Muted, AssetId::TABLER_MINUS)
        }
        SkillEngineStatus::Unavailable => ("Unavailable", ChipTone::Muted, AssetId::TABLER_MINUS),
        SkillEngineStatus::Conflict => (
            "Conflict",
            ChipTone::Warning,
            AssetId::TABLER_ALERT_TRIANGLE,
        ),
        SkillEngineStatus::Native => ("Has it", ChipTone::Info, AssetId::TABLER_CHECK),
        SkillEngineStatus::NotPresent => ("Missing", ChipTone::Muted, AssetId::TABLER_MINUS),
        SkillEngineStatus::Failed => ("Failed", ChipTone::Danger, AssetId::TABLER_X),
    };
    EngineChip::new(
        state.engine.as_str(),
        engine_name(state.engine),
        label,
        tone,
        icon,
        state.reason.as_deref(),
    )
}

/// The global prompt card's content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PromptSummary {
    /// The first line of the prompt, or a note that it is empty.
    pub preview: String,
    /// Whether the prompt is empty.
    pub empty: bool,
    pub chips: Vec<EngineChip>,
    /// A setup choice or engines edited outside Artisan.
    pub attention: Option<String>,
}

/// Summarizes the global prompt: its first non-empty line, heading marks
/// dropped, and what needs the user.
pub(crate) fn prompt_summary(prompt: &GlobalPrompt) -> PromptSummary {
    let first_line = prompt
        .text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim().to_owned());
    let drifted: Vec<&str> = prompt
        .engines
        .iter()
        .filter(|state| state.status == PromptEngineStatus::Drifted)
        .map(|state| engine_name(state.engine))
        .collect();
    let attention = if !prompt.candidates.is_empty() {
        Some("Your engines have different prompts. Choose the one Artisan keeps.".to_owned())
    } else if !drifted.is_empty() {
        Some(format!(
            "Edited outside Artisan in {}.",
            join_names(&drifted)
        ))
    } else {
        None
    };
    PromptSummary {
        empty: first_line.is_none(),
        preview: first_line.unwrap_or_else(|| "No global prompt yet.".to_owned()),
        chips: prompt.engines.iter().map(prompt_chip).collect(),
        attention,
    }
}

fn prompt_chip(state: &PromptEngineState) -> EngineChip {
    let (label, tone, icon) = match state.status {
        PromptEngineStatus::Synced => ("Synced", ChipTone::Success, AssetId::TABLER_CHECK),
        PromptEngineStatus::Drifted => {
            ("Edited", ChipTone::Warning, AssetId::TABLER_ALERT_TRIANGLE)
        }
        PromptEngineStatus::AwaitingChoice => {
            ("Waiting", ChipTone::Warning, AssetId::TABLER_ALERT_TRIANGLE)
        }
        PromptEngineStatus::EngineAbsent => {
            ("Not installed", ChipTone::Muted, AssetId::TABLER_MINUS)
        }
        PromptEngineStatus::Unavailable => ("Unavailable", ChipTone::Muted, AssetId::TABLER_MINUS),
        PromptEngineStatus::Failed => ("Failed", ChipTone::Danger, AssetId::TABLER_X),
    };
    let reason = state.reason.clone().or_else(|| {
        state
            .file
            .as_ref()
            .filter(|_| state.status == PromptEngineStatus::Synced)
            .map(|file| format!("{}: {file}", engine_name(state.engine)))
    });
    EngineChip::new(
        state.engine.as_str(),
        engine_name(state.engine),
        label,
        tone,
        icon,
        reason.as_deref(),
    )
}

/// An engine's product name, as the previous Editor wrote it.
pub(crate) const fn engine_name(engine: EngineId) -> &'static str {
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
