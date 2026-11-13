use super::{
    kit::{Corner, IconSize, Space, Spaced, Surface, TextRole, Tooltip},
    theme::{ArtisanTheme, BannerColors, Oklch, Opacity},
};
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Element},
    style::Style,
};

const CHIP_TEXT: TextRole = TextRole::Caption;
const CHIP_PAD: Space = Space::S4;
const CHIP_HEIGHT: f32 = CHIP_TEXT.spec().line + 2.0 * CHIP_PAD.px();
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
        .spaced(Space::S6)
        .style(Style::new().wrap(true));
    for chip in chips {
        let tone = tone_color(theme, chip.tone);
        let detail = chip.detail.clone();
        let t = *theme;
        let el = UiBox::new()
            .height(CHIP_HEIGHT)
            .spaced(Space::S4)
            .padded(CHIP_PAD)
            .radius(Corner::Full.resolve(CHIP_HEIGHT))
            .fill(tone.at(Opacity::ToneTint).to_paint())
            .style(Style::new().align(AlignItems::CENTER))
            .child(IconSize::Sm.glyph(chip.icon, tone.at(Opacity::ToneIcon).to_reference_paint()))
            .child(
                CHIP_TEXT
                    .text(theme, format!("{} · {}", chip.engine, chip.label))
                    .gpui_source_color(tone.to_reference_text_color())
                    .build(),
            )
            .build()
            .with_key(format!("marketplace-chip-{row_key}-{}", chip.key))
            .debug_selector(format!("marketplace-chip-{row_key}-{}", chip.key))
            .tooltip_with(0.5, crate::ui::hint_card::presence(), move || {
                Surface::Tooltip(Tooltip::Brief)
                    .build(
                        &t,
                        vec![
                            TextRole::Caption
                                .text(&t, detail.clone())
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
