//! The paired Forges and adding new ones through enrollment.

use crate::Settings;
use crate::page;
use crate::ui::{card, error_text, header, row, section_shell};
use editor_core::{ConnectionStatus, HostConfig, HostId, SettingsPage};
use editor_ui::button::ButtonVariant;
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, Context, Div, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
};

pub struct HostsForm {
    error: Option<String>,
}

impl HostsForm {
    pub(crate) const fn new() -> Self {
        Self { error: None }
    }
}

impl Settings {
    fn remove_host(&mut self, id: &HostId, cx: &mut Context<Self>) {
        if let Err(error) = self.core.update(cx, |core, cx| core.remove_host(id, cx)) {
            self.hosts.error = Some(format!("Couldn't remove the host: {error:#}"));
        }
        cx.notify();
    }

    fn open_enrollment(&self, cx: &mut Context<Self>) {
        self.core.update(cx, |core, cx| {
            core.navigate(editor_core::Screen::Settings(SettingsPage::Account), cx);
        });
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the whole Hosts page in reading order"
)]
pub fn render(settings: &Settings, theme: &ArtisanTheme, cx: &Context<Settings>) -> Div {
    let core = settings.core.read(cx);
    let paired: Vec<(HostConfig, ConnectionStatus, Option<String>)> = core
        .hosts()
        .map(|(config, host)| {
            let data = host.read(cx);
            (
                config.clone(),
                data.status.clone(),
                data.forge_version.clone(),
            )
        })
        .collect();
    let mut rows: Vec<AnyElement> = Vec::new();
    for (config, status, version) in &paired {
        let (state, colour) = match status {
            ConnectionStatus::Live => ("Connected".to_owned(), theme.colors.green.solid),
            ConnectionStatus::Connecting => ("Connecting…".to_owned(), theme.colors.text_muted),
            ConnectionStatus::Offline { reason, .. } => {
                (format!("Offline: {reason}"), theme.colors.red.solid)
            }
        };
        let mut detail = config.address.clone();
        if let Some(version) = version {
            detail = format!("{detail} · Forge {version}");
        }
        let id = config.id.clone();
        let open_id = config.id.clone();
        let live = matches!(status, ConnectionStatus::Live);
        let remove = settings.button(
            SharedString::from(format!("host-remove-{}", config.id)),
            "Remove",
            ButtonVariant::Ghost,
            false,
            cx.listener(move |this, _, _, cx| this.remove_host(&id, cx)),
        );
        let status_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .max_w(px(200.0))
            .text_size(theme.typography.label_text)
            .text_color(theme.colors.text_muted.to_paint())
            .child(
                div()
                    .size(px(6.0))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(colour.to_paint()),
            )
            .child(div().truncate().child(state));
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .child(status_row)
            .children(live.then(|| {
                settings
                    .button(
                        SharedString::from(format!("host-open-{}", config.id)),
                        "Open",
                        ButtonVariant::Outline,
                        false,
                        cx.listener(move |this, _, _, cx| {
                            this.core.update(cx, |core, cx| {
                                core.navigate(
                                    editor_core::Screen::NewThread {
                                        host: open_id.clone(),
                                        project: None,
                                    },
                                    cx,
                                );
                            });
                        }),
                    )
                    .into_any_element()
            }))
            .child(remove);
        rows.push(
            row(
                theme,
                config.name.clone(),
                detail,
                Some(controls.into_any_element()),
            )
            .into_any_element(),
        );
    }
    if rows.is_empty() {
        rows.push(
            row(
                theme,
                "No Forges yet",
                "Add one through enrollment below.",
                None,
            )
            .into_any_element(),
        );
    }

    let add = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(card(
            theme,
            vec![
                row(
                    theme,
                    "Add a Forge",
                    "Forges join this account through enrollment, where the exact account is confirmed.",
                    Some(
                        settings
                            .button(
                                "host-add-via-enrollment",
                                "Add Forge via enrollment",
                                ButtonVariant::Default,
                                false,
                                cx.listener(|this, _, _, cx| this.open_enrollment(cx)),
                            )
                            .into_any_element(),
                    ),
                )
                .into_any_element(),
            ],
        ));
    let mut page = div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::Hosts),
            page::description(SettingsPage::Hosts),
        ))
        .child(
            section_shell(theme, "Paired hosts", None, None, card(theme, rows))
                .debug_selector("settings-section-paired"),
        )
        .child(
            section_shell(theme, "Add a host", None, None, add)
                .debug_selector("settings-section-pair"),
        );
    if let Some(error) = &settings.hosts.error {
        page = page.child(error_text(theme, error.clone()));
    }
    page
}
