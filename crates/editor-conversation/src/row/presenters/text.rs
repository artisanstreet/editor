//! Canonical text sections construct existing prepared and selection leaves.
//! Preparation, source windows and selection storage keep their current owners.
use super::super::{
    PlainBody, PlainText, PreparedText, Shared, StoredPreparedText, body_line, body_text,
};
use crate::content::{SourceField, SourceOwners, StoredText};
use crate::fold::Slot;
use crate::fold::TextSection;
use artisan_api::{Item, ItemKind, NoticeKind, TextContent};
use editor_ui::{AssetId, markdown_renderer::MarkdownBodyTone, selectable_text::SelectionSource};
use gpui::{
    AnyElement, IntoElement, ParentElement as _, SharedString, Styled as _, WeakEntity, div, px,
    svg,
};
use std::{rc::Rc, sync::Arc};
pub(in crate::row) struct TextPresenter<'a> {
    pub shared: &'a Rc<Shared>,
    pub owners: &'a SourceOwners,
    pub source_host: Option<WeakEntity<editor_core::HostData>>,
}
impl TextPresenter<'_> {
    pub(in crate::row) fn assistant(&self, item: &Item, block: usize) -> AnyElement {
        let ItemKind::AssistantMessage { text, .. } = &item.kind else {
            return div().into_any_element();
        };
        self.prepared_text(item, Slot::Prose, text, MarkdownBodyTone::Foreground, block)
    }
    pub(in crate::row) fn section(
        &self,
        item: &Item,
        section: TextSection,
        block: usize,
    ) -> AnyElement {
        let Some(text) = section.text(item) else {
            return div().into_any_element();
        };
        self.prepared_text(
            item,
            section.slot(),
            text,
            if section == TextSection::Message {
                MarkdownBodyTone::Muted
            } else {
                MarkdownBodyTone::Foreground
            },
            block,
        )
    }
    pub(in crate::row) fn stored_reply(
        &self,
        item: &Item,
        block: usize,
        reveal: Rc<editor_ui::markdown_renderer::PreparedReveal>,
    ) -> AnyElement {
        let ItemKind::AssistantMessage {
            text: TextContent::Stored(source),
            ..
        } = &item.kind
        else {
            return div().into_any_element();
        };
        self.stored_formatted(
            item,
            Slot::Prose,
            source,
            block,
            MarkdownBodyTone::Foreground,
            Some(reveal),
        )
    }
    pub(super) fn output(&self, item: &Item, text: &str) -> AnyElement {
        self.plain_text(item, Slot::Output, text)
    }
    pub(super) fn answer(&self, item: &Item) -> AnyElement {
        let Some(text) = SourceField::AnswerText.text(item) else {
            return div().into_any_element();
        };
        match text {
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
            TextContent::Inline(text) => body_text(text, &self.shared.theme).into_any_element(),
            TextContent::Stored(source) => StoredText {
                host: self.source_host.clone(),
                source: source.clone(),
                theme: self.shared.theme,
                key: (item.id.clone(), field),
                owners: self.owners.clone(),
            }
            .into_any_element(),
        }
    }

    fn prepared_text(
        &self,
        item: &Item,
        section: Slot,
        text: &TextContent,
        tone: MarkdownBodyTone,
        block: usize,
    ) -> AnyElement {
        let TextContent::Inline(text) = text else {
            let TextContent::Stored(source) = text else {
                unreachable!()
            };
            return self.stored_formatted(item, section, source, block, tone, None);
        };
        let source = {
            let mut sources = self.shared.sources.borrow_mut();
            let cached = sources
                .entry((item.id.clone(), section))
                .or_insert_with(|| (item.seq, Arc::from(text.as_str())));
            if cached.0 != item.seq && cached.1.as_ref() != text {
                cached.1 = Arc::from(text.as_str());
            }
            cached.0 = item.seq;
            cached.1.clone()
        };
        PreparedText {
            shared: self.shared.clone(),
            source,
            key: format!("{}:{section}", item.id).into(),
            order: crate::fold::selection_order(
                self.shared.agent_scope.get(),
                item.position,
                section,
            ),
            block,
            tone,
        }
        .into_any_element()
    }

    fn stored_formatted(
        &self,
        item: &Item,
        slot: Slot,
        source: &artisan_api::ContentRef,
        block: usize,
        tone: MarkdownBodyTone,
        reveal: Option<Rc<editor_ui::markdown_renderer::PreparedReveal>>,
    ) -> AnyElement {
        StoredPreparedText {
            shared: self.shared.clone(),
            source: source.clone(),
            host: self.source_host.clone(),
            owners: self.owners.clone(),
            id: item.id.clone(),
            slot,
            block,
            tone,
            literal: None,
            visual: None,
            queue_key: (slot == Slot::Prose).then(|| {
                crate::fold::Row::ProseBlock {
                    id: item.id.clone(),
                    block,
                }
                .key()
            }),
            reveal,
            order: crate::fold::selection_order(self.shared.agent_scope.get(), item.position, slot),
        }
        .into_any_element()
    }

    fn plain_text(&self, item: &Item, slot: Slot, text: &str) -> AnyElement {
        let id = &item.id;
        let scope = self.shared.selection_scope.borrow().clone();
        let mut order =
            crate::fold::selection_order(self.shared.agent_scope.get(), item.position, slot);
        order.push(if slot == Slot::Output { 1 } else { 0 });
        let (text, source) = {
            let mut sources = self.shared.plain_sources.borrow_mut();
            let cached = sources.entry((id.clone(), slot)).or_insert_with(|| {
                let text: SharedString = text.to_owned().into();
                let source =
                    SelectionSource::single(scope.clone(), order.clone(), text.clone(), "\n\n");
                PlainBody {
                    seq: item.seq,
                    chunks: vec![text.clone()],
                    source,
                }
            });
            if cached.chunks.len() != 1 || cached.chunks[0].as_ref() != text {
                let text: SharedString = text.to_owned().into();
                *cached = PlainBody {
                    seq: item.seq,
                    chunks: vec![text.clone()],
                    source: SelectionSource::single(
                        scope.clone(),
                        order.clone(),
                        text.clone(),
                        "\n\n",
                    ),
                };
            }
            cached.seq = item.seq;
            (cached.chunks[0].clone(), cached.source.clone())
        };
        PlainText {
            source,
            text,
            key: format!("{slot}-{id}").into(),
            scope,
            order,
            theme: self.shared.theme,
        }
        .into_any_element()
    }

    pub(in crate::row) fn notice(&self, item: &Item) -> AnyElement {
        let ItemKind::Notice { kind, text } = &item.kind else {
            return div().into_any_element();
        };
        let theme = self.shared.theme;
        let muted = theme.colors.muted_foreground.to_paint();
        let (asset, prefix) = match kind {
            NoticeKind::Goal { state } => (AssetId::TABLER_TARGET_ARROW, format!("Goal {state}: ")),
            NoticeKind::Safety => (AssetId::TABLER_SHIELD_LOCK, String::new()),
            NoticeKind::Reroute { model } => (
                AssetId::TABLER_ARROW_RIGHT,
                format!("Switched to {model}: "),
            ),
            NoticeKind::Info => (AssetId::TABLER_BELL, String::new()),
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
