//! The shared vocabulary every rich block draws with.
//!
//! Tones, statuses, the series palette, text roles, inner surfaces, chips,
//! code references, and number formatting live here once, so blocks read as
//! one family and each kind's file holds only what is particular to it.

use editor_assets::AssetId;
use gpui::{
    AnyElement, App, Div, ElementId, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use serde::Deserialize;

use crate::asset_seam::asset_glyph;
use crate::hover_card::hover_fill_style;
use crate::theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens, ThemeMode};

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
    /// The tone's color; neutral reads as the muted foreground.
    pub(super) fn color(self, theme: &ArtisanTheme) -> Oklch {
        let colors = &theme.colors;
        match self {
            Self::Neutral => colors.muted_foreground,
            Self::Info => colors.banner_info,
            Self::Success => colors.banner_success,
            Self::Warning => colors.banner_warning,
            Self::Danger => colors.banner_error,
            Self::Accent => colors.question_to,
        }
    }
}

// ---------------------------------------------------------------------------
// Series palette

/// Hues of the categorical series palette, in order: blue, violet, teal,
/// amber, rose, green, orange, cyan.
const SERIES_HUES: [f32; 8] = [255.0, 300.0, 190.0, 80.0, 10.0, 150.0, 50.0, 220.0];

/// The color of series `index`. Lightness follows the mode so series read
/// against either canvas; past eight series the hues repeat lighter.
pub(super) fn series_color(theme: &ArtisanTheme, index: usize) -> Oklch {
    let hue = SERIES_HUES[index % SERIES_HUES.len()];
    #[expect(clippy::cast_precision_loss, reason = "the round count stays tiny")]
    let round = (index / SERIES_HUES.len()) as f32;
    let lightness = match theme.mode {
        ThemeMode::Dark => 0.72,
        ThemeMode::Light => 0.6,
    };
    Oklch::new((lightness + round * 0.08).min(0.9), 0.15, hue)
}

/// A sequential ramp from the surface to the first series hue, `t` in 0..=1.
pub(super) fn sequential_color(theme: &ArtisanTheme, t: f32) -> Oklch {
    let t = t.clamp(0.0, 1.0);
    let (from, to) = match theme.mode {
        ThemeMode::Dark => (0.3, 0.78),
        ThemeMode::Light => (0.95, 0.5),
    };
    Oklch::new(from + (to - from) * t, 0.02 + 0.14 * t, SERIES_HUES[0])
}

/// A diverging ramp, `t` in -1..=1: rose below zero, blue above.
pub(super) fn diverging_color(theme: &ArtisanTheme, t: f32) -> Oklch {
    let t = t.clamp(-1.0, 1.0);
    let hue = if t < 0.0 {
        SERIES_HUES[4]
    } else {
        SERIES_HUES[0]
    };
    let strength = t.abs();
    let (from, to) = match theme.mode {
        ThemeMode::Dark => (0.3, 0.74),
        ThemeMode::Light => (0.95, 0.55),
    };
    Oklch::new(from + (to - from) * strength, 0.15 * strength, hue)
}

// ---------------------------------------------------------------------------
// Text roles

/// A part's title: 14 px semibold foreground.
pub(super) fn title(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(14.0))
        .line_height(px(20.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.colors.foreground.to_paint())
        .child(text.into())
}

/// Running text: 14 px on 22 px in the foreground.
pub(super) fn body(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(14.0))
        .line_height(px(22.0))
        .text_color(theme.colors.foreground.to_paint())
        .child(text.into())
}

/// Secondary text: 13 px on 20 px, muted.
pub(super) fn muted(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(13.0))
        .line_height(px(20.0))
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(text.into())
}

/// A small uppercase-free label: 12 px on 16 px, muted, medium weight.
pub(super) fn label(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.0))
        .line_height(px(16.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(text.into())
}

/// Monospace text at code size: 13 px on 20 px.
pub(super) fn mono(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .font_family(theme.typography.mono.family)
        .tracking(0.0)
        .text_size(px(13.0))
        .line_height(px(20.0))
        .text_color(theme.colors.foreground.to_paint())
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
pub(super) fn inner_radius() -> gpui::Pixels {
    RadiusTokens::value(RadiusStep::Lg)
}

/// The hairline color shared by surfaces, rules, and axes.
pub(super) fn hairline(theme: &ArtisanTheme) -> Hsla {
    theme.colors.foreground.with_alpha(0.1).to_paint()
}

/// A small pill: status, kind, or tag.
pub(super) fn chip(theme: &ArtisanTheme, text: impl Into<SharedString>, tone: Tone) -> Div {
    let color = tone.color(theme);
    let (fg, bg) = if tone == Tone::Neutral {
        (
            theme.colors.foreground.to_paint(),
            theme.colors.foreground.with_alpha(0.06).to_paint(),
        )
    } else {
        (color.to_paint(), color.with_alpha(0.12).to_paint())
    };
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(4.0))
        .h(px(20.0))
        .px(px(8.0))
        .rounded(px(10.0))
        .bg(bg)
        .text_size(px(12.0))
        .line_height(px(16.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .whitespace_nowrap()
        .child(text.into())
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

/// One entry of the mode row under a block: the chosen mode in the
/// foreground and semibold, the rest muted, every entry with the app's
/// hover fill. The caller adds the click.
pub(super) fn mode_entry(
    theme: &ArtisanTheme,
    id: ElementId,
    label: impl Into<SharedString>,
    selected: bool,
) -> gpui::Stateful<Div> {
    let theme = *theme;
    div()
        .id(id)
        .px(px(8.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(if selected {
            theme.colors.foreground.to_paint()
        } else {
            theme.colors.muted_foreground.to_paint()
        })
        .when(selected, |entry| entry.font_weight(FontWeight::SEMIBOLD))
        .hover(move |style| hover_fill_style(style, &theme, px(6.0)))
        .child(label.into())
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
                        start.trim().parse().map_err(|_| invalid())?,
                        end.trim().parse().map_err(|_| invalid())?,
                    )
                } else {
                    let line = text.trim().parse().map_err(|_| invalid())?;
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

/// Escapes a Markdown table cell.
pub(super) fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// A Markdown table from a header and rows.
pub(super) fn markdown_table(header: &[String], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    let line = |cells: &[String]| {
        format!(
            "| {} |\n",
            cells
                .iter()
                .map(|text| cell(text))
                .collect::<Vec<_>>()
                .join(" | ")
        )
    };
    out.push_str(&line(header));
    out.push('|');
    out.push_str(&" --- |".repeat(header.len().max(1)));
    out.push('\n');
    for row in rows {
        out.push_str(&line(row));
    }
    out
}

/// Deserializes a list into a shared slice, so frames clone it cheaply.
pub(super) fn shared<'de, D, T>(deserializer: D) -> Result<std::rc::Rc<[T]>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Vec::<T>::deserialize(deserializer).map(std::rc::Rc::from)
}

/// The element id of one part of a view keyed under `id`.
pub(super) fn part(id: &SharedString, part: impl std::fmt::Display) -> gpui::ElementId {
    gpui::ElementId::Name(SharedString::from(format!("{id}-{part}")))
}
