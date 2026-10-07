//! What the Marketplace shows, derived from Forge data.
//!
//! Pure presentation: the Forge decides what every engine holds and reports
//! it; this module groups the catalog into sections, names each engine, and
//! turns each per-engine status into a short label, a tone, and an icon.

use artisan_assets::AssetId;
use artisan_domain::{
    Extension, ExtensionCatalog, ExtensionEngineState, ExtensionEngineStatus, ExtensionOrigin,
    GlobalPrompt, GlobalPromptEngineState, GlobalPromptEngineStatus, GlobalPromptSetup,
};

/// The colour family a status chip is painted in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChipTone {
    /// In place and working.
    Success,
    /// Present but not managed by Artisan.
    Info,
    /// Needs the user's attention.
    Warning,
    /// Failed.
    Danger,
    /// Nothing to act on.
    Muted,
}

/// One engine's chip on a row.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineChip {
    /// Engine identifier.
    pub engine_id: String,
    /// Short engine name.
    pub engine: &'static str,
    /// Status label.
    pub label: &'static str,
    /// Tone.
    pub tone: ChipTone,
    /// Icon.
    pub icon: AssetId,
    /// Hover text: the Forge's reason, or the engine and the label.
    pub detail: String,
}

/// One extension row.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ExtensionRow {
    pub id: String,
    pub name: String,
    pub description: String,
    /// `"2 files · 41 KB"`.
    pub size: String,
    pub enabled: bool,
    pub origin: ExtensionOrigin,
    pub chips: Vec<EngineChip>,
    /// The first reason that needs the user's attention, shown under the
    /// row.
    pub attention: Option<String>,
    /// For a discovered skill: where it was found, `"Found in Claude Code
    /// and Codex"`.
    pub found_in: Option<String>,
}

/// The Marketplace sections in order.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct MarketplaceSections {
    /// Artisan's own skills.
    pub artisan: Vec<ExtensionRow>,
    /// Skills the user adopted.
    pub adopted: Vec<ExtensionRow>,
    /// Skills engines have on their own.
    pub discovered: Vec<ExtensionRow>,
}

/// Groups the catalog into sections.
#[must_use]
pub fn sections(catalog: &ExtensionCatalog) -> MarketplaceSections {
    let mut sections = MarketplaceSections::default();
    for extension in catalog.extensions() {
        let row = extension_row(extension);
        match extension.origin {
            ExtensionOrigin::FirstParty => sections.artisan.push(row),
            ExtensionOrigin::Adopted => sections.adopted.push(row),
            ExtensionOrigin::Discovered => sections.discovered.push(row),
        }
    }
    sections
}

fn extension_row(extension: &Extension) -> ExtensionRow {
    let chips: Vec<EngineChip> = extension.engines.iter().map(extension_chip).collect();
    let attention = extension
        .engines
        .iter()
        .filter(|state| {
            matches!(
                state.status,
                ExtensionEngineStatus::Conflict | ExtensionEngineStatus::Failed
            )
        })
        .find_map(|state| state.reason.clone());
    let found_in = (extension.origin == ExtensionOrigin::Discovered).then(|| {
        let engines: Vec<&str> = extension
            .engines
            .iter()
            .filter(|state| state.status == ExtensionEngineStatus::Native)
            .map(|state| engine_name(&state.engine_id))
            .collect();
        format!("Found in {}", join_names(&engines))
    });
    ExtensionRow {
        id: extension.id.clone(),
        name: extension.name.clone(),
        description: extension.description.clone(),
        size: size_label(extension.file_count, extension.byte_count),
        enabled: extension.enabled,
        origin: extension.origin,
        chips,
        attention,
        found_in,
    }
}

fn extension_chip(state: &ExtensionEngineState) -> EngineChip {
    let (label, tone, icon) = match state.status {
        ExtensionEngineStatus::Synced => ("Synced", ChipTone::Success, AssetId::TABLER_CHECK),
        ExtensionEngineStatus::Off => ("Off", ChipTone::Muted, AssetId::TABLER_MINUS),
        ExtensionEngineStatus::EngineAbsent => {
            ("Not installed", ChipTone::Muted, AssetId::TABLER_MINUS)
        }
        ExtensionEngineStatus::Unavailable => {
            ("Unavailable", ChipTone::Muted, AssetId::TABLER_MINUS)
        }
        ExtensionEngineStatus::Conflict => (
            "Conflict",
            ChipTone::Warning,
            AssetId::TABLER_ALERT_TRIANGLE,
        ),
        ExtensionEngineStatus::Native => ("Has it", ChipTone::Info, AssetId::TABLER_CHECK),
        ExtensionEngineStatus::NotPresent => ("Missing", ChipTone::Muted, AssetId::TABLER_MINUS),
        ExtensionEngineStatus::LinksUnavailable => (
            "Needs Developer Mode",
            ChipTone::Warning,
            AssetId::TABLER_ALERT_TRIANGLE,
        ),
        ExtensionEngineStatus::Failed => ("Failed", ChipTone::Danger, AssetId::TABLER_X),
    };
    chip(&state.engine_id, label, tone, icon, state.reason.as_deref())
}

fn chip(
    engine_id: &str,
    label: &'static str,
    tone: ChipTone,
    icon: AssetId,
    reason: Option<&str>,
) -> EngineChip {
    let engine = engine_name(engine_id);
    EngineChip {
        engine_id: engine_id.to_owned(),
        engine,
        label,
        tone,
        icon,
        detail: reason.map_or_else(|| format!("{engine}: {label}"), str::to_owned),
    }
}

/// The global prompt card's content.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PromptSummary {
    /// The first line of the prompt, or a note that it is empty.
    pub preview: String,
    /// Whether the prompt is empty.
    pub empty: bool,
    /// Chips per engine.
    pub chips: Vec<EngineChip>,
    /// What needs the user's attention: a setup choice or drifted engines.
    pub attention: Option<String>,
}

/// Summarizes the global prompt for the Marketplace card.
#[must_use]
pub fn prompt_summary(prompt: &GlobalPrompt) -> PromptSummary {
    let first_line = prompt
        .text()
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim().to_owned());
    let chips = prompt.engines().iter().map(prompt_chip).collect();
    let drifted: Vec<&str> = prompt
        .engines()
        .iter()
        .filter(|state| state.status == GlobalPromptEngineStatus::Drifted)
        .map(|state| engine_name(&state.engine_id))
        .collect();
    let attention = match prompt.setup() {
        GlobalPromptSetup::ChoiceRequired(_) => {
            Some("Your engines have different prompts. Choose the one Artisan keeps.".to_owned())
        }
        GlobalPromptSetup::Ready if !drifted.is_empty() => Some(format!(
            "Edited outside Artisan in {}.",
            join_names(&drifted)
        )),
        GlobalPromptSetup::Ready => None,
    };
    PromptSummary {
        empty: first_line.is_none(),
        preview: first_line.unwrap_or_else(|| "No global prompt yet.".to_owned()),
        chips,
        attention,
    }
}

/// One engine's chip for the global prompt.
#[must_use]
pub fn prompt_chip(state: &GlobalPromptEngineState) -> EngineChip {
    let (label, tone, icon) = match state.status {
        GlobalPromptEngineStatus::Synced => ("Synced", ChipTone::Success, AssetId::TABLER_CHECK),
        GlobalPromptEngineStatus::Drifted => {
            ("Edited", ChipTone::Warning, AssetId::TABLER_ALERT_TRIANGLE)
        }
        GlobalPromptEngineStatus::AwaitingChoice => {
            ("Waiting", ChipTone::Warning, AssetId::TABLER_ALERT_TRIANGLE)
        }
        GlobalPromptEngineStatus::EngineAbsent => {
            ("Not installed", ChipTone::Muted, AssetId::TABLER_MINUS)
        }
        GlobalPromptEngineStatus::Unavailable => {
            ("Unavailable", ChipTone::Muted, AssetId::TABLER_MINUS)
        }
        GlobalPromptEngineStatus::Failed => ("Failed", ChipTone::Danger, AssetId::TABLER_X),
    };
    let reason = state.reason.clone().or_else(|| {
        state
            .file
            .as_ref()
            .filter(|_| state.status == GlobalPromptEngineStatus::Synced)
            .map(|file| format!("{}: {file}", engine_name(&state.engine_id)))
    });
    chip(&state.engine_id, label, tone, icon, reason.as_deref())
}

/// The short product name of an engine.
#[must_use]
pub fn engine_name(engine_id: &str) -> &'static str {
    match engine_id {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "cursor" => "Cursor",
        "grok" => "Grok",
        "opencode2" => "OpenCode",
        _ => "Engine",
    }
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
#[must_use]
pub fn size_label(files: u32, bytes: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{ExtensionKind, ExtensionLinkSupport, GlobalPromptRevision};

    fn state(engine_id: &str, status: ExtensionEngineStatus) -> ExtensionEngineState {
        ExtensionEngineState {
            engine_id: engine_id.to_owned(),
            status,
            reason: (status == ExtensionEngineStatus::Conflict)
                .then(|| "Codex has its own skill named pdf.".to_owned()),
        }
    }

    fn extension(
        id: &str,
        origin: ExtensionOrigin,
        engines: Vec<ExtensionEngineState>,
    ) -> Extension {
        Extension {
            id: id.to_owned(),
            kind: ExtensionKind::Skill,
            origin,
            name: "pdf".to_owned(),
            description: "Reads PDF files.".to_owned(),
            digest: "a".repeat(64),
            file_count: 2,
            byte_count: 41_000,
            enabled: origin != ExtensionOrigin::Discovered,
            engines,
        }
    }

    #[test]
    fn catalogs_group_into_sections_with_reasons_and_origins() {
        let catalog = ExtensionCatalog::new(
            vec![
                extension(
                    "artisan.artisan-editor",
                    ExtensionOrigin::FirstParty,
                    vec![state("claude", ExtensionEngineStatus::Synced)],
                ),
                extension(
                    "local.pdf",
                    ExtensionOrigin::Adopted,
                    vec![
                        state("claude", ExtensionEngineStatus::Synced),
                        state("codex", ExtensionEngineStatus::Conflict),
                    ],
                ),
                extension(
                    "discovered.pdf.aaaaaaaaaaaa",
                    ExtensionOrigin::Discovered,
                    vec![
                        state("claude", ExtensionEngineStatus::Native),
                        state("codex", ExtensionEngineStatus::Native),
                        state("grok", ExtensionEngineStatus::NotPresent),
                    ],
                ),
            ],
            ExtensionLinkSupport::Available,
        )
        .unwrap();
        let sections = sections(&catalog);
        assert_eq!(sections.artisan.len(), 1);
        assert_eq!(
            sections.adopted[0].attention.as_deref(),
            Some("Codex has its own skill named pdf.")
        );
        assert_eq!(sections.adopted[0].chips[1].tone, ChipTone::Warning);
        assert_eq!(
            sections.discovered[0].found_in.as_deref(),
            Some("Found in Claude Code and Codex")
        );
        assert_eq!(sections.discovered[0].size, "2 files \u{b7} 41 KB");
    }

    #[test]
    fn prompt_summaries_name_drift_and_setup() {
        let engine = |engine_id: &str, status: GlobalPromptEngineStatus| GlobalPromptEngineState {
            engine_id: engine_id.to_owned(),
            status,
            file: Some(".codex/AGENTS.md".to_owned()),
            external_text: (status == GlobalPromptEngineStatus::Drifted).then(String::new),
            reason: None,
        };
        let prompt = GlobalPrompt::new(
            GlobalPromptRevision::new(2),
            "\n# Global instructions\nBe brief.".to_owned(),
            GlobalPromptSetup::Ready,
            vec![
                engine("claude", GlobalPromptEngineStatus::Synced),
                engine("codex", GlobalPromptEngineStatus::Drifted),
                engine("grok", GlobalPromptEngineStatus::Drifted),
            ],
        )
        .unwrap();
        let summary = prompt_summary(&prompt);
        assert_eq!(summary.preview, "Global instructions");
        assert!(!summary.empty);
        assert_eq!(
            summary.attention.as_deref(),
            Some("Edited outside Artisan in Codex and Grok.")
        );
        assert_eq!(summary.chips[0].detail, "Claude Code: .codex/AGENTS.md");
        let empty = prompt_summary(&GlobalPrompt::default());
        assert!(empty.empty);
        assert_eq!(empty.preview, "No global prompt yet.");
    }

    #[test]
    fn sizes_and_name_lists_read_naturally() {
        assert_eq!(size_label(1, 812), "1 file \u{b7} 812 B");
        assert_eq!(size_label(3, 3 * 1024 * 1024), "3 files \u{b7} 3 MB");
        assert_eq!(join_names(&["A", "B", "C"]), "A, B and C");
    }
}
