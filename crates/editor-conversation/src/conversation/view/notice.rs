use crate::conversation::content::SourceField;
use editor_ui::kit::{IconSize, Space, Spaced, TextRole};
use editor_ui::theme::ArtisanTheme;
use aqua_ui::AlignItems;
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};
use artisan_api::{Item, ItemKind, NoticeKind, TextContent};
use editor_assets::AssetId;

pub fn render<E>(
    item: &Item,
    theme: ArtisanTheme,
    truncate: impl FnOnce(String, aqua_core::GpuiTextColor) -> Element,
    source: impl FnOnce(SourceField) -> Result<Element, E>,
) -> Result<Option<Element>, E> {
    let ItemKind::Notice { kind, text } = &item.kind else {
        return Ok(None);
    };
    let (asset, prefix) = match kind {
        NoticeKind::Goal { state } => (AssetId::TABLER_TARGET_ARROW, format!("Goal {state}: ")),
        NoticeKind::Safety => (AssetId::TABLER_SHIELD_LOCK, String::new()),
        NoticeKind::Reroute { model } => (
            AssetId::TABLER_ARROW_RIGHT,
            format!("Switched to {model}: "),
        ),
        NoticeKind::Info => (AssetId::TABLER_BELL, String::new()),
        NoticeKind::YouShouldKnow { .. } => (AssetId::TABLER_SPARKLES, String::new()),
    };
    let muted = theme.colors.text_muted.to_reference_text_color();
    let content = match text {
        TextContent::Inline(text) => truncate(
            format!("{prefix}{text}"),
            theme.colors.text_muted.to_reference_text_color(),
        ),
        TextContent::Stored(_) => {
            let prefix = TextRole::Prose
                .text(&theme, prefix)
                .gpui_source_color(muted)
                .build();
            UiBox::new()
                .style(Style::new().min_width(Length::Pct(0.0)))
                .child(prefix)
                .child(source(SourceField::Notice)?)
                .build()
        }
    };
    Ok(Some(
        UiBox::new()
            .layout("row")
            .spaced(Space::S4)
            .style(
                Style::new()
                    .min_width(Length::Pct(0.0))
                    .align(AlignItems::CENTER),
            )
            .child(IconSize::Md.glyph(asset, theme.colors.text_muted.to_reference_paint()))
            .child(content)
            .build(),
    ))
}
