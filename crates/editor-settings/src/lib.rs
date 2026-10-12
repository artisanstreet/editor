//! Settings, ported from the previous Editor's `native_settings`: a
//! centred row of the nav rail (the `ARTISAN` pages, then each engine)
//! and the open page.
//!
//! Everything shown comes from [`Core`] (device preferences, paired hosts)
//! and the active host's [`HostData`] (what its Forge sent). The only state
//! kept here is form fields and the outcome of the last action.

mod about;
mod appearance;
mod chrome;
mod engines;
mod extensions;
pub mod frame_rate;
mod hosts;
mod models;
mod notifications;
mod page;
mod privacy;
mod prompt;
#[cfg(test)]
mod tests;
mod threads;

use artisan_api::EngineId;
use chrome::{
    CONTENT_MAX_WIDTH, NAV_CONTENT_GAP, NAV_RAIL_WIDTH, anchor_list, nav_group_label, nav_heading,
    nav_link,
};
use editor_core::{Core, CoreEvent, HostData, HostEvent, Screen, SettingsPage};
use editor_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant};
use editor_ui::motion::MotionPolicy;
use editor_ui::switch::{Switch, SwitchSize};
use editor_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, ElementId, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

pub use about::BuildInfo;

/// What a live switch does when flipped.
type SwitchChange = Box<dyn Fn(bool, &mut Window, &mut App)>;
pub use notifications::NOTIFICATIONS_KEY;

pub struct Settings {
    core: Entity<Core>,
    build: BuildInfo,
    /// The engine the Engines page shows; the host's first engine until
    /// the user picks one.
    engine: Option<EngineId>,
    engines: engines::EnginesForm,
    extensions: extensions::ExtensionsForm,
    prompt: prompt::PromptForm,
    hosts: hosts::HostsForm,
    performance: appearance::PerformanceControl,
    archive: threads::ArchiveForm,
    /// Shared by every static control; they need a handle but never focus.
    control_focus: FocusHandle,
    host_subscriptions: Vec<Subscription>,
    _core_subscription: Subscription,
}

impl Settings {
    pub fn new(
        core: Entity<Core>,
        build: BuildInfo,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let core_subscription = cx.subscribe(&core, |this, _, event: &CoreEvent, cx| {
            if matches!(event, CoreEvent::HostsChanged) {
                this.watch_hosts(cx);
                this.sync_host(cx);
            }
            cx.notify();
        });
        let mut settings = Self {
            core,
            build,
            engine: None,
            engines: engines::EnginesForm::new(cx),
            extensions: extensions::ExtensionsForm::new(cx),
            prompt: prompt::PromptForm::new(cx),
            hosts: hosts::HostsForm::new(cx),
            performance: appearance::PerformanceControl::default(),
            archive: threads::ArchiveForm::default(),
            control_focus: cx.focus_handle(),
            host_subscriptions: Vec::new(),
            _core_subscription: core_subscription,
        };
        settings.watch_hosts(cx);
        settings.sync_host(cx);
        settings
    }

    #[must_use]
    pub fn core(&self) -> &Entity<Core> {
        &self.core
    }

    /// Re-renders when any host's projects, settings, engines or status
    /// change; items and live text don't concern settings.
    fn watch_hosts(&mut self, cx: &mut Context<Self>) {
        let hosts: Vec<Entity<HostData>> = self
            .core
            .read(cx)
            .hosts()
            .map(|(_, host)| host.clone())
            .collect();
        self.host_subscriptions = hosts
            .iter()
            .map(|host| {
                cx.subscribe(host, |this, host, event: &HostEvent, cx| {
                    if matches!(event, HostEvent::Changed | HostEvent::Reset) {
                        if this.selected_host(cx).as_ref() == Some(&host) {
                            this.sync_host(cx);
                        }
                        cx.notify();
                    }
                })
            })
            .collect();
    }

    /// Loads what the forms show from the active host.
    fn sync_host(&mut self, cx: &mut Context<Self>) {
        let host = self.selected_host(cx);
        self.extensions.sync(host.as_ref(), cx);
        self.prompt.sync(host.as_ref(), cx);
    }

    /// The host the per-host pages show: the one the Editor works on.
    fn selected_host(&self, cx: &App) -> Option<Entity<HostData>> {
        self.core.read(cx).active_host().cloned()
    }

    fn page(&self, cx: &App) -> SettingsPage {
        match self.core.read(cx).screen() {
            Screen::Settings(page) => *page,
            _ => SettingsPage::Models,
        }
    }

    fn navigate(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.core
            .update(cx, |core, cx| core.navigate(Screen::Settings(page), cx));
    }

    /// The engines the rail lists: what the host reports, else every one.
    fn listed_engines(&self, cx: &App) -> Vec<EngineId> {
        let reported: Vec<EngineId> = self
            .selected_host(cx)
            .map(|host| host.read(cx).engines.keys().copied().collect())
            .unwrap_or_default();
        if reported.is_empty() {
            EngineId::ALL.to_vec()
        } else {
            reported
        }
    }

    /// The engine the Engines page shows.
    fn shown_engine(&self, cx: &App) -> EngineId {
        let listed = self.listed_engines(cx);
        self.engine
            .filter(|engine| listed.contains(engine))
            .or_else(|| listed.first().copied())
            .unwrap_or(EngineId::Claude)
    }

    /// The rail (`nav.svelte`): the heading, the `ARTISAN` pages with the
    /// open one's anchors, then the `ENGINES` group.
    fn render_nav(&self, theme: &ArtisanTheme, page: SettingsPage, cx: &mut Context<Self>) -> Div {
        let mut rail = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(NAV_RAIL_WIDTH))
            .gap(px(4.0))
            .debug_selector(|| "settings-nav".to_owned())
            .child(nav_heading(theme))
            .child(nav_group_label(theme, "ARTISAN"));
        for target in page::ARTISAN_PAGES {
            let label = page::label(target);
            let selector = format!("settings-nav-{}", label.to_lowercase().replace(' ', "-"));
            rail = rail.child(
                nav_link(theme, label, page == target)
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.navigate(target, cx);
                    })),
            );
            if page == target {
                rail = rail.child(anchor_list(theme, page::anchors(target)));
            }
        }
        rail = rail.child(nav_group_label(theme, "ENGINES"));
        let shown = self.shown_engine(cx);
        for engine in self.listed_engines(cx) {
            let active = page == SettingsPage::Engines && engine == shown;
            let selector = format!("settings-nav-engines-{engine}");
            rail = rail.child(
                nav_link(theme, page::engine_label(engine), active)
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.engine = Some(engine);
                        this.engines.clear_errors();
                        this.navigate(SettingsPage::Engines, cx);
                        cx.notify();
                    })),
            );
            if active {
                rail = rail.child(anchor_list(theme, page::anchors(SettingsPage::Engines)));
            }
        }
        rail
    }

    fn render_main(
        &mut self,
        theme: &ArtisanTheme,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let host = self.selected_host(cx);
        match (page, host) {
            (SettingsPage::Models, _) => models::render(self, theme).into_any_element(),
            (SettingsPage::Appearance, _) => {
                appearance::render(self, theme, window, cx).into_any_element()
            }
            (SettingsPage::Notifications, _) => {
                notifications::render(self, theme, cx).into_any_element()
            }
            (SettingsPage::Privacy, _) => privacy::render(self, theme).into_any_element(),
            (SettingsPage::About, _) => about::render(&self.build, theme).into_any_element(),
            (SettingsPage::Hosts, _) => hosts::render(self, theme, cx).into_any_element(),
            (_, None) => div()
                .flex()
                .flex_col()
                .child(chrome::header(
                    theme,
                    page::label(page),
                    page::description(page),
                ))
                .child(chrome::note(theme, "Pair a host on the Hosts page first.").mt(px(32.0)))
                .into_any_element(),
            (SettingsPage::GlobalPrompt, Some(_)) => {
                prompt::render(self, theme, cx).into_any_element()
            }
            (SettingsPage::Threads, Some(host)) => {
                threads::render(self, &host, theme, cx).into_any_element()
            }
            (SettingsPage::Engines, Some(host)) => {
                let engine = self.shown_engine(cx);
                engines::render(self, &host, engine, theme, cx).into_any_element()
            }
            (SettingsPage::Extensions, Some(host)) => {
                extensions::render(self, &host, theme, cx).into_any_element()
            }
        }
    }

    /// A small button (`button.svelte` wrappers).
    fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        variant: ButtonVariant,
        disabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let label = label.into();
        match Button::new(
            id,
            self.control_focus.clone(),
            theme(),
            MotionPolicy::Full,
            variant,
            ButtonSize::Small,
            ButtonContent::text(label.clone()),
        ) {
            Ok(button) => button
                .disabled(disabled)
                .on_activate(on_click)
                .into_any_element(),
            Err(error) => {
                tracing::error!("button {label:?}: {error}");
                div().into_any_element()
            }
        }
    }

    /// A control the previous Editor painted without an action yet.
    fn static_button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        variant: ButtonVariant,
    ) -> AnyElement {
        self.button(id, label, variant, true, |_, _, _| {})
    }

    /// A switch; without `on_change` it is painted disabled.
    fn switch(
        &self,
        id: &'static str,
        checked: bool,
        on_change: Option<SwitchChange>,
    ) -> AnyElement {
        let switch = Switch::new(
            id,
            self.control_focus.clone(),
            theme(),
            SwitchSize::Default,
            checked,
        )
        .debug_selector(id);
        match on_change {
            Some(on_change) => switch
                .on_change(move |checked, _, window, cx| on_change(checked, window, cx))
                .into_any_element(),
            None => switch.disabled(true).into_any_element(),
        }
    }
}

impl Render for Settings {
    /// The frame (`+layout.svelte`): a centred `max-w-4xl` row of the
    /// 176 px rail and the growing page, 56 px apart.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme();
        let page = self.page(cx);
        let nav = self.render_nav(&theme, page, cx);
        let main = self.render_main(&theme, page, window, cx);
        let selector = format!("route-settings-{}", page::label(page).to_lowercase());
        div()
            .id("settings-screen")
            .debug_selector(move || selector.clone())
            .size_full()
            .overflow_y_scroll()
            .bg(theme.colors.background.to_paint())
            .text_color(theme.colors.foreground.to_paint())
            .child(
                div().w_full().flex().flex_row().justify_center().child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .max_w(px(CONTENT_MAX_WIDTH))
                        .px(px(24.0))
                        .py(px(48.0))
                        .gap(px(NAV_CONTENT_GAP))
                        .child(nav)
                        .child(div().flex().flex_col().flex_1().min_w_0().child(main)),
                ),
            )
    }
}

/// The Editor is dark throughout, like the old one.
fn theme() -> ArtisanTheme {
    ArtisanTheme::for_mode(ThemeMode::Dark)
}
