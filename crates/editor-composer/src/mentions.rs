//! Skills and connectors named inline in the draft.
//!
//! A mention is plain authored text: a sigil followed by a name the catalog
//! knows (`$skill`, `@connector`). The draft stores nothing else, so Forge
//! sees exactly what was typed and hands each engine its own form. Text that
//! only looks like a mention (`$5`, `@someone`, `/usr`) stays ordinary text
//! because no catalog entry has that name.
//!
//! Typing `/`, `$` or `@` at the start of a word opens one menu of skills
//! and connectors over the composer; the sigil only decides which section
//! leads (`@` connectors, otherwise skills). The text after the sigil
//! filters both, and choosing a row writes its mention into the draft. The
//! draft paints recognised mentions in their kind's colour and edits each
//! as one unit (see [`editor_ui::text_editor::Token`]).

use crate::Composer;
use editor_core::HostData;
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::hover_card::{glass_surface, hover_fill};
use editor_ui::motion::MotionDuration;
use editor_ui::picker_motion::{MenuEntrance, MenuMotion, MenuPhase, animate_menu};
use editor_ui::text_editor::{ListKey, TextEditor, Token, Tokenizer};
use editor_ui::theme::{ArtisanTheme, DesktopTheme, ThemeMode};
use gpui::{
    AnyElement, Context, Entity, Focusable as _, FontWeight, HighlightStyle,
    InteractiveElement as _, IntoElement, ParentElement as _, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, deferred, div, point, px,
};
use serde::Deserialize;
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

/// Longest query a trigger follows; anything longer is ordinary text.
const QUERY_MAX_CHARS: usize = 64;
/// Characters that end a mention name besides whitespace; Forge's list.
const NAME_TERMINATORS: &[char] = &[',', '.', ';', ':', '!', '?', ')', ']', '}', '"', '\''];
const ROW_HEIGHT: f32 = 36.0;
const HEADER_HEIGHT: f32 = 30.0;
const MAX_VISIBLE_ROWS: f32 = 9.5;
const MENU_PADDING: f32 = 6.0;
const ROW_RADIUS: f32 = 12.0;
/// The composer card's corner, which the menu card shares.
const MENU_RADIUS: f32 = 18.0;
/// The menu grows in from just under full size about its bottom edge.
const MENU_OPEN: Duration = MotionDuration::Fast.as_duration();
const MENU_CLOSE: Duration = MotionDuration::Quick.as_duration();
/// Debug selector of the menu card; rows are `{MENU_SELECTOR}-row-{n}`.
pub(crate) const MENU_SELECTOR: &str = "composer-mention-menu";

/// What a mention names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MentionKind {
    /// An Agent Skill installed on Forge.
    Skill,
    /// An MCP server from Forge's settings.
    Connector,
}

impl MentionKind {
    const fn sigil(self) -> char {
        match self {
            Self::Skill => '$',
            Self::Connector => '@',
        }
    }

    const fn for_sigil(sigil: char) -> Option<Self> {
        match sigil {
            '$' => Some(Self::Skill),
            '@' => Some(Self::Connector),
            _ => None,
        }
    }

    const fn icon(self) -> AssetId {
        match self {
            Self::Skill => AssetId::TABLER_FILE_SPARK,
            Self::Connector => AssetId::TABLER_PLUG_CONNECTED,
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Skill => "Skills",
            Self::Connector => "Connectors",
        }
    }

    fn style(self, theme: &ArtisanTheme) -> HighlightStyle {
        let color = match self {
            Self::Skill => theme.colors.mention_skill,
            Self::Connector => theme.colors.mention_connector,
        };
        HighlightStyle {
            color: Some(color.to_paint()),
            font_weight: Some(FontWeight::MEDIUM),
            ..HighlightStyle::default()
        }
    }
}

/// One skill or connector the composer can name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MentionTarget {
    /// The name exactly as engines know it; written after the sigil.
    pub(crate) name: String,
    /// Shown muted beside the name.
    pub(crate) description: String,
}

/// Everything the composer can name on the target's Forge.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MentionCatalog {
    skills: Vec<MentionTarget>,
    connectors: Vec<MentionTarget>,
}

impl MentionCatalog {
    /// Each list is deduplicated by name and sorted for display.
    pub(crate) fn new(skills: Vec<MentionTarget>, connectors: Vec<MentionTarget>) -> Self {
        Self {
            skills: normalized(skills),
            connectors: normalized(connectors),
        }
    }

    fn targets(&self, kind: MentionKind) -> &[MentionTarget] {
        match kind {
            MentionKind::Skill => &self.skills,
            MentionKind::Connector => &self.connectors,
        }
    }
}

fn normalized(mut targets: Vec<MentionTarget>) -> Vec<MentionTarget> {
    // The text after the sigil must always read as the name.
    targets.retain(|target| {
        let name = &target.name;
        !name.is_empty() && name.len() <= 128 && !name.contains(char::is_whitespace)
    });
    targets.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    targets.dedup_by(|later, earlier| later.name == earlier.name);
    targets
}

/// The mentions `text` holds, in order and never overlapping.
///
/// A sigil counts only at the start of the text or after whitespace, and
/// only when a known name follows it up to a word end; the longest known
/// name wins, so `$cloudflare:wrangler` is one skill even beside
/// `$cloudflare`.
fn recognize(text: &str, catalog: &MentionCatalog) -> Vec<(Range<usize>, MentionKind)> {
    let mut mentions = Vec::new();
    let mut resume = 0;
    for (index, character) in text.char_indices() {
        if index < resume || !at_word_start(text, index) {
            continue;
        }
        let Some(kind) = MentionKind::for_sigil(character) else {
            continue;
        };
        let rest = &text[index + character.len_utf8()..];
        let longest = catalog
            .targets(kind)
            .iter()
            .filter(|target| {
                rest.strip_prefix(target.name.as_str())
                    .is_some_and(|after| {
                        after.chars().next().is_none_or(|next| {
                            next.is_whitespace() || NAME_TERMINATORS.contains(&next)
                        })
                    })
            })
            .map(|target| target.name.len())
            .max();
        if let Some(length) = longest {
            let end = index + character.len_utf8() + length;
            mentions.push((index..end, kind));
            resume = end;
        }
    }
    mentions
}

fn at_word_start(text: &str, index: usize) -> bool {
    text[..index]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
}

/// The draft's tokens: every recognised mention in its kind's style.
fn tokenizer(catalog: Rc<MentionCatalog>) -> Tokenizer {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    Rc::new(move |text: &str| {
        recognize(text, &catalog)
            .into_iter()
            .map(|(range, kind)| Token {
                range,
                style: kind.style(&theme),
                glyph: Some(kind.icon()),
            })
            .collect()
    })
}

/// A sigil the caret is typing a name after.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Trigger {
    sigil: char,
    /// The sigil and the whole word after it, which choosing replaces.
    range: Range<usize>,
    /// The text between the sigil and the caret.
    query: String,
}

/// The trigger the caret at `caret` is inside: a word that starts with
/// `/`, `$` or `@`, with the caret after its sigil.
fn trigger(text: &str, caret: usize) -> Option<Trigger> {
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let word_start = text[..caret]
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    let sigil = text[word_start..]
        .chars()
        .next()
        .filter(|sigil| matches!(sigil, '/' | '$' | '@'))?;
    let query_start = word_start + sigil.len_utf8();
    if caret < query_start {
        return None;
    }
    let query = &text[query_start..caret];
    if query.chars().count() > QUERY_MAX_CHARS {
        return None;
    }
    let word_end = text[caret..]
        .char_indices()
        .find(|(_, character)| character.is_whitespace())
        .map_or(text.len(), |(index, _)| caret + index);
    Some(Trigger {
        sigil,
        range: word_start..word_end,
        query: query.to_owned(),
    })
}

/// One skill or connector the menu offers.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Row {
    kind: MentionKind,
    name: String,
    title: String,
    description: String,
}

/// The composer's mention state: the catalog and the menu over the draft.
#[derive(Default)]
pub(crate) struct Mentions {
    catalog: Rc<MentionCatalog>,
    /// Skills as Forge last listed them; connectors are read from the
    /// host's settings whenever they change.
    skills: Vec<MentionTarget>,
    trigger: Option<Trigger>,
    /// The trigger the user dismissed, by word start and query; typing on
    /// or leaving the word lets the menu open again.
    dismissed: Option<(usize, String)>,
    highlighted: usize,
    reveal_highlight: bool,
    scroll: ScrollHandle,
    load_skills: Option<Task<()>>,
    /// The rows on screen; kept while the menu plays its exit.
    shown: Vec<(MentionKind, Vec<Row>)>,
    /// The menu's open and close flight.
    motion: Rc<RefCell<MenuMotion>>,
    motion_task: Option<Task<()>>,
}

#[derive(Deserialize)]
struct ConnectorSetting {
    name: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    url: Option<String>,
}

/// The MCP servers in the host's `mcp_servers` setting.
fn connectors(host: &HostData) -> Vec<MentionTarget> {
    host.setting("mcp_servers")
        .and_then(|value| serde_json::from_value::<Vec<ConnectorSetting>>(value.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|server| MentionTarget {
            description: server.url.unwrap_or(server.command),
            name: server.name,
        })
        .collect()
}

impl Mentions {
    /// Drops the skills of the previous host until the new one lists its own.
    pub(crate) fn forget_skills(&mut self) {
        self.skills.clear();
        self.load_skills = None;
    }
}

impl Composer {
    /// Fetches the skills Forge has installed. Called when the target
    /// changes and whenever a trigger starts, so a skill added meanwhile
    /// shows up.
    pub(crate) fn load_skills(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host(cx) else {
            return;
        };
        let client = host.read(cx).client.clone();
        self.mentions.load_skills = Some(cx.spawn(async move |this, cx| {
            let skills = match client.skills().await {
                Ok(skills) => skills,
                Err(error) => {
                    tracing::warn!("couldn't list the skills for mentions: {error:#}");
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.mentions.skills = skills
                    .into_iter()
                    .map(|skill| MentionTarget {
                        name: skill.name,
                        description: skill.description.unwrap_or_default(),
                    })
                    .collect();
                this.refresh_mention_catalog(cx);
            });
        }));
    }

    /// Rebuilds the catalog from the listed skills and the host's MCP
    /// servers; the draft and the menu follow only if it changed.
    pub(crate) fn refresh_mention_catalog(&mut self, cx: &mut Context<Self>) {
        let connectors = self
            .host(cx)
            .map(|host| connectors(host.read(cx)))
            .unwrap_or_default();
        let catalog = MentionCatalog::new(self.mentions.skills.clone(), connectors);
        if *self.mentions.catalog == catalog {
            return;
        }
        let catalog = Rc::new(catalog);
        self.mentions.catalog = Rc::clone(&catalog);
        self.editor.update(cx, |editor, cx| {
            editor.set_tokenizer(Some(tokenizer(catalog)), cx);
        });
        self.sync_mention_menu(cx);
        self.update_list_keys(cx);
        cx.notify();
    }

    /// Follows the caret: the menu opens on a trigger word, resets its
    /// highlight when the query changes, and forgets a dismissal once the
    /// word or query changes. The editor hands over its navigation keys
    /// while the menu offers rows.
    pub(crate) fn sync_mention_menu(&mut self, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let trigger = if editor.is_disabled() {
            None
        } else {
            editor
                .caret()
                .and_then(|caret| trigger(editor.text(), caret))
        };
        let mentions = &mut self.mentions;
        let key = trigger
            .as_ref()
            .map(|trigger| (trigger.range.start, trigger.query.clone()));
        if mentions.dismissed.is_some() && mentions.dismissed != key {
            mentions.dismissed = None;
        }
        if mentions.trigger == trigger {
            return;
        }
        let started = mentions.trigger.is_none();
        mentions.highlighted = 0;
        mentions.reveal_highlight = true;
        mentions.trigger = trigger;
        if started && mentions.trigger.is_some() {
            self.load_skills(cx);
        }
        self.update_list_keys(cx);
        cx.notify();
    }

    fn update_list_keys(&mut self, cx: &mut Context<Self>) {
        let open = !self.mention_rows().is_empty();
        self.editor
            .update(cx, |editor, _| editor.set_list_open(open));
    }

    /// The rows the menu offers now, sections in the order the sigil asks
    /// for; empty while it is closed.
    fn mention_groups(&self) -> Vec<(MentionKind, Vec<Row>)> {
        let mentions = &self.mentions;
        let Some(trigger) = &mentions.trigger else {
            return Vec::new();
        };
        if mentions
            .dismissed
            .as_ref()
            .is_some_and(|(start, query)| *start == trigger.range.start && *query == trigger.query)
        {
            return Vec::new();
        }
        let order = if trigger.sigil == '@' {
            [MentionKind::Connector, MentionKind::Skill]
        } else {
            [MentionKind::Skill, MentionKind::Connector]
        };
        order
            .into_iter()
            .filter_map(|kind| {
                let rows = mentions
                    .catalog
                    .targets(kind)
                    .iter()
                    .map(|target| Row {
                        kind,
                        name: target.name.clone(),
                        title: match kind {
                            MentionKind::Skill => display_name(&target.name),
                            MentionKind::Connector => target.name.clone(),
                        },
                        description: target.description.clone(),
                    })
                    .collect();
                let rows = ranked(rows, &trigger.query);
                (!rows.is_empty()).then_some((kind, rows))
            })
            .collect()
    }

    fn mention_rows(&self) -> Vec<Row> {
        self.mention_groups()
            .into_iter()
            .flat_map(|(_, rows)| rows)
            .collect()
    }

    /// The names the menu offers, in order.
    #[cfg(test)]
    pub(crate) fn mention_menu_names(&self) -> Vec<String> {
        self.mention_rows()
            .into_iter()
            .map(|row| row.name)
            .collect()
    }

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "GPUI hands subscribers a reference"
    )]
    pub(crate) fn on_list_key(
        &mut self,
        _: Entity<TextEditor>,
        key: &ListKey,
        cx: &mut Context<Self>,
    ) {
        let rows = self.mention_rows();
        if rows.is_empty() {
            return;
        }
        let current = self.mentions.highlighted.min(rows.len() - 1);
        match key {
            ListKey::Up | ListKey::Down => {
                self.mentions.highlighted = if *key == ListKey::Up {
                    current.checked_sub(1).unwrap_or(rows.len() - 1)
                } else {
                    (current + 1) % rows.len()
                };
                self.mentions.reveal_highlight = true;
                cx.notify();
            }
            ListKey::Accept => self.choose_mention(&rows[current], cx),
            ListKey::Dismiss => {
                self.mentions.dismissed = self
                    .mentions
                    .trigger
                    .as_ref()
                    .map(|trigger| (trigger.range.start, trigger.query.clone()));
                self.update_list_keys(cx);
                cx.notify();
            }
        }
    }

    /// Writes the row's mention over the trigger word, followed by a space;
    /// whitespace already after the word is kept and stepped over.
    fn choose_mention(&mut self, row: &Row, cx: &mut Context<Self>) {
        let Some(trigger) = self.mentions.trigger.clone() else {
            return;
        };
        let text = self.editor.read(cx).text();
        let after = text[trigger.range.end..]
            .chars()
            .next()
            .filter(|next| next.is_whitespace())
            .unwrap_or(' ');
        let end = if text[trigger.range.end..].starts_with(after) {
            trigger.range.end + after.len_utf8()
        } else {
            trigger.range.end
        };
        let mention = format!("{}{}{after}", row.kind.sigil(), row.name);
        self.editor.update(cx, |editor, cx| {
            editor.replace(trigger.range.start..end, &mention, cx);
        });
    }

    /// Starts the menu's open or close flight, settling it after its
    /// duration (at once under reduced motion).
    fn begin_mention_menu_motion(&mut self, open: bool, cx: &mut Context<Self>) {
        let generation = {
            let mut motion = self.mentions.motion.borrow_mut();
            if open {
                motion.begin_open()
            } else {
                motion.begin_close()
            }
        };
        let settle = move |composer: &mut Self| {
            let mut motion = composer.mentions.motion.borrow_mut();
            if open {
                motion.finish_open(generation)
            } else {
                motion.finish_close(generation)
            }
        };
        if cx.reduce_motion() {
            settle(self);
            self.mentions.motion_task = None;
            return;
        }
        let duration = if open { MENU_OPEN } else { MENU_CLOSE };
        self.mentions.motion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            let _ = this.update(cx, |composer, cx| {
                if settle(composer) {
                    composer.mentions.motion_task = None;
                    cx.notify();
                }
            });
        }));
    }

    /// The menu card over the composer while it offers rows and the draft
    /// has focus.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the grouped rows, the scroll reveal and the entrance in visual order"
    )]
    pub(crate) fn render_mention_menu(
        &mut self,
        window: &Window,
        desktop: DesktopTheme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let groups = if self.editor.read(cx).focus_handle(cx).is_focused(window) {
            self.mention_groups()
        } else {
            Vec::new()
        };
        let phase = self.mentions.motion.borrow().phase();
        if groups.is_empty() {
            if matches!(phase, MenuPhase::Opening | MenuPhase::Open) {
                self.begin_mention_menu_motion(false, cx);
            }
        } else {
            if matches!(phase, MenuPhase::Hidden | MenuPhase::Closing) {
                self.begin_mention_menu_motion(true, cx);
            }
            self.mentions.shown = groups;
        }
        let phase = self.mentions.motion.borrow().phase();
        if phase == MenuPhase::Hidden {
            self.mentions.shown.clear();
            return None;
        }
        let interactive = phase != MenuPhase::Closing;
        let groups = self.mentions.shown.clone();
        let total = groups.iter().map(|(_, rows)| rows.len()).sum::<usize>();
        let highlighted = self.mentions.highlighted.min(total - 1);
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let mut list = div()
            .id("composer-mention-list")
            .flex()
            .flex_col()
            .w_full()
            .max_h(px(ROW_HEIGHT * MAX_VISIBLE_ROWS + HEADER_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&self.mentions.scroll);
        let (mut child, mut index, mut highlighted_child) = (0, 0, None);
        for (group, (kind, rows)) in groups.into_iter().enumerate() {
            if group > 0 {
                list = list.child(
                    div()
                        .flex_shrink_0()
                        .h(px(1.0))
                        .mx(px(10.0))
                        .my(px(4.0))
                        .bg(desktop.line),
                );
                child += 1;
            }
            list = list.child(
                div()
                    .flex_shrink_0()
                    .h(px(HEADER_HEIGHT))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .text_size(px(12.0))
                    .text_color(desktop.secondary)
                    .child(kind.title()),
            );
            child += 1;
            for row in rows {
                if index == highlighted {
                    highlighted_child = Some(child);
                }
                list = list.child(mention_row(
                    row,
                    index,
                    index == highlighted,
                    interactive,
                    &theme,
                    desktop,
                    cx,
                ));
                child += 1;
                index += 1;
            }
        }
        if std::mem::take(&mut self.mentions.reveal_highlight)
            && let Some(child) = highlighted_child
        {
            self.mentions.scroll.scroll_to_item(child);
        }
        let card = glass_surface(
            &theme,
            px(MENU_RADIUS),
            div().p(px(MENU_PADDING)).child(list),
        )
        .id(MENU_SELECTOR)
        .debug_selector(|| MENU_SELECTOR.to_owned())
        .occlude()
        .w_full();
        // The menu hangs over the composer: it grows up out of it, and
        // settles back into it when it closes.
        let card = animate_menu(
            card,
            &self.mentions.motion,
            "composer-mentions",
            MenuEntrance::Scale {
                origin: point(0.5, 1.0),
            },
            MENU_OPEN,
            MENU_CLOSE,
        );
        // Hangs over the composer card, full width, painted above the
        // conversation.
        Some(
            div()
                .absolute()
                .bottom_full()
                .left_0()
                .right_0()
                .pb(px(8.0))
                .child(deferred(card).with_priority(1))
                .into_any_element(),
        )
    }
}

fn mention_row(
    row: Row,
    index: usize,
    highlighted: bool,
    interactive: bool,
    theme: &ArtisanTheme,
    desktop: DesktopTheme,
    cx: &mut Context<Composer>,
) -> impl IntoElement {
    let mut element = div()
        .id(SharedString::from(format!("composer-mention-row-{index}")))
        .debug_selector(move || format!("{MENU_SELECTOR}-row-{index}"))
        .flex_shrink_0()
        .h(px(ROW_HEIGHT))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .rounded(px(ROW_RADIUS))
        .cursor_pointer()
        .text_size(px(14.0))
        .child(
            asset_glyph(row.kind.icon())
                .size(px(16.0))
                .text_color(desktop.secondary),
        )
        .child(
            div()
                .flex_shrink_0()
                .max_w(px(260.0))
                .truncate()
                .text_color(desktop.foreground)
                .child(row.title.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_color(desktop.secondary)
                .child(row.description.clone()),
        );
    if highlighted {
        element = hover_fill(element, theme, px(ROW_RADIUS));
    }
    if !interactive {
        // A closing menu only plays its exit.
        return element;
    }
    element
        .on_hover(cx.listener(move |composer, hovered: &bool, _, cx| {
            if *hovered && composer.mentions.highlighted != index {
                composer.mentions.highlighted = index;
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |composer, _, _, cx| composer.choose_mention(&row, cx)))
}

/// Keeps rows whose name or title contains `query`, best first: a name
/// that starts with it, then a title word that does, then the rest. An
/// empty query keeps every row in order.
fn ranked(rows: Vec<Row>, query: &str) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return rows;
    }
    let mut scored = rows
        .into_iter()
        .filter_map(|row| {
            let name = row.name.to_lowercase();
            let title = row.title.to_lowercase();
            let rank = if name.starts_with(&query) {
                0
            } else if title.split(' ').any(|word| word.starts_with(&query)) {
                1
            } else if name.contains(&query) || title.contains(&query) {
                2
            } else {
                return None;
            };
            Some((rank, row))
        })
        .collect::<Vec<_>>();
    scored.sort_by_key(|(rank, _)| *rank);
    scored.into_iter().map(|(_, row)| row).collect()
}

/// A skill's name as a title: `apple-design` reads `Apple Design`, and a
/// plugin skill `cloudflare:wrangler` reads `Wrangler`.
fn display_name(name: &str) -> String {
    let name = name.rsplit_once(':').map_or(name, |(_, skill)| skill);
    name.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}
