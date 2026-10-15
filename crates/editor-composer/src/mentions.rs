//! Catalog-backed mention completion. Forge and Client share the token grammar.
use crate::Composer;
use artisan_api::McpServer;
use artisan_mentions::{MAX_SKILL_NAME_BYTES, MentionNames, valid_mention_name};
use editor_ui::theme::{ActiveTheme as _, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use editor_ui::{
    AssetId,
    asset_seam::asset_glyph,
    motion::MotionDuration,
    picker_motion::{MenuEntrance, MenuMotion, MenuPhase, animate_menu},
    text_editor::{ListKey, TextEditor, Token, Tokenizer},
    theme::{ArtisanTheme, ColorTokens},
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, Entity, Focusable as _, FontWeight, HighlightStyle,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, deferred, div, point, px,
};
use std::{cell::RefCell, ops::Range, rc::Rc, time::Duration};
pub(crate) const MENU_SELECTOR: &str = "composer-mention-menu";
const ROW_HEIGHT: f32 = 36.0;
const MENU_OPEN: Duration = MotionDuration::Fast.as_duration();
const MENU_CLOSE: Duration = MotionDuration::Quick.as_duration();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Skill,
    Connector,
}
impl Kind {
    fn sigil(self) -> char {
        match self {
            Self::Skill => '$',
            Self::Connector => '@',
        }
    }
    fn icon(self) -> AssetId {
        match self {
            Self::Skill => AssetId::TABLER_FILE_SPARK,
            Self::Connector => AssetId::TABLER_PLUG_CONNECTED,
        }
    }
}
struct Row {
    kind: Kind,
    name: String,
    title: String,
    description: String,
    search_name: String,
    search_title: String,
}
#[derive(Default)]
struct Catalog {
    rows: Vec<Row>,
    skills: MentionNames,
    connectors: MentionNames,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Trigger {
    sigil: char,
    range: Range<usize>,
    query: String,
}
fn trigger(text: &str, caret: usize) -> Option<Trigger> {
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
    })
}
fn tokenizer(catalog: Rc<Catalog>, theme: ArtisanTheme) -> Tokenizer {
    Rc::new(move |text| {
        let mut tokens = Vec::new();
        for (kind, names) in [
            (Kind::Skill, &catalog.skills),
            (Kind::Connector, &catalog.connectors),
        ] {
            for mention in names.mentions(text, kind.sigil()) {
                tokens.push(Token {
                    range: mention.range,
                    glyph: Some(kind.icon()),
                    style: HighlightStyle {
                        color: Some(
                            match kind {
                                Kind::Skill => theme.colors.mention_skill,
                                Kind::Connector => theme.colors.mention_connector,
                            }
                            .to_paint(),
                        ),
                        font_weight: Some(FontWeight::MEDIUM),
                        ..Default::default()
                    },
                });
            }
        }
        tokens.sort_by_key(|token| token.range.start);
        tokens
    })
}
#[derive(Default)]
pub(crate) struct Mentions {
    catalog: Rc<Catalog>,
    catalog_identity: Option<(u64, usize)>,
    mcp: Option<serde_json::Value>,
    trigger: Option<Trigger>,
    dismissed: Option<(usize, String)>,
    rows: Vec<usize>,
    highlighted: usize,
    scroll: Option<gpui::ListState>,
    layout: Vec<(bool, bool)>,
    menu_focus: Option<gpui::FocusHandle>,
    motion: Rc<RefCell<MenuMotion>>,
    motion_task: Option<Task<()>>,
    shown: Vec<usize>,
}
impl Mentions {
    pub(crate) fn forget_skills(&mut self) {
        *self = Self::default();
    }
    fn rebuild_rows(&mut self) {
        self.rows.clear();
        let Some(trigger) = &self.trigger else {
            return;
        };
        if self.dismissed.as_ref() == Some(&(trigger.range.start, trigger.query.clone())) {
            return;
        }
        let query = trigger.query.trim().to_lowercase();
        let preferred = if trigger.sigil == '@' {
            Kind::Connector
        } else {
            Kind::Skill
        };
        let mut ranked: Vec<_> = self
            .catalog
            .rows
            .iter()
            .enumerate()
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
                Some((row.kind != preferred, rank, index))
            })
            .collect();
        ranked.sort_unstable();
        self.rows = ranked.into_iter().map(|(_, _, index)| index).collect();
        self.highlighted = self.highlighted.min(self.rows.len().saturating_sub(1));
    }
}
impl Composer {
    pub(crate) fn load_skills(&mut self, cx: &mut Context<Self>) {
        if let Some(host) = self.host(cx) {
            host.update(cx, |host, cx| host.load_skill_catalog(false, cx));
        }
    }
    pub(crate) fn refresh_mention_catalog(&mut self, cx: &mut Context<Self>) {
        // Connecting -> Live can leave the catalog identity unchanged. Start the
        // host-owned read before the projection's identity early return.
        self.load_skills(cx);
        let Some(host) = self.host(cx) else {
            return;
        };
        let state = host.read(cx);
        let identity = (
            state.catalog_revision,
            state
                .skill_catalog
                .as_ref()
                .map_or(0, |catalog| std::sync::Arc::as_ptr(catalog) as usize),
        );
        let mcp = state.setting("mcp_servers");
        if self.mentions.catalog_identity == Some(identity) && self.mentions.mcp.as_ref() == mcp {
            return;
        }
        self.mentions.catalog_identity = Some(identity);
        self.mentions.mcp = mcp.cloned();
        let mut rows = Vec::new();
        let mut add = |kind, name: String, description: String| {
            if !valid_mention_name(&name) {
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
        if let Some(catalog) = &state.skill_catalog {
            for skill in catalog.iter().filter(|skill| skill.enabled) {
                add(Kind::Skill, skill.name.clone(), skill.description.clone());
            }
        }
        if let Some(Ok(servers)) =
            mcp.map(|value| serde_json::from_value::<Vec<McpServer>>(value.clone()))
        {
            for server in servers {
                add(
                    Kind::Connector,
                    server.name,
                    server.url.unwrap_or(server.command),
                );
            }
        }
        rows.sort_by(|a, b| a.search_name.cmp(&b.search_name).then(a.name.cmp(&b.name)));
        rows.dedup_by(|a, b| a.kind == b.kind && a.name == b.name);
        let names = |kind| {
            rows.iter()
                .filter(|row| row.kind == kind)
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>()
        };
        let catalog = Rc::new(Catalog {
            skills: MentionNames::new(&names(Kind::Skill)),
            connectors: MentionNames::new(&names(Kind::Connector)),
            rows,
        });
        self.mentions.catalog = catalog.clone();
        let theme = *cx.theme();
        self.editor.update(cx, |editor, cx| {
            editor.set_tokenizer(Some(tokenizer(catalog, theme)), cx);
        });
        self.mentions.rebuild_rows();
        self.update_list_keys(cx);
        cx.notify();
    }
    pub(crate) fn sync_mention_menu(&mut self, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let trigger = (!editor.is_disabled())
            .then(|| {
                editor
                    .caret()
                    .and_then(|caret| trigger(editor.text(), caret))
            })
            .flatten();
        if self.mentions.trigger == trigger {
            return;
        }
        let key = trigger
            .as_ref()
            .map(|trigger| (trigger.range.start, trigger.query.clone()));
        if self.mentions.dismissed != key {
            self.mentions.dismissed = None;
        }
        self.mentions.trigger = trigger;
        self.mentions.highlighted = 0;
        self.mentions.rebuild_rows();
        self.update_list_keys(cx);
        cx.notify();
    }
    fn update_list_keys(&mut self, cx: &mut Context<Self>) {
        let open = !self.mentions.rows.is_empty();
        self.editor
            .update(cx, |editor, _| editor.set_list_open(open));
    }
    #[cfg(test)]
    pub(crate) fn mention_menu_names(&self) -> Vec<String> {
        self.mentions
            .rows
            .iter()
            .map(|&index| self.mentions.catalog.rows[index].name.clone())
            .collect()
    }
    pub(crate) fn on_list_key(
        &mut self,
        _: Entity<TextEditor>,
        key: &ListKey,
        cx: &mut Context<Self>,
    ) {
        let count = self.mentions.rows.len();
        if count == 0 {
            return;
        }
        match key {
            ListKey::Up => {
                self.mentions.highlighted = (self.mentions.highlighted + count - 1) % count
            }
            ListKey::Down => self.mentions.highlighted = (self.mentions.highlighted + 1) % count,
            ListKey::Accept => {
                self.choose_mention(self.mentions.highlighted, cx);
                return;
            }
            ListKey::Dismiss => {
                self.mentions.dismissed = self
                    .mentions
                    .trigger
                    .as_ref()
                    .map(|trigger| (trigger.range.start, trigger.query.clone()));
                self.mentions.rebuild_rows();
                self.update_list_keys(cx);
            }
        }
        if let Some(scroll) = &self.mentions.scroll {
            let mut groups: usize = 0;
            let mut previous = None;
            for &index in self
                .mentions
                .rows
                .iter()
                .take(self.mentions.highlighted + 1)
            {
                let kind = self.mentions.catalog.rows[index].kind;
                if previous != Some(kind) {
                    groups += 1;
                    previous = Some(kind);
                }
            }
            scroll.scroll_to_reveal_item(
                self.mentions.highlighted + groups + groups.saturating_sub(1),
            );
        }
        cx.notify();
    }
    fn choose_mention(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(trigger) = self.mentions.trigger.clone() else {
            return;
        };
        let Some(row) = self
            .mentions
            .rows
            .get(index)
            .map(|index| &self.mentions.catalog.rows[*index])
        else {
            return;
        };
        let text = self.editor.read(cx).text();
        if trigger.range.end > text.len() {
            return;
        }
        let after = text[trigger.range.end..]
            .chars()
            .next()
            .filter(|next| next.is_whitespace())
            .unwrap_or(' ');
        let end = trigger.range.end
            + if text[trigger.range.end..].starts_with(after) {
                after.len_utf8()
            } else {
                0
            };
        let mention = format!("{}{}{after}", row.kind.sigil(), row.name);
        self.editor.update(cx, |editor, cx| {
            editor.replace(trigger.range.start..end, &mention, cx)
        });
    }
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

    pub(crate) fn render_mention_menu(
        &mut self,
        window: &Window,
        desktop: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let desktop = *desktop;
        let focus = self
            .mentions
            .menu_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let visible = self.mentions.trigger.is_some()
            && (self.editor.read(cx).focus_handle(cx).is_focused(window)
                || focus.contains_focused(window, cx));
        let phase = self.mentions.motion.borrow().phase();
        if visible && !self.mentions.rows.is_empty() {
            if matches!(phase, MenuPhase::Hidden | MenuPhase::Closing) {
                self.begin_mention_menu_motion(true, cx);
            }
            self.mentions.shown = self.mentions.rows.clone();
        } else if matches!(phase, MenuPhase::Opening | MenuPhase::Open) {
            self.begin_mention_menu_motion(false, cx);
        }
        let phase = self.mentions.motion.borrow().phase();
        let theme = *cx.theme();
        if visible && self.mentions.rows.is_empty() && self.mentions.shown.is_empty() {
            let host = self.host(cx)?;
            let error = host.read(cx).catalog_load().error.clone()?;
            let editor = self.editor.clone();
            let content = div()
                .child(editor_ui::page::error_text(&theme, error))
                .child(
                    editor_ui::button::Button::text("retry-mention-catalog", theme, "Retry skills")
                        .on_activate(move |_, window, cx| {
                            editor.focus_handle(cx).focus(window, cx);
                            host.update(cx, |host, cx| host.load_skill_catalog(true, cx));
                        }),
                );
            let panel = div()
                .id(MENU_SELECTOR)
                .track_focus(&focus)
                .tab_group()
                .tab_stop(false)
                .child(editor_ui::popover::PopoverPanel::new(
                    "panel", theme, content,
                ));
            return Some(
                div()
                    .absolute()
                    .bottom_full()
                    .left_0()
                    .right_0()
                    .pb(px(8.))
                    .child(
                        deferred(panel)
                            .with_priority(editor_ui::overlay::popup_priority(window, cx)),
                    )
                    .into_any_element(),
            );
        }
        if phase == MenuPhase::Hidden {
            self.mentions.shown.clear();
            return None;
        }
        let mut entries: Vec<(Option<Kind>, usize)> = Vec::new();
        let mut previous = None;
        for (index, &catalog_index) in self.mentions.shown.iter().enumerate() {
            let Some(row) = self.mentions.catalog.rows.get(catalog_index) else {
                continue;
            };
            if previous != Some(row.kind) {
                if previous.is_some() {
                    entries.push((None, usize::MAX));
                }
                entries.push((Some(row.kind), index));
                previous = Some(row.kind);
            }
            entries.push((None, index));
        }
        let count = entries.len();
        let layout: Vec<_> = entries
            .iter()
            .map(|(heading, index)| (heading.is_some(), *index == usize::MAX))
            .collect();
        let height: f32 = layout
            .iter()
            .map(|(heading, separator)| {
                if *separator {
                    9.0
                } else if *heading {
                    30.0
                } else {
                    ROW_HEIGHT
                }
            })
            .sum();
        let state = self
            .mentions
            .scroll
            .get_or_insert_with(|| gpui::ListState::new(count, gpui::ListAlignment::Top, px(0.0)))
            .clone();
        if state.item_count() != count {
            state.splice(0..state.item_count(), count);
        } else if self.mentions.layout != layout {
            state.remeasure();
        }
        self.mentions.layout = layout;
        let list = gpui::list(
            state,
            cx.processor(move |this, entry: usize, _, cx| {
                let Some((heading, index)) = entries.get(entry).copied() else {
                    return div().into_any_element();
                };
                if index == usize::MAX {
                    return div()
                        .h(px(9.0))
                        .py(px(4.0))
                        .mx(px(10.0))
                        .child(div().h(px(1.0)).bg(desktop.line.to_paint()))
                        .into_any_element();
                }
                if let Some(kind) = heading {
                    return div()
                        .h(px(30.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .text_size(TypographyTokens::size(TextStep::Xs))
                        .text_color(desktop.workspace_secondary.to_paint())
                        .child(if kind == Kind::Skill {
                            "Skills"
                        } else {
                            "Connectors"
                        })
                        .into_any_element();
                }
                let Some(row) = this
                    .mentions
                    .shown
                    .get(index)
                    .and_then(|index| this.mentions.catalog.rows.get(*index))
                else {
                    return div().into_any_element();
                };
                let owner = cx.entity().downgrade();
                let hover_owner = owner.clone();
                div()
                    .id(SharedString::from(format!("composer-mention-row-{index}")))
                    .role(gpui::Role::MenuItem)
                    .aria_label(row.title.clone())
                    .aria_selected(index == this.mentions.highlighted)
                    .focusable()
                    .tab_index(0)
                    .debug_selector(move || format!("{MENU_SELECTOR}-row-{index}"))
                    .h(px(ROW_HEIGHT))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(RadiusTokens::value(RadiusStep::LgMid))
                    .text_size(TypographyTokens::size(TextStep::Sm))
                    .cursor_pointer()
                    // The keyboard's row wears the shared highlight, never
                    // the pointer's hover fill.
                    .when(index == this.mentions.highlighted, |row| {
                        editor_ui::hover_card::highlight_fill(row, &theme)
                    })
                    .child(
                        asset_glyph(row.kind.icon())
                            .size(px(16.0))
                            .text_color(desktop.workspace_secondary.to_paint()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(260.0))
                            .truncate()
                            .text_color(desktop.workspace_foreground.to_paint())
                            .child(row.title.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(desktop.workspace_secondary.to_paint())
                            .child(row.description.clone()),
                    )
                    .on_hover(move |hovered: &bool, _, cx| {
                        if *hovered {
                            let _ = hover_owner.update(cx, |this, cx| {
                                this.mentions.highlighted = index;
                                cx.notify();
                            });
                        }
                    })
                    .on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this.mentions.motion.borrow().phase() != MenuPhase::Closing {
                                this.choose_mention(index, cx);
                                this.editor.focus_handle(cx).focus(window, cx);
                            }
                        });
                    })
                    .into_any_element()
            }),
        )
        .h(px(height.min(36.0 * 9.5 + 30.0)));
        let card = div()
            .id(MENU_SELECTOR)
            .track_focus(&focus)
            .tab_group()
            .tab_stop(false)
            .debug_selector(|| MENU_SELECTOR.to_owned())
            .w_full()
            .child(
                editor_ui::popover::glass_menu(
                    "composer-mentions-panel",
                    &theme,
                    RadiusTokens::value(RadiusStep::X2l),
                )
                .child(div().p(px(6.0)).child(list)),
            );
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
        Some(
            div()
                .absolute()
                .bottom_full()
                .left_0()
                .right_0()
                .pb(px(8.0))
                .child(deferred(card).with_priority(editor_ui::overlay::popup_priority(window, cx)))
                .into_any_element(),
        )
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
