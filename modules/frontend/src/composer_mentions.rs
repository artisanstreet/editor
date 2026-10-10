//! Skills, connectors, and commands named inline in the composer.
//!
//! A mention is plain authored text: a sigil followed by a name the catalog
//! knows (`$skill`, `@connector`, `/goal`). The draft stores nothing else, so
//! recall, the queue, and the Forge see exactly what was typed. The composer
//! recognizes mentions against the catalog to paint them and to edit each as
//! one unit; the Forge recognizes them again to hand every engine its native
//! form. Text that only looks like a mention (`$5`, `@someone`, `/usr`) stays
//! ordinary text because no catalog entry has that name.

#![forbid(unsafe_code)]

use std::ops::Range;

use artisan_assets::AssetId;

/// Longest query a trigger follows; anything longer is ordinary text.
const TRIGGER_QUERY_MAX_CHARS: usize = 64;

/// Characters that end a mention name besides whitespace.
const NAME_TERMINATORS: &[char] = &[',', '.', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

/// What a mention names.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum MentionKind {
    /// An Agent Skill the engine can load.
    Skill,
    /// An MCP server the engine can call.
    Connector,
    /// A command the composer sends as text (`/goal`).
    Command,
}

impl MentionKind {
    /// The character that introduces this kind in the draft.
    #[must_use]
    pub(crate) const fn sigil(self) -> char {
        match self {
            Self::Skill => '$',
            Self::Connector => '@',
            Self::Command => '/',
        }
    }

    /// The kind a typed sigil asks for first.
    #[must_use]
    pub(crate) const fn for_sigil(sigil: char) -> Option<Self> {
        match sigil {
            '$' => Some(Self::Skill),
            '@' => Some(Self::Connector),
            '/' => Some(Self::Command),
            _ => None,
        }
    }

    /// The glyph painted in place of the sigil and beside menu rows.
    #[must_use]
    pub(crate) const fn icon(self) -> AssetId {
        match self {
            Self::Skill => AssetId::TABLER_FILE_SPARK,
            Self::Connector => AssetId::TABLER_PLUG_CONNECTED,
            Self::Command => AssetId::TABLER_TARGET_ARROW,
        }
    }
}

/// One skill or connector the composer can name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MentionTarget {
    /// The name exactly as engines know it; inserted after the sigil.
    pub(crate) name: String,
    /// What it does, or how a connector stands, shown muted beside the
    /// name.
    pub(crate) description: String,
}

/// A command the draft carries as text, recognized only at its start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextCommand {
    /// The command after its `/`.
    pub(crate) name: &'static str,
    /// Its glyph.
    pub(crate) icon: AssetId,
}

/// `/goal <objective>`: every goal-capable engine reads it from the message.
pub(crate) const GOAL_COMMAND: TextCommand = TextCommand {
    name: "goal",
    icon: AssetId::TABLER_TARGET_ARROW,
};

/// Everything the composer can name for the current engine and project.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MentionCatalog {
    skills: Vec<MentionTarget>,
    connectors: Vec<MentionTarget>,
    commands: Vec<TextCommand>,
}

impl MentionCatalog {
    /// Builds a catalog; each list is deduplicated by name and sorted for
    /// display.
    #[must_use]
    pub(crate) fn new(
        skills: Vec<MentionTarget>,
        connectors: Vec<MentionTarget>,
        commands: Vec<TextCommand>,
    ) -> Self {
        Self {
            skills: normalized(skills),
            connectors: normalized(connectors),
            commands,
        }
    }

    /// Skills, by name.
    #[must_use]
    pub(crate) fn skills(&self) -> &[MentionTarget] {
        &self.skills
    }

    /// Connectors, by name.
    #[must_use]
    pub(crate) fn connectors(&self) -> &[MentionTarget] {
        &self.connectors
    }

    fn targets(&self, kind: MentionKind) -> &[MentionTarget] {
        match kind {
            MentionKind::Skill => &self.skills,
            MentionKind::Connector => &self.connectors,
            MentionKind::Command => &[],
        }
    }
}

fn normalized(mut targets: Vec<MentionTarget>) -> Vec<MentionTarget> {
    targets.retain(|target| valid_name(&target.name));
    targets.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    targets.dedup_by(|later, earlier| later.name == earlier.name);
    targets
}

/// A name a mention can carry: non-empty, one line, and not opening with
/// whitespace, so the text after the sigil always reads as the name.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with(char::is_whitespace)
        && !name.ends_with(char::is_whitespace)
        && !name.contains(['\n', '\r'])
}

/// One recognized mention in the draft.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MentionSpan {
    /// Byte range of the sigil and name.
    pub(crate) range: Range<usize>,
    /// What it names.
    pub(crate) kind: MentionKind,
    /// The glyph painted over its sigil.
    pub(crate) icon: AssetId,
}

/// Finds every mention `text` holds, in order and never overlapping.
///
/// A sigil counts only at the start of the text or after whitespace, and
/// only when a known name follows it up to a word end; the longest known
/// name wins, so `@claude.ai Notion` is one connector even beside `@claude`.
/// A command counts only at the very start, where engines read it.
#[must_use]
pub(crate) fn recognize_mentions(text: &str, catalog: &MentionCatalog) -> Vec<MentionSpan> {
    let mut spans = Vec::new();
    let mut resume = 0;
    for (index, character) in text.char_indices() {
        if index < resume || !at_token_start(text, index) {
            continue;
        }
        let Some(kind) = MentionKind::for_sigil(character) else {
            continue;
        };
        let rest = &text[index + character.len_utf8()..];
        let matched = match kind {
            MentionKind::Command => (index == 0)
                .then(|| {
                    catalog
                        .commands
                        .iter()
                        .find(|command| name_matches(rest, command.name))
                        .map(|command| (command.name.len(), command.icon))
                })
                .flatten(),
            MentionKind::Skill | MentionKind::Connector => catalog
                .targets(kind)
                .iter()
                .filter(|target| name_matches(rest, &target.name))
                .map(|target| target.name.len())
                .max()
                .map(|length| (length, kind.icon())),
        };
        if let Some((length, icon)) = matched {
            let end = index + character.len_utf8() + length;
            spans.push(MentionSpan {
                range: index..end,
                kind,
                icon,
            });
            resume = end;
        }
    }
    spans
}

fn at_token_start(text: &str, index: usize) -> bool {
    text[..index]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
}

fn name_matches(rest: &str, name: &str) -> bool {
    rest.strip_prefix(name).is_some_and(|after| {
        after
            .chars()
            .next()
            .is_none_or(|next| next.is_whitespace() || NAME_TERMINATORS.contains(&next))
    })
}

/// A sigil the caret is typing a name after.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MentionTrigger {
    /// The typed sigil.
    pub(crate) sigil: char,
    /// The sigil and the whole word after it, which accepting replaces.
    pub(crate) range: Range<usize>,
    /// The text between the sigil and the caret.
    pub(crate) query: String,
}

/// Returns the trigger the caret at `caret` is inside, if any.
///
/// The caret must sit in a word that starts with a sigil at a token start,
/// after the sigil itself. The word runs to the next whitespace; the query
/// is only its part before the caret, so editing mid-word filters by what
/// precedes the caret.
#[must_use]
pub(crate) fn mention_trigger(text: &str, caret: usize) -> Option<MentionTrigger> {
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let word_start = text[..caret]
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    let sigil = text[word_start..].chars().next()?;
    MentionKind::for_sigil(sigil)?;
    let query_start = word_start + sigil.len_utf8();
    if caret < query_start {
        return None;
    }
    let query = &text[query_start..caret];
    if query.chars().count() > TRIGGER_QUERY_MAX_CHARS {
        return None;
    }
    let word_end = text[caret..]
        .char_indices()
        .find(|(_, character)| character.is_whitespace())
        .map_or(text.len(), |(index, _)| caret + index);
    Some(MentionTrigger {
        sigil,
        range: word_start..word_end,
        query: query.to_owned(),
    })
}

/// The mention one delete removes whole: the span the caret ends (going
/// backward) or starts (going forward), or one the caret is inside.
#[must_use]
pub(crate) fn mention_deletion(
    spans: &[MentionSpan],
    caret: usize,
    forward: bool,
) -> Option<Range<usize>> {
    spans
        .iter()
        .find(|span| {
            let inside = span.range.start < caret && caret < span.range.end;
            let at_edge = if forward {
                span.range.start == caret
            } else {
                span.range.end == caret
            };
            inside || at_edge
        })
        .map(|span| span.range.clone())
}

/// Moves a caret target that landed inside a mention to the mention's edge
/// in the direction of travel, so arrows step over a mention in one press.
#[must_use]
pub(crate) fn step_over_mention(spans: &[MentionSpan], target: usize, forward: bool) -> usize {
    spans
        .iter()
        .find(|span| span.range.start < target && target < span.range.end)
        .map_or(target, |span| {
            if forward {
                span.range.end
            } else {
                span.range.start
            }
        })
}

/// The composer's current catalog, shared with the transcript so a sent
/// message shows its mentions as the composer did.
#[derive(Clone, Debug, Default)]
pub(crate) struct SharedMentionCatalog(pub(crate) MentionCatalog);

impl gpui::Global for SharedMentionCatalog {}

/// The colour a mention of `kind` paints in.
#[must_use]
pub(crate) fn mention_color(
    theme: &artisan_ui::theme::ArtisanTheme,
    kind: MentionKind,
) -> gpui::Hsla {
    match kind {
        MentionKind::Skill => theme.colors.mention_skill.to_paint(),
        MentionKind::Connector => theme.colors.mention_connector.to_paint(),
        MentionKind::Command => theme.colors.mention_command.to_paint(),
    }
}

/// Colours the mentions in a sent message's text.
#[must_use]
pub(crate) fn mention_highlights(
    text: &str,
    catalog: &MentionCatalog,
    theme: &artisan_ui::theme::ArtisanTheme,
) -> Vec<(Range<usize>, gpui::HighlightStyle)> {
    recognize_mentions(text, catalog)
        .into_iter()
        .map(|span| {
            (
                span.range,
                gpui::HighlightStyle {
                    color: Some(mention_color(theme, span.kind)),
                    font_weight: Some(gpui::FontWeight::MEDIUM),
                    ..Default::default()
                },
            )
        })
        .collect()
}

/// Engines that read `/goal` from a message.
const GOAL_ENGINES: &[&str] = &["claude", "codex", "grok"];

/// What the composer can name for `engine` in `project`: the skills the
/// Forge placed or found in that engine (every enabled or discovered skill
/// while no engine is chosen), the skills and connectors the engine itself
/// last reported, and the commands the engine reads.
#[must_use]
pub(crate) fn catalog_for_engine(
    extensions: Option<&artisan_domain::ExtensionCatalog>,
    engine: Option<&str>,
    project: Option<&str>,
) -> MentionCatalog {
    use artisan_domain::{ExtensionEngineStatus, ExtensionKind};

    let mut skills: Vec<MentionTarget> = extensions
        .map(|catalog| {
            catalog
                .extensions()
                .iter()
                .filter(|extension| extension.kind == ExtensionKind::Skill)
                .filter(|extension| {
                    let present = |status| {
                        matches!(
                            status,
                            ExtensionEngineStatus::Synced
                                | ExtensionEngineStatus::Native
                                | ExtensionEngineStatus::Conflict
                        )
                    };
                    match engine.and_then(|engine| extension.engine(engine)) {
                        Some(state) => present(state.status),
                        None => {
                            extension.enabled
                                || (engine.is_none()
                                    && extension.engines.iter().any(|state| present(state.status)))
                        }
                    }
                })
                .map(|extension| MentionTarget {
                    name: extension.name.clone(),
                    description: extension.description.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut connectors = Vec::new();
    if let Some(inventory) = engine
        .zip(extensions)
        .and_then(|(engine, catalog)| catalog.inventory(engine, project))
    {
        // The Forge's own entries come first, so a reported skill only adds
        // what Artisan does not already describe.
        skills.extend(inventory.skills.iter().map(|skill| MentionTarget {
            name: skill.name.clone(),
            description: skill.description.clone(),
        }));
        connectors.extend(inventory.connectors.iter().map(|connector| MentionTarget {
            name: connector.name.clone(),
            description: connector_description(connector.status).to_owned(),
        }));
    }
    let commands = engine
        .filter(|engine| GOAL_ENGINES.contains(engine))
        .map(|_| vec![GOAL_COMMAND])
        .unwrap_or_default();
    MentionCatalog::new(skills, connectors, commands)
}

const fn connector_description(status: artisan_domain::ConnectorStatus) -> &'static str {
    use artisan_domain::ConnectorStatus;
    match status {
        ConnectorStatus::Connected => "Connected",
        ConnectorStatus::Pending => "Connecting",
        ConnectorStatus::NeedsAuth => "Needs sign-in",
        ConnectorStatus::Failed => "Failed to connect",
        ConnectorStatus::Disabled => "Turned off",
    }
}

#[cfg(test)]
#[path = "composer_mentions/tests.rs"]
mod tests;
