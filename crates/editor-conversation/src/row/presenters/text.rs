//! Text inside cards and activities: tool output, answers, notices and
//! stored fields. Text rows draw the body the conversation prepared.
use super::super::{body_line, body_text};
use crate::content::{SourceField, SourceOwners, StoredText};
use crate::fold::Slot;
use artisan_api::{Item, ItemId, ItemKind, NoticeKind, TextContent};
use editor_ui::selectable_text::{SelectableText, register_selection_source};
use editor_ui::{AssetId, selectable_text::SelectionSource, theme::ArtisanTheme};
use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    WeakEntity, Window, div, px, svg,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

const LEGACY_ANSWER_PREFIX: &str = "You answered: ";

/// A row's own tool output and answers, each one selectable source, kept
/// while their text stays the same.
pub(in crate::row) type PlainSources =
    RefCell<HashMap<(ItemId, Slot), (SharedString, Rc<SelectionSource>)>>;

/// Literal text registered with the window's selection as it draws.
#[derive(IntoElement)]
struct PlainText {
    source: Rc<SelectionSource>,
    text: SharedString,
    key: SharedString,
    scope: SharedString,
    order: Vec<u64>,
    theme: ArtisanTheme,
}
impl RenderOnce for PlainText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        register_selection_source(&self.source, window, cx);
        SelectableText::retained(self.key, self.text, self.theme, Vec::new())
            .selection_scope(self.scope)
            .selection_order(self.order, "\n\n")
    }
}

pub(in crate::row) struct TextPresenter<'a> {
    pub theme: ArtisanTheme,
    pub owners: &'a SourceOwners,
    pub host: Option<WeakEntity<editor_core::HostData>>,
    pub plain: &'a PlainSources,
    pub scope: &'a SharedString,
    pub agent_scope: bool,
}
impl TextPresenter<'_> {
    pub(super) fn output(&self, item: &Item, text: &str) -> AnyElement {
        self.plain_text(item, Slot::Output, text)
    }
    pub(super) fn answer(&self, item: &Item) -> AnyElement {
        let Some(text) = SourceField::AnswerText.text(item) else {
            return div().into_any_element();
        };
        match text {
            // Answers saved before the dashed bubble carried a "You answered: " prefix.
            TextContent::Inline(text) if text.contains(LEGACY_ANSWER_PREFIX) => {
                self.plain_text(item, Slot::Answer, &text.replace(LEGACY_ANSWER_PREFIX, ""))
            }
            TextContent::Inline(text) => self.plain_text(item, Slot::Answer, text),
            TextContent::Stored(_) => self.content_text(item, SourceField::AnswerText),
        }
    }

    pub(super) fn inherited_content(&self, item: &Item, field: SourceField) -> AnyElement {
        match field.text(item) {
            Some(TextContent::Inline(text)) => text.clone().into_any_element(),
            _ => self.content_text(item, field),
        }
    }

    pub(in crate::row) fn content_text(&self, item: &Item, field: SourceField) -> AnyElement {
        let Some(text) = field.text(item) else {
            return div().into_any_element();
        };
        match text {
            TextContent::Inline(text) => body_text(text, &self.theme).into_any_element(),
            TextContent::Stored(source) => StoredText {
                host: self.host.clone(),
                source: source.clone(),
                theme: self.theme,
                key: (item.id.clone(), field),
                owners: Rc::clone(self.owners),
            }
            .into_any_element(),
        }
    }

    /// Tool output or an answer: literal, selectable as authored.
    fn plain_text(&self, item: &Item, slot: Slot, text: &str) -> AnyElement {
        let mut order = crate::fold::selection_order(self.agent_scope, item.position, slot);
        order.push(u64::from(slot == Slot::Output));
        let (text, source) = {
            let mut plain = self.plain.borrow_mut();
            let cached = plain.entry((item.id.clone(), slot)).or_insert_with(|| {
                let text: SharedString = text.to_owned().into();
                let source = SelectionSource::single(
                    self.scope.clone(),
                    order.clone(),
                    text.clone(),
                    "\n\n",
                );
                (text, source)
            });
            if cached.0.as_ref() != text {
                let text: SharedString = text.to_owned().into();
                *cached = (
                    text.clone(),
                    SelectionSource::single(self.scope.clone(), order.clone(), text, "\n\n"),
                );
            }
            cached.clone()
        };
        let (scope, key, theme) = (
            self.scope.clone(),
            SharedString::from(format!("{slot}-{}", item.id)),
            self.theme,
        );
        PlainText {
            source,
            text,
            key,
            scope,
            order,
            theme,
        }
        .into_any_element()
    }

    pub(in crate::row) fn notice(&self, item: &Item) -> AnyElement {
        let ItemKind::Notice { kind, text } = &item.kind else {
            return div().into_any_element();
        };
        let theme = self.theme;
        let muted = theme.colors.text_muted.to_paint();
        let (asset, prefix) = match kind {
            NoticeKind::Goal { state } => (AssetId::TABLER_TARGET_ARROW, format!("Goal {state}: ")),
            NoticeKind::Safety => (AssetId::TABLER_SHIELD_LOCK, String::new()),
            NoticeKind::Reroute { model } => (
                AssetId::TABLER_ARROW_RIGHT,
                format!("Switched to {model}: "),
            ),
            NoticeKind::Info => (AssetId::TABLER_BELL, String::new()),
            // Its own card in the thread; a line wherever else it shows.
            NoticeKind::YouShouldKnow { .. } => (AssetId::TABLER_SPARKLES, String::new()),
        };
        body_line(div(), &theme)
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.spacing.steps(1.0))
            .text_color(muted)
            .child(
                svg()
                    .path(asset.as_str())
                    .size(px(16.0))
                    .flex_shrink_0()
                    .text_color(muted),
            )
            .child(match text {
                TextContent::Inline(text) => {
                    div().min_w_0().truncate().child(format!("{prefix}{text}"))
                }
                TextContent::Stored(_) => div()
                    .min_w_0()
                    .child(prefix)
                    .child(self.content_text(item, SourceField::Notice)),
            })
            .into_any_element()
    }
}
