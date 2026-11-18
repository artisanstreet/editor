use artisan_mentions::MAX_SKILL_NAME_BYTES;
use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trigger {
    pub sigil: char,
    pub range: Range<usize>,
    pub query: String,
    pub at_start: bool,
}

pub fn trigger(text: &str, caret: usize) -> Option<Trigger> {
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let start = text[..caret]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(at, c)| at + c.len_utf8());
    let sigil = text[start..]
        .chars()
        .next()
        .filter(|sigil| matches!(sigil, '$' | '@' | '/'))?;
    let query_start = start + sigil.len_utf8();
    if caret < query_start || caret - query_start > MAX_SKILL_NAME_BYTES {
        return None;
    }
    let end = text[caret..]
        .char_indices()
        .find(|(_, c)| c.is_whitespace())
        .map_or(text.len(), |(at, _)| caret + at);
    Some(Trigger {
        sigil,
        range: start..end,
        query: text[query_start..caret].to_owned(),
        at_start: text[..start].trim().is_empty(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Command,
    Skill,
    Connector,
    Thread,
}
impl Kind {
    pub const fn sigil(self) -> char {
        match self {
            Self::Skill => '$',
            Self::Connector | Self::Thread => '@',
            Self::Command => '/',
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub kind: Kind,
    pub name: String,
    pub title: String,
    pub description: String,
    search_name: String,
    search_title: String,
}

impl Row {
    #[must_use]
    pub fn new(kind: Kind, name: String, title: String, description: String) -> Self {
        Self {
            kind,
            search_name: name.to_lowercase(),
            search_title: title.to_lowercase(),
            name,
            title,
            description,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reveal {
    Section,
    Highlighted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Steering {
    #[default]
    Pointer,
    Keys,
}

#[derive(Default)]
pub struct Mentions {
    pub catalog: Vec<Row>,
    pub threads: Vec<String>,
    pub current: Option<Trigger>,
    pub rows: Vec<usize>,
    pub highlighted: usize,
    reveal: Option<Reveal>,
    steering: Steering,
    hovered: Option<usize>,
    pointer: Option<aqua_core::Point>,
    dismissed: Option<(usize, String)>,
}

fn offered_threads<'a, T>(
    newest_first: impl IntoIterator<Item = (T, &'a str, bool)>,
) -> (Vec<(T, &'a str)>, Vec<String>) {
    let mut titles = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut offered = Vec::new();
    let mut offered_titles = std::collections::HashSet::new();
    for (thread, title, archived) in newest_first {
        if seen.insert(title) {
            titles.push(title.to_owned());
        }
        if !archived && offered_titles.insert(title) {
            offered.push((thread, title));
        }
    }
    (offered, titles)
}

impl Mentions {
    pub fn refresh(
        &mut self,
        host: &editor_core::HostData,
        engine: Option<artisan_api::EngineId>,
        own: Option<&artisan_api::ThreadId>,
    ) {
        let mut rows = Vec::new();
        let mut add = |kind: Kind, name: String, description: String| {
            if !artisan_mentions::valid_mention_name(&name) {
                return;
            }
            let title = if kind == Kind::Skill {
                display_name(&name)
            } else {
                name.clone()
            };
            rows.push(Row::new(kind, name, title, description));
        };
        if let Some(catalog) = &host.skill_catalog {
            for skill in catalog.iter().filter(|skill| skill.enabled) {
                add(Kind::Skill, skill.name.clone(), skill.description.clone());
            }
        }
        if let Ok(servers) = host.mcp_servers() {
            for server in servers {
                add(
                    Kind::Connector,
                    server.name.clone(),
                    server.url.clone().unwrap_or_else(|| server.command.clone()),
                );
            }
        }
        if let Some(status) = engine.and_then(|engine| host.engines.get(&engine)) {
            for command in status.commands.iter() {
                let description = match &command.hint {
                    Some(hint) if command.description.is_empty() => hint.clone(),
                    Some(hint) => format!("{hint} · {}", command.description),
                    None => command.description.clone(),
                };
                add(Kind::Command, command.name.clone(), description);
            }
        }
        rows.sort_by(|a, b| a.search_name.cmp(&b.search_name).then(a.name.cmp(&b.name)));
        rows.dedup_by(|a, b| a.kind == b.kind && a.name == b.name);
        let mut threads: Vec<_> = host
            .threads
            .values()
            .filter(|thread| Some(&thread.id) != own)
            .filter_map(|thread| {
                let title = thread.title.as_deref()?;
                artisan_mentions::valid_thread_title(title).then_some((thread, title))
            })
            .collect();
        threads.sort_by(|(a, _), (b, _)| {
            (b.activity_seq, b.seq, &b.id).cmp(&(a.activity_seq, a.seq, &a.id))
        });
        let (offered, titles) = offered_threads(
            threads
                .into_iter()
                .map(|(thread, title)| (thread, title, thread.archived)),
        );
        for (thread, title) in offered {
            rows.push(Row::new(
                Kind::Thread,
                title.to_owned(),
                title.to_owned(),
                host.projects
                    .get(&thread.project_id)
                    .map(|project| project.name.clone())
                    .unwrap_or_default(),
            ));
        }
        self.set_catalog(rows, titles);
    }

    fn set_catalog(&mut self, rows: Vec<Row>, titles: Vec<String>) {
        let highlighted = self
            .highlighted_row()
            .map(|row| (row.kind, row.name.clone()));
        self.catalog = rows;
        self.threads = titles;
        self.rebuild();
        self.hovered = self.hovered.filter(|hovered| *hovered < self.rows.len());
        if self.rows.is_empty() {
            return;
        }
        let kept = highlighted.and_then(|(kind, name)| {
            self.rows.iter().position(|index| {
                self.catalog
                    .get(*index)
                    .is_some_and(|row| row.kind == kind && row.name == name)
            })
        });
        match kept {
            Some(index) if index == self.highlighted => {}
            Some(index) => {
                self.highlighted = index;
                self.reveal_at(Reveal::Highlighted);
            }
            None => {
                self.highlighted = self.first_of_trigger();
                self.reveal_at(Reveal::Section);
            }
        }
    }

    fn highlighted_row(&self) -> Option<&Row> {
        self.rows
            .get(self.highlighted)
            .and_then(|index| self.catalog.get(*index))
    }
    pub fn sync(&mut self, text: &str, caret: usize) {
        let current = trigger(text, caret);
        if self.current == current {
            return;
        }
        let key = current
            .as_ref()
            .map(|trigger| (trigger.range.start, trigger.query.clone()));
        if self.dismissed != key {
            self.dismissed = None;
        }
        self.current = current;
        self.rebuild();
        self.hovered = None;
        self.highlighted = self.first_of_trigger();
        self.reveal_at(Reveal::Section);
    }
    pub fn clear_trigger(&mut self) {
        if self.current.is_some() {
            self.current = None;
            self.dismissed = None;
            self.highlighted = 0;
            self.rows.clear();
        }
    }

    pub fn dismiss(&mut self) {
        if let Some(trigger) = &self.current {
            self.dismissed = Some((trigger.range.start, trigger.query.clone()));
        }
        self.rebuild();
    }
    pub fn up(&mut self) {
        if !self.rows.is_empty() {
            self.highlighted = (self.highlighted + self.rows.len() - 1) % self.rows.len();
            self.steer_by_keys();
        }
    }
    pub fn down(&mut self) {
        if !self.rows.is_empty() {
            self.highlighted = (self.highlighted + 1) % self.rows.len();
            self.steer_by_keys();
        }
    }
    fn steer_by_keys(&mut self) {
        self.steering = Steering::Keys;
        self.reveal_at(Reveal::Highlighted);
    }
    pub fn hover(&mut self, index: usize, inside: bool) {
        match inside {
            true if index < self.rows.len() => {
                self.hovered = Some(index);
                if self.steering == Steering::Pointer {
                    self.highlighted = index;
                }
            }
            true => {}
            false => {
                if self.hovered == Some(index) {
                    self.hovered = None;
                }
            }
        }
    }
    pub fn pointer_at(&mut self, pointer: Option<aqua_core::Point>) -> bool {
        if pointer == self.pointer {
            return false;
        }
        self.pointer = pointer;
        if self.steering == Steering::Pointer {
            return false;
        }
        self.steering = Steering::Pointer;
        match self.hovered {
            Some(hovered) if hovered != self.highlighted => {
                self.highlighted = hovered;
                true
            }
            Some(_) | None => false,
        }
    }
    pub fn kinds(&self) -> Vec<Kind> {
        self.rows
            .iter()
            .filter_map(|index| self.catalog.get(*index).map(|row| row.kind))
            .collect()
    }
    fn reveal_at(&mut self, reveal: Reveal) {
        self.reveal = Some(reveal);
    }

    pub const fn mark_revealed(&mut self) {
        self.reveal = None;
    }

    #[must_use]
    pub fn scroll(&self, kept: f32, metrics: Metrics) -> f32 {
        self.reveal.map_or(kept, |reveal| {
            scroll_offset(&self.kinds(), self.highlighted, reveal, kept, metrics)
        })
    }
    fn first_of_trigger(&self) -> usize {
        let Some(trigger) = &self.current else {
            return 0;
        };
        self.rows
            .iter()
            .position(|index| {
                self.catalog
                    .get(*index)
                    .is_some_and(|row| row.kind.sigil() == trigger.sigil)
            })
            .unwrap_or(0)
    }
    pub fn replacement(&self, text: &str, index: usize) -> Option<(Range<usize>, String)> {
        let trigger = self.current.as_ref()?;
        let row = self.catalog.get(*self.rows.get(index)?)?;
        let tail = text.get(trigger.range.end..)?;
        let after = tail
            .chars()
            .next()
            .filter(|next| next.is_whitespace())
            .unwrap_or(' ');
        let end = trigger.range.end
            + if tail.starts_with(after) {
                after.len_utf8()
            } else {
                0
            };
        Some((
            trigger.range.start..end,
            format!("{}{}{after}", row.kind.sigil(), row.name),
        ))
    }
    fn rebuild(&mut self) {
        self.rows.clear();
        let Some(trigger) = &self.current else {
            return;
        };
        if self.dismissed.as_ref() == Some(&(trigger.range.start, trigger.query.clone()))
            || (trigger.sigil == '/' && !trigger.at_start)
        {
            return;
        }
        let query = trigger.query.trim().to_lowercase();
        let commands = trigger.at_start;
        let mut ranked: Vec<_> = self
            .catalog
            .iter()
            .enumerate()
            .filter(|(_, row)| row.kind != Kind::Command || commands)
            .filter_map(|(index, row)| {
                let rank = if query.is_empty() || row.search_name.starts_with(&query) {
                    0
                } else if row
                    .search_title
                    .split_whitespace()
                    .any(|part| part.starts_with(&query))
                {
                    1
                } else if row.search_name.contains(&query) || row.search_title.contains(&query) {
                    2
                } else {
                    return None;
                };
                Some((row.kind, rank, index))
            })
            .collect();
        ranked.sort_unstable();
        self.rows = ranked.into_iter().map(|(_, _, index)| index).collect();
        self.highlighted = self.highlighted.min(self.rows.len().saturating_sub(1));
    }
}

fn display_name(name: &str) -> String {
    let name = name.rsplit_once(':').map_or(name, |(_, name)| name);
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect::<String>()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Mentions {
    pub fn tokenizer(
        &self,
        theme: &editor_ui::theme::ArtisanTheme,
    ) -> editor_ui::text::editor::StyledTokenizer {
        use editor_ui::text::editor::StyledToken;
        let strong = editor_ui::kit::TextRole::LabelStrong.spec().weight;
        let skills = artisan_mentions::MentionNames::new(
            &self
                .catalog
                .iter()
                .filter(|row| row.kind == Kind::Skill)
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
        );
        let connectors = artisan_mentions::MentionNames::new(
            &self
                .catalog
                .iter()
                .filter(|row| row.kind == Kind::Connector)
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
        );
        let threads = self.threads.clone();
        let colors = theme.colors;
        std::rc::Rc::new(move |text| {
            let mut tokens = Vec::new();
            for (names, sigil, glyph, color) in [
                (
                    &skills,
                    '$',
                    editor_assets::AssetId::TABLER_FILE_SPARK,
                    colors.mention_skill,
                ),
                (
                    &connectors,
                    '@',
                    editor_assets::AssetId::TABLER_PLUG_CONNECTED,
                    colors.mention_connector,
                ),
            ] {
                for mention in names.mentions(text, sigil) {
                    tokens.push(StyledToken {
                        range: mention.range,
                        glyph: Some(glyph),
                        color: Some(color),
                        font_weight: Some(strong),
                    });
                }
            }
            let titles = threads.iter().map(String::as_str).collect::<Vec<_>>();
            for mention in artisan_mentions::thread_mentions(text, &titles) {
                tokens.push(StyledToken {
                    range: mention.range,
                    glyph: Some(editor_assets::AssetId::TABLER_MESSAGE_CIRCLE),
                    color: Some(colors.mention_connector),
                    font_weight: Some(strong),
                });
            }
            if let Some(range) = artisan_mentions::special_command(text) {
                tokens.push(StyledToken {
                    range,
                    glyph: Some(editor_assets::AssetId::TABLER_TARGET_ARROW),
                    color: Some(colors.mention_command),
                    font_weight: Some(strong),
                });
            }
            tokens.sort_by_key(|token| (token.range.start, std::cmp::Reverse(token.range.end)));
            let mut end = 0;
            tokens.retain(|token| {
                let keep = token.range.start >= end;
                if keep {
                    end = token.range.end;
                }
                keep
            });
            tokens
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub divider: f32,
    pub heading: f32,
    pub row: f32,
    pub viewport: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    section_top: f32,
    top: f32,
    bottom: f32,
}

fn spans(kinds: &[Kind], metrics: Metrics) -> (Vec<Span>, f32) {
    let mut spans = Vec::with_capacity(kinds.len());
    let mut y = 0.0;
    let mut previous = None;
    let mut section_top = 0.0;
    for kind in kinds {
        if previous != Some(*kind) {
            if previous.is_some() {
                y += metrics.divider;
            }
            section_top = y;
            y += metrics.heading;
            previous = Some(*kind);
        }
        spans.push(Span {
            section_top,
            top: y,
            bottom: y + metrics.row,
        });
        y += metrics.row;
    }
    (spans, y)
}

fn scroll_offset(
    kinds: &[Kind],
    highlighted: usize,
    reveal: Reveal,
    kept: f32,
    metrics: Metrics,
) -> f32 {
    let (spans, height) = spans(kinds, metrics);
    let max = (height - metrics.viewport).max(0.0);
    let Some(span) = spans.get(highlighted) else {
        return kept.clamp(0.0, max);
    };
    let section_first = highlighted == 0 || kinds.get(highlighted - 1) != kinds.get(highlighted);
    let top = match section_first {
        true => span.section_top,
        false => span.top,
    };
    let offset = match reveal {
        Reveal::Section => top,
        Reveal::Highlighted if top < kept => top,
        Reveal::Highlighted if span.bottom > kept + metrics.viewport => {
            span.bottom - metrics.viewport
        }
        Reveal::Highlighted => kept,
    };
    offset.clamp(0.0, max)
}

#[cfg(test)]
#[path = "../../tests/composer/mentions.rs"]
mod tests;
