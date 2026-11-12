use crate::ui::kit::{Space, Surface, Tone, Tooltip};
use crate::ui::rich::narrative::callout::{text, text_with_wrap};
use crate::ui::theme::{ArtisanTheme, Opacity, RadiusStep, RadiusTokens};
use aqua_core::{GpuiHsla, Insets};
use aqua_text::FontWeight;
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};

pub(crate) fn empty(theme: &ArtisanTheme, label: &str) -> Element {
    Surface::Callout(Tone::Neutral)
        .build(
            theme,
            vec![text(
                theme,
                label,
                14.0,
                20.0,
                FontWeight::NORMAL,
                theme.colors.text_muted,
            )],
        )
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .min_width(Length::Px(0.0))
                .min_height(Length::Px(72.0))
                .align(aqua_ui::AlignItems::CENTER)
                .justify(aqua_ui::JustifyContent::CENTER),
        )
}

pub(crate) fn navigation(content: Element, ring: GpuiHsla) -> Element {
    content.focus_visible_style(Style::new().gpui_border(Insets::uniform(1.0), ring, false))
}

pub(crate) fn concentration_frame(
    theme: &ArtisanTheme,
    plot: Element,
    y_title: &str,
    x_title: &str,
) -> Element {
    let title = |value: &str| {
        text_with_wrap(
            theme,
            value,
            11.0,
            16.0,
            FontWeight::MEDIUM,
            theme.colors.text_muted,
            false,
        )
    };
    let mut y_axis = ui::Box::new()
        .width(48.0)
        .height(240.0)
        .style(Style::new().flex_shrink(0.0));
    for step in 0..=4 {
        let label = format!("{}%", 100 - step * 25);
        y_axis = y_axis.child(
            ui::Box::new()
                .height(16.0)
                .style(
                    Style::new()
                        .absolute_at(
                            Some(0.0),
                            Some(super::scale::f32_of_count(step) * 60.0 - 8.0),
                            Some(8.0),
                            None,
                        )
                        .justify(aqua_ui::JustifyContent::END)
                        .clip(true),
                )
                .child(text_with_wrap(
                    theme,
                    &label,
                    11.0,
                    16.0,
                    FontWeight::NORMAL,
                    theme.colors.text_muted,
                    false,
                ))
                .build(),
        );
    }
    let plot_row = ui::Box::new()
        .layout("row")
        .width(Length::Pct(100.0))
        .height(240.0)
        .child(y_axis.build())
        .child(
            ui::Box::new()
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .min_width(Length::Px(0.0))
                        .height(Length::Pct(100.0)),
                )
                .child(plot)
                .build(),
        )
        .child(
            ui::Box::new()
                .width(12.0)
                .height(Length::Pct(100.0))
                .style(Style::new().flex_shrink(0.0))
                .build(),
        )
        .build();
    let mut ticks = ui::Box::new().width(Length::Pct(100.0)).height(16.0);
    for step in 0..=4 {
        let label = format!("{}%", step * 25);
        let tick = ui::Box::new()
            .width(88.0)
            .height(16.0)
            .style(
                Style::new()
                    .absolute_at(Some(-44.0), Some(0.0), None, None)
                    .justify(aqua_ui::JustifyContent::CENTER)
                    .clip(true),
            )
            .child(text_with_wrap(
                theme,
                &label,
                11.0,
                16.0,
                FontWeight::NORMAL,
                theme.colors.text_muted,
                false,
            ))
            .build();
        let anchor = ui::Box::new()
            .layout("row")
            .width(Length::Pct(100.0))
            .height(16.0)
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, None))
            .child(
                ui::Box::new()
                    .width(Length::Pct(super::scale::f32_of_count(step) * 25.0))
                    .style(Style::new().flex_shrink(0.0))
                    .build(),
            )
            .child(ui::Box::new().width(0.0).height(16.0).child(tick).build())
            .build();
        ticks = ticks.child(anchor);
    }
    let tick_row = ui::Box::new()
        .layout("row")
        .width(Length::Pct(100.0))
        .child(
            ui::Box::new()
                .width(48.0)
                .style(Style::new().flex_shrink(0.0))
                .build(),
        )
        .child(
            ui::Box::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(ticks.build())
                .build(),
        )
        .child(
            ui::Box::new()
                .width(12.0)
                .style(Style::new().flex_shrink(0.0))
                .build(),
        )
        .build();
    ui::Box::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .gap(4.0)
        .child(title(y_title))
        .child(plot_row)
        .child(tick_row)
        .child(
            ui::Box::new()
                .width(Length::Pct(100.0))
                .style(Style::new().justify(aqua_ui::JustifyContent::END))
                .child(title(x_title))
                .build(),
        )
        .build()
}

pub(crate) fn plot_label(
    theme: &ArtisanTheme,
    x: f32,
    y: f32,
    label: &str,
    color: GpuiHsla,
) -> Element {
    plot_label_positioned(theme, x, y, label, color, true)
}

pub(crate) fn plot_label_positioned(
    theme: &ArtisanTheme,
    x: f32,
    y: f32,
    label: &str,
    color: GpuiHsla,
    below: bool,
) -> Element {
    let style = aqua_text::TextStyle {
        families: vec![aqua_text::FontFamily::Named(
            theme.typography.sans.family.to_owned(),
        )],
        size_px: 11.0,
        line_height_px: 16.0,
        weight: FontWeight::NORMAL,
        ..aqua_text::TextStyle::default()
    };
    let label = ui::Box::new()
        .radius(RadiusTokens::value(RadiusStep::Xs))
        .style(
            Style::new()
                .max_width(Length::Px(220.0))
                .clip(true)
                .padding_insets(Insets {
                    left: 4.0,
                    right: 4.0,
                    ..Insets::default()
                })
                .gpui_background(aqua_core::GpuiBackground::Solid(
                    theme
                        .colors
                        .background
                        .at(Opacity::LabelBacking)
                        .to_reference_paint(),
                )),
        )
        .child(
            ui::Text::new(label.to_owned())
                .text_style(style)
                .gpui_color(color)
                .wrap(false)
                .build(),
        )
        .build();
    let x = x.clamp(0.0, 1.0);
    let mut row = ui::Box::new()
        .layout("row")
        .width(Length::Pct(100.0))
        .style(Style::new().absolute_at(
            Some(0.0),
            Some(y.clamp(0.0, 1.0) * 240.0 + if below { 3.0 } else { -18.0 }),
            None,
            None,
        ));
    if x > 0.7 {
        row = row.child(
            ui::Box::new()
                .layout("row")
                .width(Length::Pct(x * 100.0))
                .style(
                    Style::new()
                        .justify(aqua_ui::JustifyContent::END)
                        .padding_insets(Insets {
                            right: 2.0,
                            ..Insets::default()
                        }),
                )
                .child(label)
                .build(),
        );
    } else {
        row = row
            .child(
                ui::Box::new()
                    .width(Length::Pct(x * 100.0))
                    .style(Style::new().flex_shrink(0.0))
                    .build(),
            )
            .child(
                ui::Box::new()
                    .style(Style::new().padding_insets(Insets {
                        left: 2.0,
                        ..Insets::default()
                    }))
                    .child(label)
                    .build(),
            );
    }
    row.build()
}

pub(crate) fn readout(theme: &ArtisanTheme, x: f32, heading: &str, value: &str) -> Element {
    readout_row(theme, x, heading, "Value", value, None)
}

pub(crate) fn readout_row(
    theme: &ArtisanTheme,
    x: f32,
    heading: &str,
    label: &str,
    value: &str,
    swatch: Option<GpuiHsla>,
) -> Element {
    readout_rows(
        theme,
        x,
        heading,
        vec![super::distribution::ReadoutRow {
            color: swatch,
            label: label.to_owned(),
            value: value.to_owned(),
        }],
    )
}

pub(crate) fn readout_rows(
    theme: &ArtisanTheme,
    x: f32,
    heading: &str,
    rows: Vec<super::distribution::ReadoutRow>,
) -> Element {
    readout_rows_at(theme, x, heading, rows, false)
}

pub(crate) fn readout_rows_at(
    theme: &ArtisanTheme,
    x: f32,
    heading: &str,
    rows: Vec<super::distribution::ReadoutRow>,
    low: bool,
) -> Element {
    let mut rendered_rows = Vec::with_capacity(rows.len());
    for item in rows {
        let swatch = item.color;
        let label = &item.label;
        let value = &item.value;
        let mut row = ui::Box::new()
            .layout("row")
            .gap(8.0)
            .style(Style::new().align(aqua_ui::AlignItems::CENTER));
        if let Some(color) = swatch {
            row = row.child(
                ui::Box::new()
                    .width(8.0)
                    .height(8.0)
                    .radius(RadiusTokens::value(RadiusStep::Xs))
                    .style(
                        Style::new()
                            .flex_shrink(0.0)
                            .gpui_background(aqua_core::GpuiBackground::Solid(color)),
                    )
                    .build(),
            );
        }
        let row = row
            .child(
                ui::Box::new()
                    .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                    .child(text_with_wrap(
                        theme,
                        label,
                        theme.typography.label_text,
                        19.0,
                        FontWeight::NORMAL,
                        theme.colors.text_muted,
                        false,
                    ))
                    .build(),
            )
            .child(text_with_wrap(
                theme,
                value,
                theme.typography.label_text,
                19.0,
                FontWeight::NORMAL,
                theme.colors.text,
                false,
            ))
            .build();
        rendered_rows.push(row);
    }
    let mut content = ui::Box::new()
        .layout("column")
        .gap(4.0)
        .style(Style::new().padding_insets(Insets {
            left: 14.0,
            right: 14.0,
            top: 12.0,
            bottom: 12.0,
        }));
    if !heading.is_empty() {
        content = content.child(text_with_wrap(
            theme,
            heading,
            theme.typography.control_text,
            18.0,
            FontWeight(600),
            theme.colors.text_muted,
            false,
        ));
    }
    for row in rendered_rows {
        content = content.child(row);
    }
    let content = content.build();
    let card = Surface::Tooltip(Tooltip::Card)
        .build(theme, vec![content])
        .style(
            Style::new()
                .min_width(Length::Px(140.0))
                .max_width(Length::Px(280.0))
                .flex_shrink(0.0),
        );
    let x = x.clamp(0.0, 1.0);
    let mut wrapper = ui::Box::new().layout("row").style(
        Style::new()
            .absolute_at(
                Some(0.0),
                if low { None } else { Some(6.0) },
                Some(0.0),
                if low { Some(6.0) } else { None },
            )
            .width(Length::Pct(100.0)),
    );
    if x > 0.55 {
        wrapper = wrapper.child(
            ui::Box::new()
                .layout("row")
                .style(
                    Style::new()
                        .width(Length::Pct(x * 100.0))
                        .justify(aqua_ui::JustifyContent::END)
                        .padding_insets(Insets {
                            right: 10.0,
                            ..Insets::default()
                        }),
                )
                .child(card)
                .build(),
        );
    } else {
        wrapper = wrapper
            .child(
                ui::Box::new()
                    .width(Length::Pct(x * 100.0))
                    .style(Style::new().flex_shrink(0.0))
                    .build(),
            )
            .child(
                ui::Box::new()
                    .style(Style::new().padding_insets(Insets {
                        left: 10.0,
                        ..Insets::default()
                    }))
                    .child(card)
                    .build(),
            );
    }
    wrapper.build()
}

pub(crate) fn tick_strip(theme: &ArtisanTheme, ticks: Vec<super::distribution::Tick>) -> Element {
    let mut strip = ui::Box::new().width(Length::Pct(100.0)).height(16.0);
    for tick in ticks {
        let label = ui::Box::new()
            .width(88.0)
            .height(16.0)
            .style(
                Style::new()
                    .absolute_at(Some(-44.0), Some(0.0), None, None)
                    .justify(aqua_ui::JustifyContent::CENTER)
                    .clip(true),
            )
            .child(text_with_wrap(
                theme,
                &tick.label,
                11.0,
                16.0,
                FontWeight::NORMAL,
                theme.colors.text_muted,
                false,
            ))
            .build();
        let anchor = ui::Box::new()
            .layout("row")
            .width(Length::Pct(100.0))
            .height(16.0)
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, None))
            .child(
                ui::Box::new()
                    .width(Length::Pct(tick.at * 100.0))
                    .style(Style::new().flex_shrink(0.0))
                    .build(),
            )
            .child(ui::Box::new().width(0.0).height(16.0).child(label).build())
            .build();
        strip = strip.child(anchor);
    }
    strip.build()
}

pub(crate) fn numeric_frame(
    theme: &ArtisanTheme,
    plot: Element,
    frame: super::distribution::PlotFrame,
) -> Element {
    numeric_frame_with_labels(theme, plot, frame, Vec::new())
}

pub(crate) fn numeric_frame_with_labels(
    theme: &ArtisanTheme,
    plot: Element,
    frame: super::distribution::PlotFrame,
    labels: Vec<(f32, String, GpuiHsla)>,
) -> Element {
    let gutter_width = if labels.is_empty() { 12.0 } else { 104.0 };
    let mut gutter = ui::Box::new()
        .width(gutter_width)
        .height(frame.height)
        .style(Style::new().flex_shrink(0.0));
    for (y, value, color) in labels {
        let style = aqua_text::TextStyle {
            families: vec![aqua_text::FontFamily::Named(
                theme.typography.sans.family.to_owned(),
            )],
            size_px: 11.0,
            line_height_px: 16.0,
            weight: FontWeight::NORMAL,
            ..aqua_text::TextStyle::default()
        };
        gutter = gutter.child(
            ui::Box::new()
                .height(16.0)
                .style(
                    Style::new()
                        .absolute_at(Some(8.0), Some(y * frame.height - 8.0), Some(0.0), None)
                        .clip(true),
                )
                .child(
                    ui::Text::new(value)
                        .text_style(style)
                        .gpui_color(color)
                        .wrap(false)
                        .build(),
                )
                .build(),
        );
    }
    let mut y_axis = ui::Box::new()
        .width(frame.y_axis_width)
        .height(frame.height)
        .style(Style::new().flex_shrink(0.0));
    for tick in frame.y_ticks {
        y_axis = y_axis.child(
            ui::Box::new()
                .height(16.0)
                .style(
                    Style::new()
                        .absolute_at(
                            Some(0.0),
                            Some(tick.at * frame.height - 8.0),
                            Some(8.0),
                            None,
                        )
                        .justify(aqua_ui::JustifyContent::END)
                        .clip(true),
                )
                .child(text_with_wrap(
                    theme,
                    &tick.label,
                    11.0,
                    16.0,
                    FontWeight::NORMAL,
                    theme.colors.text_muted,
                    false,
                ))
                .build(),
        );
    }
    let title = |value: &str| {
        text_with_wrap(
            theme,
            value,
            11.0,
            16.0,
            FontWeight::MEDIUM,
            theme.colors.text_muted,
            false,
        )
    };
    let mut outer = ui::Box::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .gap(4.0);
    if let Some(value) = frame.y_title {
        outer = outer.child(title(&value));
    }
    outer = outer
        .child(
            ui::Box::new()
                .layout("row")
                .width(Length::Pct(100.0))
                .height(frame.height)
                .child(y_axis.build())
                .child(
                    ui::Box::new()
                        .style(
                            Style::new()
                                .flex_grow(1.0)
                                .min_width(Length::Px(0.0))
                                .height(Length::Pct(100.0)),
                        )
                        .child(plot)
                        .build(),
                )
                .child(gutter.build())
                .build(),
        )
        .child(
            ui::Box::new()
                .layout("row")
                .width(Length::Pct(100.0))
                .child(
                    ui::Box::new()
                        .width(frame.y_axis_width)
                        .style(Style::new().flex_shrink(0.0))
                        .build(),
                )
                .child(
                    ui::Box::new()
                        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                        .child(tick_strip(theme, frame.x_ticks))
                        .build(),
                )
                .child(
                    ui::Box::new()
                        .width(gutter_width)
                        .style(Style::new().flex_shrink(0.0))
                        .build(),
                )
                .build(),
        );
    if let Some(value) = frame.x_title {
        outer = outer.child(
            ui::Box::new()
                .width(Length::Pct(100.0))
                .style(Style::new().justify(aqua_ui::JustifyContent::END))
                .child(title(&value))
                .build(),
        );
    }
    outer.build()
}

pub(crate) fn legend(theme: &ArtisanTheme, entries: Vec<(String, GpuiHsla)>) -> Element {
    let mut legend = ui::Box::new()
        .layout("row")
        .gap(12.0)
        .style(Style::new().wrap(true).align(aqua_ui::AlignItems::CENTER));
    for (name, color) in entries {
        let swatch = ui::Box::new()
            .width(12.0)
            .height(4.0)
            .radius(RadiusTokens::value(RadiusStep::X2s))
            .style(
                Style::new()
                    .flex_shrink(0.0)
                    .gpui_background(aqua_core::GpuiBackground::Solid(color)),
            )
            .build();
        legend = legend.child(
            ui::Box::new()
                .layout("row")
                .gap(6.0)
                .style(Style::new().align(aqua_ui::AlignItems::CENTER))
                .child(swatch)
                .child(text_with_wrap(
                    theme,
                    &name,
                    12.0,
                    16.0,
                    FontWeight::NORMAL,
                    theme.colors.text,
                    false,
                ))
                .build(),
        );
    }
    legend.build()
}

pub(crate) fn stats_hint(theme: &ArtisanTheme, heading: &str, body: &str, left: f32) -> Element {
    let content = ui::Box::new()
        .layout("column")
        .gap(Space::S4.px())
        .child(text(
            theme,
            heading,
            theme.typography.control_text,
            18.0,
            FontWeight(600),
            theme.colors.text,
        ))
        .child(text(
            theme,
            body,
            theme.typography.label_text,
            19.0,
            FontWeight::NORMAL,
            theme.colors.text_muted,
        ))
        .build();
    let card = Surface::Tooltip(Tooltip::Card)
        .build(theme, vec![content])
        .style(Style::new().width(Length::Px(260.0)));
    ui::Box::new()
        .width(260.0)
        .height(0.0)
        .style(Style::new().absolute_at(Some(left), None, None, Some(0.0)))
        .child(
            ui::Box::new()
                .width(260.0)
                .style(Style::new().absolute_at(Some(0.0), Some(8.0), None, None))
                .child(card)
                .build(),
        )
        .build()
}
