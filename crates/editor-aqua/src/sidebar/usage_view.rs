use std::rc::Rc;

use aqua_core::Insets;
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element},
    style::{Length, Style},
};
use artisan_api::{EngineId, UsageLimits};
use editor_assets::AssetId;

use super::usage::{ROW_SEGMENTS, headline, lit_of};
use crate::ui::{
    asset::source_asset_glyph,
    theme::{ArtisanTheme, Opacity, ThemeMode},
};

pub fn engine_icon(theme: &ArtisanTheme, engine: EngineId) -> AssetId {
    match engine {
        EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
        EngineId::Codex => AssetId::SVGL_OPENAI,
        EngineId::OpenCode if theme.colors.mode == ThemeMode::Dark => AssetId::BRANDS_OPENCODE,
        EngineId::OpenCode => AssetId::BRANDS_OPENCODE_LIGHT,
    }
}

pub fn engine_accent(theme: &ArtisanTheme, engine: EngineId) -> aqua_core::GpuiHsla {
    match engine {
        EngineId::Claude => theme.colors.claude,
        EngineId::Codex => theme.colors.text,
        EngineId::OpenCode => theme.colors.gray.step(9),
    }
    .to_reference_paint()
}

pub fn profile_usage(
    reports: &[(EngineId, UsageLimits)],
    theme: ArtisanTheme,
    hover: Rc<dyn Fn(EngineId, usize, bool)>,
) -> Element {
    let mut section = UiBox::new()
        .layout("column")
        .gap(8.0)
        .width(Length::Pct(100.0))
        .style(Style::new().padding_insets(Insets {
            left: 12.0,
            right: 12.0,
            top: 8.0,
            bottom: 8.0,
        }));
    for (engine, usage) in reports {
        let Some((index, window)) = headline(usage) else {
            continue;
        };
        let engine = *engine;
        let lit = lit_of(window.used_percent, ROW_SEGMENTS);
        let accent = engine_accent(&theme, engine);
        let dim = theme
            .colors
            .text
            .at(Opacity::MeterTrack)
            .to_reference_paint();
        let mut ticks = UiBox::new()
            .height(16.0)
            .gap(1.5)
            .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)));
        for tick in 0..ROW_SEGMENTS {
            ticks = ticks.child(
                UiBox::new()
                    .height(16.0)
                    .style(
                        Style::new()
                            .gpui_background(aqua_core::GpuiBackground::Solid(if tick < lit {
                                accent
                            } else {
                                dim
                            }))
                            .flex_grow(1.0)
                            .min_width(Length::Px(0.0)),
                    )
                    .build(),
            );
        }
        let icon_width = if engine == EngineId::OpenCode {
            12.8
        } else {
            16.0
        };
        let mark = UiBox::new()
            .width(16.0)
            .height(16.0)
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::CENTER)
                    .flex_shrink(0.0),
            )
            .child(source_asset_glyph(
                engine_icon(&theme, engine),
                icon_width,
                theme.colors.text.to_reference_paint(),
            ))
            .build();
        let callback = hover.clone();
        section = section.child(
            UiBox::new()
                .height(24.0)
                .gap(12.0)
                .style(Style::new().align(AlignItems::CENTER))
                .child(mark)
                .child(ticks.build())
                .build()
                .with_key(format!("artisan-profile-usage-meter-{engine}"))
                .aria_label(format!("{} usage", engine.display_name()))
                .on_hover_change(move |hovered| callback(engine, index, hovered)),
        );
    }
    section.build().with_key("artisan-profile-usage")
}

pub fn tooltip(
    sidebar: &super::controller::Sidebar,
    core: &crate::core::Core,
    theme: ArtisanTheme,
    anchor: aqua_core::Rect,
    menu_bounds: aqua_core::Rect,
    now: artisan_api::Timestamp,
) -> Option<Element> {
    Some(
        tooltip_panel(sidebar, core, theme, now)?.style(Style::new().absolute_at(
            Some(anchor.x - menu_bounds.x + anchor.w + 8.0),
            Some(anchor.y - menu_bounds.y),
            None,
            None,
        )),
    )
}

pub fn tooltip_panel(
    sidebar: &super::controller::Sidebar,
    core: &crate::core::Core,
    theme: ArtisanTheme,
    now: artisan_api::Timestamp,
) -> Option<Element> {
    use crate::ui::{
        glass::{GlassStrength, glass_card_shadows, glass_layers},
        theme::{Elevation, RadiusStep, RadiusTokens, TextStep, TypographyTokens},
    };
    use aqua_text::{FontFamily, StyledSpan, TextStyle};
    use aqua_ui::element::Text;

    let (engine, index) = sidebar.usage_hover?;
    let window = sidebar
        .host_data(core)?
        .engines
        .get(&engine)?
        .usage
        .as_ref()?
        .windows
        .get(index)?;
    let value = super::usage::tooltip_text(window, now);
    let text_color = theme.colors.text.to_reference_text_color();
    let highlight = [
        text_color.hue_degrees,
        text_color.saturation,
        text_color.lightness,
        text_color.alpha,
    ];
    let text_style = TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: TypographyTokens::size(TextStep::Xs),
        line_height_px: 16.0,
        ..TextStyle::default()
    };
    let mut spans = Vec::new();
    let mut start = 0;
    let mut previous = None;
    for (index, character) in value.char_indices() {
        let marked = character.is_ascii_digit() || character == '%';
        if previous.is_some_and(|old| old != marked) {
            let span = StyledSpan::new(&value[start..index], text_style.clone());
            spans.push(if previous == Some(true) {
                span.gpui_source_color(highlight)
            } else {
                span
            });
            start = index;
        }
        previous = Some(marked);
    }
    let span = StyledSpan::new(&value[start..], text_style.clone());
    spans.push(if previous == Some(true) {
        span.gpui_source_color(highlight)
    } else {
        span
    });
    let text = Text::new(value)
        .spans(spans)
        .text_style(text_style)
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .build();
    let radius = RadiusTokens::value(RadiusStep::X2l);
    Some(
        UiBox::new()
            .layout("column")
            .radius(radius)
            .clip(true)
            .style(
                Style::new()
                    .max_width(Length::Px(224.0))
                    .padding_insets(Insets {
                        left: 12.0,
                        right: 12.0,
                        top: 8.0,
                        bottom: 8.0,
                    })
                    .shadows(
                        glass_card_shadows(&theme, Elevation::Overlay)
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                    ),
            )
            .child(glass_layers(&theme, GlassStrength::Strong, radius))
            .child(text)
            .build()
            .with_key("artisan-profile-usage-tooltip")
            .debug_selector("artisan-profile-usage-tooltip"),
    )
}
