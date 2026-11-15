use super::Surface;
use super::glass::{self, Shade};
use crate::ui::kit::tokens::Radius;
use crate::ui::theme::ArtisanTheme;
use aqua_core::{Insets, Transform2D};
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};

const SHEET: Surface = Surface::Menu;

#[must_use]
pub fn build(theme: &ArtisanTheme, width: f32, progress: f32, children: Vec<Element>) -> Element {
    let card = Radius::Shell.px();
    let radius = SHEET.radius_px();
    let pad = SHEET.padding_px();
    let hidden = 1.0 - progress.clamp(0.0, 1.0);
    let sheet = UiBox::new()
        .layout("column")
        .radius(radius)
        .style(
            Style::new()
                .absolute_at(Some(-radius), Some(0.0), None, Some(0.0))
                .width(Length::Px(width + radius))
                .padding_insets(Insets {
                    left: pad + radius,
                    top: pad,
                    right: pad,
                    bottom: pad,
                })
                .shadows(SHEET.elevation().shadows(theme))
                .transform(Transform2D::translate(-width * hidden, 0.0)),
        )
        .child(glass::paint(theme, Shade::Clear, radius))
        .children(children)
        .build();
    UiBox::new()
        .layout("column")
        .radius(card)
        .clip(true)
        .style(
            Style::new()
                .width(Length::Px(width + card))
                .height(Length::Pct(100.0)),
        )
        .child(sheet)
        .build()
}
