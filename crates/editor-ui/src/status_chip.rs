//! Engine status chips (master's `engine_status_chips`): one small tinted
//! pill per engine with an icon, the engine's name, its status, and the
//! Forge's reason on hover. Shared by the Marketplace and Settings.

use crate::AssetId;
use crate::asset_seam::asset_glyph;
use crate::hover_card::{glass_hover_card, hover_card_text};
use crate::theme::{ArtisanTheme, Oklch};
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
    let tooltip_theme = *theme;
    let selector = format!("marketplace-chip-{row_key}-{}", chip.key);
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .h(px(20.0))
        .px(px(7.0))
        .rounded(px(10.0))
        .bg(tone.with_alpha(0.10).to_paint())
        .text_xs()
        .text_color(tone.to_paint())
        .child(
            asset_glyph(chip.icon)
                .size(px(11.0))
                .text_color(tone.with_alpha(0.8).to_paint()),
        )
        .child(format!("{} \u{b7} {}", chip.engine, chip.label))
        .tooltip(move |_window, cx| {
            let detail = detail.clone();
            cx.new(|_| ChipTooltip {
                theme: tooltip_theme,
                text: detail,
            })
            .into()
        })
}

/// The hover text of one chip.
struct ChipTooltip {
    theme: ArtisanTheme,
    text: String,
}

impl Render for ChipTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        glass_hover_card(
            &self.theme,
            div().px(px(12.0)).py(px(8.0)).child(hover_card_text(
                &self.theme,
                self.text.clone(),
                &[],
            )),
        )
        .max_w(px(280.0))
    }
}

/// A tone's colour. Text at full strength, icons at 80%, tinted
/// backgrounds at 10%.
#[must_use]
pub const fn tone_color(theme: &ArtisanTheme, tone: ChipTone) -> Oklch {
    match tone {
        ChipTone::Success => theme.colors.banner_success,
        ChipTone::Info => theme.colors.banner_info,
        ChipTone::Warning => theme.colors.banner_warning,
        ChipTone::Danger => theme.colors.destructive,
        ChipTone::Muted => theme.colors.muted_foreground,
    }
}
