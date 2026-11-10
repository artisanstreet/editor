use aqua_core::Insets;
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};

use editor_ui::{
    glass::{self, GlassStrength},
    theme::{
        ArtisanTheme, Elevation, Opacity, RadiusStep, RadiusTokens, TextStep, TypographyTokens,
    },
};

fn text(
    theme: &ArtisanTheme,
    value: impl Into<String>,
    size: f32,
    line_height: f32,
    color: aqua::GpuiTextColor,
    medium: bool,
) -> Element {
    Text::new(value.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            line_height_px: line_height,
            weight: if medium {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            },
            ..TextStyle::default()
        })
        .gpui_source_color(color)
        .wrap(true)
        .build()
}

pub fn header(theme: &ArtisanTheme, title: &str, description: &str) -> Element {
    UiBox::new()
        .layout("column")
        .gap(8.0)
        .child(text(
            theme,
            title,
            TypographyTokens::size(TextStep::Xl),
            28.0,
            theme.colors.text.to_reference_text_color(),
            true,
        ))
        .child(text(
            theme,
            description,
            TypographyTokens::size(TextStep::Sm),
            20.0,
            theme.colors.text_muted.to_reference_text_color(),
            false,
        ))
        .build()
}

pub fn section(
    theme: &ArtisanTheme,
    title: &str,
    intro: Option<&str>,
    action: Option<Element>,
    body: Element,
) -> Element {
    let mut heading = UiBox::new()
        .layout("column")
        .gap(4.0)
        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
        .child(text(
            theme,
            title,
            TypographyTokens::size(TextStep::Sm),
            20.0,
            theme.colors.text.to_reference_text_color(),
            true,
        ));
    if let Some(intro) = intro {
        heading = heading.child(text(
            theme,
            intro,
            TypographyTokens::size(TextStep::XsMid),
            18.0,
            theme.colors.text_muted.to_reference_text_color(),
            false,
        ));
    }
    let mut head = UiBox::new()
        .gap(16.0)
        .style(
            Style::new()
                .align(AlignItems::END)
                .justify(JustifyContent::SPACE_BETWEEN)
                .padding_insets(Insets {
                    left: 4.0,
                    right: 4.0,
                    ..Insets::default()
                }),
        )
        .child(heading.build());
    if let Some(action) = action {
        head = head.child(action);
    }
    UiBox::new()
        .layout("column")
        .gap(14.0)
        .style(Style::new().margin_insets(Insets {
            top: 44.0,
            ..Insets::default()
        }))
        .child(head.build())
        .child(body)
        .build()
}

pub fn card(theme: &ArtisanTheme, blocks: Vec<Element>) -> Element {
    let radius = RadiusTokens::value(RadiusStep::X2l);
    let shadows = glass::glass_card_shadows(theme, Elevation::Raised)
        .into_iter()
        .map(aqua_ui::style::GpuiShadowLayer::from)
        .collect();
    let material = glass::glass_material(theme, GlassStrength::Quiet);
    let highlight = glass::glass_highlight(theme, GlassStrength::Quiet);
    let layer = |gradient: glass::Gradient| {
        UiBox::new()
            .radius(radius)
            .style(
                Style::new()
                    .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .gpui_background(aqua::GpuiBackground::LinearGradient {
                        angle: gradient.angle_degrees,
                        color_space: aqua::GpuiGradientColorSpace::Oklab,
                        stops: [
                            aqua::GpuiGradientStop {
                                percentage: 0.0,
                                color: gradient.start.to_reference_paint(),
                            },
                            aqua::GpuiGradientStop {
                                percentage: gradient.end_position,
                                color: gradient.end.to_reference_paint(),
                            },
                        ],
                    }),
            )
            .build()
    };
    let mut card = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .radius(radius)
        .style(
            Style::new()
                .min_width(Length::Px(0.0))
                .gpui_background(aqua::GpuiBackground::Solid(
                    theme
                        .colors
                        .text
                        .at(Opacity::GlassBase)
                        .to_reference_paint(),
                ))
                .gpui_shadows(shadows)
                .gpui_source_text_color(theme.colors.text.to_reference_text_color()),
        )
        .child(layer(material))
        .child(layer(highlight));
    for (index, block) in blocks.into_iter().enumerate() {
        if index != 0 {
            card = card.child(
                UiBox::new()
                    .height(1.0)
                    .style(
                        Style::new()
                            .gpui_background(aqua::GpuiBackground::Solid(
                                theme.colors.text.at(Opacity::Hairline).to_reference_paint(),
                            ))
                            .margin_insets(Insets {
                                left: 18.0,
                                right: 18.0,
                                ..Insets::default()
                            }),
                    )
                    .build(),
            );
        }
        card = card.child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(Style::new().padding_insets(Insets {
                    left: 18.0,
                    right: 18.0,
                    ..Insets::default()
                }))
                .child(block)
                .build(),
        );
    }
    card.build()
}

pub fn row(
    theme: &ArtisanTheme,
    title: impl Into<String>,
    description: impl Into<String>,
    control: Option<Element>,
) -> Element {
    let description = description.into();
    let mut content = UiBox::new()
        .layout("column")
        .gap(3.0)
        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
        .child(text(
            theme,
            title,
            TypographyTokens::size(TextStep::Sm),
            20.0,
            theme.colors.text.to_reference_text_color(),
            false,
        ));
    if !description.is_empty() {
        content = content.child(text(
            theme,
            description,
            TypographyTokens::size(TextStep::XsMid),
            18.0,
            theme.colors.text_muted.to_reference_text_color(),
            false,
        ));
    }
    let mut row = UiBox::new()
        .width(Length::Pct(100.0))
        .gap(32.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::SPACE_BETWEEN)
                .padding_insets(Insets {
                    top: 16.0,
                    bottom: 16.0,
                    ..Insets::default()
                }),
        )
        .child(content.build());
    if let Some(control) = control {
        row = row.child(
            UiBox::new()
                .style(Style::new().flex_shrink(0.0))
                .child(control)
                .build(),
        );
    }
    row.build()
}

pub fn note(theme: &ArtisanTheme, value: &str) -> Element {
    UiBox::new()
        .width(Length::Pct(100.0))
        .style(Style::new().padding_insets(Insets {
            top: 16.0,
            bottom: 16.0,
            ..Insets::default()
        }))
        .child(text(
            theme,
            value,
            TypographyTokens::size(TextStep::Sm),
            20.0,
            theme.colors.text_muted.to_reference_text_color(),
            false,
        ))
        .build()
}

pub fn error_text(theme: &ArtisanTheme, value: impl Into<String>) -> Element {
    UiBox::new()
        .width(Length::Pct(100.0))
        .style(Style::new().padding_insets(Insets {
            left: 4.0,
            right: 4.0,
            ..Insets::default()
        }))
        .child(text(
            theme,
            value,
            TypographyTokens::size(TextStep::XsMid),
            18.0,
            theme.colors.destructive_text.to_reference_text_color(),
            false,
        ))
        .build()
}
