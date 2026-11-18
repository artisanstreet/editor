use super::{
    ComposerAction, ComposerOwner, Dispatch, MENTION_DETAIL_WIDTH, MENTION_ROWS_SHOWN,
    MENTION_SCROLL, MENU_VIEWPORT_MARGIN,
};
use aqua_text::overflow::OverflowMode;
use aqua_ui::{
    element::{Box as UiBox, Element, Scroll},
    style::{Length, Style},
};
use editor_assets::AssetId;
use editor_ui::{
    kit::{Divider, IconSize, Picker, Row, RowItem, Surface, TextRole},
    theme::ArtisanTheme,
};
use std::rc::Rc;

pub(super) fn mention_icon(kind: super::super::mentions::Kind) -> AssetId {
    use super::super::mentions::Kind;
    match kind {
        Kind::Command => AssetId::TABLER_TERMINAL_2,
        Kind::Skill => AssetId::TABLER_FILE_SPARK,
        Kind::Connector => AssetId::TABLER_PLUG_CONNECTED,
        Kind::Thread => AssetId::TABLER_MESSAGE_CIRCLE,
    }
}

pub(super) fn mention_heading(kind: super::super::mentions::Kind) -> &'static str {
    use super::super::mentions::Kind;
    match kind {
        Kind::Command => "Commands",
        Kind::Skill => "Skills",
        Kind::Connector => "Connectors",
        Kind::Thread => "Threads",
    }
}

pub(super) fn mention_metrics(viewport: aqua_core::Size) -> super::super::mentions::Metrics {
    let row = Row::MenuItem.height().unwrap_or_default();
    let heading = Row::Heading.height().unwrap_or_default();
    let fits = viewport.h - 2.0 * MENU_VIEWPORT_MARGIN - 2.0 * MENTION_SURFACE.padding_px();
    super::super::mentions::Metrics {
        divider: Divider::Rule.height(),
        heading,
        row,
        viewport: row.mul_add(MENTION_ROWS_SHOWN, heading).min(fits).max(row),
    }
}

pub(super) const MENTION_SURFACE: Surface = Surface::Picker(Picker::List);

#[expect(
    clippy::disallowed_methods,
    reason = "the scroll clips its content, so the list carries the menu's side inset and the section rules can reach the card edges"
)]
pub(super) fn inset_sides(style: Style, side: f32, ends: f32) -> Style {
    style.padding_insets(aqua_core::Insets {
        left: side,
        right: side,
        top: ends,
        bottom: ends,
    })
}

pub(super) fn mention_menu(
    theme: ArtisanTheme,
    mentions: &super::super::mentions::Mentions,
    viewport: aqua_core::Size,
    width: f32,
    scroll: f32,
    dispatch: Dispatch,
) -> Option<Element> {
    use super::super::mentions::Kind;
    if mentions.rows.is_empty() {
        return None;
    }
    let surface = MENTION_SURFACE;
    let muted = theme.colors.text_muted;
    let mut entries = UiBox::new().layout("column");
    let mut previous = None;
    for (index, catalog_index) in mentions.rows.iter().copied().enumerate() {
        let Some(row) = mentions.catalog.get(catalog_index) else {
            continue;
        };
        let first = previous != Some(row.kind);
        if first {
            if previous.is_some() {
                entries = entries.child(Divider::Rule.build(&theme, surface.padding_px()));
            }
            entries = entries.child(
                RowItem::new(
                    format!("composer-mention-heading-{index}"),
                    theme,
                    Row::Heading,
                    surface,
                    mention_heading(row.kind),
                )
                .build(),
            );
            previous = Some(row.kind);
        }
        let leading = match first {
            true => IconSize::Md.glyph(mention_icon(row.kind), muted.to_reference_paint()),
            false => UiBox::new()
                .width(IconSize::Md.px())
                .height(IconSize::Md.px())
                .style(Style::new().flex_shrink(0.0))
                .build(),
        };
        let action = Rc::clone(&dispatch);
        let hover = Rc::clone(&dispatch);
        let mut item = RowItem::new(
            format!("composer-mention-row-{index}"),
            theme,
            Row::MenuItem,
            surface,
            row.title.clone(),
        )
        .leading(leading)
        .selected(index == mentions.highlighted)
        .on_activate(move || action(ComposerAction::SelectMention(index)))
        .on_hover(move |inside| hover(ComposerAction::HoverMention(index, inside)));
        if row.kind != Kind::Skill && !row.description.is_empty() {
            item = item.trailing(
                UiBox::new()
                    .style(
                        Style::new()
                            .flex_shrink(0.0)
                            .max_width(Length::Px(MENTION_DETAIL_WIDTH)),
                    )
                    .child(
                        TextRole::Label
                            .text(&theme, row.description.clone())
                            .gpui_source_color(muted.to_reference_text_color())
                            .overflow(OverflowMode::EndEllipsis)
                            .style(Style::new().min_width(Length::Pct(0.0)).text_wrap(false))
                            .build(),
                    )
                    .build(),
            );
        }
        entries = entries.child(item.build());
    }
    let inset = surface.padding_px();
    let list = Scroll::new()
        .layout("column")
        .offset(aqua_core::Point::new(0.0, scroll))
        .style(Style::new().max_height(Length::Px(mention_metrics(viewport).viewport)))
        .child(entries.style(inset_sides(Style::new(), inset, 0.0)).build())
        .build()
        .with_key(MENTION_SCROLL);
    let width = width.min(viewport.w - 2.0 * MENU_VIEWPORT_MARGIN);
    Some(
        surface
            .build(&theme, vec![list])
            .style(inset_sides(
                Style::new()
                    .width(Length::Px(width))
                    .gpui_source_text_color(theme.colors.text.to_reference_text_color()),
                0.0,
                inset,
            ))
            .with_key("composer-mention-menu"),
    )
}

impl ComposerOwner {
    pub fn mention_menu(
        &self,
        theme: ArtisanTheme,
        viewport: aqua_core::Size,
        width: f32,
        kept: f32,
    ) -> Option<Element> {
        let composer = self.controller.borrow();
        let mentions = &composer.mentions;
        let scroll = mentions.scroll(kept, mention_metrics(viewport));
        mention_menu(
            theme,
            mentions,
            viewport,
            width,
            scroll,
            Rc::clone(&self.dispatch),
        )
    }

    pub fn mention_revealed(&self) {
        self.controller.borrow_mut().mentions.mark_revealed();
    }

    pub fn mention_pointer(&self, pointer: Option<aqua_core::Point>) -> bool {
        self.controller.borrow_mut().mentions.pointer_at(pointer)
    }
}
