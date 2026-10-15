//! Account controls own their request state and one minute clock, only while open.
use crate::{avatar, usage_visual};
use artisan_api::{EngineId, UsageLimits};
#[cfg(test)]
use artisan_api::{Timestamp, UsageCadence, UsageWindow};
use editor_core::{ConnectionStatus, Core, HostData, HostEvent, Screen, SettingsPage};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::measure::Measured;
use editor_ui::picker_motion::HoverSurface;
use editor_ui::theme::{ActiveTheme as _, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Bounds, Context, Entity, FontWeight, Image, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, anchored, deferred, div, point, px,
};
use std::{cell::Cell, rc::Rc, sync::Arc};
use std::{collections::HashMap, time::Duration};
pub(crate) struct Account {
    core: Entity<Core>,
    host: Option<Entity<HostData>>,
    pub(crate) refreshing: HashMap<EngineId, Task<()>>,
    pub(crate) ui: usage_visual::UsageUi,
    hover: HoverSurface,
    machines_hover: HoverSurface,
    machines_open: bool,
    host_origin: Rc<Cell<Bounds<Pixels>>>,
    submenu_bounds: Measured,
    avatars: HashMap<String, Arc<Image>>,
    close: Option<Rc<dyn Fn(&mut gpui::App)>>,
    usage_height: Pixels,
    error: Option<String>,
    #[cfg(test)]
    refresh_completion_gate: Option<(
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        tokio::sync::oneshot::Receiver<bool>,
    )>,
    _events: Option<Subscription>,
    _clock: Task<()>,
}
impl Account {
    pub(crate) fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        let host = core.read(cx).active_host().cloned();
        let events = host.as_ref().map(|host| {
            cx.subscribe(host, |_, _, event: &HostEvent, cx| {
                if matches!(event, HostEvent::Changed | HostEvent::Reset) {
                    cx.notify();
                }
            })
        });
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            core,
            host,
            refreshing: HashMap::new(),
            ui: usage_visual::UsageUi::default(),
            hover: HoverSurface::default(),
            machines_hover: HoverSurface::default(),
            machines_open: false,
            host_origin: Rc::default(),
            submenu_bounds: Measured::default(),
            avatars: HashMap::new(),
            close: None,
            usage_height: px(0.0),
            error: None,
            #[cfg(test)]
            refresh_completion_gate: None,
            _events: events,
            _clock: clock,
        }
    }
    pub(crate) fn refresh(&mut self, engine: EngineId, cx: &mut Context<Self>) {
        if self.refreshing.contains_key(&engine) {
            return;
        }
        let Some(host) = &self.host else {
            return;
        };
        let client = host.read(cx).client.clone();
        #[cfg(test)]
        let completion_gate = self.refresh_completion_gate.take();
        let task = cx.spawn(async move |this, cx| {
            let result = client
                .engine_action(engine, &artisan_api::EngineCommand::Refresh {})
                .await;
            #[cfg(test)]
            let result = if let Some((arrived, release)) = completion_gate {
                arrived.store(true, std::sync::atomic::Ordering::Release);
                if release.await.unwrap_or(false) {
                    Err(anyhow::anyhow!("injected account refresh failure"))
                } else {
                    result
                }
            } else {
                result
            };
            let _ = this.update(cx, |this, cx| {
                this.refreshing.remove(&engine);
                this.error = result
                    .err()
                    .map(|error| format!("Couldn't refresh {}: {error:#}", engine.display_name()));
                cx.notify();
            });
        });
        self.refreshing.insert(engine, task);
        cx.notify();
    }
}
#[cfg(test)]
fn title(window: &UsageWindow) -> &str {
    match window.cadence {
        UsageCadence::Session => "Session",
        UsageCadence::Week => "Week",
        UsageCadence::Other => &window.label,
    }
}
#[cfg(test)]
fn checked_label(read_at: Timestamp, now: Timestamp) -> String {
    let minutes = now.saturating_sub(read_at).max(0) / 60_000;
    match minutes {
        0 => "Checked now".into(),
        1..=59 => format!("Checked {minutes} min ago"),
        60..=1439 => format!("Checked {} hr ago", minutes / 60),
        _ => format!("Checked {} d ago", minutes / 1440),
    }
}
#[cfg(test)]
fn reset_label(at: Timestamp, now: Timestamp) -> String {
    let remaining = at.saturating_sub(now);
    if remaining <= 0 {
        return "Reset time reached".into();
    }
    let minutes = remaining.saturating_add(59_999) / 60_000;
    if minutes < 60 {
        format!("Resets in {minutes} min")
    } else if minutes < 1440 {
        format!("Resets in {} hr", minutes.saturating_add(59) / 60)
    } else {
        format!("Resets in {} d", minutes.saturating_add(1439) / 1440)
    }
}
impl Account {
    pub(crate) fn with_close(mut self, close: impl Fn(&mut gpui::App) + 'static) -> Self {
        self.close = Some(Rc::new(close));
        self
    }
    pub(crate) fn set_usage_height(&mut self, height: Pixels, cx: &mut Context<Self>) {
        if self.usage_height != height {
            self.usage_height = height;
            cx.notify();
        }
    }
    pub(crate) fn clear_interaction(&mut self, cx: &mut Context<Self>) {
        self.machines_open = false;
        self.hover.clear();
        self.machines_hover.clear();
        self.ui.clear();
        cx.notify();
    }
    pub(crate) fn contains_popup(&self, position: &gpui::Point<Pixels>) -> bool {
        self.machines_open && self.submenu_bounds.contains(position)
    }
    fn action(
        &self,
        id: &'static str,
        asset: AssetId,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = *cx.theme();
        let hover = self.hover.clone();
        div()
            .id(id)
            .role(gpui::Role::MenuItem)
            .aria_label(label)
            .focusable()
            .tab_index(0)
            .debug_selector(move || id.to_owned())
            .relative()
            .px(px(12.0))
            .py(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(RadiusTokens::value(RadiusStep::LgMid))
            .cursor_pointer()
            .child(self.hover.row_probe(id))
            .child(
                asset_glyph(asset)
                    .size(px(15.0))
                    .text_color(theme.colors.workspace_secondary.to_paint()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(TypographyTokens::size(TextStep::Sm))
                    .line_height(px(20.0))
                    .text_color(theme.colors.workspace_foreground.to_paint())
                    .child(label),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.machines_open = false;
                    hover.hovered(id, true);
                    cx.notify();
                }
            }))
    }
    fn host_menu(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let current = self.host.as_ref().map(|host| host.read(cx).id.clone());
        let hosts: Vec<_> = self
            .core
            .read(cx)
            .hosts()
            .map(|(config, host)| (config.clone(), host.read(cx).status.clone()))
            .collect();
        let mut rows = div()
            .relative()
            .flex()
            .flex_col()
            .child(self.machines_hover.surface_probe())
            .child(
                self.machines_hover
                    .pill("machines", &theme, px(12.0), cx.reduce_motion()),
            );
        for (config, status) in hosts {
            let id = config.id.clone();
            let image = self
                .avatars
                .entry(config.fingerprint.clone())
                .or_insert_with(|| avatar::avatar_image(&config.fingerprint))
                .clone();
            let secondary = match status {
                ConnectionStatus::Live => config.address.clone(),
                ConnectionStatus::Connecting => "Connecting…".into(),
                ConnectionStatus::Offline { reason, .. } => format!("Offline: {reason}"),
            };
            let hover = self.machines_hover.clone();
            let hover_id = format!("host-{id}");
            let active = current.as_ref() == Some(&id);
            let core = self.core.clone();
            let close = self.close.clone();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("machine-option-{id}")))
                    .role(gpui::Role::MenuItem)
                    .aria_label(config.name.clone())
                    .aria_selected(active)
                    .focusable()
                    .tab_index(0)
                    .relative()
                    .px(px(8.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(RadiusTokens::value(RadiusStep::LgMid))
                    .cursor_pointer()
                    .child(self.machines_hover.row_probe(hover_id.clone()))
                    .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                        hover.hovered(&hover_id, *hovered);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(32.0))
                            .flex_none()
                            .rounded(RadiusTokens::value(RadiusStep::Md))
                            .overflow_hidden()
                            .child(avatar::avatar(image)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(TypographyTokens::size(TextStep::Sm))
                                    .line_height(px(20.0))
                                    .child(config.name),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(TypographyTokens::size(TextStep::Xs))
                                    .line_height(px(16.0))
                                    .text_color(theme.colors.workspace_secondary.to_paint())
                                    .child(secondary),
                            ),
                    )
                    .when(active, |row| {
                        row.child(asset_glyph(AssetId::TABLER_CHECK).size(px(15.0)))
                    })
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        if let Some(close) = &close {
                            close(cx);
                        }
                        core.update(cx, |core, cx| {
                            core.navigate(
                                Screen::NewThread {
                                    host: id.clone(),
                                    project: None,
                                },
                                cx,
                            )
                        });
                    }),
            );
        }
        let core = self.core.clone();
        let close = self.close.clone();
        rows = rows
            .child(
                div()
                    .h(px(1.0))
                    .my(px(4.0))
                    .bg(theme.colors.foreground.with_alpha(0.2).to_paint()),
            )
            .child(
                self.action(
                    "sidebar-manage-hosts",
                    AssetId::TABLER_SETTINGS,
                    "Manage hosts…",
                    cx,
                )
                .on_click(move |_, _, cx| {
                    if let Some(close) = &close {
                        close(cx);
                    }
                    core.update(cx, |core, cx| {
                        core.navigate(Screen::Settings(SettingsPage::Hosts), cx)
                    })
                }),
            );
        let bounds = self.host_origin.get();
        let width = px(300.0).min((window.viewport_size().width - px(16.0)).max(px(0.0)));
        let right = bounds.right() + px(6.0);
        let left = if right + width <= window.viewport_size().width - px(8.0) {
            right
        } else {
            (bounds.left() - width - px(6.0)).max(px(8.0))
        };
        let submenu_bounds = self.submenu_bounds.clone();
        deferred(
            anchored()
                .position(point(left, bounds.top()))
                .snap_to_window_with_margin(px(8.0))
                .child(editor_ui::hover_card::glass_hover_card(
                    &theme,
                    div()
                        .id("machine-dropdown")
                        .relative()
                        .child(submenu_bounds.record())
                        .debug_selector(|| "machine-dropdown".into())
                        .w(width)
                        .max_h(px(400.0))
                        .overflow_y_scroll()
                        .p(px(4.0))
                        .child(rows),
                )),
        )
        .with_priority(2)
        .into_any_element()
    }
}
impl Render for Account {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let (seed, user, name, status) = self
            .host
            .as_ref()
            .map(|host| {
                let host = host.read(cx);
                let seed = self
                    .core
                    .read(cx)
                    .hosts()
                    .find(|(config, _)| config.id == host.id)
                    .map_or_else(
                        || host.id.0.clone(),
                        |(config, _)| config.fingerprint.clone(),
                    );
                let status = match &host.status {
                    ConnectionStatus::Live => host.name.clone(),
                    ConnectionStatus::Connecting => "Connecting…".into(),
                    ConnectionStatus::Offline { reason, .. } => format!("Offline: {reason}"),
                };
                (
                    seed,
                    host.account
                        .as_ref()
                        .map_or_else(|| host.name.clone(), |account| account.user.clone()),
                    host.name.clone(),
                    status,
                )
            })
            .unwrap_or_else(|| {
                (
                    "no-host".into(),
                    "Artisan".into(),
                    "No host".into(),
                    "No host".into(),
                )
            });
        let image = self
            .avatars
            .entry(seed.clone())
            .or_insert_with(|| avatar::avatar_image(&seed))
            .clone();
        let _ = name;
        let origin = self.host_origin.clone();
        let host_hover = self.hover.clone();
        let host_row = div()
            .id("artisan-desktop-profile-machine")
            .role(gpui::Role::Button)
            .aria_label("Choose Forge")
            .aria_expanded(self.machines_open)
            .focusable()
            .tab_index(0)
            .debug_selector(|| "artisan-desktop-profile-machine".into())
            .relative()
            .p(px(8.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .rounded(RadiusTokens::value(RadiusStep::LgMid))
            .cursor_pointer()
            .child(self.hover.row_probe("profile-host"))
            .child(
                gpui::canvas(|_, _, _| {}, move |bounds, (), _, _| origin.set(bounds))
                    .absolute()
                    .size_full(),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.machines_open = true;
                    host_hover.hovered("profile-host", true);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                this.machines_open = !this.machines_open;
                cx.notify();
            }))
            .child(
                div()
                    .size(px(32.0))
                    .flex_none()
                    .rounded(RadiusTokens::value(RadiusStep::Md))
                    .overflow_hidden()
                    .child(avatar::avatar(image)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(TypographyTokens::size(TextStep::Sm))
                            .line_height(px(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(user),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(TypographyTokens::size(TextStep::Xs))
                            .line_height(px(16.0))
                            .text_color(theme.colors.workspace_secondary.to_paint())
                            .child(status),
                    ),
            )
            .child(
                asset_glyph(AssetId::TABLER_CHEVRON_RIGHT)
                    .size(px(15.0))
                    .text_color(theme.colors.workspace_secondary.to_paint()),
            );
        let reports: Vec<(EngineId, UsageLimits)> = self
            .host
            .as_ref()
            .map(|host| {
                host.read(cx)
                    .engines
                    .values()
                    .filter_map(|status| {
                        status
                            .usage
                            .as_ref()
                            .map(|usage| (status.engine, usage.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let separator = theme
            .colors
            .border
            .with_alpha(theme.colors.border.a * 0.5)
            .to_paint();
        let mut body = div()
            .id("artisan-profile-content")
            .relative()
            .flex()
            .flex_col()
            .child(self.ui.menu_bounds.track())
            .child(self.hover.surface_probe())
            .child(
                self.hover
                    .pill("profile", &theme, px(12.0), cx.reduce_motion()),
            )
            .child(div().p(px(4.0)).child(host_row))
            .child(div().h(px(1.0)).my(px(4.0)).bg(separator));
        if !reports.is_empty() {
            body = body
                .child(
                    div()
                        .id("artisan-profile-usage-scroll")
                        .debug_selector(|| "artisan-profile-usage-scroll".into())
                        .min_h_0()
                        .max_h(self.usage_height)
                        .overflow_y_scroll()
                        .child(usage_visual::section(
                            self,
                            &reports,
                            &theme,
                            &theme.colors,
                            separator,
                            cx,
                        )),
                )
                .child(div().h(px(1.0)).my(px(4.0)).bg(separator));
        }
        let core = self.core.clone();
        let close = self.close.clone();
        let mut actions = div()
            .id("artisan-desktop-profile-actions-hover-surface")
            .debug_selector(|| "artisan-desktop-profile-actions-hover-surface".into())
            .w_full()
            .flex()
            .flex_col()
            .p(px(4.0))
            .child(
                self.action(
                    "artisan-desktop-profile-action-0",
                    AssetId::TABLER_SETTINGS,
                    "Settings",
                    cx,
                )
                .on_click(move |_, _, cx| {
                    if let Some(close) = &close {
                        close(cx);
                    }
                    core.update(cx, |core, cx| {
                        core.navigate(Screen::Settings(SettingsPage::Models), cx)
                    })
                }),
            );
        if !reports.is_empty() {
            let engines: Vec<_> = reports.iter().map(|(engine, _)| *engine).collect();
            actions = actions.child(
                self.action(
                    "artisan-desktop-profile-action-1",
                    AssetId::TABLER_LIST_DETAILS,
                    "Usage",
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    for engine in &engines {
                        this.refresh(*engine, cx);
                    }
                })),
            );
        }
        body = body.child(actions);
        if let Some(tooltip) = usage_visual::tooltip(self, &reports, &theme, &theme.colors) {
            body = body.child(tooltip);
        }
        let submenu = self.machines_open.then(|| self.host_menu(window, cx));
        body.children(self.error.clone().map(|error| {
            div()
                .text_color(theme.colors.destructive.to_paint())
                .child(error)
        }))
        .children(submenu)
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if !*hovered {
                this.hover.clear();
                this.ui.clear();
                cx.notify();
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cadence_is_data_not_label_grammar() {
        let window = UsageWindow {
            label: "Any translated label".into(),
            cadence: UsageCadence::Week,
            scope: Some("model:x".into()),
            used_percent: 42.0,
            resets_at: None,
        };
        assert_eq!(title(&window), "Week");
    }
    #[test]
    fn times_never_overflow() {
        assert_eq!(checked_label(i64::MAX, i64::MIN), "Checked now");
        assert_eq!(reset_label(i64::MIN, i64::MAX), "Reset time reached");
        assert!(reset_label(i64::MAX, i64::MIN).starts_with("Resets in "));
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;
    use crate::tests::Setup;
    use gpui::TestAppContext;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[gpui::test]
    fn switching_hosts_drops_pending_account_refresh_and_preserves_the_new_account(
        cx: &mut TestAppContext,
    ) {
        let setup = Setup::new(cx);
        setup.wait(cx, "paired host", |setup, cx| {
            setup.core.read_with(cx, |core, cx| {
                core.active_host()
                    .is_some_and(|host| host.read(cx).is_live())
            })
        });
        setup
            .window
            .update(cx, |sidebar, _, cx| sidebar.show_profile(cx))
            .unwrap();
        let old_account = setup
            .window
            .read_with(cx, |sidebar, _| sidebar.profile.clone().unwrap())
            .unwrap();
        let old_weak = old_account.downgrade();
        let arrived = Arc::new(AtomicBool::new(false));
        let (release, receive) = tokio::sync::oneshot::channel();
        old_account.update(cx, |account, cx| {
            account.refresh_completion_gate = Some((arrived.clone(), receive));
            account.refresh(EngineId::Claude, cx);
        });
        setup.wait(cx, "account RPC response held", |_, _| {
            arrived.load(Ordering::Acquire)
        });
        let server = setup
            .forge
            .block_on(forge_http::start(forge_http::ServeConfig {
                data_dir: setup._dir.path().join("second-forge"),
                listen: "127.0.0.1:0".parse().unwrap(),
            }))
            .unwrap();
        let address = server.local_addr().unwrap().to_string();
        let store = server.store().clone();
        setup.forge.spawn(server.run());
        let (_, host) = super::super::tests::forge_fixture::pair(
            &store,
            &setup.forge,
            &address,
            &setup._dir.path().join("editor"),
        );
        setup
            .core
            .update(cx, |core, cx| core.add_host(host.clone(), cx).unwrap());
        setup.wait(cx, "new host clears old account", |setup, cx| {
            setup
                .window
                .read_with(cx, |sidebar, cx| {
                    sidebar.profile.is_none()
                        && sidebar
                            .host
                            .as_ref()
                            .is_some_and(|data| data.read(cx).id == host.id)
                })
                .unwrap()
        });
        drop(old_account);
        cx.run_until_parked();
        assert!(
            old_weak.upgrade().is_none(),
            "account and its owned request must release on host switch"
        );
        setup.wait(
            cx,
            "cancelled account request releases its future",
            |_, _| release.is_closed(),
        );
        setup
            .window
            .update(cx, |sidebar, _, cx| sidebar.show_profile(cx))
            .unwrap();
        let new_account = setup
            .window
            .read_with(cx, |sidebar, _| sidebar.profile.clone().unwrap())
            .unwrap();
        new_account.update(cx, |account, _| {
            account.error = Some("new account error".into())
        });
        assert!(
            release.send(true).is_err(),
            "dropping account cancels its pending request"
        );
        cx.run_until_parked();
        assert_eq!(
            new_account.read_with(cx, |account, _| account.error.clone()),
            Some("new account error".into())
        );
        assert!(new_account.read_with(cx, |account, _| account.refreshing.is_empty()));
    }
}
