//! The Usage page: every paired host's signed-in engine accounts, each with
//! its limits metered the way the profile menu meters them. Opening the page
//! asks every account for a fresh reading.

use crate::usage_visual::{
    METER_SEGMENTS, checked_label, engine_accent, engine_name, groups, lit_ticks,
    remaining_percent, reset_duration,
};
use artisan_api::{
    EngineId, EngineStatus, SignInState, SpendLine, Timestamp, UsageLimits, UsageSpend,
};
use editor_core::{Core, CoreEvent, HostData, HostEvent, HostId, Screen};
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::icon::{IconSize, IconStyle, IconTint, icon};
use editor_ui::page;
use editor_ui::theme::{ActiveTheme as _, ArtisanTheme, Opacity, TextStep, TypographyTokens};
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    Window, div, px,
};
use std::collections::HashMap;
use std::time::Duration;

pub const USAGE_SCREEN_SELECTOR: &str = "route-usage";
/// How often "last checked" and the reset sentences move on.
const CLOCK: Duration = Duration::from_secs(60);
/// The menu's fourteen ticks at twice its width.
const METER_WIDTH: f32 = 144.0;
#[expect(
    clippy::cast_precision_loss,
    reason = "fourteen segments are exact in f32; From is not const"
)]
const METER_TICK: f32 = METER_WIDTH / METER_SEGMENTS as f32 - 2.0;

pub struct UsagePage {
    core: Entity<Core>,
    shown: bool,
    refreshing: HashMap<(HostId, EngineId), Task<()>>,
    errors: HashMap<HostId, String>,
    host_events: Vec<Subscription>,
    _core: Subscription,
    _clock: Task<()>,
}

impl UsagePage {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&core, |this, core, event: &CoreEvent, cx| {
            let shown = matches!(core.read(cx).screen(), Screen::Usage);
            if matches!(event, CoreEvent::HostsChanged) {
                this.follow_hosts(cx);
            }
            if shown && !this.shown {
                this.refresh_all(cx);
            }
            this.shown = shown;
            cx.notify();
        });
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CLOCK).await;
                let Ok(()) = this.update(cx, |this, cx| {
                    if this.shown {
                        cx.notify();
                    }
                }) else {
                    break;
                };
            }
        });
        let shown = matches!(core.read(cx).screen(), Screen::Usage);
        let mut page = Self {
            core,
            shown,
            refreshing: HashMap::new(),
            errors: HashMap::new(),
            host_events: Vec::new(),
            _core: subscription,
            _clock: clock,
        };
        page.follow_hosts(cx);
        if shown {
            page.refresh_all(cx);
        }
        page
    }

    fn follow_hosts(&mut self, cx: &mut Context<Self>) {
        let hosts: Vec<_> = self
            .core
            .read(cx)
            .hosts()
            .map(|(_, host)| host.clone())
            .collect();
        self.host_events = hosts
            .iter()
            .map(|host| {
                cx.subscribe(host, |this, _, event: &HostEvent, cx| {
                    if this.shown && matches!(event, HostEvent::Changed | HostEvent::Reset) {
                        cx.notify();
                    }
                })
            })
            .collect();
    }

    fn refresh_all(&mut self, cx: &mut Context<Self>) {
        let hosts: Vec<_> = self
            .core
            .read(cx)
            .hosts()
            .map(|(_, host)| host.clone())
            .collect();
        for host in hosts {
            self.refresh_host(&host, cx);
        }
    }

    fn refresh_host(&mut self, host: &Entity<HostData>, cx: &mut Context<Self>) {
        let (id, client, engines) = {
            let host = host.read(cx);
            let engines: Vec<_> = accounts(host).map(|status| status.engine).collect();
            (host.id.clone(), host.client.clone(), engines)
        };
        self.errors.remove(&id);
        for engine in engines {
            let key = (id.clone(), engine);
            if self.refreshing.contains_key(&key) {
                continue;
            }
            let client = client.clone();
            let host = id.clone();
            let task = cx.spawn(async move |this, cx| {
                let result = client
                    .engine_action(engine, &artisan_api::EngineCommand::Refresh {})
                    .await;
                this.update(cx, |this, cx| {
                    this.refreshing.remove(&(host.clone(), engine));
                    if let Err(error) = result {
                        this.errors.insert(
                            host,
                            format!("Couldn't refresh {}: {error:#}", engine.display_name()),
                        );
                    }
                    cx.notify();
                })
                .ok();
            });
            self.refreshing.insert(key, task);
        }
        cx.notify();
    }

    fn host_section(
        &self,
        theme: &ArtisanTheme,
        host: &Entity<HostData>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let now = artisan_api::now();
        let (id, name, status, bands) = {
            let data = host.read(cx);
            let bands: Vec<AnyElement> = accounts(data)
                .map(|status| engine_band(theme, status, now).into_any_element())
                .collect();
            (
                data.id.clone(),
                data.name.clone(),
                data.status_line(),
                bands,
            )
        };
        let refreshing = self.refreshing.keys().any(|(host, _)| host == &id);
        let empty = bands.is_empty();
        let body = if empty {
            page::card(
                theme,
                vec![page::note(theme, "No engine is signed in on this host.").into_any_element()],
            )
        } else {
            page::card(theme, bands)
        };
        let selector = format!("usage-host-{}", id.0);
        let entity = host.clone();
        let refresh = (!empty).then(|| {
            Button::text(
                gpui::SharedString::from(format!("usage-refresh-{}", id.0)),
                *theme,
                if refreshing {
                    "Refreshing…"
                } else {
                    "Refresh"
                },
            )
            .variant(ButtonVariant::Outline)
            .disabled(refreshing)
            .on_activate(cx.listener(move |this, _, _, cx| this.refresh_host(&entity, cx)))
            .into_any_element()
        });
        div()
            .child(
                page::section_shell(theme, &name, status, refresh, body).debug_selector(selector),
            )
            .children(
                self.errors
                    .get(&id)
                    .map(|error| page::error_text(theme, error.clone())),
            )
            .into_any_element()
    }
}

/// The engines with an account: signed in, or still holding a reading.
fn accounts(host: &HostData) -> impl Iterator<Item = &EngineStatus> {
    host.engines.values().filter(|status| {
        matches!(status.sign_in, SignInState::SignedIn { .. }) || status.usage.is_some()
    })
}

/// One account: the engine's mark, name and account, when it was last
/// read, then a group of meters per cadence with when its limit resets.
fn engine_band(theme: &ArtisanTheme, status: &EngineStatus, now: Timestamp) -> Div {
    let desktop = theme.colors;
    let engine = status.engine;
    let account = match &status.sign_in {
        SignInState::SignedIn {
            account: Some(account),
        } => Some(account.clone()),
        SignInState::Unknown
        | SignInState::SignedOut
        | SignInState::LoggingIn { .. }
        | SignInState::SignedIn { .. }
        | SignInState::Failed { .. } => None,
    };
    let reading = status.usage.as_ref().map_or_else(
        || "No limits reported".to_owned(),
        |usage| {
            let label = checked_label(usage.read_at, now);
            let mut chars = label.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or(label)
        },
    );
    let title = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .child(
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(px(8.0))
                .text_color(desktop.text.to_paint())
                .child(
                    icon(IconStyle::resolve(
                        *theme,
                        editor_ui::engine_icon(theme, engine),
                        IconSize::Default,
                        IconTint::Inherit,
                    ))
                    .size(px(16.0))
                    .flex_shrink_0(),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(engine_name(engine)),
                )
                .children(account.map(|account| {
                    div()
                        .truncate()
                        .text_sm()
                        .text_color(desktop.text_muted.to_paint())
                        .child(account)
                })),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(desktop.text_muted.to_paint())
                .child(reading),
        );
    let selector = format!("usage-engine-{engine}");
    let mut band = div()
        .debug_selector(move || selector)
        .w_full()
        .py(px(14.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(title);
    if let Some(usage) = &status.usage {
        band = band.children(meter_groups(theme, engine, usage, now));
        if let Some(spend) = &usage.spend {
            band = band.children(spend_groups(theme, spend));
        }
    }
    band
}

/// What the engine priced on this host: a line per period, then the
/// longest period by model.
fn spend_groups(theme: &ArtisanTheme, spend: &UsageSpend) -> Vec<Div> {
    let desktop = theme.colors;
    let group = |title: &str, lines: &[SpendLine]| {
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(desktop.text.to_paint())
                    .child(title.to_owned()),
            )
            .children(lines.iter().map(|line| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .text_color(desktop.text_muted.to_paint())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(line.label.clone()),
                    )
                    .child(div().flex_shrink_0().child(token_count(line.tokens)))
                    .child(
                        div()
                            .w(px(56.0))
                            .flex_shrink_0()
                            .flex()
                            .justify_end()
                            .text_color(desktop.text.to_paint())
                            .child(format!("${:.2}", line.cost_usd)),
                    )
            }))
    };
    let mut groups = Vec::new();
    if !spend.periods.is_empty() {
        groups.push(
            group("Spend", &spend.periods).child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .text_color(desktop.text_muted.to_paint())
                    .child("Priced by the engine for runs on this host."),
            ),
        );
    }
    if !spend.models.is_empty() {
        let title = spend.periods.last().map_or_else(
            || "By model".to_owned(),
            |period| format!("By model, {}", period.label.to_lowercase()),
        );
        groups.push(group(&title, &spend.models));
    }
    groups
}

/// `840 tokens`, `12.4K tokens`, `2.3M tokens`.
#[expect(
    clippy::cast_precision_loss,
    reason = "token counts shown to one decimal"
)]
fn token_count(tokens: u64) -> String {
    match tokens {
        0..1_000 => format!("{tokens} tokens"),
        1_000..1_000_000 => format!("{:.1}K tokens", tokens as f64 / 1e3),
        _ => format!("{:.1}M tokens", tokens as f64 / 1e6),
    }
}

fn meter_groups(
    theme: &ArtisanTheme,
    engine: EngineId,
    usage: &UsageLimits,
    now: Timestamp,
) -> Vec<Div> {
    let desktop = theme.colors;
    let accent = engine_accent(engine, theme);
    let dim = desktop.text.at(Opacity::MeterTrack).to_paint();
    groups(usage)
        .into_iter()
        .map(|(cadence, meters)| {
            let mut group = div().flex().flex_col().gap(px(6.0)).child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(desktop.text.to_paint())
                    .child(cadence.title()),
            );
            for meter in &meters {
                let lit = lit_ticks(meter.used_percent);
                let bar = div()
                    .w(px(METER_WIDTH))
                    .h(px(8.0))
                    .flex_shrink_0()
                    .flex()
                    .children((0..METER_SEGMENTS).map(|index| {
                        div()
                            .w(px(METER_TICK))
                            .mr(px(2.0))
                            .h_full()
                            .bg(if index < lit { accent } else { dim })
                    }));
                group = group.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16.0))
                        .text_size(TypographyTokens::size(TextStep::Xs))
                        .text_color(desktop.text_muted.to_paint())
                        .child(
                            div().flex_1().min_w_0().truncate().child(
                                meter
                                    .scope
                                    .clone()
                                    .unwrap_or_else(|| "All models".to_owned()),
                            ),
                        )
                        .child(bar)
                        .child(
                            div()
                                .w(px(56.0))
                                .flex_shrink_0()
                                .flex()
                                .justify_end()
                                .child(format!("{}% left", remaining_percent(meter.used_percent))),
                        ),
                );
            }
            if let Some(duration) = reset_duration(&meters, now) {
                group = group.child(
                    div()
                        .text_size(TypographyTokens::size(TextStep::Xs))
                        .text_color(desktop.text_muted.to_paint())
                        .child(format!(
                            "Your {} limit resets in {duration}.",
                            cadence.title().to_lowercase()
                        )),
                );
            }
            group
        })
        .collect()
}

impl Render for UsagePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let hosts: Vec<_> = self
            .core
            .read(cx)
            .hosts()
            .map(|(_, host)| host.clone())
            .collect();
        let sections: Vec<AnyElement> = if hosts.is_empty() {
            vec![
                div()
                    .mt(px(40.0))
                    .child(page::card(
                        &theme,
                        vec![
                            page::note(&theme, "Pair a host to see its engines' usage.")
                                .into_any_element(),
                        ],
                    ))
                    .into_any_element(),
            ]
        } else {
            hosts
                .iter()
                .map(|host| self.host_section(&theme, host, cx))
                .collect()
        };
        div()
            .id("usage-screen")
            .debug_selector(|| USAGE_SCREEN_SELECTOR.to_owned())
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div().w_full().flex().justify_center().child(
                    div()
                        .w_full()
                        .max_w(px(page::CONTENT_MAX_WIDTH))
                        .px(px(24.0))
                        .pt(px(48.0))
                        .pb(px(48.0))
                        .child(page::header(
                            &theme,
                            "Usage",
                            "The limits and spend of every engine account on your hosts.",
                        ))
                        .children(sections),
                ),
            )
    }
}
