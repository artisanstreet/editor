//! Account controls own their request state and one minute clock, only while open.
use crate::{avatar, usage_visual};
use artisan_api::{EngineId, UsageLimits};
#[cfg(test)]
use artisan_api::{Timestamp, UsageCadence, UsageWindow};
use editor_core::{Core, HostData, HostEvent, Screen, SettingsPage};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::hover_card::{HoverFill, hover_fill};
use editor_ui::measure::Measured;
use editor_ui::picker_motion::HoverSurface;
use editor_ui::theme::{
    ActiveTheme as _, ArtisanTheme, RadiusStep, RadiusTokens, TextStep, ThemeMode, TypographyTokens,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Bounds, Context, Entity, FontWeight, Image, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, anchored, deferred, div, point, px,
};
use std::{cell::Cell, rc::Rc, sync::Arc};
use std::{collections::HashMap, time::Duration};

type Close = Rc<dyn Fn(&mut gpui::App)>;
pub struct Account {
    core: Entity<Core>,
    host: Option<Entity<HostData>>,
    pub(crate) ui: usage_visual::UsageUi,
    hover: HoverSurface,
    machines_hover: HoverSurface,
    machines_open: bool,
    host_origin: Rc<Cell<Bounds<Pixels>>>,
    submenu_bounds: Measured,
    avatars: HashMap<(ThemeMode, String), Arc<Image>>,
    close: Option<Close>,
    usage_height: Pixels,
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
            ui: usage_visual::UsageUi::default(),
            hover: HoverSurface::default(),
            machines_hover: HoverSurface::default(),
            machines_open: false,
            host_origin: Rc::default(),
            submenu_bounds: Measured::default(),
            avatars: HashMap::new(),
            close: None,
            usage_height: px(0.0),
            _events: events,
            _clock: clock,
        }
    }
}
#[cfg(test)]
fn title(window: &UsageWindow) -> &str {
    match window.cadence {
        UsageCadence::Session => "Session",
        UsageCadence::Week => "Week",
        UsageCadence::Month => "Month",
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
    pub(crate) fn contains_popup(&self, position: gpui::Point<Pixels>) -> bool {
        self.machines_open && self.submenu_bounds.contains(&position)
    }
    /// "Theme" with a ghost button showing the current mode: the moon in
    /// dark, the sun in light. Pressing it flips the mode behind a diagonal
    /// shimmer across the window. The row has the action rows' height; the
    /// button's corner is concentric with the menu's: the menu radius less
    /// the group's padding and the row's inset.
    fn theme_row(&self, theme: &ArtisanTheme) -> gpui::Div {
        const ROW_HEIGHT: f32 = 36.0;
        const INSET: f32 = 4.0;
        /// The actions group's padding around its rows.
        const GROUP_PADDING: f32 = 4.0;
        let row_radius = RadiusTokens::value(RadiusStep::LgMid);
        let menu_radius = RadiusTokens::value(RadiusStep::X2l);
        let button_size = px(2.0f32.mul_add(-INSET, ROW_HEIGHT));
        let mode = theme.colors.mode;
        let core = self.core.clone();
        let (icon, label) = if mode.is_dark() {
            (AssetId::TABLER_MOON, "Switch to light theme")
        } else {
            (AssetId::TABLER_SUN, "Switch to dark theme")
        };
        let hover_theme = *theme;
        let button = div()
            .id("artisan-desktop-profile-theme")
            .role(gpui::Role::Button)
            .aria_label(label)
            .focusable()
            .tab_index(0)
            .debug_selector(|| "artisan-desktop-profile-theme".into())
            .size(button_size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(RadiusTokens::nested(menu_radius, px(GROUP_PADDING + INSET)))
            .text_color(theme.colors.text_muted.to_paint())
            .cursor_pointer()
            .hover(move |style| hover_fill(style, &hover_theme, HoverFill::Plain))
            .child(asset_glyph(icon).size(px(15.0)))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                let next = cx.theme().colors.mode.toggled();
                editor_ui::theme::begin_mode_transition(next, window, cx);
                core.update(cx, |core, cx| {
                    core.set_setting("dark_mode", serde_json::Value::Bool(next.is_dark()), cx);
                });
            });
        div()
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .justify_between()
            .pl(px(12.0))
            .pr(px(INSET))
            .rounded(row_radius)
            .child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Sm))
                    .line_height(px(20.0))
                    .text_color(theme.colors.text.to_paint())
                    .child("Theme"),
            )
            .child(button)
    }

    /// A menu row. `submenu` rows sit in the host dropdown: they use its
    /// hover surface and keep it open; the profile menu's rows close it.
    fn action(
        &self,
        submenu: bool,
        id: &'static str,
        asset: AssetId,
        label: &'static str,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = *cx.theme();
        let surface = if submenu {
            &self.machines_hover
        } else {
            &self.hover
        };
        let hover = surface.clone();
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
            .child(surface.row_probe(id))
            .child(
                asset_glyph(asset)
                    .size(px(15.0))
                    .text_color(theme.colors.text_muted.to_paint()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(TypographyTokens::size(TextStep::Sm))
                    .line_height(px(20.0))
                    .text_color(theme.colors.text.to_paint())
                    .child(label),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if submenu {
                    hover.hovered(id, *hovered);
                    cx.notify();
                } else if *hovered {
                    this.machines_open = false;
                    hover.hovered(id, true);
                    cx.notify();
                }
            }))
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree for the machine menu, ported from the old Editor"
    )]
    fn host_menu(&mut self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let current = self.host.as_ref().map(|host| host.read(cx).id.clone());
        let hosts: Vec<_> = self
            .core
            .read(cx)
            .hosts()
            .map(|(config, host)| (config.clone(), host.read(cx).status_line()))
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
            let image = Arc::clone(
                self.avatars
                    .entry((theme.colors.mode, config.fingerprint.clone()))
                    .or_insert_with(|| avatar::avatar_image(&config.fingerprint)),
            );
            let secondary = status.map_or_else(|| config.address.clone(), str::to_owned);
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
                                    .text_color(theme.colors.text_muted.to_paint())
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
                            );
                        });
                    }),
            );
        }
        let core = self.core.clone();
        let close = self.close.clone();
        rows = rows
            .child(editor_ui::glass::glass_menu_separator(&theme))
            .child(
                self.action(
                    true,
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
                        core.navigate(Screen::Settings(SettingsPage::Hosts), cx);
                    });
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
                .child(
                    div()
                        .occlude()
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
                ),
        )
        .with_priority(2)
        .into_any_element()
    }
}
impl Render for Account {
    #[expect(
        clippy::too_many_lines,
        reason = "the account menu's root element tree, ported from the old Editor"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let (seed, user, name, status) = self.host.as_ref().map_or_else(
            || {
                (
                    "no-host".into(),
                    "Artisan".into(),
                    "No host".into(),
                    "No host".into(),
                )
            },
            |host| {
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
                let status = host
                    .status_line()
                    .map_or_else(|| host.name.clone(), str::to_owned);
                (
                    seed,
                    host.account
                        .as_ref()
                        .map_or_else(|| host.name.clone(), |account| account.user.clone()),
                    host.name.clone(),
                    status,
                )
            },
        );
        let image = Arc::clone(
            self.avatars
                .entry((theme.colors.mode, seed.clone()))
                .or_insert_with(|| avatar::avatar_image(&seed)),
        );
        let _ = name;
        let origin = Rc::clone(&self.host_origin);
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
                            .text_color(theme.colors.text_muted.to_paint())
                            .child(status),
                    ),
            )
            .child(
                asset_glyph(AssetId::TABLER_CHEVRON_RIGHT)
                    .size(px(15.0))
                    .text_color(theme.colors.text_muted.to_paint()),
            );
        let reports: Vec<(EngineId, UsageLimits)> = self
            .host
            .as_ref()
            .map(|host| {
                host.read(cx)
                    .engines
                    .values()
                    .filter_map(|status| {
                        // The menu meters limits; spend is the Usage page's.
                        status
                            .usage
                            .as_ref()
                            .filter(|usage| !usage.windows.is_empty())
                            .map(|usage| (status.engine, usage.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let separator = editor_ui::glass::glass_menu_line(&theme);
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
                        .child(usage_visual::section(self, &reports, &theme, cx)),
                )
                .child(div().h(px(1.0)).my(px(4.0)).bg(separator));
        }
        let core = self.core.clone();
        let close = self.close.clone();
        let usage_core = self.core.clone();
        let usage_close = self.close.clone();
        let actions = div()
            .id("artisan-desktop-profile-actions-hover-surface")
            .debug_selector(|| "artisan-desktop-profile-actions-hover-surface".into())
            .w_full()
            .flex()
            .flex_col()
            .p(px(4.0))
            .child(
                self.action(
                    false,
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
                        core.navigate(Screen::Settings(SettingsPage::General), cx);
                    });
                }),
            )
            .child(
                self.action(
                    false,
                    "artisan-desktop-profile-action-1",
                    AssetId::TABLER_CHART_COLUMN,
                    "Usage",
                    cx,
                )
                .on_click(move |_, _, cx| {
                    if let Some(close) = &usage_close {
                        close(cx);
                    }
                    usage_core.update(cx, |core, cx| core.navigate(Screen::Usage, cx));
                }),
            );
        // Theme sits apart from the actions, the line edge to edge like the
        // menu's other separators.
        body = body.child(
            actions
                // The line above the actions sits 4 px over the group's 4 px
                // padding, 8 px from Settings; this one gets the same 8 px
                // below Usage, then 4 px over the theme row as before.
                .child(
                    div()
                        .h(px(1.0))
                        .mt(px(8.0))
                        .mb(px(4.0))
                        .mx(px(-4.0))
                        .bg(separator),
                )
                .child(self.theme_row(&theme)),
        );
        if let Some(tooltip) = usage_visual::tooltip(self, &reports, &theme, &theme.colors) {
            body = body.child(tooltip);
        }
        let submenu = self.machines_open.then(|| self.host_menu(window, cx));
        body.children(submenu)
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
