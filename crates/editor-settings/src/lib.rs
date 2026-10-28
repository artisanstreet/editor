//! Settings reads device preferences from Core and application data from the selected HostData.
//! It retains form drafts and pending-action state only.

mod about;
pub mod account;
mod engines;
mod extensions;
pub mod frame_rate;
mod general;
mod hosts;
mod page;
mod prompt;
#[cfg(test)]
mod tests;
mod threads;
mod ui;

use editor_core::{Core, CoreEvent, HostData, HostEvent, Screen, SettingsPage};
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::switch::{Switch, SwitchSize};
use editor_ui::theme::{ActiveTheme as _, ArtisanTheme};
use editor_ui::wheel_glide::WheelGlide;
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, ElementId, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, point, px,
};

pub use about::BuildInfo;

/// What a live switch does when flipped.
type SwitchChange = Box<dyn Fn(bool, &mut Window, &mut App)>;
pub use general::NOTIFICATIONS_KEY;

pub struct Settings {
    #[cfg(test)]
    completion_gate: Option<tests::CompletionGate>,
    core: Entity<Core>,
    /// The application theme, refreshed every render (the mode can flip).
    theme: ArtisanTheme,
    build: BuildInfo,
    engines: engines::EnginesForm,
    sign_in: Entity<account::sign_in::SignInView>,
    forges: Entity<account::forges::ForgesView>,
    extensions: extensions::ExtensionsForm,
    prompt: prompt::PromptForm,
    archive: threads::ArchiveForm,
    scroll: ScrollHandle,
    scroll_page: Option<SettingsPage>,
    glide: WheelGlide,
    glide_scheduled: bool,
    host_subscription: Option<Subscription>,
    _core_subscription: Subscription,
    _account_subscription: Subscription,
}

impl Settings {
    pub fn new(
        core: Entity<Core>,
        build: BuildInfo,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let core_subscription = cx.subscribe(&core, |this, _, event: &CoreEvent, cx| {
            if !matches!(this.core.read(cx).screen(), Screen::Settings(_)) {
                this.host_subscription = None;
                return;
            }
            if matches!(event, CoreEvent::ScreenChanged) {
                this.glide.cancel();
                this.engines.clear_errors();
            }
            if matches!(event, CoreEvent::HostsChanged | CoreEvent::ScreenChanged) {
                this.archive.invalidate();
                this.watch_hosts(cx);
                this.sync_host(cx);
            }
            cx.notify();
        });
        let account = core.read(cx).account().clone();
        let account_subscription =
            cx.subscribe(&account, |_, _, _: &editor_core::AccountEvent, cx| {
                cx.notify();
            });
        let mut settings = Self {
            #[cfg(test)]
            completion_gate: None,
            theme: *cx.theme(),
            build,
            engines: engines::EnginesForm::new(cx),
            sign_in: cx.new(|cx| account::sign_in::SignInView::new(core.clone(), cx)),
            forges: cx.new(|cx| account::forges::ForgesView::new(core.clone(), cx)),
            core,
            extensions: extensions::ExtensionsForm::new(cx),
            prompt: prompt::PromptForm::new(cx),
            archive: threads::ArchiveForm::default(),
            scroll: ScrollHandle::new(),
            scroll_page: None,
            glide: WheelGlide::default(),
            glide_scheduled: false,
            host_subscription: None,
            _core_subscription: core_subscription,
            _account_subscription: account_subscription,
        };
        settings.watch_hosts(cx);
        settings.sync_host(cx);
        settings
    }

    #[must_use]
    pub const fn core(&self) -> &Entity<Core> {
        &self.core
    }

    /// Re-renders when any host's projects, settings, engines or status
    /// change; items and live text don't concern settings.
    fn watch_hosts(&mut self, cx: &mut Context<Self>) {
        self.host_subscription = self.selected_host(cx).map(|host| {
            cx.subscribe(&host, |this, _, event: &HostEvent, cx| {
                if matches!(
                    event,
                    HostEvent::ThreadChanged { .. } | HostEvent::Reset | HostEvent::Changed
                ) {
                    this.archive.invalidate();
                }
                if matches!(event, HostEvent::EngineInstall { engine } if this.page(cx).parent() == SettingsPage::Engine(*engine)) {
                    cx.notify();
                }
                let archive_changed = matches!(event, HostEvent::ThreadChanged { .. })
                    && this.page(cx).parent() == SettingsPage::Threads;
                if archive_changed || matches!(event, HostEvent::Changed | HostEvent::Reset) {
                    this.sync_host(cx);
                    cx.notify();
                }
            })
        });
    }

    /// Loads what the forms show from the active host.
    fn sync_host(&mut self, cx: &mut Context<Self>) {
        let host = self.selected_host(cx);
        self.engines
            .sync(host.as_ref().map(|host| host.read(cx).id.clone()));
        if self.page(cx).parent() == SettingsPage::Threads {
            if let Some(host) = &host
                && !host.read(cx).thread_page(true).loaded
                && !host.read(cx).thread_page(true).loading
            {
                host.update(cx, |host, cx| {
                    host.load_threads(true, editor_core::ThreadPageDirection::Latest, cx);
                });
            }
            self.archive.sync(host.as_ref(), cx);
        }
        self.extensions.sync(host.as_ref(), cx);
        if self.page(cx) == SettingsPage::GlobalPrompt {
            self.prompt.sync(host.as_ref(), cx);
        }
    }

    /// The host the per-host pages show: the one the Editor works on.
    fn selected_host(&self, cx: &App) -> Option<Entity<HostData>> {
        self.core.read(cx).active_host().cloned()
    }

    fn page(&self, cx: &App) -> SettingsPage {
        match self.core.read(cx).screen() {
            Screen::Settings(page) => *page,
            Screen::Home
            | Screen::Thread { .. }
            | Screen::NewThread { .. }
            | Screen::Marketplace
            | Screen::Usage => SettingsPage::General,
        }
    }

    fn wheel(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.delta.precise() || cx.reduce_motion() {
            self.glide.cancel();
            return;
        }
        let max = f32::from(self.scroll.max_offset().y).max(0.0);
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if max <= 0.0 || delta.abs() <= f32::EPSILON {
            return;
        }
        cx.stop_propagation();
        self.glide
            .push(f32::from(self.scroll.offset().y), delta, max);
        self.schedule_glide(window, cx);
    }

    fn schedule_glide(&mut self, window: &mut Window, cx: &Context<Self>) {
        if !self.glide.active() || self.glide_scheduled {
            return;
        }
        self.glide_scheduled = true;
        cx.on_next_frame(window, |this, window, cx| {
            this.glide_scheduled = false;
            if !matches!(this.core.read(cx).screen(), Screen::Settings(_)) || cx.reduce_motion() {
                this.glide.cancel();
                return;
            }
            let offset = this.scroll.offset();
            let max = f32::from(this.scroll.max_offset().y).max(0.0);
            if let Some(next) = this.glide.step(f32::from(offset.y), max) {
                this.scroll.set_offset(point(offset.x, px(next)));
                cx.notify();
            }
            this.schedule_glide(window, cx);
        });
    }

    fn render_main(
        &mut self,
        theme: &ArtisanTheme,
        page: SettingsPage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let host = self.selected_host(cx);
        match (page.parent(), host) {
            (SettingsPage::General, _) => general::render(self, theme, cx).into_any_element(),
            (SettingsPage::Section(_), _) => unreachable!("parent resolves sections"),
            (SettingsPage::About, _) => about::render(&self.build, theme).into_any_element(),
            (SettingsPage::Hosts, _) => hosts::render(self, theme, cx).into_any_element(),
            (SettingsPage::Account, _) => account::render(self, theme, cx).into_any_element(),
            (_, None) => div()
                .flex()
                .flex_col()
                .child(ui::header(
                    theme,
                    page::label(page),
                    page::description(page),
                ))
                .child(
                    div()
                        .mt(px(32.0))
                        .child(ui::note(theme, "Pair a host on the Hosts page first.")),
                )
                .into_any_element(),
            (SettingsPage::GlobalPrompt, Some(_)) => {
                prompt::render(self, theme, cx).into_any_element()
            }
            (SettingsPage::Threads, Some(host)) => {
                threads::render(self, &host, theme, cx).into_any_element()
            }
            (SettingsPage::Engine(engine), Some(host)) => {
                self.engines.prepare(engine, cx);
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
        Button::text(id, self.theme, label)
            .variant(variant)
            .disabled(disabled)
            .on_activate(on_click)
            .into_any_element()
    }

    /// A switch that acts when flipped.
    fn switch(&self, id: &'static str, checked: bool, on_change: SwitchChange) -> AnyElement {
        Switch::new(
            id,
            self.theme,
            SwitchSize::Default,
            checked,
            id.trim_start_matches("settings-").replace('-', " "),
        )
        .debug_selector(id)
        .on_change(move |checked, _, window, cx| on_change(checked, window, cx))
        .into_any_element()
    }
}

impl Render for Settings {
    /// One centred column on the shell's black; the sidebar lists the
    /// pages.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = *cx.theme();
        let theme = self.theme;
        let page = self.page(cx);
        if self.scroll_page != Some(page) {
            self.glide.cancel();
            self.scroll = ScrollHandle::new();
            self.scroll_page = Some(page);
        }
        let main = self.render_main(&theme, page, cx);
        let selector = format!(
            "route-settings-{}",
            page::label(page).to_lowercase().replace(' ', "-")
        );
        div()
            .id(SharedString::from(format!("settings-screen-{page:?}")))
            .debug_selector(move || selector)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .text_color(theme.colors.text.to_paint())
            .child(
                // Handle ticks inside the content before the outer scroller's
                // native listener. Nested controls still receive input first.
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .justify_center()
                    .on_scroll_wheel(cx.listener(Self::wheel))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(ui::COLUMN_MAX_WIDTH))
                            .px(px(40.0))
                            .pt(px(64.0))
                            .pb(px(120.0))
                            .flex()
                            .flex_col()
                            .child(main),
                    ),
            )
    }
}
