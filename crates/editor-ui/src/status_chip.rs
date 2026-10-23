//! Shared engine status chips for Marketplace and Settings.
//! Each chip shows the engine name and status, with the Forge reason on hover.

use crate::AssetId;
use crate::asset_seam::asset_glyph;
use crate::hover_card::{glass_hover_card, hover_card_text};
use crate::theme::{
    ActiveTheme as _, ArtisanTheme, BannerColors, Oklch, Opacity, RadiusStep, RadiusTokens,
};
use gpui::prelude::*;
use gpui::{Context, Div, SharedString, Window, div, px};

/// The colour family a chip is painted in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChipTone {
    /// In place and working.
    Success,
    /// Present but not managed by Artisan.
    Info,
    /// Needs the user's attention.
    Warning,
    /// Failed.
    Danger,
    /// Nothing to act on.
    Muted,
}

/// One engine's chip.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineChip {
    /// Keeps the chip's selector unique in its row (the engine id).
    pub key: String,
    /// Short engine name.
    pub engine: &'static str,
    pub label: &'static str,
    pub tone: ChipTone,
    pub icon: AssetId,
    /// Hover text: Forge's reason, or the engine and the label.
    pub detail: String,
}

impl EngineChip {
    #[must_use]
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

/// Paints one wrapping row of chips; `row_key` keeps selectors unique per
/// row (`marketplace-chip-{row_key}-{chip.key}`).
#[must_use]
pub fn chips_row(theme: &ArtisanTheme, chips: &[EngineChip], row_key: &str) -> Div {
    let mut row = div().flex().flex_row().flex_wrap().gap(px(6.0)).pt(px(2.0));
    for chip in chips {
        row = row.child(engine_chip(theme, chip, row_key));
    }
    row
}

fn engine_chip(theme: &ArtisanTheme, chip: &EngineChip, row_key: &str) -> impl IntoElement {
    let tone = tone_color(theme, chip.tone);
    let detail = chip.detail.clone();
    let selector = format!("marketplace-chip-{row_key}-{}", chip.key);
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .h(px(20.0))
        .px(px(7.0))
        .rounded(RadiusTokens::value(RadiusStep::Lg))
        .bg(tone.at(Opacity::ToneTint).to_paint())
        .text_xs()
        .text_color(tone.to_paint())
        .child(
            asset_glyph(chip.icon)
                .size(px(11.0))
                .text_color(tone.at(Opacity::ToneIcon).to_paint()),
        )
        .child(format!("{} \u{b7} {}", chip.engine, chip.label))
        .tooltip(move |_window, cx| {
            let detail = detail.clone();
            cx.new(|_| ChipTooltip { text: detail }).into()
        })
}

/// The hover text of one chip.
struct ChipTooltip {
    text: String,
}

impl Render for ChipTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        div().max_w(px(280.0)).child(glass_hover_card(
            &theme,
            div()
                .px(px(12.0))
                .py(px(8.0))
                .child(hover_card_text(&theme, self.text.clone(), &[])),
        ))
    }
}

/// A tone's colour. Text at full strength, icons at [`Opacity::ToneIcon`],
/// tinted backgrounds at [`Opacity::ToneTint`].
#[must_use]
pub const fn tone_color(theme: &ArtisanTheme, tone: ChipTone) -> Oklch {
    tone_colors(theme, tone).text
}

/// A tone's tinted background, border, and text and icon colour.
#[must_use]
pub const fn tone_colors(theme: &ArtisanTheme, tone: ChipTone) -> BannerColors {
    let colors = &theme.colors;
    match tone {
        ChipTone::Success => colors.banner_success,
        ChipTone::Info => colors.banner_info,
        ChipTone::Warning => colors.banner_warning,
        ChipTone::Danger => colors.banner_error,
        ChipTone::Muted => BannerColors {
            bg: colors.raised,
            border: colors.line,
            text: colors.text_muted,
        },
    }
}

/// One presentation of Forge's prompt status, shared by Settings and Marketplace.
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
