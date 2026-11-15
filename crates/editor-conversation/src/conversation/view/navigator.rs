use super::Dispatch;
use crate::conversation::adapter::Action;
use aqua_text::overflow::OverflowMode;
use aqua_ui::AlignItems;
use aqua_ui::collection::List;
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};
use artisan_api::{OutlineEntry, Seq};
use editor_ui::kit::surface::drawer;
use editor_ui::kit::{Row, RowItem, Space, Spaced, Surface, TextRole};
use editor_ui::theme::ArtisanTheme;
use std::rc::Rc;

const PANEL: Surface = Surface::Menu;
const PANEL_ROW: Row = Row::MenuItem;
pub const PANEL_WIDTH: f32 = 320.0;
const DIGIT_WIDTH: f32 = 7.5;

fn number_width(count: usize) -> f32 {
    let digits = count.max(1).ilog10() + 1;
    (digits as f32 + 1.0) * DIGIT_WIDTH
}

fn entry(
    theme: &ArtisanTheme,
    number: usize,
    label: &str,
    current: bool,
    width: f32,
) -> Element {
    let role = match current {
        true => TextRole::LabelStrong,
        false => TextRole::Label,
    };
    let number = UiBox::new()
        .width(width)
        .style(Style::new().flex_shrink(0.0))
        .child(
            TextRole::Label
                .text(theme, format!("{number}."))
                .gpui_source_color(theme.colors.text_muted_glass.to_reference_text_color())
                .wrap(false)
                .build(),
        )
        .build();
    let label = role
        .text(theme, label.to_owned())
        .gpui_source_color(theme.colors.text.to_reference_text_color())
        .overflow(OverflowMode::EndEllipsis)
        .wrap(false)
        .style(Style::new().min_width(Length::Pct(0.0)).flex_shrink(1.0))
        .build();
    UiBox::new()
        .layout("row")
        .spaced(Space::S4)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Pct(0.0)),
        )
        .child(number)
        .child(label)
        .build()
}

pub fn panel(
    markers: &[OutlineEntry],
    active: Option<Seq>,
    theme: ArtisanTheme,
    progress: f32,
    dispatch: Dispatch,
) -> Element {
    let pitch = PANEL_ROW.height().unwrap_or_default();
    let width = number_width(markers.len());
    let list = List::new()
        .items(markers.iter().cloned().enumerate())
        .key(|(_, marker): &(usize, OutlineEntry)| format!("navigator-row-{}", marker.id))
        .row(move |(index, marker): &(usize, OutlineEntry)| {
            let current = active == Some(marker.position);
            let position = marker.position;
            let callback = Rc::clone(&dispatch);
            RowItem::new(
                format!("navigator-row-hit-{}", marker.id),
                theme,
                PANEL_ROW,
                PANEL,
                marker.label.clone(),
            )
            .body(entry(&theme, index + 1, &marker.label, current, width))
            .on_activate(move || callback(Action::Jump(position)))
            .build()
        })
        .virtualize()
        .estimated_row_height(Length::Px(pitch))
        .style(
            Style::new()
                .flex_grow(1.0)
                .min_height(Length::Pct(0.0))
                .width(Length::Pct(100.0)),
        )
        .build()
        .with_key("turn-navigator-list");
    drawer::build(&theme, PANEL_WIDTH, progress, vec![list])
}

pub fn panel_scroll_offset(active: usize, count: usize, viewport_height: f32) -> f32 {
    let pitch = PANEL_ROW.height().unwrap_or_default();
    let full = count as f32 * pitch;
    ((active as f32 + 0.5) * pitch - viewport_height / 2.0)
        .clamp(0.0, (full - viewport_height).max(0.0))
}
