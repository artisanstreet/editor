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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Skill,
    Connector,
    Command,
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

#[derive(Default)]
pub struct Mentions {
    pub catalog: Vec<Row>,
    pub threads: Vec<String>,
    pub current: Option<Trigger>,
    pub rows: Vec<usize>,
    pub highlighted: usize,
    dismissed: Option<(usize, String)>,
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
            rows.push(Row {
                kind,
                search_name: name.to_lowercase(),
                search_title: title.to_lowercase(),
                name,
                title,
                description,
            });
        };
        if let Some(catalog) = &host.skill_catalog {
            for skill in catalog.iter().filter(|skill| skill.enabled) {
                add(Kind::Skill, skill.name.clone(), skill.description.clone());
            }
        }
        if let Some(Ok(servers)) = host
            .setting("mcp_servers")
            .map(|value| serde_json::from_value::<Vec<artisan_api::McpServer>>(value.clone()))
        {
            for server in servers {
                add(
                    Kind::Connector,
                    server.name,
                    server.url.unwrap_or(server.command),
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
        let mut titles = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (thread, title) in threads {
            if !seen.insert(title) {
                continue;
            }
            titles.push(title.to_owned());
            if thread.archived {
                continue;
            }
            rows.push(Row {
                kind: Kind::Thread,
                name: title.to_owned(),
                title: title.to_owned(),
                description: host
                    .projects
                    .get(&thread.project_id)
                    .map(|project| project.name.clone())
                    .unwrap_or_default(),
                search_name: title.to_lowercase(),
                search_title: title.to_lowercase(),
            });
        }
        self.catalog = rows;
        self.threads = titles;
        self.rebuild();
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
        self.highlighted = 0;
        self.rebuild();
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
        }
    }
    pub fn down(&mut self) {
        if !self.rows.is_empty() {
            self.highlighted = (self.highlighted + 1) % self.rows.len();
        }
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
        let mut ranked: Vec<_> = self
            .catalog
            .iter()
            .enumerate()
            .filter(|(_, row)| row.kind.sigil() == trigger.sigil)
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
                Some((row.kind == Kind::Thread, rank, index))
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
        use aqua_text::FontWeight;
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
                        font_weight: Some(FontWeight::MEDIUM),
                    });
                }
            }
            let titles = threads.iter().map(String::as_str).collect::<Vec<_>>();
            for mention in artisan_mentions::thread_mentions(text, &titles) {
                tokens.push(StyledToken {
                    range: mention.range,
                    glyph: Some(editor_assets::AssetId::TABLER_MESSAGE_CIRCLE),
                    color: Some(colors.mention_connector),
                    font_weight: Some(FontWeight::MEDIUM),
                });
            }
            if let Some(range) = artisan_mentions::special_command(text) {
                tokens.push(StyledToken {
                    range,
                    glyph: Some(editor_assets::AssetId::TABLER_TARGET_ARROW),
                    color: Some(colors.mention_command),
                    font_weight: Some(FontWeight::MEDIUM),
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
