use super::{
    usage::{METER_SEGMENTS, checked_label, groups, lit_of, remaining_percent, reset_duration},
    usage_controller::UsagePage,
    usage_view::{engine_accent, engine_icon},
};
use editor_core::{Core, HostId, HostInstance};
use editor_ui::{
    asset::source_asset_glyph,
    button::{Button, ButtonVariant},
    page,
    theme::{ArtisanTheme, Opacity, TextStep, TypographyTokens},
};
use aqua_core::{Insets, Size};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Scroll, Text},
    style::{Length, Style},
};
use artisan_api::{
    EngineId, EngineStatus, SignInState, SpendLine, Timestamp, UsageLimits, UsageSpend,
};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum Action {
    Refresh {
        host: HostId,
        instance: HostInstance,
    },
}

pub fn dispatch(page: &mut UsagePage, core: &Core, action: Action) -> bool {
    match action {
        Action::Refresh { host, instance } if core.host_if_instance(&host, instance).is_some() => {
            page.refresh_host(core, &host)
        }
        _ => false,
    }
}

fn text(
    theme: ArtisanTheme,
    value: impl Into<String>,
    size: f32,
    color: aqua::GpuiTextColor,
    medium: bool,
) -> Element {
    Text::new(value.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            weight: if medium {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            },
            ..TextStyle::default()
        })
        .gpui_source_color(color)
        .build()
}

fn truncated(
    theme: ArtisanTheme,
    value: impl Into<String>,
    size: f32,
    line_height: f32,
    color: aqua::GpuiTextColor,
    medium: bool,
) -> Element {
    Text::new(value.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            line_height_px: line_height,
            weight: if medium {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            },
            ..TextStyle::default()
        })
        .gpui_source_color(color)
        .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
        .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
        .build()
}

fn meters(
    theme: ArtisanTheme,
    engine: EngineId,
    usage: &UsageLimits,
    now: Timestamp,
) -> Vec<Element> {
    groups(usage)
        .into_iter()
        .map(|(cadence, meters)| {
            let mut group = UiBox::new().layout("column").gap(6.0).child(text(
                theme,
                cadence.title(),
                TypographyTokens::size(TextStep::Xs),
                theme.colors.text.to_reference_text_color(),
                true,
            ));
            for meter in &meters {
                let lit = lit_of(meter.used_percent, METER_SEGMENTS);
                let mut bar = UiBox::new()
                    .width(144.0)
                    .height(8.0)
                    .style(Style::new().flex_shrink(0.0));
                for index in 0..METER_SEGMENTS {
                    bar = bar.child(
                        UiBox::new()
                            .width(144.0 / METER_SEGMENTS as f32 - 2.0)
                            .height(8.0)
                            .style(
                                Style::new()
                                    .gpui_background(aqua_core::GpuiBackground::Solid(
                                        if index < lit {
                                            engine_accent(&theme, engine)
                                        } else {
                                            theme
                                                .colors
                                                .text
                                                .at(Opacity::MeterTrack)
                                                .to_reference_paint()
                                        },
                                    ))
                                    .margin_insets(Insets {
                                        right: 2.0,
                                        ..Insets::default()
                                    }),
                            )
                            .build(),
                    );
                }
                group = group.child(
                    UiBox::new()
                        .gap(16.0)
                        .style(Style::new().align(AlignItems::CENTER))
                        .child(
                            UiBox::new()
                                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                                .child(truncated(
                                    theme,
                                    meter.scope.as_deref().unwrap_or("All models"),
                                    TypographyTokens::size(TextStep::Xs),
                                    16.0,
                                    theme.colors.text_muted.to_reference_text_color(),
                                    false,
                                ))
                                .build(),
                        )
                        .child(bar.build())
                        .child(
                            UiBox::new()
                                .width(56.0)
                                .style(Style::new().justify(JustifyContent::END).flex_shrink(0.0))
                                .child(text(
                                    theme,
                                    format!("{}% left", remaining_percent(meter.used_percent)),
                                    TypographyTokens::size(TextStep::Xs),
                                    theme.colors.text_muted.to_reference_text_color(),
                                    false,
                                ))
                                .build(),
                        )
                        .build(),
                );
            }
            if let Some(duration) = reset_duration(&meters, now) {
                group = group.child(text(
                    theme,
                    format!(
                        "Your {} limit resets in {duration}.",
                        cadence.title().to_lowercase()
                    ),
                    TypographyTokens::size(TextStep::Xs),
                    theme.colors.text_muted.to_reference_text_color(),
                    false,
                ));
            }
            group.build()
        })
        .collect()
}

fn token_count(tokens: u64) -> String {
    match tokens {
        0..1_000 => format!("{tokens} tokens"),
        1_000..1_000_000 => format!("{:.1}K tokens", tokens as f64 / 1e3),
        _ => format!("{:.1}M tokens", tokens as f64 / 1e6),
    }
}

fn spend_group(
    theme: ArtisanTheme,
    title: String,
    lines: &[SpendLine],
    explanation: bool,
) -> Element {
    let mut group = UiBox::new().layout("column").gap(6.0).child(text(
        theme,
        title,
        TypographyTokens::size(TextStep::Xs),
        theme.colors.text.to_reference_text_color(),
        true,
    ));
    for line in lines {
        group = group.child(
            UiBox::new()
                .gap(16.0)
                .style(Style::new().align(AlignItems::CENTER))
                .child(
                    UiBox::new()
                        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                        .child(truncated(
                            theme,
                            line.label.clone(),
                            TypographyTokens::size(TextStep::Xs),
                            16.0,
                            theme.colors.text_muted.to_reference_text_color(),
                            false,
                        ))
                        .build(),
                )
                .child(text(
                    theme,
                    token_count(line.tokens),
                    TypographyTokens::size(TextStep::Xs),
                    theme.colors.text_muted.to_reference_text_color(),
                    false,
                ))
                .child(
                    UiBox::new()
                        .width(56.0)
                        .style(Style::new().justify(JustifyContent::END).flex_shrink(0.0))
                        .child(text(
                            theme,
                            format!("${:.2}", line.cost_usd),
                            TypographyTokens::size(TextStep::Xs),
                            theme.colors.text.to_reference_text_color(),
                            false,
                        ))
                        .build(),
                )
                .build(),
        );
    }
    if explanation {
        group = group.child(text(
            theme,
            "Priced by the engine for runs on this host.",
            TypographyTokens::size(TextStep::Xs),
            theme.colors.text_muted.to_reference_text_color(),
            false,
        ));
    }
    group.build()
}

fn spend(theme: ArtisanTheme, value: &UsageSpend) -> Vec<Element> {
    let mut result = Vec::new();
    if !value.periods.is_empty() {
        result.push(spend_group(theme, "Spend".into(), &value.periods, true));
    }
    if !value.models.is_empty() {
        result.push(spend_group(
            theme,
            value.periods.last().map_or_else(
                || "By model".into(),
                |period| format!("By model, {}", period.label.to_lowercase()),
            ),
            &value.models,
            false,
        ));
    }
    result
}

fn band(theme: ArtisanTheme, status: &EngineStatus, now: Timestamp) -> Element {
    let engine = status.engine;
    let account = match &status.sign_in {
        SignInState::SignedIn {
            account: Some(account),
        } => Some(account.clone()),
        _ => None,
    };
    let reading = status.usage.as_ref().map_or_else(
        || "No limits reported".to_owned(),
        |usage| {
            let value = checked_label(usage.read_at, now);
            let mut chars = value.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or(value)
        },
    );
    let mut title = UiBox::new()
        .gap(8.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Px(0.0)),
        )
        .child(source_asset_glyph(
            engine_icon(&theme, engine),
            16.0,
            theme.colors.text.to_reference_paint(),
        ))
        .child(
            UiBox::new()
                .style(Style::new().flex_shrink(0.0))
                .child(truncated(
                    theme,
                    engine.display_name(),
                    14.0,
                    20.0,
                    theme.colors.text.to_reference_text_color(),
                    true,
                ))
                .build(),
        );
    if let Some(account) = account {
        title = title.child(truncated(
            theme,
            account,
            14.0,
            20.0,
            theme.colors.text_muted.to_reference_text_color(),
            false,
        ));
    }
    let mut body = UiBox::new()
        .layout("column")
        .gap(12.0)
        .width(Length::Pct(100.0))
        .style(Style::new().padding_insets(Insets {
            top: 14.0,
            bottom: 14.0,
            left: 0.0,
            right: 0.0,
        }))
        .child(
            UiBox::new()
                .gap(16.0)
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .justify(JustifyContent::SPACE_BETWEEN),
                )
                .child(title.build())
                .child(text(
                    theme,
                    reading,
                    TypographyTokens::size(TextStep::Xs),
                    theme.colors.text_muted.to_reference_text_color(),
                    false,
                ))
                .build(),
        );
    if let Some(usage) = &status.usage {
        body = body.children(meters(theme, engine, usage, now));
        if let Some(value) = &usage.spend {
            body = body.children(spend(theme, value));
        }
    }
    body.build().with_key(format!("usage-engine-{engine}"))
}

pub fn build(
    controller: &UsagePage,
    core: &Core,
    theme: ArtisanTheme,
    now: Timestamp,
    _viewport: Size,
    emit: Rc<dyn Fn(Action)>,
) -> Element {
    let mut content = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .max_width(Length::Px(page::CONTENT_MAX_WIDTH))
                .padding_insets(Insets {
                    left: 24.0,
                    right: 24.0,
                    top: 48.0,
                    bottom: 48.0,
                }),
        )
        .child(
            page::header(
                &theme,
                "Usage",
                "The limits and spend of every engine account on your hosts.",
            )
            .build(),
        );
    let mut count = 0;
    for (_, host) in core.hosts() {
        count += 1;
        let accounts: Vec<_> = host
            .engines
            .values()
            .filter(|status| {
                matches!(status.sign_in, SignInState::SignedIn { .. }) || status.usage.is_some()
            })
            .collect();
        let refreshing = controller.refreshing(&host.id, host.instance());
        let refresh = if accounts.is_empty() {
            None
        } else {
            let callback = emit.clone();
            let id = host.id.clone();
            let instance = host.instance();
            Some(
                Button::text(
                    format!("usage-refresh-{}", host.id.0),
                    theme,
                    if refreshing {
                        "Refreshing…"
                    } else {
                        "Refresh"
                    },
                )
                .variant(ButtonVariant::Outline)
                .disabled(refreshing)
                .on_activate(move |_| {
                    callback(Action::Refresh {
                        host: id.clone(),
                        instance,
                    })
                })
                .build(),
            )
        };
        let bands = if accounts.is_empty() {
            vec![page::note(&theme, "No engine is signed in on this host.").build()]
        } else {
            accounts
                .into_iter()
                .map(|status| band(theme, status, now))
                .collect()
        };
        content = content.child(
            page::section_shell(
                &theme,
                &host.name,
                host.status_line(),
                refresh,
                page::card(&theme, bands).build(),
            )
            .build()
            .with_key(format!("usage-host-{}", host.id.0)),
        );
        if let Some(error) = controller.error(&host.id, host.instance()) {
            content = content.child(page::error_text(&theme, error).build());
        }
    }
    if count == 0 {
        content = content.child(
            UiBox::new()
                .style(Style::new().margin_insets(Insets {
                    top: 40.0,
                    ..Insets::default()
                }))
                .child(
                    page::card(
                        &theme,
                        vec![page::note(&theme, "Pair a host to see its engines' usage.").build()],
                    )
                    .build(),
                )
                .build(),
        );
    }
    Scroll::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(Style::new().justify(JustifyContent::CENTER))
                .child(content.build())
                .build(),
        )
        .build()
        .with_key("usage-screen")
}
