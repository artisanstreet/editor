use crate::ui::kit::tokens::{Space, TextRole};
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_core::{GpuiBackground, Insets};
use aqua_ui::AlignItems;
use aqua_ui::element::{Box as UiBox, Element, Text};
use aqua_ui::style::{Length, Style};

const WHITE: Oklch = Oklch::new(1.0, 0.0, 0.0);
const BLACK: Oklch = Oklch::new(0.0, 0.0, 0.0);
const GRAY_900: Oklch = Oklch::new(0.21, 0.034, 264.665);
const GAP: Space = Space::S4;
const LINE: f32 = 1.0;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Divider {
    Rule,
    Labelled(String),
}

fn shades(theme: &ArtisanTheme) -> (Oklch, Oklch) {
    match theme.colors.mode.is_dark() {
        true => (BLACK.with_alpha(0.22), WHITE.with_alpha(0.08)),
        false => (GRAY_900.with_alpha(0.08), WHITE.with_alpha(0.9)),
    }
}

fn line(color: Oklch) -> Element {
    UiBox::new()
        .height(LINE)
        .style(Style::new().gpui_background(GpuiBackground::Solid(color.to_reference_paint())))
        .build()
}

fn upright(color: Oklch) -> Element {
    UiBox::new()
        .width(LINE)
        .style(
            Style::new()
                .height(Length::Pct(100.0))
                .gpui_background(GpuiBackground::Solid(color.to_reference_paint())),
        )
        .build()
}

fn rule(theme: &ArtisanTheme) -> UiBox {
    let (shade, light) = shades(theme);
    UiBox::new()
        .layout("column")
        .child(line(shade))
        .child(line(light))
}

impl Divider {
    #[must_use]
    pub fn guide(theme: &ArtisanTheme) -> Style {
        Style::new().gpui_background(GpuiBackground::Solid(theme.colors.line.to_reference_paint()))
    }

    #[must_use]
    pub fn vertical(theme: &ArtisanTheme, height: f32) -> Element {
        let (_, light) = shades(theme);
        let shade = match theme.colors.mode.is_dark() {
            true => BLACK.with_alpha(0.6),
            false => GRAY_900.with_alpha(0.16),
        };
        UiBox::new()
            .layout("row")
            .height(height)
            .gap(LINE)
            .style(Style::new().flex_shrink(0.0))
            .child(upright(shade))
            .child(upright(light))
            .build()
    }

    #[must_use]
    pub fn height(&self) -> f32 {
        let body = match self {
            Self::Rule => 2.0 * LINE,
            Self::Labelled(_) => TextRole::Caption.spec().line,
        };
        body + 2.0 * GAP.px()
    }

    #[must_use]
    pub fn build(self, theme: &ArtisanTheme, bleed: f32) -> Element {
        let margin = Insets {
            left: -bleed,
            right: -bleed,
            top: GAP.px(),
            bottom: GAP.px(),
        };
        match self {
            Self::Rule => rule(theme)
                .style(Style::new().flex_shrink(0.0).margin_insets(margin))
                .build(),
            Self::Labelled(label) => {
                let grow = || {
                    rule(theme)
                        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                        .build()
                };
                UiBox::new()
                    .gap(Space::S12.px())
                    .style(
                        Style::new()
                            .align(AlignItems::CENTER)
                            .flex_shrink(0.0)
                            .margin_insets(margin),
                    )
                    .child(grow())
                    .child(
                        Text::new(label)
                            .text_style(TextRole::Caption.style(theme))
                            .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                            .build(),
                    )
                    .child(grow())
                    .build()
            }
        }
    }
}
