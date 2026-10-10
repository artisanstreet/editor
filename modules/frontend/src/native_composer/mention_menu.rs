//! The composer's mention menu: one list of skills and connectors, opened
//! by typing `/`, `$`, or `@` at the start of a word.
//!
//! Every sigil opens the same menu; the sigil only decides which section
//! leads (`@` connectors, otherwise skills). The text after the sigil
//! filters every section at once, and choosing a row writes its mention
//! into the draft. The menu grows in from the composer it hangs over and
//! settles away when it closes, keeping its last rows for the exit.

#![forbid(unsafe_code)]

use std::cell::RefCell;

use super::*;
use crate::command_ranking::score_with_keywords;
use crate::composer_mentions::{MentionKind, MentionTrigger, mention_trigger};
use crate::native_model_selector::{
    PickerMenuEntrance, PickerMenuMotion, PickerMenuPhase, animate_picker_menu_timed,
};
use artisan_ui::motion::MotionDuration;

const MENU_ROW_HEIGHT_PX: f32 = 36.0;
const MENU_HEADER_HEIGHT_PX: f32 = 30.0;
const MENU_MAX_VISIBLE_ROWS: f32 = 9.5;
const MENU_PADDING_PX: f32 = 6.0;
const MENU_ROW_RADIUS_PX: f32 = 12.0;
const MENU_SELECTOR: &str = "artisan-composer-mention-menu";
/// The transitions-dev dropdown open and close durations.
const MENU_OPEN: Duration = MotionDuration::Fast.as_duration();
const MENU_CLOSE: Duration = MotionDuration::Quick.as_duration();

/// A list the menu shows, in the order the trigger asks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Section {
    Skills,
    Connectors,
}

impl Section {
    const fn title(self) -> &'static str {
        match self {
            Self::Skills => "Skills",
            Self::Connectors => "Connectors",
        }
    }

    /// Sections in the order a typed sigil asks for: connectors lead after
    /// `@`, skills otherwise.
    fn order(sigil: char) -> [Self; 2] {
        match MentionKind::for_sigil(sigil) {
            Some(MentionKind::Connector) => [Self::Connectors, Self::Skills],
            _ => [Self::Skills, Self::Connectors],
        }
    }

    const fn kind(self) -> MentionKind {
        match self {
            Self::Skills => MentionKind::Skill,
            Self::Connectors => MentionKind::Connector,
        }
    }
}

/// One skill or connector the menu offers.
#[derive(Clone, Debug, Eq, PartialEq)]
struct MenuRow {
    kind: MentionKind,
    /// The name written after the sigil.
    name: String,
    title: String,
    description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MenuGroup {
    section: Section,
    rows: Vec<MenuRow>,
}

/// The menu's interaction and motion state; its rows are rebuilt from the
/// draft and the catalog whenever they are needed.
pub(super) struct MentionMenu {
    trigger: Option<MentionTrigger>,
    /// The trigger the user dismissed, by word start and query; typing on
    /// or leaving the word lets the menu open again.
    dismissed: Option<(usize, String)>,
    highlighted: usize,
    reveal_highlight: bool,
    scroll: ScrollHandle,
    motion: Rc<RefCell<PickerMenuMotion>>,
    motion_task: Option<Task<()>>,
    /// The rows last shown, painted while the menu closes.
    shown: Vec<MenuGroup>,
}

impl Default for MentionMenu {
    fn default() -> Self {
        Self {
            trigger: None,
            dismissed: None,
            highlighted: 0,
            reveal_highlight: false,
            scroll: ScrollHandle::new(),
            motion: Rc::new(RefCell::new(PickerMenuMotion::default())),
            motion_task: None,
            shown: Vec::new(),
        }
    }
}

impl NativeComposer {
    /// Follows the caret: the menu opens on a trigger word, resets its
    /// highlight when the query changes, and forgets a dismissal once the
    /// query or word changes.
    pub(super) fn sync_mention_menu(&mut self) {
        let trigger = (self.selection.is_empty()
            && self.marked_range.is_none()
            && !self.state.is_disabled()
            && !self.state.is_submitting())
        .then(|| mention_trigger(self.state.draft(), self.selection.end))
        .flatten();
        let menu = &mut self.mention_menu;
        if menu.dismissed.as_ref()
            != trigger
                .as_ref()
                .map(|trigger| (trigger.range.start, trigger.query.clone()))
                .as_ref()
        {
            menu.dismissed = None;
        }
        if menu.trigger != trigger {
            menu.highlighted = 0;
            menu.reveal_highlight = true;
        }
        menu.trigger = trigger;
    }

    /// The rows the menu shows now; empty while it is closed.
    fn mention_menu_groups(&self) -> Vec<MenuGroup> {
        let menu = &self.mention_menu;
        let Some(trigger) = menu.trigger.as_ref() else {
            return Vec::new();
        };
        if menu
            .dismissed
            .as_ref()
            .is_some_and(|(start, query)| *start == trigger.range.start && *query == trigger.query)
        {
            return Vec::new();
        }
        Section::order(trigger.sigil)
            .into_iter()
            .filter_map(|section| {
                let rows = ranked(self.section_rows(section), &trigger.query);
                (!rows.is_empty()).then_some(MenuGroup { section, rows })
            })
            .collect()
    }

    fn section_rows(&self, section: Section) -> Vec<MenuRow> {
        let (targets, kind) = match section {
            Section::Skills => (self.mentions.skills(), section.kind()),
            Section::Connectors => (self.mentions.connectors(), section.kind()),
        };
        targets
            .iter()
            .map(|target| MenuRow {
                kind,
                name: target.name.clone(),
                title: match kind {
                    MentionKind::Skill => display_name(&target.name),
                    MentionKind::Connector | MentionKind::Command => target.name.clone(),
                },
                description: target.description.clone(),
            })
            .collect()
    }

    /// Whether the menu is offering rows, after following the caret.
    pub(super) fn mention_menu_open(&mut self) -> bool {
        self.sync_mention_menu();
        !self.mention_menu_groups().is_empty()
    }

    fn mention_menu_rows(&self) -> Vec<MenuRow> {
        self.mention_menu_groups()
            .into_iter()
            .flat_map(|group| group.rows)
            .collect()
    }

    /// Moves the highlight by `delta` rows, wrapping. Returns whether the
    /// menu was open to take the key.
    pub(super) fn move_mention_highlight(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        if !self.mention_menu_open() {
            return false;
        }
        let count = self.mention_menu_rows().len();
        if count > 0 {
            let current = self.mention_menu.highlighted.min(count - 1);
            let count = count.cast_signed();
            self.mention_menu.highlighted = (current.cast_signed() + delta)
                .rem_euclid(count)
                .cast_unsigned();
            self.mention_menu.reveal_highlight = true;
            cx.notify();
        }
        true
    }

    /// Chooses the highlighted row. Returns whether the menu was open to
    /// take the key.
    pub(super) fn accept_mention_highlight(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.mention_menu_open() {
            return false;
        }
        let rows = self.mention_menu_rows();
        if let Some(row) = rows.get(
            self.mention_menu
                .highlighted
                .min(rows.len().saturating_sub(1)),
        ) {
            self.choose_mention_row(row, cx);
        }
        true
    }

    /// Escape: closes the menu until the query or word changes. Returns
    /// whether the menu was open to take the key.
    pub(super) fn dismiss_mention_menu(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.mention_menu_open() {
            return false;
        }
        self.mention_menu.dismissed = self
            .mention_menu
            .trigger
            .as_ref()
            .map(|trigger| (trigger.range.start, trigger.query.clone()));
        cx.notify();
        true
    }

    /// Writes the row's mention over the typed trigger word, followed by a
    /// space unless one already follows.
    fn choose_mention_row(&mut self, row: &MenuRow, cx: &mut Context<Self>) {
        let Some(trigger) = self.mention_menu.trigger.clone() else {
            return;
        };
        let spaced = self.state.draft()[trigger.range.end..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace);
        let mention = format!(
            "{}{}{}",
            row.kind.sigil(),
            row.name,
            if spaced { "" } else { " " }
        );
        self.replace_range(trigger.range, &mention, None, cx);
        if spaced {
            let caret = next_character_boundary(self.state.draft(), self.selection.end);
            self.move_to(caret, cx);
        }
        cx.notify();
    }

    /// Starts the menu's open or close flight, settling it after its
    /// duration (at once under reduced motion).
    fn begin_mention_menu_motion(&mut self, open: bool, cx: &mut Context<Self>) {
        let generation = {
            let mut motion = self.mention_menu.motion.borrow_mut();
            if open {
                motion.begin_open()
            } else {
                motion.begin_close()
            }
        };
        let settle = move |composer: &mut Self| {
            let mut motion = composer.mention_menu.motion.borrow_mut();
            if open {
                motion.finish_open(generation)
            } else {
                motion.finish_close(generation)
            }
        };
        if cx.reduce_motion() {
            settle(self);
            self.mention_menu.motion_task = None;
            return;
        }
        let duration = if open { MENU_OPEN } else { MENU_CLOSE };
        self.mention_menu.motion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            let _ = this.update(cx, |composer, cx| {
                if settle(composer) {
                    composer.mention_menu.motion_task = None;
                    cx.notify();
                }
            });
        }));
    }

    /// The menu card, or `None` once it has fully closed. The caller places
    /// it above the composer card.
    pub(super) fn render_mention_menu(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.sync_mention_menu();
        let groups = if self.focus_handle.is_focused(window) {
            self.mention_menu_groups()
        } else {
            Vec::new()
        };
        let phase = self.mention_menu.motion.borrow().phase();
        if groups.is_empty() {
            if matches!(phase, PickerMenuPhase::Opening | PickerMenuPhase::Open) {
                self.begin_mention_menu_motion(false, cx);
            }
        } else {
            if matches!(phase, PickerMenuPhase::Hidden | PickerMenuPhase::Closing) {
                self.begin_mention_menu_motion(true, cx);
            }
            self.mention_menu.shown = groups;
        }
        let phase = self.mention_menu.motion.borrow().phase();
        if phase == PickerMenuPhase::Hidden {
            self.mention_menu.shown.clear();
            return None;
        }
        let interactive = phase != PickerMenuPhase::Closing;
        let card = self.render_menu_card(interactive, cx);
        let snapshot = *self.mention_menu.motion.borrow();
        Some(animate_picker_menu_timed(
            card,
            Rc::clone(&self.mention_menu.motion),
            snapshot,
            "composer-mentions",
            // The menu hangs over the composer: it grows up out of it.
            PickerMenuEntrance::Scale {
                origin: point(0.5, 1.0),
            },
            MENU_OPEN,
            MENU_CLOSE,
        ))
    }

    fn render_menu_card(&mut self, interactive: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let desktop_theme = DesktopTheme::neutral_dark();
        let groups = self.mention_menu.shown.clone();
        let row_count = groups.iter().map(|group| group.rows.len()).sum::<usize>();
        let highlighted = self
            .mention_menu
            .highlighted
            .min(row_count.saturating_sub(1));
        self.mention_menu.highlighted = highlighted;

        let mut list = div()
            .id("artisan-composer-mention-menu-list")
            .flex()
            .flex_col()
            .w_full()
            .max_h(px(
                MENU_ROW_HEIGHT_PX * MENU_MAX_VISIBLE_ROWS + MENU_HEADER_HEIGHT_PX
            ))
            .overflow_y_scroll()
            .track_scroll(&self.mention_menu.scroll);
        let mut child = 0usize;
        let mut flat = 0usize;
        let mut highlighted_child = None;
        for (index, group) in groups.iter().enumerate() {
            if index > 0 {
                list = list.child(
                    div()
                        .flex_shrink_0()
                        .h(px(1.0))
                        .mx(px(10.0))
                        .my(px(4.0))
                        .bg(desktop_theme.line),
                );
                child += 1;
            }
            list = list.child(
                div()
                    .flex_shrink_0()
                    .h(px(MENU_HEADER_HEIGHT_PX))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .text_size(px(12.0))
                    .text_color(desktop_theme.secondary)
                    .child(group.section.title()),
            );
            child += 1;
            for row in &group.rows {
                if flat == highlighted {
                    highlighted_child = Some(child);
                }
                list = list.child(Self::render_menu_row(
                    row,
                    flat,
                    flat == highlighted,
                    interactive,
                    &theme,
                    desktop_theme,
                    cx,
                ));
                child += 1;
                flat += 1;
            }
        }
        if std::mem::take(&mut self.mention_menu.reveal_highlight)
            && let Some(child) = highlighted_child
        {
            self.mention_menu.scroll.scroll_to_item(child);
        }

        artisan_ui::hover_card::glass_surface(
            &theme,
            px(NATIVE_COMPOSER_RADIUS_PX),
            div().p(px(MENU_PADDING_PX)).child(list),
        )
        .id(MENU_SELECTOR)
        .debug_selector(|| MENU_SELECTOR.to_owned())
        .w_full()
        .occlude()
    }

    fn render_menu_row(
        row: &MenuRow,
        index: usize,
        highlighted: bool,
        interactive: bool,
        theme: &ArtisanTheme,
        desktop_theme: DesktopTheme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let mut element = div()
            .id(SharedString::from(format!(
                "artisan-composer-mention-row-{index}"
            )))
            .debug_selector(move || format!("{MENU_SELECTOR}-row-{index}"))
            .flex_shrink_0()
            .h(px(MENU_ROW_HEIGHT_PX))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .rounded(px(MENU_ROW_RADIUS_PX))
            .text_size(px(14.0))
            .child(
                asset_glyph(row.kind.icon())
                    .size(px(16.0))
                    .text_color(desktop_theme.secondary),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(260.0))
                    .truncate()
                    .text_color(desktop_theme.foreground)
                    .child(row.title.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(desktop_theme.secondary)
                    .child(row.description.clone()),
            );
        if highlighted {
            element = artisan_ui::hover_card::hover_fill(element, theme, px(MENU_ROW_RADIUS_PX));
        }
        if !interactive {
            return element;
        }
        let chosen = row.clone();
        element
            .cursor_pointer()
            .on_hover(cx.listener(move |composer, hovered: &bool, _, cx| {
                if *hovered && composer.mention_menu.highlighted != index {
                    composer.mention_menu.highlighted = index;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |composer, _, _, cx| {
                composer.choose_mention_row(&chosen, cx);
            }))
    }

    /// Whether the menu offers rows now, for tests that do not wait out its
    /// exit.
    #[cfg(test)]
    pub(super) fn mention_menu_offers_rows(&mut self) -> bool {
        self.mention_menu_open()
    }
}

/// Keeps rows whose title or name matches `query`, best first; an empty
/// query keeps every row in order.
fn ranked(rows: Vec<MenuRow>, query: &str) -> Vec<MenuRow> {
    if query.trim().is_empty() {
        return rows;
    }
    let mut scored = rows
        .into_iter()
        .filter_map(|row| {
            let score = score_with_keywords(&row.title, query, &[row.name.as_str()]);
            (score > 0.0).then_some((row, score))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.1.total_cmp(&left.1));
    scored.into_iter().map(|(row, _)| row).collect()
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

#[cfg(test)]
mod tests {
    use super::{Section, display_name};

    #[test]
    fn skill_names_read_as_titles() {
        assert_eq!(display_name("apple-design"), "Apple Design");
        assert_eq!(display_name("cloudflare:wrangler"), "Wrangler");
        assert_eq!(display_name("x"), "X");
    }

    #[test]
    fn connectors_lead_after_at_and_skills_otherwise() {
        assert_eq!(Section::order('@')[0], Section::Connectors);
        assert_eq!(Section::order('$')[0], Section::Skills);
        assert_eq!(Section::order('/')[0], Section::Skills);
    }
}
