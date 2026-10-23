//! Home: with no host yet, the pairing entry for local, WSL and remote
//! Forges.

use editor_core::{Core, LocalTarget, Screen, SettingsPage, hosts};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::theme::{ActiveTheme as _, ColorTokens};
use gpui::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px,
};

/// The heading under the emblem (`HOME_HEADLINE_TEXT_PX`).
const HEADLINE: f32 = 28.0;
/// The muted terminal emblem (`HOME_EMBLEM_SIZE_PX`).
const EMBLEM: f32 = 30.0;

pub struct Home {
    core: Entity<Core>,
    _targets: Subscription,
    pairing: Option<LocalTarget>,
    error: Option<String>,
}

impl Home {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        core.update(cx, |core, cx| core.load_pairing_targets(false, cx));
        let targets = cx.subscribe(&core, |_, _, _: &editor_core::CoreEvent, cx| cx.notify());
        Self {
            core,
            _targets: targets,
            pairing: None,
            error: None,
        }
    }

    fn pair(&mut self, target: LocalTarget, cx: &mut Context<Self>) {
        if self.pairing.is_some() {
            return;
        }
        let (identity, device_name) = {
            let core = self.core.read(cx);
            (core.identity.clone(), core.device_name.clone())
        };
        let Some(identity) = identity else {
            self.error = Some("This device's identity could not be loaded. Repair its configuration before pairing.".into());
            cx.notify();
            return;
        };
        self.pairing = Some(target.clone());
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = hosts::pair_local(target, &identity, &device_name).await;
            this.update(cx, |home, cx| {
                home.pairing = None;
                match result.and_then(|config| {
                    let host = config.id.clone();
                    home.core.update(cx, |core, cx| {
                        core.add_host(config, cx)?;
                        core.navigate(
                            Screen::NewThread {
                                host,
                                project: None,
                            },
                            cx,
                        );
                        anyhow::Ok(())
                    })
                }) {
                    Ok(()) => {}
                    Err(error) => home.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn button(
        &self,
        desktop: &ColorTokens,
        id: SharedString,
        label: String,
        cx: &Context<Self>,
        target: Option<LocalTarget>,
    ) -> AnyElement {
        let busy = target.is_some() && self.pairing == target;
        let label = if busy { format!("{label}…") } else { label };
        let selector = id.to_string();
        let button = div()
            .id(id)
            .debug_selector(move || selector)
            .focusable()
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .cursor_pointer()
            .px(px(16.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(desktop.line.to_paint())
            .text_color(desktop.text.to_paint())
            .child(label);

        match target {
            Some(target) => button.on_click(cx.listener(move |home, _, _, cx| {
                if home.pairing.is_none() {
                    home.pair(target.clone(), cx);
                }
            })),
            None => button.on_click(cx.listener(|home, _, _, cx| {
                home.core.update(cx, |core, cx| {
                    core.navigate(Screen::Settings(SettingsPage::Hosts), cx);
                });
            })),
        }
        .into_any_element()
    }
}

impl Render for Home {
    #[expect(
        clippy::too_many_lines,
        reason = "the pairing page's buttons and copy build one element tree"
    )]
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let desktop = cx.theme().colors;
        let targets = std::iter::once(LocalTarget::ThisMachine).chain(
            self.core
                .read(cx)
                .pairing_targets()
                .distros
                .iter()
                .cloned()
                .map(|distro| LocalTarget::Wsl { distro }),
        );
        let mut buttons: Vec<AnyElement> = targets
            .enumerate()
            .map(|(index, target)| {
                let label = match &target {
                    LocalTarget::ThisMachine => "Use the Forge on this computer".to_owned(),
                    LocalTarget::Wsl { distro } => format!("Use the Forge in WSL: {distro}"),
                };
                self.button(
                    &desktop,
                    format!("pair-{index}").into(),
                    label,
                    cx,
                    Some(target),
                )
            })
            .collect();
        if let Some(error) = self.core.read(cx).pairing_targets().error.clone() {
            buttons.push(editor_ui::page::error_text(cx.theme(), error).into_any_element());
            buttons.push(
                editor_ui::button::Button::text(
                    "retry-local-targets",
                    *cx.theme(),
                    "Retry WSL discovery",
                )
                .on_activate(cx.listener(|this, _, _, cx| {
                    this.core
                        .update(cx, |core, cx| core.load_pairing_targets(true, cx));
                }))
                .into_any_element(),
            );
        }
        buttons.push(self.button(
            &desktop,
            "pair-remote".into(),
            "Pair a Forge on another machine".to_owned(),
            cx,
            None,
        ));
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .debug_selector(|| "artisan-desktop-home".to_owned())
                    .pb(px(32.0))
                    .child(
                        div().mb(px(16.0)).child(
                            asset_glyph(AssetId::TABLER_TERMINAL_2)
                                .size(px(EMBLEM))
                                .text_color(desktop.text_muted.to_paint()),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(HEADLINE))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(desktop.text.to_paint())
                            .child("Add a host"),
                    )
                    .child(
                        div()
                            .mt(px(8.0))
                            .max_w(px(440.0))
                            .text_size(px(15.0))
                            .text_color(desktop.text_muted.to_paint())
                            .child(
                                "Artisan works on a Forge host you trust. Use the Forge on this computer, or pair with one on another machine.",
                            ),
                    )
                    .child(
                        div()
                            .mt(px(16.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(8.0))
                            .children(buttons),
                    )
                    .children(self.error.clone().map(|error| {
                        div()
                            .mt(px(16.0))
                            .max_w(px(440.0))
                            .text_size(px(15.0))
                            .text_color(desktop.destructive_text.to_paint())
                            .child(error)
                    })),
            )
            .into_any_element()
    }
}
