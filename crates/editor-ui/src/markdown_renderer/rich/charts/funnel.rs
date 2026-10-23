//! `funnel`: centered bars narrowing step by step; hovering a step reads its
//! value, its share of the first step, and the drop from the step before.

use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use std::sync::Arc;

use gpui::{
    AnyElement, App, ElementId, FontWeight, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::*, px, relative,
};
use serde::Deserialize;

use super::super::{BlockContext, RichBlock, kit};
use super::frame;
use super::scale::{f32_of, f32_of_count, percent};

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Step {
    #[serde(default)]
    label: String,
    value: f64,
}

/// The `funnel` spec.
#[derive(Debug, Deserialize)]
struct FunnelSpec {
    steps: Vec<Step>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "FunnelSpec")]
pub(super) struct Funnel {
    steps: Arc<[Step]>,
    widest: f64,
}
impl TryFrom<FunnelSpec> for Funnel {
    type Error = String;
    fn try_from(spec: FunnelSpec) -> Result<Self, String> {
        if spec
            .steps
            .iter()
            .any(|step| !step.value.is_finite() || step.value < 0.0)
        {
            return Err("funnel values must be finite and nonnegative".to_owned());
        }
        if spec.steps.len() > 64 {
            return Err("funnel preview supports at most 64 steps".to_owned());
        }
        let widest = spec.steps.iter().map(|step| step.value).fold(0.0, f64::max);
        Ok(Self {
            steps: Arc::from(spec.steps),
            widest,
        })
    }
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

    fn width(&self, index: usize) -> f32 {
        if self.widest > 0.0 {
            f32_of(self.value(index) / self.widest)
        } else {
            0.0
        }
    }

    #[cfg(test)]
    pub(super) fn widths(&self) -> Vec<f32> {
        (0..self.steps.len())
            .map(|index| self.width(index))
            .collect()
    }
}

impl RichBlock for Funnel {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of_val(&*self.steps)
            + self
                .steps
                .iter()
                .map(|step| step.label.capacity())
                .sum::<usize>()
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        FunnelView {
            id: cx.key("funnel"),
            theme: *cx.theme,
            funnel: self.clone(),
        }
        .into_any_element()
    }
}

fn drop_text(drop: f64, share: Option<f64>) -> String {
    let amount = kit::format_number(drop.abs());
    let sign = if drop >= 0.0 { "−" } else { "+" };
    share.map_or_else(
        || format!("{sign}{amount}"),
        |share| format!("{sign}{amount} ({sign}{})", percent(share.abs())),
    )
}

#[derive(IntoElement)]
struct FunnelView {
    id: SharedString,
    theme: ArtisanTheme,
    funnel: Funnel,
}

struct FunnelState {
    model: Arc<[Step]>,
    hover: Option<usize>,
    rows: gpui::UniformListScrollHandle,
}

impl RenderOnce for FunnelView {
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree; splitting it would only thread its locals through helpers"
    )]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let funnel = &self.funnel;
        if funnel.steps.is_empty() {
            return frame::empty(&theme, "No steps");
        }
        let state = kit::keyed(window, cx, ElementId::Name(self.id.clone()), || {
            let rows = gpui::UniformListScrollHandle::new();
            FunnelState {
                model: Arc::clone(&funnel.steps),
                hover: None,
                rows,
            }
        });
        state.update(cx, |state, _| {
            if !Arc::ptr_eq(&state.model, &funnel.steps) {
                let changed = *state.model != *funnel.steps;
                state.model = Arc::clone(&funnel.steps);
                if changed {
                    state.hover = None;
                    state.rows = gpui::UniformListScrollHandle::new();
                }
            }
        });
        let hover = state.read(cx).hover;
        let list_state = state.read(cx).rows.clone();
        // The bars take the first series colour; while one is hovered the
        // others step back.
        let series = kit::series_color(&theme, 0);
        let color = series.at(crate::theme::Opacity::BarSolo).to_paint();
        let dimmed = series.at(crate::theme::Opacity::Dimmed).to_paint();
        let row_funnel = funnel.clone();
        let row_state = state.clone();
        let row_id = self.id.clone();
        let rows = gpui::uniform_list(
            kit::part(&self.id, "rows"),
            funnel.steps.len(),
            move |range, _, _| {
                range
                    .map(|index| {
                        let funnel = &row_funnel;
                        let reading = funnel.reading(index);
                        let hovered = hover == Some(index);
                        let fill = match hover {
                            Some(_) if !hovered => dimmed,
                            _ => color,
                        };
                        let entering = row_state.clone();
                        div()
                            .id(kit::part(&row_id, format_args!("step-{index}")))
                            .debug_selector({
                                let id = row_id.clone();
                                move || format!("{id}-step-{index}")
                            })
                            .w_full()
                            .h(theme.density.control_sm)
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
                                    .w(relative(0.24))
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(kit::muted(&theme, funnel.label(index))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(theme.density.control_sm - px(4.0))
                                    .flex()
                                    .justify_center()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(funnel.width(index)))
                                            .rounded(RadiusTokens::value(RadiusStep::Xs))
                                            .bg(fill),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(relative(0.28))
                                    .min_w_0()
                                    .overflow_hidden()
                                    .flex()
                                    .justify_end()
                                    .gap(px(6.0))
                                    .text_size(TypographyTokens::size(TextStep::XsMid))
                                    .line_height(px(20.0))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.colors.text.to_paint())
                                            .child(kit::format_number(reading.value)),
                                    )
                                    .child(
                                        div().text_color(theme.colors.text_muted.to_paint()).child(
                                            reading.of_first.map(percent).unwrap_or_default(),
                                        ),
                                    ),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&list_state)
        .w_full()
        .h(theme.density.control_sm * f32_of_count(funnel.steps.len().min(10)));
        let summary = self.summary(hover);
        let selection_label = hover.map(|index| {
            format!(
                "{}: {}; {}",
                funnel.label(index),
                kit::format_number(funnel.value(index)),
                summary
            )
        });
        let content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(rows)
            .child(div().pt(px(4.0)).child(kit::muted(&theme, summary)));
        let keyboard = state.clone();
        let mut navigation = crate::chart_navigation::ChartNavigation::new(
            ElementId::Name(format!("{}-navigation", self.id).into()),
            theme,
            "Funnel. Arrows explore steps; Escape clears the reading.",
            0..funnel.steps.len(),
            content,
            move |command, _, cx| {
                keyboard.update(cx, |state, cx| {
                    use crate::chart_navigation::ChartCommand;
                    state.hover = match command {
                        ChartCommand::Select(index) | ChartCommand::Activate(index) => {
                            list_state.scroll_to_item(index, gpui::ScrollStrategy::Nearest);
                            Some(index)
                        }
                        ChartCommand::Parent | ChartCommand::Reset => None,
                    };
                    cx.notify();
                });
            },
        )
        .selected(hover)
        .content_height();
        if let Some(label) = selection_label {
            navigation = navigation.selection_label(label);
        }
        navigation.into_any_element()
    }
}

impl FunnelView {
    /// The hovered step against the one before, or the overall conversion.
    #[expect(
        clippy::option_if_let_else,
        reason = "the hovered and overall readings read clearer as two arms"
    )]
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

#[cfg(test)]
mod native_window_tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};
    struct Fixture {
        funnel: Funnel,
        focus: gpui::FocusHandle,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(640.0))
                .track_focus(&self.focus)
                .tab_stop(false)
                .on_action(crate::focus_navigation::focus_next)
                .on_action(crate::focus_navigation::focus_previous)
                .child(FunnelView {
                    id: "native-funnel".into(),
                    theme: ArtisanTheme::resolved(),
                    funnel: self.funnel.clone(),
                })
        }
    }
    #[gpui::test]
    fn steps_keep_native_row_pitch_and_keyboard_reveals_last(cx: &mut TestAppContext) {
        cx.update(crate::focus_navigation::bind_actions);
        let (view, cx) = cx.add_window_view(|window, cx| Fixture {
            focus: crate::focus_navigation::fixture_root_focus(window, cx),
            funnel: Funnel::try_from(FunnelSpec {
                steps: (0..64)
                    .map(|index| Step {
                        label: format!("Step {index}"),
                        value: f64::from(64 - index),
                    })
                    .collect(),
            })
            .unwrap(),
        });
        assert_eq!(
            cx.debug_bounds("native-funnel-step-0").unwrap().size.height,
            ArtisanTheme::resolved().density.control_sm
        );
        assert!(cx.debug_bounds("native-funnel-step-40").is_none());
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.simulate_keystrokes("end");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            cx.debug_bounds("native-funnel-step-63")
                .unwrap()
                .size
                .height,
            ArtisanTheme::resolved().density.control_sm
        );
        // Parsing an unchanged source can produce a new Arc. It must preserve
        // the selected endpoint and the native list's viewport.
        cx.update(|_, cx| {
            view.update(cx, |fixture, cx| {
                fixture.funnel.steps = Arc::from(fixture.funnel.steps.to_vec());
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            cx.debug_bounds("native-funnel-step-63")
                .unwrap()
                .size
                .height,
            ArtisanTheme::resolved().density.control_sm
        );
        cx.simulate_keystrokes("home");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            cx.debug_bounds("native-funnel-step-0").unwrap().size.height,
            ArtisanTheme::resolved().density.control_sm
        );
        assert!(cx.debug_bounds("native-funnel-step-63").is_none());
    }
}
