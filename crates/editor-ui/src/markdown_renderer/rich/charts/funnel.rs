//! `funnel`: centered bars narrowing step by step; hovering a step reads its
//! value, its share of the first step, and the drop from the step before.

use gpui::{
    AnyElement, App, ElementId, FontWeight, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::*, px, relative,
};
use serde::Deserialize;

use super::super::{BlockContext, RichBlock, kit};
use super::frame;
use super::scale::{exact, f32_of, percent};
use crate::theme::ArtisanTheme;

#[derive(Clone, Debug, Deserialize)]
struct Step {
    #[serde(default)]
    label: String,
    value: f64,
}

/// The `funnel` spec.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct Funnel {
    steps: Vec<Step>,
}

/// What a step reads against the first and the one before.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StepReading {
    pub(super) value: f64,
    /// Share of the first step; `None` when the first step is not positive.
    pub(super) of_first: Option<f64>,
    /// Change from the previous step and its share of it.
    pub(super) drop: Option<(f64, Option<f64>)>,
}

impl Funnel {
    fn value(&self, index: usize) -> f64 {
        self.steps
            .get(index)
            .map(|step| step.value)
            .filter(|value| value.is_finite())
            .unwrap_or(0.0)
    }

    fn label(&self, index: usize) -> String {
        let label = self.steps.get(index).map_or("", |step| step.label.trim());
        if label.is_empty() {
            format!("Step {}", index + 1)
        } else {
            label.to_owned()
        }
    }

    /// Step `index` read against the first step and the one before.
    pub(super) fn reading(&self, index: usize) -> StepReading {
        let value = self.value(index);
        let first = self.value(0);
        let share = |part: f64, whole: f64| (whole > 0.0).then(|| part / whole);
        StepReading {
            value,
            of_first: share(value, first),
            drop: index.checked_sub(1).map(|previous| {
                let before = self.value(previous);
                (before - value, share(before - value, before))
            }),
        }
    }

    /// Bar widths as fractions of the widest step.
    pub(super) fn widths(&self) -> Vec<f32> {
        let widest = (0..self.steps.len())
            .map(|index| self.value(index))
            .fold(0.0, f64::max);
        (0..self.steps.len())
            .map(|index| {
                if widest > 0.0 {
                    f32_of(self.value(index).max(0.0) / widest)
                } else {
                    0.0
                }
            })
            .collect()
    }
}

impl RichBlock for Funnel {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        FunnelView {
            id: cx.key("funnel"),
            theme: *cx.theme,
            funnel: self.clone(),
        }
        .into_any_element()
    }

    fn markdown(&self) -> String {
        if self.steps.is_empty() {
            return "_No steps._\n".to_owned();
        }
        let header: Vec<String> = ["Step", "Value", "Of first", "Drop from previous"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let rows: Vec<Vec<String>> = (0..self.steps.len())
            .map(|index| {
                let reading = self.reading(index);
                vec![
                    self.label(index),
                    exact(reading.value),
                    reading.of_first.map(percent).unwrap_or_default(),
                    reading
                        .drop
                        .map(|(drop, share)| drop_text(drop, share))
                        .unwrap_or_default(),
                ]
            })
            .collect();
        kit::markdown_table(&header, &rows)
    }
}

fn drop_text(drop: f64, share: Option<f64>) -> String {
    let amount = kit::format_number(drop.abs());
    let sign = if drop >= 0.0 { "−" } else { "+" };
    match share {
        Some(share) => format!("{sign}{amount} ({sign}{})", percent(share.abs())),
        None => format!("{sign}{amount}"),
    }
}

#[derive(IntoElement)]
struct FunnelView {
    id: SharedString,
    theme: ArtisanTheme,
    funnel: Funnel,
}

struct FunnelState {
    hover: Option<usize>,
}

impl RenderOnce for FunnelView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let funnel = &self.funnel;
        if funnel.steps.is_empty() {
            return frame::empty(&theme, "No steps");
        }
        let state = kit::keyed(window, cx, ElementId::Name(self.id.clone()), || {
            FunnelState { hover: None }
        });
        let hover = state.read(cx).hover;
        let widths = funnel.widths();
        let color = kit::series_color(&theme, 0);
        let rows = widths.iter().enumerate().map(|(index, width)| {
            let reading = funnel.reading(index);
            let hovered = hover == Some(index);
            let alpha = match hover {
                Some(_) if !hovered => 0.45,
                _ => 0.85,
            };
            let entering = state.clone();
            div()
                .id(ElementId::Name(format!("{}-step-{index}", self.id).into()))
                .w_full()
                .flex()
                .items_center()
                .gap(px(12.0))
                .on_hover(move |hovered, _, cx| {
                    entering.update(cx, |state, cx| {
                        if *hovered {
                            state.hover = Some(index);
                        } else if state.hover == Some(index) {
                            state.hover = None;
                        }
                        cx.notify();
                    });
                })
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(140.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(kit::muted(&theme, funnel.label(index))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(28.0))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .h_full()
                                .w(relative(width.max(0.004)))
                                .rounded(px(4.0))
                                .bg(color.with_alpha(alpha).to_paint()),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(120.0))
                        .flex()
                        .justify_end()
                        .gap(px(6.0))
                        .text_size(px(13.0))
                        .line_height(px(20.0))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.colors.foreground.to_paint())
                                .child(kit::format_number(reading.value)),
                        )
                        .child(
                            div()
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child(reading.of_first.map(percent).unwrap_or_default()),
                        ),
                )
        });
        let summary = self.summary(hover);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .children(rows)
            .child(div().pt(px(4.0)).child(kit::muted(&theme, summary)))
            .into_any_element()
    }
}

impl FunnelView {
    /// The hovered step against the one before, or the overall conversion.
    fn summary(&self, hover: Option<usize>) -> String {
        let funnel = &self.funnel;
        let last = funnel.steps.len().saturating_sub(1);
        match hover {
            Some(index) => {
                let reading = funnel.reading(index);
                let mut parts = vec![format!(
                    "{}: {}",
                    funnel.label(index),
                    kit::format_number(reading.value)
                )];
                if let Some(share) = reading.of_first {
                    parts.push(format!("{} of {}", percent(share), funnel.label(0)));
                }
                if let Some((drop, share)) = reading.drop {
                    parts.push(format!(
                        "{} from {}",
                        drop_text(drop, share),
                        funnel.label(index - 1)
                    ));
                }
                parts.join(" · ")
            }
            None => match funnel.reading(last).of_first {
                Some(share) if last > 0 => format!(
                    "{} of {} reach {}",
                    percent(share),
                    funnel.label(0),
                    funnel.label(last)
                ),
                _ => String::new(),
            },
        }
    }
}
