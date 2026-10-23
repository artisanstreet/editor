//! The sidebar while Settings is open: the way back, then the settings
//! pages grouped by where they are kept (this Editor, the active host, the
//! host's engines), on the same rows and hover pill as New thread.

use crate::{HISTORY_SEPARATOR, HISTORY_SEPARATOR_GAP, Sidebar};
use artisan_api::{EngineStatus, InstallState, SignInState};
use editor_core::{Core, Screen, SettingsPage};
use editor_ui::AssetId;
use editor_ui::theme::{ActiveTheme as _, ArtisanTheme};
use gpui::{
    Context, Div, IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    Styled as _, div, px,
};

/// The Editor's own pages, kept in this device's `settings.json`.
const EDITOR_PAGES: [(SettingsPage, AssetId); 3] = [
    (SettingsPage::General, AssetId::TABLER_SETTINGS),
    (SettingsPage::Hosts, AssetId::TABLER_DEVICE_LAPTOP),
    (SettingsPage::About, AssetId::TABLER_LIST_DETAILS),
];

/// The active host's pages, kept by its Forge.
const HOST_PAGES: [(SettingsPage, AssetId); 3] = [
    (SettingsPage::Threads, AssetId::TABLER_MESSAGES),
    (SettingsPage::GlobalPrompt, AssetId::TABLER_FILE_TEXT),
    (SettingsPage::Extensions, AssetId::TABLER_PLUG_CONNECTED),
];

/// What an engine still needs before it can run, said beside its row.
const fn engine_need(status: &EngineStatus) -> Option<&'static str> {
    match (&status.install, &status.sign_in) {
        (InstallState::Absent, _) => Some("Install"),
        (InstallState::Failed { .. }, _) => Some("Failed"),
        (InstallState::Downloading { .. } | InstallState::Verifying { .. }, _) => {
            Some("Installing")
        }
        (_, SignInState::SignedOut | SignInState::Failed { .. }) => Some("Sign in"),
        _ => None,
    }
}

impl Sidebar {
    /// A group's heading, as the history's day headings are drawn.
    fn settings_heading(label: impl Into<SharedString>, theme: &ArtisanTheme) -> Div {
        div()
            .h(px(21.0))
            .mt(px(14.0))
            .flex()
            .items_center()
            .px(px(8.0))
            .text_xs()
            .text_color(theme.colors.text_muted.to_paint())
            .truncate()
            .child(label.into())
    }

    fn settings_link(
        &self,
        target: SettingsPage,
        icon: AssetId,
        current: SettingsPage,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let label = target.label();
        let id = format!(
            "sidebar-settings-{}",
            label.to_lowercase().replace(' ', "-")
        );
        Self::nav_row(
            id.clone(),
            id,
            &self.nav_hover,
            icon,
            label,
            target == current,
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.navigate(Screen::Settings(target), cx);
        }))
    }

    pub(crate) fn render_settings_nav(&self, page: SettingsPage, cx: &Context<Self>) -> Div {
        let theme = *cx.theme();
        let back = Self::nav_row(
            "sidebar-settings-back",
            "settings-back",
            &self.nav_hover,
            AssetId::TABLER_ARROW_NARROW_LEFT,
            "Back",
            false,
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.core.update(cx, Core::close_settings);
        }));
        // Back sits above a full-width separator, like History, so the
        // settings pages read as their own list.
        div()
            .flex()
            .flex_col()
            .child(div().px(px(8.0)).child(back))
            .child(
                div()
                    .my(px(HISTORY_SEPARATOR_GAP))
                    .h(px(HISTORY_SEPARATOR))
                    .bg(theme.colors.line.to_paint()),
            )
            .child(self.settings_pages(page, cx).px(px(8.0)))
    }

    fn settings_pages(&self, page: SettingsPage, cx: &Context<Self>) -> Div {
        let theme = *cx.theme();
        let mut nav = div().flex().flex_col().gap(px(2.0));
        nav = nav.child(Self::settings_heading("Editor", &theme));
        for (target, icon) in EDITOR_PAGES {
            nav = nav.child(self.settings_link(target, icon, page, cx));
        }
        let Some(host) = self.host.clone() else {
            return nav;
        };
        nav = nav.child(Self::settings_heading(host.read(cx).name.clone(), &theme));
        for (target, icon) in HOST_PAGES {
            nav = nav.child(self.settings_link(target, icon, page, cx));
        }
        let engines: Vec<EngineStatus> = host.read(cx).engines.values().cloned().collect();
        if engines.is_empty() {
            return nav;
        }
        nav = nav.child(Self::settings_heading("Engines", &theme));
        for status in engines {
            let engine = status.engine;
            let target = SettingsPage::Engine(engine);
            let id = format!("sidebar-settings-engine-{engine}");
            let need = engine_need(&status).map(|need| {
                div()
                    .flex_none()
                    .text_size(theme.typography.label_text)
                    .text_color(theme.colors.text_muted.to_paint())
                    .child(need)
            });
            nav = nav.child(
                Self::nav_row(
                    id.clone(),
                    id,
                    &self.nav_hover,
                    editor_ui::engine_icon(&theme, engine),
                    engine.display_name(),
                    target == page,
                    cx,
                )
                .child(div().flex_1())
                .children(need)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.navigate(Screen::Settings(target), cx);
                })),
            );
        }
        nav
    }
}
