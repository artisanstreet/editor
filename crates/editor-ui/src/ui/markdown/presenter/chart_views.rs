use super::{HeatmapServices, MarkdownPresenter, PresenterError, RetainedRich, enveloped};
use crate::ui::kit::{
    Button, ButtonSize, ButtonTone, Hover, Label, Radius, Space, Spaced, TextRole, live,
};
use crate::ui::rich::charts::{
    self, funnel::TextRole as FunnelTextRole, heatmap::TextRole as HeatmapTextRole,
};
use crate::ui::rich::{self, Prepared, RichFence};
use crate::ui::theme::ArtisanTheme;
use aqua_core::{GpuiHsla, Insets};
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};
use editor_assets::AssetId;
use std::rc::Rc;
use std::sync::Arc;

fn legend_toggle(
    key: String,
    theme: &ArtisanTheme,
    swatch: Element,
    name: String,
    hidden: bool,
    activate: impl Fn() + 'static,
) -> Element {
    let ink = if hidden {
        theme.colors.text_muted
    } else {
        theme.colors.text
    };
    let element = ui::Button::new()
        .gap(Space::S6.px())
        .style(
            Radius::Field
                .style()
                .padding(Length::Px(Space::S4.px()))
                .align(aqua_ui::AlignItems::CENTER)
                .flex_shrink(0.0),
        )
        .child(live::layer(
            Hover::Neutral,
            theme,
            Radius::Field.px(),
            false,
        ))
        .child(swatch)
        .child(
            TextRole::Caption
                .text(theme, name.clone())
                .gpui_source_color(ink.to_reference_text_color())
                .wrap(false)
                .build(),
        )
        .on_activate(activate)
        .build()
        .with_key(key)
        .role(aqua_core::Role::Button)
        .aria_label(name)
        .aria_selected(!hidden)
        .focusable(true)
        .cursor(aqua_core::CursorIcon::Pointer);
    live::track(element)
}

impl MarkdownPresenter {
    pub(super) fn render_heatmap_chart(
        &self,
        spec: &charts::heatmap::Heatmap,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: &Rc<dyn Fn()>,
        heatmap: Option<&HeatmapServices>,
    ) -> Result<Element, PresenterError> {
        let services = heatmap.ok_or(PresenterError::Unavailable("heatmap services"))?;
        let current_theme = *theme;
        let text: charts::heatmap::TextFactory = Rc::new(
            move |role: HeatmapTextRole,
                  value: &str,
                  size,
                  line,
                  weight,
                  color: crate::ui::theme::Oklch| {
                if role == HeatmapTextRole::Ramp {
                    return charts::frame::mark_text(
                        &current_theme,
                        value,
                        size,
                        line,
                        weight,
                        color,
                        true,
                    );
                }
                let style = aqua_text::TextStyle {
                    families: vec![aqua_text::FontFamily::Named(
                        current_theme.typography.sans.family.to_owned(),
                    )],
                    size_px: size,
                    line_height_px: line,
                    weight,
                    ..aqua_text::TextStyle::default()
                };
                let mut text = ui::Text::new(value.to_owned())
                    .text_style(style)
                    .gpui_source_color(color.to_reference_text_color())
                    .wrap(false);
                if role == HeatmapTextRole::RowLabel {
                    text = text.overflow(aqua_text::overflow::OverflowMode::EndEllipsis);
                } else {
                    text = text.clip(true);
                }
                text.build()
            },
        );
        let block_id = format!("{id}-rich-{}-heatmap", entry.id);
        self.heatmap_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let content = spec.view(
            &block_id,
            theme,
            self.heatmaps.clone(),
            &text,
            &services.empty,
            &services.readout,
            &services.navigation_frame,
            invalidate,
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_funnel_chart(
        &self,
        spec: &charts::funnel::Funnel,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: &Rc<dyn Fn()>,
        heatmap: Option<&HeatmapServices>,
    ) -> Result<Element, PresenterError> {
        let services = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
        let current_theme = *theme;
        let text: charts::funnel::TextFactory = Rc::new(
            move |role: FunnelTextRole, value: &str, size, line, weight, color| {
                let text = charts::frame::mark_text(
                    &current_theme,
                    value,
                    size,
                    line,
                    weight,
                    color,
                    role != FunnelTextRole::RowLabel,
                );
                if role == FunnelTextRole::RowLabel {
                    text.style(Style::new().clip(true))
                } else {
                    text
                }
            },
        );
        let block_id = format!("{id}-rich-{}-funnel", entry.id);
        self.funnel_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let content = spec.view(
            &block_id,
            theme,
            self.funnels.clone(),
            &text,
            &services.empty,
            &services.navigation_frame,
            Rc::clone(invalidate),
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_spans_chart(
        &self,
        layout: &Arc<charts::spans::Layout>,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: &Rc<dyn Fn()>,
        heatmap: Option<&HeatmapServices>,
    ) -> Result<Element, PresenterError> {
        let shared = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
        let block_id = format!("{id}-rich-{}-spans", entry.id);
        self.spans_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let current_theme = *theme;
        let services = charts::spans::Services {
            text: Rc::new(move |value, size, color| {
                let style = aqua_text::TextStyle {
                    families: vec![aqua_text::FontFamily::Named(
                        current_theme.typography.sans.family.to_owned(),
                    )],
                    size_px: size,
                    line_height_px: current_theme.density.control_sm,
                    ..aqua_text::TextStyle::default()
                };
                ui::Text::new(value.to_owned())
                    .text_style(style)
                    .gpui_source_color(color.to_reference_text_color())
                    .wrap(false)
                    .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                    .build()
            }),
            bar_label: Rc::new(move |label: charts::spans::BarLabel| {
                let style = aqua_text::TextStyle {
                    families: vec![aqua_text::FontFamily::Named(
                        current_theme.typography.sans.family.to_owned(),
                    )],
                    size_px: current_theme.typography.control_text,
                    line_height_px: label.row_pitch - 4.0,
                    weight: aqua_text::FontWeight::MEDIUM,
                    ..aqua_text::TextStyle::default()
                };
                let text = ui::Text::new(label.text)
                    .text_style(style)
                    .gpui_source_color(current_theme.colors.text.to_reference_text_color())
                    .wrap(false)
                    .clip(true)
                    .build();
                let bar = ui::Box::new()
                    .width(Length::Pct((label.x1 - label.x0) * 100.0))
                    .height(label.row_pitch - 4.0)
                    .style(
                        Style::new()
                            .clip(true)
                            .padding_insets(Insets {
                                left: 6.0,
                                right: 6.0,
                                ..Insets::default()
                            })
                            .align(aqua_ui::AlignItems::CENTER),
                    )
                    .child(text)
                    .build();
                ui::Box::new()
                    .layout("row")
                    .width(Length::Pct(100.0))
                    .height(label.row_pitch - 4.0)
                    .style(Style::new().absolute_at(Some(0.0), Some(2.0), None, None))
                    .child(
                        ui::Box::new()
                            .width(Length::Pct(label.x0 * 100.0))
                            .style(Style::new().flex_shrink(0.0))
                            .build(),
                    )
                    .child(bar)
                    .build()
            }),
            marker: Rc::new(move |x, label, color| {
                charts::frame::plot_label_positioned(&current_theme, x, 0.0, label, color, true)
            }),
            ticks: Rc::new(move |ticks: Vec<charts::spans::Tick>| {
                charts::frame::tick_strip(
                    &current_theme,
                    ticks
                        .into_iter()
                        .map(|tick| charts::distribution::Tick {
                            at: tick.at,
                            label: tick.label,
                        })
                        .collect(),
                )
            }),
            readout: Rc::new(move |x, heading, rows| {
                charts::frame::readout_rows(
                    &current_theme,
                    x,
                    heading.unwrap_or(""),
                    rows.into_iter()
                        .map(|(color, label, value)| charts::distribution::ReadoutRow {
                            color,
                            label,
                            value,
                        })
                        .collect(),
                )
            }),
            empty: Rc::clone(&shared.empty),
            navigation: Rc::clone(&shared.navigation_frame),
        };
        let content = charts::spans::view(
            &block_id,
            theme,
            layout,
            self.spans.clone(),
            &services,
            Rc::clone(invalidate),
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_hierarchy_chart(
        &self,
        spec: &charts::hierarchy::Prepared,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: &Rc<dyn Fn()>,
        heatmap: Option<&HeatmapServices>,
    ) -> Result<Element, PresenterError> {
        let shared = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
        let block_id = format!("{id}-rich-{}-hierarchy", entry.id);
        self.hierarchy_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let current_theme = *theme;
        let trail_id = block_id.clone();
        let services = charts::hierarchy::Services {
            label: Rc::new(move |part: charts::hierarchy::PartLabel| {
                let text_style = |weight| aqua_text::TextStyle {
                    families: vec![aqua_text::FontFamily::Named(
                        current_theme.typography.sans.family.to_owned(),
                    )],
                    size_px: 11.0,
                    line_height_px: 14.0,
                    weight,
                    ..aqua_text::TextStyle::default()
                };
                let name = ui::Text::new(part.name)
                    .text_style(text_style(aqua_text::FontWeight::MEDIUM))
                    .gpui_source_color(current_theme.colors.text.to_reference_text_color())
                    .wrap(false)
                    .clip(true)
                    .build();
                let mut label = ui::Box::new()
                    .layout(if part.stacked { "column" } else { "row" })
                    .width(part.width * part.plot_width)
                    .height(part.height)
                    .gap(if part.stacked { 0.0 } else { 6.0 })
                    .style(
                        Style::new()
                            .absolute_at(
                                Some(part.x * part.plot_width),
                                Some(part.y * part.plot_height),
                                None,
                                None,
                            )
                            .clip(true)
                            .padding_insets(Insets {
                                left: 6.0,
                                right: 6.0,
                                top: if part.stacked { 3.0 } else { 0.0 },
                                bottom: if part.stacked { 3.0 } else { 0.0 },
                            })
                            .align(if part.stacked {
                                aqua_ui::AlignItems::START
                            } else {
                                aqua_ui::AlignItems::CENTER
                            }),
                    );
                label = label.child(name);
                if let Some(value) = part.value {
                    label = label.child(
                        ui::Text::new(value)
                            .text_style(text_style(aqua_text::FontWeight::NORMAL))
                            .gpui_source_color(
                                current_theme.colors.text_muted.to_reference_text_color(),
                            )
                            .wrap(false)
                            .clip(true)
                            .build(),
                    );
                }
                label.build()
            }),
            readout: Rc::new(move |high, x, heading, rows| {
                let card = charts::frame::readout_rows_at(
                    &current_theme,
                    x,
                    heading,
                    rows.into_iter()
                        .map(|(color, label, value)| charts::distribution::ReadoutRow {
                            color,
                            label,
                            value,
                        })
                        .collect(),
                    high,
                );
                card.on_input(
                    aqua_ui::input::InputKind::PointerMove,
                    aqua_ui::input::EventPhase::Bubble,
                    |_| aqua_ui::input::InputResponse {
                        stop_propagation: true,
                        ..Default::default()
                    },
                )
                .on_input(
                    aqua_ui::input::InputKind::PointerDown,
                    aqua_ui::input::EventPhase::Bubble,
                    |_| aqua_ui::input::InputResponse {
                        stop_propagation: true,
                        ..Default::default()
                    },
                )
                .on_input(
                    aqua_ui::input::InputKind::PointerUp,
                    aqua_ui::input::EventPhase::Bubble,
                    |_| aqua_ui::input::InputResponse {
                        stop_propagation: true,
                        ..Default::default()
                    },
                )
            }),
            trail: Rc::new(move |entries, select| {
                let mut trail = ui::Box::new()
                    .layout("row")
                    .spaced(Space::S2)
                    .style(Style::new().wrap(true).align(aqua_ui::AlignItems::CENTER));
                for (step, (node, name, current)) in entries.into_iter().enumerate() {
                    if step > 0 {
                        trail = trail.child(rich::narrative::callout::text(
                            &current_theme,
                            "/",
                            TextRole::Caption,
                            current_theme.colors.text_muted,
                        ));
                    }
                    let choose = Rc::clone(&select);
                    trail = trail.child(
                        Button::text(
                            format!("{trail_id}-trail-{node}"),
                            current_theme,
                            ButtonTone::Quiet,
                            name,
                        )
                        .size(ButtonSize::Sm)
                        .disabled(current)
                        .on_activate(move |_| choose(node))
                        .build(),
                    );
                }
                trail.build()
            }),
            click: Rc::new(move |element, handler| {
                ui::Button::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0))
                    .child(element)
                    .on_activate_event(move |event| {
                        if let Some(aqua_ui::tree::ActivationSource::Pointer {
                            up,
                            button: aqua_core::PointerButton::Primary,
                            ..
                        }) = event.source
                        {
                            handler(up.position);
                        }
                    })
                    .build()
                    .focusable(false)
            }),
            navigation: Rc::clone(&shared.navigation_frame),
        };
        let content = charts::hierarchy::view(
            block_id,
            theme,
            &spec.tree,
            spec.layout,
            spec.unit.clone().as_ref(),
            charts::hierarchy::levels(spec.layout, spec.depth),
            self.hierarchies.clone(),
            &services,
            invalidate,
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_sankey_chart(
        &self,
        graph: &Arc<charts::sankey::Graph>,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: &Rc<dyn Fn()>,
        heatmap: Option<&HeatmapServices>,
    ) -> Result<Element, PresenterError> {
        let shared = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
        let block_id = format!("{id}-rich-{}-sankey", entry.id);
        self.sankey_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let current_theme = *theme;
        let label: charts::sankey::LabelFactory =
            Rc::new(move |label: charts::sankey::NodeLabel| {
                let style = aqua_text::TextStyle {
                    families: vec![aqua_text::FontFamily::Named(
                        current_theme.typography.sans.family.to_owned(),
                    )],
                    size_px: current_theme.typography.control_text,
                    line_height_px: label.line_height,
                    ..aqua_text::TextStyle::default()
                };
                let name = ui::Text::new(label.name)
                    .text_style(style.clone())
                    .gpui_source_color(current_theme.colors.text.to_reference_text_color())
                    .wrap(false)
                    .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                    .build();
                let value = ui::Text::new(label.value)
                    .text_style(style)
                    .gpui_source_color(current_theme.colors.text_muted.to_reference_text_color())
                    .wrap(false)
                    .build();
                let (left, right) = if label.last {
                    (None, Some(label.x))
                } else {
                    (Some(label.x), None)
                };
                ui::Box::new()
                    .layout("row")
                    .gap(4.0)
                    .height(label.line_height)
                    .style(
                        Style::new()
                            .absolute_at(left, Some(label.y), right, None)
                            .max_width(Length::Px(label.max_width))
                            .min_width(Length::Px(0.0))
                            .clip(true)
                            .opacity(label.alpha),
                    )
                    .child(
                        ui::Box::new()
                            .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                            .child(name)
                            .build(),
                    )
                    .child(
                        ui::Box::new()
                            .style(Style::new().flex_shrink(0.0))
                            .child(value)
                            .build(),
                    )
                    .build()
            });
        let readout: charts::sankey::ReadoutFactory = Rc::new(
            move |x, heading: &str, rows: Vec<(Option<GpuiHsla>, String, String)>| {
                charts::frame::readout_rows(
                    &current_theme,
                    x,
                    heading,
                    rows.into_iter()
                        .map(|(color, label, value)| charts::distribution::ReadoutRow {
                            color,
                            label,
                            value,
                        })
                        .collect(),
                )
            },
        );
        let content = charts::sankey::view(
            &block_id,
            theme,
            graph,
            self.sankeys.clone(),
            &label,
            &readout,
            &shared.empty,
            &shared.navigation_frame,
            Rc::clone(invalidate),
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_line_chart(
        &self,
        style: &Arc<charts::chart::ChartStyle>,
        model: &Arc<charts::chart::model::Model>,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: &Rc<dyn Fn()>,
        heatmap: Option<&HeatmapServices>,
    ) -> Result<Element, PresenterError> {
        let shared = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
        let block_id = format!("{id}-rich-{}-chart", entry.id);
        self.chart_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let current_theme = *theme;
        let legend_id = block_id.clone();
        let reset_id = format!("{block_id}-reset");
        let services = charts::chart::Services {
            press: Rc::new(charts::chart::press),
            frame: Rc::new(
                move |plot, y_ticks, x_ticks, end_labels, y_title, x_title| {
                    let ticks = |values: Vec<charts::chart::plot::Tick>| {
                        values
                            .into_iter()
                            .map(|tick| charts::distribution::Tick {
                                at: tick.at,
                                label: tick.label,
                            })
                            .collect()
                    };
                    let frame = charts::distribution::PlotFrame {
                        x_ticks: ticks(x_ticks),
                        y_ticks: ticks(y_ticks),
                        y_title,
                        x_title,
                        y_axis_width: 48.0,
                        height: 240.0,
                    };
                    charts::frame::numeric_frame_with_labels(
                        &current_theme,
                        plot,
                        frame,
                        end_labels
                            .into_iter()
                            .map(|label| (label.y, label.text, label.color))
                            .collect(),
                    )
                },
            ),
            label: Rc::new(move |label| {
                charts::frame::plot_label_positioned(
                    &current_theme,
                    label.x,
                    label.y,
                    &label.text,
                    label.color,
                    label.below,
                )
            }),
            readout: Rc::new(move |x, heading, rows| {
                charts::frame::readout_rows(
                    &current_theme,
                    x,
                    heading,
                    rows.into_iter()
                        .map(|(color, label, value)| charts::distribution::ReadoutRow {
                            color,
                            label,
                            value,
                        })
                        .collect(),
                )
            }),
            legend: Rc::new(move |entries, toggle| {
                let mut legend = ui::Box::new()
                    .layout("row")
                    .spaced(Space::S4)
                    .style(Style::new().wrap(true).align(aqua_ui::AlignItems::CENTER));
                for (index, (name, color, hidden)) in entries.into_iter().enumerate() {
                    let color = if hidden {
                        current_theme
                            .colors
                            .text
                            .at(crate::ui::theme::Opacity::Rule)
                            .to_reference_paint()
                    } else {
                        color
                    };
                    let swatch = ui::Box::new()
                        .width(12.0)
                        .height(4.0)
                        .radius(crate::ui::theme::RadiusTokens::value(
                            crate::ui::theme::RadiusStep::X2s,
                        ))
                        .style(
                            Style::new()
                                .flex_shrink(0.0)
                                .gpui_background(aqua_core::GpuiBackground::Solid(color)),
                        )
                        .build();
                    let activate = Rc::clone(&toggle);
                    let button = legend_toggle(
                        format!("{legend_id}-legend-{index}"),
                        &current_theme,
                        swatch,
                        name,
                        hidden,
                        move || activate(index),
                    );
                    legend = legend.child(button);
                }
                legend.build()
            }),
            reset: Rc::new(move |reset| {
                Button::new(
                    reset_id.clone(),
                    current_theme,
                    ButtonTone::Quiet,
                    ButtonSize::Md,
                    Label::IconText(AssetId::TABLER_ZOOM_OUT, "Reset zoom".to_owned()),
                )
                .on_activate(move |_| reset())
                .build()
            }),
            empty: Rc::clone(&shared.empty),
            navigation: Rc::clone(&shared.navigation_frame),
        };
        let content = charts::chart::view(
            &block_id,
            theme,
            style,
            model,
            &self.charts.clone(),
            &services,
            invalidate,
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_distribution_chart(
        &self,
        spec: &Arc<charts::distribution::Distribution>,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: &Rc<dyn Fn()>,
        heatmap: Option<&HeatmapServices>,
    ) -> Result<Element, PresenterError> {
        let shared = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
        let block_id = format!("{id}-rich-{}-distribution", entry.id);
        self.distribution_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let current_theme = *theme;
        let services = charts::distribution::Services {
            frame: Rc::new(move |plot, frame| {
                charts::frame::numeric_frame(&current_theme, plot, frame)
            }),
            readout: Rc::new(move |x, heading, rows| {
                charts::frame::readout_rows(&current_theme, x, heading, rows)
            }),
            plot_label: Rc::new(move |x, y, label, color| {
                charts::frame::plot_label(&current_theme, x, y, label, color)
            }),
            legend: Rc::new(move |entries| charts::frame::legend(&current_theme, entries)),
            text: Rc::new(move |value, size, line, weight, color, ellipsis| {
                let style = aqua_text::TextStyle {
                    families: vec![aqua_text::FontFamily::Named(
                        current_theme.typography.sans.family.to_owned(),
                    )],
                    size_px: size,
                    line_height_px: line,
                    weight,
                    ..aqua_text::TextStyle::default()
                };
                let text = ui::Text::new(value.to_owned())
                    .text_style(style)
                    .gpui_source_color(color.to_reference_text_color())
                    .wrap(false);
                if ellipsis {
                    text.overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                        .build()
                } else {
                    text.clip(true).build()
                }
            }),
            empty: Rc::clone(&shared.empty),
            navigation: Rc::clone(&shared.navigation_frame),
            hover_fill: Rc::new(move || {
                let (top, bottom) =
                    crate::ui::hover::hover_fill_stops(&current_theme, current_theme.colors.text);
                aqua_core::GpuiBackground::LinearGradient {
                    angle: 180.0,
                    color_space: aqua_core::GpuiGradientColorSpace::Oklab,
                    stops: [
                        aqua_core::GpuiGradientStop {
                            percentage: 0.0,
                            color: top.to_reference_paint(),
                        },
                        aqua_core::GpuiGradientStop {
                            percentage: 1.0,
                            color: bottom.to_reference_paint(),
                        },
                    ],
                }
            }),
            tick_strip: Rc::new(move |ticks| charts::frame::tick_strip(&current_theme, ticks)),
        };
        let content = charts::distribution::view(
            &block_id,
            theme,
            &Arc::clone(spec),
            self.distributions.clone(),
            &services,
            Rc::clone(invalidate),
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_concentration_chart(
        &self,
        spec: &charts::concentration::Concentration,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: &Rc<dyn Fn()>,
        heatmap: Option<&HeatmapServices>,
    ) -> Result<Element, PresenterError> {
        let services = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
        let block_id = format!("{id}-rich-{}-concentration", entry.id);
        self.concentration_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let content = spec.view(
            &block_id,
            theme,
            self.concentrations.clone(),
            &services.concentration.frame,
            &services.concentration.plot_label,
            &services.concentration.readout,
            &services.empty,
            &services.navigation_frame,
            Rc::clone(invalidate),
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub fn heatmap_reveals(&self, id: &str) -> Vec<(String, String, usize)> {
        self.rich.iter().filter_map(|block| {
            let entry = block.as_ref()?;
            if !matches!(&entry.prepared, RichFence::Ready(envelope) if matches!(envelope.block, Prepared::Chart(charts::Prepared::Heatmap(_)))) { return None; }
            let block_id = format!("{id}-rich-{}-heatmap", entry.id);
            let (key, row) = self.heatmaps.pending_reveal(&block_id)?;
            Some((block_id, key, row))
        }).collect()
    }

    pub fn heatmap_revealed(&self, id: &str, key: &str, row: usize) {
        self.heatmaps.revealed(id, key, row);
    }

    pub fn funnel_reveals(&self, id: &str) -> Vec<(String, String, usize)> {
        self.rich.iter().filter_map(|block| {
            let entry = block.as_ref()?;
            if !matches!(&entry.prepared, RichFence::Ready(envelope) if matches!(envelope.block, Prepared::Chart(charts::Prepared::Funnel(_)))) { return None; }
            let block_id = format!("{id}-rich-{}-funnel", entry.id);
            let (key, row) = self.funnels.pending_reveal(&block_id)?;
            Some((block_id, key, row))
        }).collect()
    }

    pub fn funnel_revealed(&self, id: &str, key: &str, row: usize) {
        self.funnels.revealed(id, key, row);
    }

    pub fn distribution_reveals(&self, id: &str) -> Vec<(String, String, usize)> {
        self.rich.iter().filter_map(|block| {
            let entry = block.as_ref()?;
            if !matches!(&entry.prepared, RichFence::Ready(envelope) if matches!(envelope.block, Prepared::Chart(charts::Prepared::Distribution(_)))) { return None; }
            let block_id = format!("{id}-rich-{}-distribution", entry.id);
            let (key, row) = self.distributions.pending_reveal(&block_id)?;
            Some((block_id, key, row))
        }).collect()
    }

    pub fn distribution_revealed(&self, id: &str, key: &str, row: usize) {
        self.distributions.revealed(id, key, row);
    }

    pub fn spans_reveals(&self, id: &str) -> Vec<(String, String, usize)> {
        self.rich.iter().filter_map(|block| {
            let entry = block.as_ref()?;
            if !matches!(&entry.prepared, RichFence::Ready(envelope) if matches!(envelope.block, Prepared::Chart(charts::Prepared::Spans(_)))) { return None; }
            let block_id = format!("{id}-rich-{}-spans", entry.id);
            let (key, row) = self.spans.pending_reveal(&block_id)?;
            Some((block_id, key, row))
        }).collect()
    }

    pub fn spans_revealed(&self, id: &str, key: &str, row: usize) {
        self.spans.revealed(id, key, row);
    }
}
