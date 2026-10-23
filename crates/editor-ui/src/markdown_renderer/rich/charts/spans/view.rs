//! Drawing a span [`Layout`]: rows of bars over a time axis, row names on
//! the left, markers as labelled rules, and a glass readout on hover.

use crate::theme::{ArtisanTheme, Opacity, RadiusStep, RadiusTokens};
use gpui::{
    AnyElement, App, ElementId, Entity, FontWeight, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::*, px, relative,
};
use std::sync::Arc;

use super::super::super::kit;
use super::super::frame::{self, PlotLabel, ReadoutRow, Tick};
use super::super::scale::{Domain, f32_of, f32_of_count, f64_of, nice_ticks};
use super::super::scene::{Probe, Shape, hrule, plot_canvas, probe, probed_width, vrule};
use super::{Layout, Span};
use crate::hover_card::hover_highlight_shadow;

/// Room above the first row for marker labels.
const MARKER_ROOM: f32 = 20.0;

/// Width of the row names.
const NAME_WIDTH: f32 = 96.0;

/// A bar narrower than this hides its text.
const MIN_TEXT_WIDTH: f32 = 44.0;

/// Plot width assumed before the first paint.
const FALLBACK_WIDTH: f32 = 560.0;

#[derive(IntoElement, Clone)]
pub(super) struct SpansView {
    pub(super) id: SharedString,
    pub(super) theme: ArtisanTheme,
    pub(super) layout: Arc<Layout>,
}

/// Where things land in the plot.
#[derive(Clone, Copy)]
struct Placement {
    domain: Domain,
    width: f32,
    /// Room above the first row for marker labels.
    height: f32,
    row_base: usize,
    row_pitch: f32,
}

impl Placement {
    /// The top of `row` as a fraction of the plot height.
    fn row_at(self, row: f32) -> f32 {
        ((row - f32_of_count(self.row_base)) * self.row_pitch) / self.height
    }
}

impl SpansView {
    /// The grid, lane dividers and marker rules, with the markers' labels.
    fn guides(
        &self,
        place: Placement,
        x_ticks: &[Tick],
        include_labels: bool,
    ) -> (Vec<Shape>, Vec<PlotLabel>) {
        let theme = &self.theme;
        let mut shapes = frame::grid(theme, &[], x_ticks);
        for lane in self
            .layout
            .lanes
            .iter()
            .skip(1)
            .filter(|lane| lane.first_row == place.row_base)
        {
            shapes.push(hrule(
                place.row_at(f32_of(f64_of(lane.first_row))),
                kit::hairline(theme),
                false,
            ));
        }
        let mut labels = Vec::new();
        for marker in &self.layout.markers {
            let x = place.domain.fraction(marker.at);
            shapes.push(vrule(
                x,
                theme.colors.text.at(Opacity::EventRule).to_paint(),
                true,
            ));
            if include_labels && !marker.label.is_empty() {
                labels.push(PlotLabel {
                    x,
                    y: 0.0,
                    below: true,
                    text: marker.label.clone(),
                    color: theme.colors.text.to_paint(),
                });
            }
        }
        (shapes, labels)
    }
}

struct SpansState {
    model: Arc<Layout>,
    rows: gpui::UniformListScrollHandle,
    hover: Option<usize>,
    probe: Probe,
}

impl RenderOnce for SpansView {
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree; splitting it would only thread its locals through helpers"
    )]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let layout = &self.layout;
        if layout.spans.is_empty() {
            return frame::empty(&theme, "No spans");
        }
        let state = kit::keyed(window, cx, kit::part(&self.id, "state"), || {
            let rows = gpui::UniformListScrollHandle::new();
            SpansState {
                model: Arc::clone(layout),
                rows,
                hover: None,
                probe: probe(),
            }
        });
        state.update(cx, |state, _| {
            if !Arc::ptr_eq(&state.model, layout) {
                let changed = *state.model != **layout;
                state.model = Arc::clone(layout);
                if changed {
                    state.hover = None;
                    state.rows = gpui::UniformListScrollHandle::new();
                }
            }
        });
        let (hover, plot_probe) = {
            let current = state.read(cx);
            (current.hover, std::rc::Rc::clone(&current.probe))
        };
        let width = probed_width(&plot_probe, FALLBACK_WIDTH);
        let ticks = if layout.dates {
            let step = super::super::scale::nice_step(
                layout.domain.span()
                    / super::super::scale::f64_of(frame::label_budget(width, 96.0)),
            )
            .ceil()
            .max(1.0);
            let first = (layout.domain.lo / step).ceil() * step;
            let values = (0..=200)
                .map(|i| f64::from(i).mul_add(step, first))
                .take_while(|value| *value <= layout.domain.hi)
                .collect();
            super::super::scale::Ticks {
                domain: layout.domain,
                values,
            }
        } else {
            nice_ticks(
                layout.domain,
                frame::label_budget(width, 96.0),
                false,
                false,
            )
        };
        let domain = ticks.domain;
        let x_ticks: Vec<Tick> = ticks
            .values
            .iter()
            .map(|value| Tick {
                at: domain.fraction(*value),
                label: SharedString::from(layout.time(*value)),
            })
            .collect();
        let marker_room = if layout.markers.is_empty() {
            0.0
        } else {
            MARKER_ROOM
        };
        let pitch = f32::from(theme.density.control_sm);
        let header_place = Placement {
            domain,
            width,
            height: MARKER_ROOM,
            row_base: 0,
            row_pitch: pitch,
        };
        let (_, labels) = self.guides(header_place, &x_ticks, true);
        let row_state = state.clone();
        let list_state = state.read(cx).rows.clone();
        let view = self.clone();
        let row_ticks = x_ticks.clone();
        let rows = gpui::uniform_list(
            kit::part(&self.id, "rows"),
            layout.rows,
            move |range, _, _| {
                range
                    .map(|row| {
                        let place = Placement {
                            domain,
                            width,
                            height: pitch,
                            row_base: row,
                            row_pitch: pitch,
                        };
                        let (shapes, _) = view.guides(place, &row_ticks, false);
                        let name = if view.layout.lanes.is_empty() {
                            view.layout.row_spans[row]
                                .first()
                                .map(|index| view.layout.spans[*index].label.clone())
                                .unwrap_or_default()
                        } else {
                            let lane = view
                                .layout
                                .lanes
                                .partition_point(|lane| lane.first_row <= row)
                                .saturating_sub(1);
                            view.layout
                                .lanes
                                .get(lane)
                                .map(|lane| lane.name.clone())
                                .unwrap_or_default()
                        };
                        let bars = view.layout.row_spans[row].iter().map(|index| {
                            view.bar(
                                &row_state,
                                *index,
                                &view.layout.spans[*index],
                                hover == Some(*index),
                                place,
                            )
                        });
                        let selector = view.id.clone();
                        div()
                            .id(kit::part(&view.id, format_args!("row-{row}")))
                            .debug_selector(move || format!("{selector}-row-{row}"))
                            .w_full()
                            .h(px(pitch))
                            .flex()
                            .child(
                                div()
                                    .w(px(NAME_WIDTH))
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .pr(px(8.0))
                                    .text_size(theme.typography.control_text)
                                    .text_color(theme.colors.text_muted.to_paint())
                                    .child(name),
                            )
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .child(plot_canvas(shapes, probe()))
                                    .children(bars),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&list_state)
        .w_full()
        .h(theme.density.control_sm * f32_of_count(layout.rows.min(10)));
        let (readout, selection_label) = hover
            .and_then(|index| layout.spans.get(index))
            .map(|span| {
                self.readout(
                    span,
                    f32::midpoint(domain.fraction(span.start), domain.fraction(span.end)),
                    false,
                )
            })
            .unzip();
        let plot = div()
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .when(marker_room > 0.0, |plot| {
                plot.child(
                    div()
                        .ml(px(NAME_WIDTH))
                        .relative()
                        .h(px(marker_room))
                        .children(
                            labels
                                .into_iter()
                                .map(|label| frame::plot_label(&theme, label)),
                        ),
                )
            })
            .child(rows)
            .child(
                div()
                    .pl(px(NAME_WIDTH))
                    .child(frame::x_tick_strip(&theme, x_ticks)),
            )
            .children(readout)
            .child(
                div()
                    .absolute()
                    .left(px(NAME_WIDTH))
                    .right_0()
                    .top_0()
                    .h(px(1.0))
                    .child(plot_canvas(Vec::new(), plot_probe)),
            );
        let keyboard = state.clone();
        let mut navigation = crate::chart_navigation::ChartNavigation::new(
            ElementId::Name(format!("{}-navigation", self.id).into()), theme,
            "Timeline. Arrows explore spans; Home and End select endpoints; Escape clears the reading.",
            0..layout.spans.len(), plot, move |command, _, cx| keyboard.update(cx, |state, cx| {
                use crate::chart_navigation::ChartCommand;
                state.hover = match command {
                    ChartCommand::Select(index) | ChartCommand::Activate(index) => {
                        list_state.scroll_to_item(state.model.spans[index].row, gpui::ScrollStrategy::Nearest);
                        Some(index)
                    },
                    ChartCommand::Parent | ChartCommand::Reset => None,
                };
                cx.notify();
            }),
        ).selected(hover).content_height();
        if let Some(label) = selection_label {
            navigation = navigation.selection_label(label);
        }
        navigation.into_any_element()
    }
}

impl SpansView {
    fn bar(
        &self,
        state: &Entity<SpansState>,
        index: usize,
        span: &Span,
        hovered: bool,
        place: Placement,
    ) -> AnyElement {
        let theme = self.theme;
        let layout = &self.layout;
        let x0 = place.domain.fraction(span.start);
        let x1 = place.domain.fraction(span.end);
        let wide = (x1 - x0) * place.width >= MIN_TEXT_WIDTH;
        // Laned rows name the lane, so bars carry their own label; a
        // waterfall names each row by its span, so bars carry the length.
        let text = if layout.lanes.is_empty() {
            SharedString::from(layout.duration(span))
        } else {
            span.label.clone()
        };
        let color = kit::series_color(&theme, span.color);
        let hovering = state.clone();
        div()
            .id(kit::part(&self.id, format_args!("span-{index}")))
            .absolute()
            .left(relative(x0))
            .w(relative(x1 - x0))
            .min_w(px(2.0))
            .top(relative(place.row_at(f32_of(f64_of(span.row)))))
            .mt(px(2.0))
            .h(px(place.row_pitch - 4.0))
            .flex()
            .items_center()
            .px(px(6.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .rounded(RadiusTokens::value(RadiusStep::Xs))
            .bg(color
                .at(if hovered {
                    Opacity::SpanHover
                } else {
                    Opacity::Span
                })
                .to_paint())
            .when(hovered, |bar| bar.shadow(hover_highlight_shadow(&theme)))
            .text_size(theme.typography.control_text)
            .line_height(px(place.row_pitch - 4.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.text.to_paint())
            .when(wide, |bar| bar.child(text))
            .on_hover(move |entered, _, cx| {
                hovering.update(cx, |state, cx| {
                    let next = if *entered {
                        Some(index)
                    } else if state.hover == Some(index) {
                        None
                    } else {
                        return;
                    };
                    if state.hover != next {
                        state.hover = next;
                        cx.notify();
                    }
                });
            })
            .into_any_element()
    }

    fn readout(&self, span: &Span, x: f32, high: bool) -> (gpui::Div, SharedString) {
        let layout = &self.layout;
        let theme = &self.theme;
        let mut rows = vec![
            ReadoutRow::new(
                Some(kit::series_color(theme, span.color).to_paint()),
                "Length",
                layout.duration(span),
            ),
            ReadoutRow::new(None, "Start", layout.time(span.start)),
            ReadoutRow::new(None, "End", layout.time(span.end)),
        ];
        if let Some(lane) = span.lane.and_then(|lane| layout.lanes.get(lane)) {
            rows.push(ReadoutRow::new(None, "Lane", lane.name.clone()));
        }
        if let Some(group) = &span.group {
            rows.push(ReadoutRow::new(None, "Group", group.clone()));
        }
        let heading = (!span.label.is_empty()).then(|| span.label.clone());
        let selection_label = frame::reading_label(heading.as_deref(), &rows);
        let card = if high {
            frame::readout_low(theme, x, heading, rows)
        } else {
            frame::readout(theme, x, heading, rows)
        };
        (card, selection_label)
    }
}

#[cfg(test)]
mod native_window_tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};
    struct Fixture {
        layout: Arc<Layout>,
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
                .child(SpansView {
                    id: "native-spans".into(),
                    theme: ArtisanTheme::resolved(),
                    layout: Arc::clone(&self.layout),
                })
        }
    }
    #[gpui::test]
    fn waterfall_rows_are_windowed_and_keyboard_reveals_last(cx: &mut TestAppContext) {
        cx.update(crate::focus_navigation::bind_actions);
        let (view,cx)=cx.add_window_view(|window,cx| {
            let spans: Vec<_>=(0..512).map(|index|serde_json::json!({"label":format!("Span {index}"),"start":index,"end":index+1})).collect();
            let spec=serde_json::from_value(serde_json::json!({"spans":spans})).unwrap();
            let layout=Layout::from_spec(&spec).unwrap();
            assert_eq!(layout.row_spans.len(),512);
            assert_eq!(layout.row_spans.iter().flatten().count(),layout.spans.len());
            Fixture{layout:Arc::new(layout),focus:crate::focus_navigation::fixture_root_focus(window,cx)}
        });
        assert_eq!(
            cx.debug_bounds("native-spans-row-0").unwrap().size.height,
            ArtisanTheme::resolved().density.control_sm
        );
        assert!(cx.debug_bounds("native-spans-row-50").is_none());
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
            cx.debug_bounds("native-spans-row-511").unwrap().size.height,
            ArtisanTheme::resolved().density.control_sm
        );
        // Parsing an unchanged source can produce a new Arc. It must preserve
        // the selected endpoint and the native list's viewport.
        cx.update(|_, cx| {
            view.update(cx, |fixture, cx| {
                fixture.layout = Arc::new((*fixture.layout).clone());
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            cx.debug_bounds("native-spans-row-511").unwrap().size.height,
            ArtisanTheme::resolved().density.control_sm
        );
        cx.simulate_keystrokes("home");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            cx.debug_bounds("native-spans-row-0").unwrap().size.height,
            ArtisanTheme::resolved().density.control_sm
        );
        assert!(cx.debug_bounds("native-spans-row-511").is_none());
    }
}
