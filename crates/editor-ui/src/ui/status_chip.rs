use super::{
    asset::source_asset_glyph,
    kit::{Surface, TextRole, Tooltip},
    theme::{ArtisanTheme, BannerColors, Oklch, Opacity, RadiusStep, RadiusTokens},
};
use aqua_core::Insets;
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Element, Text},
    style::Style,
};
use editor_assets::AssetId;
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChipTone {
    Success,
    Info,
    Warning,
    Danger,
    Muted,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineChip {
    pub key: String,
    pub engine: &'static str,
    pub label: &'static str,
    pub tone: ChipTone,
    pub icon: AssetId,
    pub detail: String,
}
impl EngineChip {
    pub fn new(
        key: impl Into<String>,
        engine: &'static str,
        label: &'static str,
        tone: ChipTone,
        icon: AssetId,
        reason: Option<&str>,
    ) -> Self {
        Self {
            key: key.into(),
            engine,
            label,
            tone,
            icon,
            detail: reason.map_or_else(|| format!("{engine}: {label}"), str::to_owned),
        }
    }
}
pub const fn tone_color(theme: &ArtisanTheme, tone: ChipTone) -> Oklch {
    tone_colors(theme, tone).text
}
pub const fn tone_colors(theme: &ArtisanTheme, tone: ChipTone) -> BannerColors {
    let c = &theme.colors;
    match tone {
        ChipTone::Success => c.banner_success,
        ChipTone::Info => c.banner_info,
        ChipTone::Warning => c.banner_warning,
        ChipTone::Danger => c.banner_error,
        ChipTone::Muted => BannerColors {
            bg: c.raised,
            border: c.line,
            text: c.text_muted,
        },
    }
}
pub fn chips_row(theme: &ArtisanTheme, chips: &[EngineChip], row_key: &str) -> Element {
    let mut row = UiBox::new()
        .gap(6.0)
        .style(Style::new().wrap(true).padding_insets(Insets {
            top: 2.0,
            ..Insets::default()
        }));
    for chip in chips {
        let tone = tone_color(theme, chip.tone);
        let detail = chip.detail.clone();
        let t = *theme;
        let el = UiBox::new()
            .height(20.0)
            .gap(4.0)
            .radius(RadiusTokens::value(RadiusStep::Lg))
            .fill(tone.at(Opacity::ToneTint).to_paint())
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .padding_insets(Insets {
                        left: 7.0,
                        right: 7.0,
                        top: 0.0,
                        bottom: 0.0,
                    }),
            )
            .child(source_asset_glyph(
                chip.icon,
                11.0,
                tone.at(Opacity::ToneIcon).to_reference_paint(),
            ))
            .child(
                Text::new(format!("{} · {}", chip.engine, chip.label))
                    .font_size(12.0)
                    .gpui_source_color(tone.to_reference_text_color())
                    .build(),
            )
            .build()
            .with_key(format!("marketplace-chip-{row_key}-{}", chip.key))
            .debug_selector(format!("marketplace-chip-{row_key}-{}", chip.key))
            .tooltip(0.5, move || {
                Surface::Tooltip(Tooltip::Brief)
                    .build(
                        &t,
                        vec![
                            Text::new(detail.clone())
                                .text_style(TextRole::Caption.style(&t))
                                .gpui_source_color(t.colors.text_muted.to_reference_text_color())
                                .wrap(true)
                                .build(),
                        ],
                    )
                    .style(Style::new().max_width(aqua_ui::style::Length::Px(280.0)))
            });
        row = row.child(el);
    }
    row.build()
}
pub fn prompt_status(state: &artisan_api::PromptEngineState) -> EngineChip {
    use artisan_api::PromptEngineStatus as Status;
    let (label, tone, icon) = match state.status {
        Status::Synced => ("Synced", ChipTone::Success, AssetId::TABLER_CHECK),
        Status::Drifted => ("Edited", ChipTone::Warning, AssetId::TABLER_ALERT_TRIANGLE),
        Status::AwaitingChoice => ("Waiting", ChipTone::Warning, AssetId::TABLER_ALERT_TRIANGLE),
        Status::EngineAbsent => ("Not installed", ChipTone::Muted, AssetId::TABLER_MINUS),
        Status::Unavailable => ("Unavailable", ChipTone::Muted, AssetId::TABLER_MINUS),
        Status::Failed => ("Failed", ChipTone::Danger, AssetId::TABLER_X),
    };
    let name = state.engine.display_name();
    let reason = state.reason.clone().or_else(|| {
        state
            .file
            .as_ref()
            .filter(|_| state.status == Status::Synced)
            .map(|file| format!("{name}: {file}"))
    });
    EngineChip::new(
        state.engine.as_str(),
        name,
        label,
        tone,
        icon,
        reason.as_deref(),
    )
}
