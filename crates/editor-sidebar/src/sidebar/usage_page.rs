use super::{
    usage::{METER_SEGMENTS, checked_label, groups, lit_of, remaining_percent, reset_duration},
    usage_controller::UsagePage,
};
use editor_core::{Core, HostId, HostInstance};
use editor_ui::{
    engine::{engine_accent, engine_icon},
    kit::{Button, ButtonTone, IconSize, Space, Spaced, TextRole, surface::band::Band},
    page,
    theme::{ArtisanTheme, Opacity},
};
use aqua_core::Size;
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Scroll},
    style::{Length, Style},
};
use artisan_api::{
    EngineId, EngineStatus, SignInState, SpendLine, Timestamp, UsageLimits, UsageSpend,
};
use std::rc::Rc;

const METER_WIDTH: f32 = 144.0;
const METER_HEIGHT: f32 = 8.0;
const TRAILING_WIDTH: f32 = 56.0;

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

fn text(theme: ArtisanTheme, value: impl Into<String>, color: aqua::GpuiTextColor) -> Element {
    TextRole::Caption
        .text(&theme, value.into())
        .gpui_source_color(color)
        .build()
}

fn truncated(
    theme: ArtisanTheme,
    role: TextRole,
    value: impl Into<String>,
    color: aqua::GpuiTextColor,
) -> Element {
    role.text(&theme, value.into())
        .gpui_source_color(color)
        .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
        .style(Style::new().min_width(Length::Pct(0.0)).flex_shrink(1.0))
        .build()
}

#[expect(
    clippy::disallowed_methods,
    reason = "meter segments are painted data marks, not chrome"
)]
fn segment(paint: aqua::GpuiHsla) -> Element {
    UiBox::new()
        .height(METER_HEIGHT)
        .style(
            Style::new()
                .flex_grow(1.0)
                .min_width(Length::Pct(0.0))
                .gpui_background(aqua_core::GpuiBackground::Solid(paint)),
        )
        .build()
}

fn meter_bar(theme: ArtisanTheme, engine: EngineId, used_percent: f64) -> Element {
    let lit = lit_of(used_percent, METER_SEGMENTS);
    let accent = engine_accent(&theme, engine);
    let track = theme
        .colors
        .text
        .at(Opacity::MeterTrack)
        .to_reference_paint();
    UiBox::new()
        .width(METER_WIDTH)
        .height(METER_HEIGHT)
        .spaced(Space::S2)
        .style(Style::new().flex_shrink(0.0))
        .children((0..METER_SEGMENTS).map(|index| segment(if index < lit { accent } else { track })))
        .build()
}

fn trailing(content: Element) -> Element {
    UiBox::new()
        .width(TRAILING_WIDTH)
        .style(Style::new().justify(JustifyContent::END).flex_shrink(0.0))
        .child(content)
        .build()
}

fn line(label: Element, rest: Vec<Element>) -> Element {
    UiBox::new()
        .spaced(Space::S16)
        .style(Style::new().align(AlignItems::CENTER))
        .child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(label)
                .build(),
        )
        .children(rest)
        .build()
}

fn group(theme: ArtisanTheme, title: impl Into<String>) -> UiBox {
    UiBox::new()
        .layout("column")
        .spaced(Space::S6)
        .child(text(theme, title, theme.colors.text.to_reference_text_color()))
}

fn meters(
    theme: ArtisanTheme,
    engine: EngineId,
    usage: &UsageLimits,
    now: Timestamp,
) -> Vec<Element> {
    let muted = theme.colors.text_muted.to_reference_text_color();
    groups(usage)
        .into_iter()
        .map(|(cadence, meters)| {
            let mut column = group(theme, cadence.title());
            for meter in &meters {
                column = column.child(line(
                    truncated(
                        theme,
                        TextRole::Caption,
                        meter.scope.as_deref().unwrap_or("All models"),
                        muted,
                    ),
                    vec![
                        meter_bar(theme, engine, meter.used_percent),
                        trailing(text(
                            theme,
                            format!("{}% left", remaining_percent(meter.used_percent)),
                            muted,
                        )),
                    ],
                ));
            }
            if let Some(duration) = reset_duration(&meters, now) {
                column = column.child(text(
                    theme,
                    format!(
                        "Your {} limit resets in {duration}.",
                        cadence.title().to_lowercase()
                    ),
                    muted,
                ));
            }
            column.build()
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
    let muted = theme.colors.text_muted.to_reference_text_color();
    let mut column = group(theme, title);
    for spend in lines {
        column = column.child(line(
            truncated(theme, TextRole::Caption, spend.label.clone(), muted),
            vec![
                text(theme, token_count(spend.tokens), muted),
                trailing(text(
                    theme,
                    format!("${:.2}", spend.cost_usd),
                    theme.colors.text.to_reference_text_color(),
                )),
            ],
        ));
    }
    if explanation {
        column = column.child(text(
            theme,
            "Priced by the engine for runs on this host.",
            muted,
        ));
    }
    column.build()
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
        .spaced(Space::S8)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Pct(0.0)),
        )
        .child(IconSize::Md.glyph(
            engine_icon(&theme, engine),
            theme.colors.text.to_reference_paint(),
        ))
        .child(
            UiBox::new()
                .style(Style::new().flex_shrink(0.0))
                .child(truncated(
                    theme,
                    TextRole::LabelStrong,
                    engine.display_name(),
                    theme.colors.text.to_reference_text_color(),
                ))
                .build(),
        );
    if let Some(account) = account {
        title = title.child(truncated(
            theme,
            TextRole::Label,
            account,
            theme.colors.text_muted.to_reference_text_color(),
        ));
    }
    let mut body = UiBox::new()
        .layout("column")
        .spaced(Space::S12)
        .width(Length::Pct(100.0))
        .child(
            UiBox::new()
                .spaced(Space::S16)
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .justify(JustifyContent::SPACE_BETWEEN),
                )
                .child(title.build())
                .child(text(
                    theme,
                    reading,
                    theme.colors.text_muted.to_reference_text_color(),
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

fn note(theme: ArtisanTheme, words: &'static str) -> Element {
    TextRole::Label
        .text(&theme, words)
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .build()
}

fn bands(theme: ArtisanTheme, blocks: Vec<Element>) -> Element {
    let last = blocks.len().saturating_sub(1);
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .children(
            blocks
                .into_iter()
                .enumerate()
                .map(|(index, block)| Band::of(index == 0, index == last).build(&theme, false, block)),
        )
        .build()
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
        .padded(Space::S24)
        .style(Style::new().max_width(Length::Px(page::CONTENT_MAX_WIDTH)))
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
                    ButtonTone::Neutral,
                    if refreshing {
                        "Refreshing…"
                    } else {
                        "Refresh"
                    },
                )
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
        let blocks = if accounts.is_empty() {
            vec![note(theme, "No engine is signed in on this host.")]
        } else {
            accounts
                .into_iter()
                .map(|status| band(theme, status, now))
                .collect()
        };
        content = content.child(
            page::section_shell(&theme, &host.name, host.status_line(), refresh, bands(theme, blocks))
                .build()
                .with_key(format!("usage-host-{}", host.id.0)),
        );
        if let Some(error) = controller.error(&host.id, host.instance()) {
            content = content.child(page::error_text(&theme, error).build());
        }
    }
    if count == 0 {
        content = content.child(bands(
            theme,
            vec![note(theme, "Pair a host to see its engines' usage.")],
        ));
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
