//! The shared vocabulary every rich block draws with.
//!
//! Tones, statuses, the series palette, text roles, inner surfaces, chips,
//! code references, and number formatting live here once, so blocks read as
//! one family and each kind's file holds only what is particular to it.

use crate::theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use editor_assets::AssetId;
use gpui::{
    AnyElement, App, Div, ElementId, Entity, FontWeight, Hsla, IntoElement, ParentElement as _,
    SharedString, Styled as _, Window, div, px,
};
use serde::Deserialize;

use crate::asset_seam::asset_glyph;

// ---------------------------------------------------------------------------
// Tones and statuses

/// The semantic color of a part: callouts, cards, chips, and events.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Tone {
    #[default]
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
    Accent,
}

impl Tone {
    /// The tone's text and icon colour; neutral reads as muted text.
    pub(super) const fn color(self, theme: &ArtisanTheme) -> Oklch {
        let colors = &theme.colors;
        match self {
            Self::Neutral => colors.text_muted,
            Self::Info => colors.banner_info.text,
            Self::Success => colors.banner_success.text,
            Self::Warning => colors.banner_warning.text,
            Self::Danger => colors.banner_error.text,
            Self::Accent => colors.link,
        }
    }
}

// ---------------------------------------------------------------------------
// Series palette

/// The colour of series `index`: the theme's data palette, repeating past
/// eight series.
pub(super) const fn series_color(theme: &ArtisanTheme, index: usize) -> Oklch {
    let palette = &theme.colors.palette;
    palette[index % palette.len()]
}

/// Accent steps of the sequential ramp, from the surface outwards. Steps
/// 3–8 and 11 move away from the background in both modes.
pub(super) const SEQUENTIAL_STEPS: [usize; 7] = [3, 4, 5, 6, 7, 8, 11];

/// A sequential ramp in discrete accent steps, `t` in 0..=1.
pub(super) fn sequential_color(theme: &ArtisanTheme, t: f32) -> Oklch {
    let t = t.clamp(0.0, 1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "t is clamped, so the index stays inside the seven steps"
    )]
    let index = (t * (SEQUENTIAL_STEPS.len() - 1) as f32).round() as usize;
    theme.colors.accent.step(SEQUENTIAL_STEPS[index])
}

/// A diverging ramp in discrete steps, `t` in -1..=1: red below zero,
/// accent above, the raised fill at zero.
pub(super) fn diverging_color(theme: &ArtisanTheme, t: f32) -> Oklch {
    let colors = &theme.colors;
    let t = t.clamp(-1.0, 1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "|t| is clamped to 0..=1, so the index stays inside 0..=4"
    )]
    let index = (t.abs() * 4.0).round() as usize;
    if index == 0 {
        return colors.raised;
    }
    let side = if t < 0.0 {
        [
            colors.red.bg,
            colors.red.border,
            colors.red.solid,
            colors.red.text,
        ]
    } else {
        [
            colors.accent.step(3),
            colors.accent.step(6),
            colors.accent.step(8),
            colors.accent.step(11),
        ]
    };
    side[index - 1]
}

// ---------------------------------------------------------------------------
// Text roles

/// A part's title: 14 px semibold foreground.
pub(super) fn title(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(TypographyTokens::size(TextStep::Sm))
        .line_height(px(20.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.colors.text.to_paint())
        .child(text.into())
}

/// Running text: 14 px on 22 px in the foreground.
pub(super) fn body(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(TypographyTokens::size(TextStep::Sm))
        .line_height(px(22.0))
        .text_color(theme.colors.text.to_paint())
        .child(text.into())
}

/// Secondary text: 13 px on 20 px, muted.
pub(super) fn muted(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(TypographyTokens::size(TextStep::XsMid))
        .line_height(px(20.0))
        .text_color(theme.colors.text_muted.to_paint())
        .child(text.into())
}

/// A small uppercase-free label: 12 px on 16 px, muted, medium weight.
pub(super) fn label(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(TypographyTokens::size(TextStep::Xs))
        .line_height(px(16.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.text_muted.to_paint())
        .child(text.into())
}

/// Monospace text at code size: 13 px on 20 px.
pub(super) fn mono(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .font_family(theme.typography.mono.family)
        .tracking(0.0)
        .text_size(TypographyTokens::size(TextStep::XsMid))
        .line_height(px(20.0))
        .text_color(theme.colors.text.to_paint())
        .child(text.into())
}

/// `text` split on blank lines into running-text paragraphs.
pub(super) fn paragraphs(theme: &ArtisanTheme, text: &str) -> Div {
    div().flex().flex_col().gap(px(8.0)).children(
        text.split("\n\n")
            .map(str::trim)
            .filter(|paragraph| !paragraph.is_empty())
            .map(|paragraph| body(theme, paragraph.to_owned())),
    )
}

// ---------------------------------------------------------------------------
// Surfaces, chips, and glyphs

/// Corner radius of a surface nested inside the block card.
pub(super) const fn inner_radius() -> gpui::Pixels {
    RadiusTokens::value(RadiusStep::Lg)
}

/// The hairline color shared by surfaces, rules, and axes.
pub(super) fn hairline(theme: &ArtisanTheme) -> Hsla {
    theme.colors.text.at(crate::theme::Opacity::Rule).to_paint()
}

/// A tinted glyph at `size` in `color`.
pub(super) fn glyph(id: AssetId, size: f32, color: Hsla) -> AnyElement {
    div()
        .flex_shrink_0()
        .text_color(color)
        .child(asset_glyph(id).size(px(size)))
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Interaction state

/// The window state keyed under `id`, created by `init` on first use.
pub(super) fn keyed<T: 'static>(
    window: &mut Window,
    cx: &mut App,
    id: impl Into<ElementId>,
    init: impl FnOnce() -> T,
) -> Entity<T> {
    window.use_keyed_state(id.into(), cx, |_, _| init())
}

// ---------------------------------------------------------------------------
// Code references

/// A path with an optional line range: `path`, `path:42`, or `path:42-60`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CodeRef {
    pub(super) path: SharedString,
    pub(super) start: Option<u32>,
    pub(super) end: Option<u32>,
}

impl CodeRef {
    pub(super) fn parse(text: &str) -> Self {
        let text = text.trim();
        let parsed = text.rsplit_once(':').and_then(|(path, lines)| {
            let (start, end) = match lines.split_once('-') {
                Some((start, end)) => (start.parse().ok()?, Some(end.parse().ok()?)),
                None => (lines.parse().ok()?, None),
            };
            Some((path, start, end))
        });
        match parsed {
            Some((path, start, end)) => Self {
                path: SharedString::from(path.to_owned()),
                start: Some(start),
                end,
            },
            None => Self {
                path: SharedString::from(text.to_owned()),
                start: None,
                end: None,
            },
        }
    }

    /// The reference as written: `path:start-end`.
    pub(super) fn display(&self) -> String {
        match (self.start, self.end) {
            (Some(start), Some(end)) => format!("{}:{start}-{end}", self.path),
            (Some(start), None) => format!("{}:{start}", self.path),
            _ => self.path.to_string(),
        }
    }
}

impl<'de> Deserialize<'de> for CodeRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|text| Self::parse(&text))
    }
}

/// A line or an inclusive line range: `4`, `"4"`, or `"3-5"`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LineSpan {
    pub(super) start: u32,
    pub(super) end: u32,
}

impl LineSpan {
    pub(super) fn contains(self, line: u32) -> bool {
        (self.start..=self.end).contains(&line)
    }
}

impl<'de> Deserialize<'de> for LineSpan {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Line(u32),
            Text(String),
            Pair([u32; 2]),
        }
        let invalid = || serde::de::Error::custom("expected a line like 4 or a range like \"3-5\"");
        match Raw::deserialize(deserializer)? {
            Raw::Line(line) => Ok(Self {
                start: line,
                end: line,
            }),
            Raw::Pair([start, end]) => Ok(Self {
                start: start.min(end),
                end: start.max(end),
            }),
            Raw::Text(text) => {
                let (start, end) = if let Some((start, end)) = text.trim().split_once('-') {
                    (
                        start.trim().parse().map_err(|_unparsable| invalid())?,
                        end.trim().parse().map_err(|_unparsable| invalid())?,
                    )
                } else {
                    let line = text.trim().parse().map_err(|_unparsable| invalid())?;
                    (line, line)
                };
                Ok(Self {
                    start: u32::min(start, end),
                    end: u32::max(start, end),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Numbers and Markdown

/// A number for reading: grouped, trimmed, and compact past a million.
pub(super) fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return "–".to_owned();
    }
    let magnitude = value.abs();
    let (scaled, suffix) = if magnitude >= 1e9 {
        (value / 1e9, "B")
    } else if magnitude >= 1e6 {
        (value / 1e6, "M")
    } else if magnitude >= 1e4 {
        (value / 1e3, "k")
    } else {
        (value, "")
    };
    let decimals = if scaled.abs() >= 100.0 || scaled.fract() == 0.0 {
        0
    } else if scaled.abs() >= 10.0 {
        1
    } else {
        2
    };
    let text = format!("{scaled:.decimals$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    };
    format!("{text}{suffix}")
}

/// `value` with `unit` appended: `%` and `x` attach, other units space.
pub(super) fn with_unit(value: &str, unit: Option<&str>) -> String {
    match unit.map(str::trim).filter(|unit| !unit.is_empty()) {
        Some(unit) if unit == "%" || unit == "x" || unit == "×" => format!("{value}{unit}"),
        Some(unit) => format!("{value} {unit}"),
        None => value.to_owned(),
    }
}

/// The element id of one part of a view keyed under `id`.
pub(super) fn part(id: &SharedString, part: impl std::fmt::Display) -> gpui::ElementId {
    gpui::ElementId::Name(part_key(id, part))
}

/// String identity for loaders and controls requiring SharedString.
pub(super) fn part_key(id: &SharedString, part: impl std::fmt::Display) -> SharedString {
    SharedString::from(format!("{id}-{part}"))
}
