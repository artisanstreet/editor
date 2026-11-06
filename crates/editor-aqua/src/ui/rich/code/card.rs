use crate::ui::glass::{GlassStrength, glass_card_shadows, glass_highlight, glass_material};
use crate::ui::rich::narrative::callout::text;
use crate::ui::theme::{ArtisanTheme, Elevation, Opacity, RadiusStep, RadiusTokens};
use aqua_core::{
    CornerRadii, DisplayItem, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, Insets,
};
use aqua_text::FontWeight;
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};

#[derive(Clone, Copy)]
pub(crate) enum CopyPlacement {
    TopRight,
    CenterRight,
}

pub(crate) fn envelope(
    theme: &ArtisanTheme,
    title: Option<&str>,
    subtitle: Option<&str>,
    caption: Option<&str>,
    content: Element,
) -> Element {
    let mut body = ui::Box::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .gap(12.0);
    if title.is_some() || subtitle.is_some() {
        let mut heading = ui::Box::new().layout("column").gap(2.0);
        if let Some(title) = title {
            heading = heading.child(text(
                theme,
                title,
                15.0,
                22.0,
                FontWeight(600),
                theme.colors.text,
            ));
        }
        if let Some(subtitle) = subtitle {
            heading = heading.child(text(
                theme,
                subtitle,
                13.0,
                20.0,
                FontWeight::NORMAL,
                theme.colors.text_muted,
            ));
        }
        body = body.child(heading.build());
    }
    body = body.child(content);
    if let Some(caption) = caption {
        body = body.child(text(
            theme,
            caption,
            13.0,
            20.0,
            FontWeight::NORMAL,
            theme.colors.text_muted,
        ));
    }
    body.build()
}

pub(crate) fn render(
    theme: &ArtisanTheme,
    content: Element,
    copy: Option<(Element, CopyPlacement)>,
    balanced: bool,
) -> Element {
    let radius = RadiusTokens::value(RadiusStep::X2l);
    let inset = theme.spacing.steps(2.0);
    let gutter = inset * 2.0 + theme.density.control_sm;
    let pad = theme.density.card_padding_compact;
    let has_copy = copy.is_some();
    let material = glass_material(theme, GlassStrength::Quiet);
    let highlight = glass_highlight(theme, GlassStrength::Quiet);
    let layers = ui::Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .scroll_stable()
        .on_paint(move |bounds| {
            [material, highlight]
                .into_iter()
                .map(|gradient| DisplayItem::GpuiBackground {
                    bounds,
                    corners: CornerRadii::uniform(radius),
                    background: GpuiBackground::LinearGradient {
                        angle: gradient.angle_degrees,
                        color_space: GpuiGradientColorSpace::Oklab,
                        stops: [
                            GpuiGradientStop {
                                percentage: 0.0,
                                color: gradient.start.to_reference_paint(),
                            },
                            GpuiGradientStop {
                                percentage: gradient.end_position,
                                color: gradient.end.to_reference_paint(),
                            },
                        ],
                    },
                })
                .collect()
        })
        .build();
    let mut card = ui::Box::new()
        .width(Length::Pct(100.0))
        .radius(radius)
        .style(
            Style::new()
                .min_width(Length::Px(0.0))
                .padding_insets(Insets {
                    left: if has_copy && balanced { gutter } else { pad },
                    right: if has_copy { gutter } else { pad },
                    top: pad,
                    bottom: pad,
                })
                .gpui_source_text_color(theme.colors.text.to_reference_text_color())
                .gpui_background(GpuiBackground::Solid(
                    theme
                        .colors
                        .text
                        .at(Opacity::GlassBase)
                        .to_reference_paint(),
                ))
                .gpui_shadows(
                    glass_card_shadows(theme, Elevation::Raised)
                        .into_iter()
                        .map(aqua_ui::style::GpuiShadowLayer::from)
                        .collect(),
                ),
        )
        .child(layers);
    if let Some((action, placement)) = copy {
        let absolute = match placement {
            CopyPlacement::TopRight => {
                Style::new().absolute_at(None, Some(inset), Some(inset), None)
            }
            CopyPlacement::CenterRight => {
                Style::new().absolute_at(None, Some(0.0), Some(inset), Some(0.0))
            }
        };
        card = card.child(
            ui::Box::new()
                .layout("row")
                .style(absolute.align(aqua_ui::AlignItems::CENTER))
                .child(action)
                .build(),
        );
    }
    card.child(
        ui::Box::new()
            .width(Length::Pct(100.0))
            .style(Style::new().min_width(Length::Px(0.0)))
            .child(content)
            .build(),
    )
    .build()
}
