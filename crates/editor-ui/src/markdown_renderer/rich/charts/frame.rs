//! The parts around a plot: tick labels, axis titles, gridlines, the
//! floating readout, the legend, and the empty state.
//!
//! Tick labels are divs placed at the same fractions the canvas paints at,
//! so labels and marks share one scale without measuring text.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, ElementId, FontWeight, Hsla, IntoElement, ParentElement as _,
    SharedString, Styled as _, div, prelude::*, px, relative,
};

use super::super::kit;
use super::scale::Ticks;
use super::scene::{Shape, hrule, vrule};
use crate::hover_card::{glass_hover_card, hover_card_row, hover_card_title};
use crate::theme::ArtisanTheme;

/// Height of a chart's plot area.
pub(super) const PLOT_HEIGHT: f32 = 240.0;

/// Width of the y tick column for numeric ticks.
pub(super) const NUMBER_AXIS_WIDTH: f32 = 48.0;

/// Width reserved for one x tick label, centered on its tick.
const X_LABEL_WIDTH: f32 = 88.0;

/// A tick label at plot fraction `at`.
#[derive(Clone, Debug)]
pub(super) struct Tick {
    pub(super) at: f32,
    pub(super) label: SharedString,
}

/// Labels for numeric `ticks` along an axis; `down` measures from the top.
pub(super) fn number_ticks(ticks: &Ticks, unit: Option<&str>, down: bool) -> Vec<Tick> {
    ticks
        .values
        .iter()
        .map(|value| Tick {
            at: if down {
                ticks.domain.fraction_down(*value)
            } else {
                ticks.domain.fraction(*value)
            },
            label: SharedString::from(kit::with_unit(&kit::format_number(*value), unit)),
        })
        .collect()
}

/// Gridlines at the y ticks, plus vertical ones at `x_ticks` when asked.
pub(super) fn grid(theme: &ArtisanTheme, y_ticks: &[Tick], x_ticks: &[Tick]) -> Vec<Shape> {
    let color = kit::hairline(theme);
    y_ticks
        .iter()
        .map(|tick| hrule(tick.at, color, false))
        .chain(x_ticks.iter().map(|tick| vrule(tick.at, color, false)))
        .collect()
}

/// The axis titles and their units, as drawn beside the axes.
pub(super) fn axis_title(label: Option<&str>, unit: Option<&str>) -> Option<SharedString> {
    let label = label.map(str::trim).filter(|label| !label.is_empty());
    let unit = unit.map(str::trim).filter(|unit| !unit.is_empty());
    match (label, unit) {
        (Some(label), Some(unit)) => Some(format!("{label} ({unit})")),
        (Some(label), None) => Some(label.to_owned()),
        (None, Some(unit)) => Some(unit.to_owned()),
        (None, None) => None,
    }
    .map(SharedString::from)
}

/// What sits around a plot.
pub(super) struct Frame {
    /// Series names and values at the right end of the plot, when the chart
    /// labels its series directly; empty otherwise.
    pub(super) end_labels: Vec<EndLabel>,
    pub(super) y_ticks: Vec<Tick>,
    pub(super) x_ticks: Vec<Tick>,
    pub(super) y_title: Option<SharedString>,
    pub(super) x_title: Option<SharedString>,
    pub(super) y_axis_width: f32,
    pub(super) height: f32,
}

impl Frame {
    /// `plot` framed by its tick labels and titles. `plot` should fill its
    /// parent and position its own overlays.
    pub(super) fn wrap(self, theme: &ArtisanTheme, plot: impl IntoElement) -> Div {
        let muted = theme.colors.muted_foreground.to_paint();
        let y_labels = self.y_ticks.into_iter().map(|tick| {
            div()
                .absolute()
                .left_0()
                .right(px(8.0))
                .top(relative(tick.at))
                .mt(px(-8.0))
                .h(px(16.0))
                .flex()
                .justify_end()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(11.0))
                .line_height(px(16.0))
                .text_color(muted)
                .child(tick.label)
        });
        let x_labels = self.x_ticks.into_iter().map(|tick| {
            div()
                .absolute()
                .top_0()
                .left(relative(tick.at))
                .ml(px(-X_LABEL_WIDTH / 2.0))
                .w(px(X_LABEL_WIDTH))
                .flex()
                .justify_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(11.0))
                .line_height(px(16.0))
                .text_color(muted)
                .child(tick.label)
        });
        let title = |text: SharedString| {
            div()
                .text_size(px(11.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(text)
        };
        let gutter_width = if self.end_labels.is_empty() {
            12.0
        } else {
            END_LABEL_GUTTER
        };
        let gutter = gutter(self.end_labels, gutter_width);
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(self.y_title.map(title))
            .child(
                div()
                    .w_full()
                    .h(px(self.height))
                    .flex()
                    .child(
                        div()
                            .relative()
                            .flex_shrink_0()
                            .w(px(self.y_axis_width))
                            .h_full()
                            .children(y_labels),
                    )
                    .child(div().relative().flex_1().min_w_0().h_full().child(plot))
                    .child(gutter),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .child(div().flex_shrink_0().w(px(self.y_axis_width)))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h(px(16.0))
                            .children(x_labels),
                    )
                    .child(div().flex_shrink_0().w(px(gutter_width))),
            )
            .children(
                self.x_title
                    .map(|text| div().w_full().flex().justify_end().child(title(text))),
            )
    }
}

/// The right gutter holding direct series labels.
fn gutter(labels: Vec<EndLabel>, width: f32) -> Div {
    div()
        .relative()
        .flex_shrink_0()
        .w(px(width))
        .h_full()
        .children(labels.into_iter().map(|label| {
            div()
                .absolute()
                .left(px(8.0))
                .right_0()
                .top(relative(label.y))
                .mt(px(-8.0))
                .h(px(16.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(11.0))
                .line_height(px(16.0))
                .text_color(label.color)
                .child(label.text)
        }))
}

/// How many labels fit across `width` pixels at `spacing` pixels each.
pub(super) fn label_budget(width: f32, spacing: f32) -> usize {
    let budget = (width / spacing.max(1.0)).floor().clamp(2.0, 24.0);
    super::scale::index_of(f64::from(budget))
}

/// Every `stride`-th of `count` indices so at most `budget` remain.
pub(super) fn thinned(count: usize, budget: usize) -> impl Iterator<Item = usize> {
    let stride = count.div_ceil(budget.max(1)).max(1);
    (0..count).step_by(stride)
}

/// One line of a readout: an optional swatch, a name, and a value.
pub(super) struct ReadoutRow {
    pub(super) color: Option<Hsla>,
    pub(super) label: SharedString,
    pub(super) value: SharedString,
}

impl ReadoutRow {
    pub(super) fn new(
        color: Option<Hsla>,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        Self {
            color,
            label: label.into(),
            value: value.into(),
        }
    }
}

/// The glass hover card near plot fraction `x`, flipping to the left of it
/// past the middle so it stays inside the plot. It wears the app's hover
/// card: a semibold heading, then one row per value with the label muted and
/// the value in the foreground.
pub(super) fn readout(
    theme: &ArtisanTheme,
    x: f32,
    heading: Option<SharedString>,
    rows: Vec<ReadoutRow>,
) -> Div {
    readout_card(theme, x, heading, rows).top(px(6.0))
}

/// [`readout`] along the bottom of the plot, for marks in its top half.
pub(super) fn readout_low(
    theme: &ArtisanTheme,
    x: f32,
    heading: Option<SharedString>,
    rows: Vec<ReadoutRow>,
) -> Div {
    readout_card(theme, x, heading, rows).bottom(px(6.0))
}

fn readout_card(
    theme: &ArtisanTheme,
    x: f32,
    heading: Option<SharedString>,
    rows: Vec<ReadoutRow>,
) -> Div {
    let x = x.clamp(0.0, 1.0);
    let content = div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .px(px(14.0))
        .py(px(12.0))
        .children(heading.map(|heading| hover_card_title(theme, heading).whitespace_nowrap()))
        .children(
            rows.into_iter()
                .map(|row| hover_card_row(theme, row.color, row.label, row.value)),
        );
    let card = glass_hover_card(theme, content)
        .absolute()
        .min_w(px(140.0))
        .max_w(px(280.0));
    if x > 0.55 {
        card.right(relative(1.0 - x)).mr(px(10.0))
    } else {
        card.left(relative(x)).ml(px(10.0))
    }
}

/// Something a legend entry does when clicked.
pub(super) type Toggle = Rc<dyn Fn(usize, &mut App)>;

/// One legend entry.
pub(super) struct LegendEntry {
    pub(super) name: SharedString,
    pub(super) color: Hsla,
    pub(super) hidden: bool,
}

/// Series swatches and names; with `toggle`, each entry toggles its series.
pub(super) fn legend(
    theme: &ArtisanTheme,
    id: &SharedString,
    entries: Vec<LegendEntry>,
    toggle: Option<&Toggle>,
) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(12.0))
        .children(entries.into_iter().enumerate().map(|(index, entry)| {
            let alpha = if entry.hidden { 0.35 } else { 1.0 };
            let swatch = div()
                .flex_shrink_0()
                .w(px(12.0))
                .h(px(4.0))
                .rounded(px(2.0))
                .bg(if entry.hidden {
                    kit::hairline(theme)
                } else {
                    entry.color
                });
            let row = div()
                .id(ElementId::Name(SharedString::from(format!(
                    "{id}-legend-{index}"
                ))))
                .flex()
                .items_center()
                .gap(px(6.0))
                .opacity(alpha)
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(theme.colors.foreground.to_paint())
                .child(swatch)
                .child(entry.name);
            match toggle.cloned() {
                Some(toggle) => row
                    .cursor_pointer()
                    .on_click(move |_, _, cx| toggle(index, cx))
                    .into_any_element(),
                None => row.into_any_element(),
            }
        }))
}

/// The quiet stand-in for a block with nothing to draw.
pub(super) fn empty(theme: &ArtisanTheme, text: &str) -> AnyElement {
    div()
        .w_full()
        .h(px(72.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(kit::inner_radius())
        .border_1()
        .border_color(kit::hairline(theme))
        .child(kit::muted(theme, text.to_owned()))
        .into_any_element()
}

/// Width of the right gutter that holds direct series labels.
pub(super) const END_LABEL_GUTTER: f32 = 104.0;

/// A series' name and value at the plot's right edge.
#[derive(Clone, Debug)]
pub(super) struct EndLabel {
    /// The vertical position, in plot fractions.
    pub(super) y: f32,
    pub(super) text: SharedString,
    pub(super) color: Hsla,
}

/// Spreads labels so none overlap: sorted by `y`, each at least `gap`
/// below the one above, the whole run shifted up if it overflows.
pub(super) fn spread(labels: &mut [EndLabel], gap: f32) {
    labels.sort_by(|a, b| a.y.total_cmp(&b.y));
    for index in 1..labels.len() {
        let floor = labels[index - 1].y + gap;
        if labels[index].y < floor {
            labels[index].y = floor;
        }
    }
    if let Some(last) = labels.last() {
        let overflow = last.y - 1.0;
        if overflow > 0.0 {
            for label in labels.iter_mut() {
                label.y -= overflow;
            }
        }
    }
    for index in (0..labels.len().saturating_sub(1)).rev() {
        let ceiling = labels[index + 1].y - gap;
        if labels[index].y > ceiling {
            labels[index].y = ceiling;
        }
    }
}

/// A short label in the plot.
#[derive(Clone, Debug)]
pub(super) struct PlotLabel {
    /// The anchor, in plot fractions.
    pub(super) x: f32,
    pub(super) y: f32,
    /// Whether the label hangs below its anchor rather than sitting above.
    pub(super) below: bool,
    pub(super) text: SharedString,
    pub(super) color: Hsla,
}

/// `label` placed beside its anchor, flipping left near the right edge.
pub(super) fn plot_label(theme: &ArtisanTheme, label: PlotLabel) -> Div {
    let x = label.x.clamp(0.0, 1.0);
    let text = label.text;
    let color = label.color;
    let label = div()
        .absolute()
        .top(relative(label.y.clamp(0.0, 1.0)))
        .mt(px(if label.below { 3.0 } else { -18.0 }))
        .max_w(px(220.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .px(px(4.0))
        .rounded(px(4.0))
        .bg(theme.colors.background.with_alpha(0.7).to_paint())
        .text_size(px(11.0))
        .line_height(px(16.0))
        .text_color(color)
        .child(text);
    if x > 0.7 {
        label.right(relative(1.0 - x)).mr(px(2.0))
    } else {
        label.left(relative(x)).ml(px(2.0))
    }
}
