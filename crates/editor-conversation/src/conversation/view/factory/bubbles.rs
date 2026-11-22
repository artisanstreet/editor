use super::super::work;
use super::RowFactory;
use crate::conversation::fold::{self, Row, RowKey, Slot};
use artisan_api::ItemKind;
use editor_ui::kit::{Space, Spaced};
use editor_ui::theme::ArtisanTheme;

pub(super) fn truncate(
    text: String,
    color: aqua_core::GpuiTextColor,
    mono: bool,
    theme: &ArtisanTheme,
) -> aqua_ui::element::Element {
    use aqua_text::overflow::OverflowMode;
    use aqua_ui::style::{Length, Style};
    let role = work::line_role(mono);
    role.text(theme, text)
        .gpui_source_color(color)
        .overflow(OverflowMode::EndEllipsis)
        .style(
            Style::new()
                .min_width(Length::Pct(0.0))
                .flex_grow(1.0)
                .max_height(Length::Px(role.line_px(theme))),
        )
        .build()
}

pub(super) fn media_width(reading: f32, slot: Slot, lists: usize) -> f32 {
    use editor_ui::chat_bubble::{ChatBubbleSegment, SURFACE};
    let width = match (slot.is_bubble(), slot.is_card()) {
        (true, _) => 2.0f32.mul_add(
            -SURFACE.padding_px(),
            reading.min(ChatBubbleSegment::MAX_WIDTH),
        ),
        (false, true) => reading - super::super::CHAIN_INDENT.px(),
        (false, false) => reading,
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "list nesting depth is a handful of levels"
    )]
    let lists = lists as f32;
    lists.mul_add(-editor_ui::markdown::LIST_INDENT, width)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the user bubble composes content, segment position, attachments, sender and sticker"
)]
pub(super) fn user_bubble(
    content: aqua_ui::element::Element,
    index: usize,
    count: usize,
    empty: bool,
    attachments: Option<aqua_ui::element::Element>,
    sender: Option<artisan_api::EngineId>,
    sticker: Option<editor_ui::sticker::Sticker>,
    theme: &ArtisanTheme,
) -> aqua_ui::element::Element {
    use aqua_ui::element::Box as UiBox;
    use aqua_ui::style::{Length, Style};
    use editor_ui::chat_bubble::{ChatBubbleSegment, SegmentPosition};
    let mut root = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .spaced(Space::S8)
        .style(Style::new().align(aqua_ui::AlignItems::END));
    if let Some(attachments) = attachments {
        root = root.child(attachments);
    }
    if empty {
        return root.build();
    }
    let bubble = ChatBubbleSegment::new(*theme, SegmentPosition::for_index(index, count), content)
        .sticker(sticker);
    let bubble = match sender {
        Some(engine) => bubble.tinted(match engine {
            artisan_api::EngineId::Claude => theme.colors.claude,
            artisan_api::EngineId::Codex => theme.colors.text,
            artisan_api::EngineId::OpenCode => theme.colors.gray.step(9),
        }),
        None => bubble,
    };
    root.child(bubble.build()).build()
}

pub(super) fn bubble_sender(
    item: &artisan_api::Item,
    author: Option<artisan_api::EngineId>,
) -> Option<artisan_api::EngineId> {
    match &item.kind {
        ItemKind::UserMessage { sender, .. } => sender.as_ref().map(|sender| sender.engine),
        ItemKind::AssistantMessage { .. }
        | ItemKind::Tool { .. }
        | ItemKind::Agent { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => author,
    }
}

pub(crate) fn message_author(
    row: &Row,
    item: Option<&artisan_api::Item>,
    author: Option<artisan_api::EngineId>,
) -> Option<&'static str> {
    match (row, item.map(|item| &item.kind)) {
        (
            Row::Text {
                slot: Slot::Text(fold::TextSection::User),
                block: 0,
                ..
            },
            Some(ItemKind::UserMessage { sender, .. }),
        ) => Some(
            sender
                .as_ref()
                .map_or("You", |sender| sender.engine.display_name()),
        ),
        (
            Row::Text {
                slot: Slot::Prose,
                block: 0,
                ..
            },
            Some(ItemKind::AssistantMessage { .. }),
        ) => Some(author.map_or("Assistant", artisan_api::EngineId::display_name)),
        _ => None,
    }
}

pub(super) fn authored(
    element: aqua_ui::element::Element,
    author: &str,
) -> aqua_ui::element::Element {
    aqua_ui::element::Box::new()
        .width(aqua_ui::style::Length::Pct(100.0))
        .child(element)
        .build()
        .role(aqua_core::Role::Group)
        .aria_label(author)
}

impl RowFactory {
    pub(super) fn bubble_sticker(
        &self,
        owner: &super::super::row_cache::RowOwner,
        key: &RowKey,
        fresh: bool,
    ) -> Option<editor_ui::sticker::Sticker> {
        let now = self.now;
        owner
            .motion
            .borrow_mut()
            .sticker(key.clone(), fresh, self.reduced_motion, now)
    }
}
