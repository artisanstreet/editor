//! Paired hosts and pairing new ones: local and WSL with no typing, remote
//! by pasting what `ae forge pair` printed.

use crate::Settings;
use crate::page;
use anyhow::Result;
use editor_core::hosts::{self, LocalTarget};
use editor_core::{ConnectionStatus, HostConfig, HostId, HostKind, SettingsPage};
use editor_ui::button::ButtonVariant;
use editor_ui::page::{card, error_text, header, row, section_shell};
use editor_ui::text_editor::{TextEditor, TextEditorEvent};
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, div, px,
};

pub(crate) struct HostsForm {
    pub(crate) pairing: Entity<TextEditor>,
    /// What is being paired right now ("this computer", an address, …).
    pending: Option<String>,
    error: Option<String>,
    _submit: Subscription,
}

impl HostsForm {
    pub(crate) fn new(cx: &mut Context<Settings>) -> Self {
        let pairing =
            cx.new(|cx| TextEditor::new("", "host:port CODE fingerprint", cx).single_line());
        let submit = cx.subscribe(
            &pairing,
            |this, _, event: &TextEditorEvent, cx| match event {
                TextEditorEvent::Submitted => this.pair_pasted(cx),
                TextEditorEvent::Changed => cx.notify(),
            },
        );
        Self {
            pairing,
            pending: None,
            error: None,
            _submit: submit,
        }
    }
}

impl Settings {
    pub(crate) fn discover_hosts(&mut self, cx: &mut Context<Self>) {
        self.core
            .update(cx, |core, cx| core.load_pairing_targets(false, cx));
    }

    #[cfg(test)]
    pub(crate) fn hosts_error(&self) -> Option<&str> {
        self.hosts.error.as_deref()
    }

    /// Pairs with the Forge in the pasted `<host:port> <code> <fingerprint>`.
    pub(crate) fn pair_pasted(&mut self, cx: &mut Context<Self>) {
        let text = self.hosts.pairing.read(cx).text().trim().to_owned();
        let (address, code, fingerprint) = match hosts::parse_pairing(&text) {
            Ok(parts) => parts,
            Err(error) => {
                self.hosts.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        let core = self.core.read(cx);
        let (identity, device) = (core.identity.clone(), core.device_name.clone());
        let Some(identity) = identity else {
            self.hosts.error = Some("This device's identity could not be loaded. Repair its configuration before pairing.".into());
            cx.notify();
            return;
        };
        let name = address
            .rsplit_once(':')
            .map_or(address.as_str(), |(host, _)| host)
            .to_owned();
        self.start_pairing(address.clone(), cx, async move {
            hosts::pair(
                &address,
                &code,
                &fingerprint,
                &identity,
                &device,
                name,
                HostKind::Remote,
            )
            .await
        });
    }

    fn pair_local(&mut self, target: LocalTarget, cx: &mut Context<Self>) {
        let core = self.core.read(cx);
        let (identity, device) = (core.identity.clone(), core.device_name.clone());
        let Some(identity) = identity else {
            self.hosts.error = Some("This device's identity could not be loaded. Repair its configuration before pairing.".into());
            cx.notify();
            return;
        };
        let label = match &target {
            LocalTarget::ThisMachine => "this computer".to_owned(),
            LocalTarget::Wsl { distro } => format!("WSL: {distro}"),
        };
        self.start_pairing(label, cx, async move {
            hosts::pair_local(target, &identity, &device).await
        });
    }

    fn start_pairing(
        &mut self,
        label: String,
        cx: &mut Context<Self>,
        pairing: impl std::future::Future<Output = Result<HostConfig>> + 'static,
    ) {
        if self.hosts.pending.is_some() {
            return;
        }
        self.hosts.pending = Some(label.clone());
        self.hosts.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = pairing.await;
            let _ = this.update(cx, |this, cx| {
                this.hosts.pending = None;
                let added = result
                    .and_then(|config| this.core.update(cx, |core, cx| core.add_host(config, cx)));
                match added {
                    Ok(()) => this
                        .hosts
                        .pairing
                        .update(cx, |editor, cx| editor.set_text("", cx)),
                    Err(error) => {
                        tracing::warn!("pairing with {label} failed: {error:#}");
                        this.hosts.error = Some(format!("Couldn't pair with {label}: {error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remove_host(&mut self, id: &HostId, cx: &mut Context<Self>) {
        if let Err(error) = self.core.update(cx, |core, cx| core.remove_host(id, cx)) {
            self.hosts.error = Some(format!("Couldn't remove the host: {error:#}"));
        }
        cx.notify();
    }
}

/// A muted 14 px line.
fn muted(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_sm()
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(text.into())
}

#[allow(clippy::too_many_lines)]
pub(crate) fn render(settings: &Settings, theme: &ArtisanTheme, cx: &mut Context<Settings>) -> Div {
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
            ConnectionStatus::Live => ("Connected".to_owned(), theme.colors.banner_success),
            ConnectionStatus::Connecting => {
                ("Connecting…".to_owned(), theme.colors.muted_foreground)
            }
            ConnectionStatus::Offline { reason, .. } => {
                (format!("Offline: {reason}"), theme.colors.destructive)
            }
        };
        let mut detail = config.address.clone();
        if let Some(version) = version {
            detail = format!("{detail} · Forge {version}");
        }
        let id = config.id.clone();
        let remove = settings.button(
            SharedString::from(format!("host-remove-{}", config.id)),
            "Remove",
            ButtonVariant::Ghost,
            false,
            cx.listener(move |this, _, _, cx| this.remove_host(&id, cx)),
        );
        rows.push(
            div()
                .flex()
                .flex_col()
                .child(row(theme, config.name.clone(), detail, Some(remove)))
                .child(
                    div()
                        .pb(px(12.0))
                        .mt(px(-8.0))
                        .text_xs()
                        .text_color(colour.to_paint())
                        .child(state),
                )
                .into_any_element(),
        );
    }
    if rows.is_empty() {
        rows.push(
            row(
                theme,
                "No hosts yet",
                "Pair this computer or a remote Forge below.",
                None,
            )
            .into_any_element(),
        );
    }

    let pending = settings.hosts.pending.is_some();
    let local_button =
        |id: String, label: String, target: LocalTarget, cx: &mut Context<Settings>| {
            settings.button(
                SharedString::from(id),
                label,
                ButtonVariant::Outline,
                pending,
                cx.listener(move |this, _, _, cx| this.pair_local(target.clone(), cx)),
            )
        };
    let has_kind = |kind: &HostKind| paired.iter().any(|(config, _, _)| &config.kind == kind);
    let mut local = div().flex().flex_row().flex_wrap().gap(px(6.0));
    if !has_kind(&HostKind::Local) {
        local = local.child(local_button(
            "pair-local".into(),
            "This computer".into(),
            LocalTarget::ThisMachine,
            cx,
        ));
    }
    for distro in settings.core.read(cx).pairing_targets().distros.clone() {
        if !has_kind(&HostKind::Wsl {
            distro: distro.clone(),
        }) {
            local = local.child(local_button(
                format!("pair-wsl-{distro}"),
                format!("WSL: {distro}"),
                LocalTarget::Wsl { distro },
                cx,
            ));
        }
    }

    let pair_button = settings.button(
        "pair-remote",
        "Pair",
        ButtonVariant::Default,
        pending || settings.hosts.pairing.read(cx).text().trim().is_empty(),
        cx.listener(|this, _, _, cx| this.pair_pasted(cx)),
    );
    let mut add = div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(muted(
            theme,
            "On this machine (Forge must be installed with `ae install forge`):",
        ))
        .child(local)
        .child(muted(
            theme,
            "Remote: run `ae forge pair` on the host and paste what it prints.",
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(settings.hosts.pairing.clone()),
                )
                .child(pair_button),
        );
    if let Some(error) = settings.core.read(cx).pairing_targets().error.clone() {
        add = add.child(error_text(theme, error)).child(settings.button(
            "retry-local-targets",
            "Retry WSL discovery",
            ButtonVariant::Outline,
            false,
            cx.listener(|this, _, _, cx| {
                this.core
                    .update(cx, |core, cx| core.load_pairing_targets(true, cx))
            }),
        ));
    }
    if let Some(label) = &settings.hosts.pending {
        add = add.child(muted(theme, format!("Pairing with {label}…")));
    }
    if let Some(error) = &settings.hosts.error {
        add = add.child(error_text(theme, error.clone()));
    }
    div()
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
            section_shell(theme, "Pair a host", None, None, add)
                .debug_selector("settings-section-pair"),
        )
}
