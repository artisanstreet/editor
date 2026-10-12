//! The first screen, the previous Editor's "Add a host" branch of
//! `new_thread_surface_section`: the terminal emblem, the heading, what a
//! host is, and the ways to add one. A Forge on this machine or in WSL
//! pairs with one click (`ae forge pair --json` runs there); a remote one is
//! pasted in Settings → Hosts.

use editor_core::{Core, LocalTarget, Screen, SettingsPage, hosts};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::theme::DesktopTheme;
use gpui::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px,
};

/// The heading under the emblem (`HOME_HEADLINE_TEXT_PX`).
const HEADLINE: f32 = 28.0;
/// The muted terminal emblem (`HOME_EMBLEM_SIZE_PX`).
const EMBLEM: f32 = 30.0;

pub struct Home {
    core: Entity<Core>,
    targets: Vec<LocalTarget>,
    pairing: Option<LocalTarget>,
    error: Option<String>,
}

impl Home {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        if cfg!(windows) {
            let distros = cx.background_executor().spawn(async { wsl_distros() });
            cx.spawn(async move |this, cx| {
                let distros = distros.await;
                let _ = this.update(cx, |home, cx| {
                    home.targets.extend(
                        distros
                            .into_iter()
                            .map(|distro| LocalTarget::Wsl { distro }),
                    );
                    cx.notify();
                });
            })
            .detach();
        }
        Self {
            core,
            targets: vec![LocalTarget::ThisMachine],
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
        self.pairing = Some(target.clone());
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = hosts::pair_local(target, &identity, &device_name).await;
            let _ = this.update(cx, |home, cx| {
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
            });
        })
        .detach();
    }

    fn button(
        &self,
        theme: DesktopTheme,
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
            .debug_selector(move || selector.clone())
            .cursor_pointer()
            .px(px(16.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(theme.line)
            .text_color(theme.foreground)
            .child(label);
        match target {
            Some(target) => {
                button.on_click(cx.listener(move |home, _, _, cx| home.pair(target.clone(), cx)))
            }
            None => button.on_click(cx.listener(|home, _, _, cx| {
                home.core.update(cx, |core, cx| {
                    core.navigate(Screen::Settings(SettingsPage::Hosts), cx)
                });
            })),
        }
        .into_any_element()
    }
}

impl Render for Home {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::neutral_dark();
        let mut buttons: Vec<AnyElement> = self
            .targets
            .iter()
            .enumerate()
            .map(|(index, target)| {
                let label = match target {
                    LocalTarget::ThisMachine => "Use the Forge on this computer".to_owned(),
                    LocalTarget::Wsl { distro } => format!("Use the Forge in WSL: {distro}"),
                };
                self.button(
                    theme,
                    format!("pair-{index}").into(),
                    label,
                    cx,
                    Some(target.clone()),
                )
            })
            .collect();
        buttons.push(self.button(
            theme,
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
                                .text_color(theme.secondary),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(HEADLINE))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(theme.foreground)
                            .child("Add a host"),
                    )
                    .child(
                        div()
                            .mt(px(8.0))
                            .max_w(px(440.0))
                            .text_size(px(15.0))
                            .text_color(theme.secondary)
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
                            .text_color(gpui::rgb(0x00f8_7171))
                            .child(error)
                    })),
            )
    }
}

/// Installed WSL distributions. `wsl.exe -l -q` prints UTF-16.
fn wsl_distros() -> Vec<String> {
    let Ok(output) = std::process::Command::new("wsl.exe")
        .args(["-l", "-q"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let units: Vec<u16> = output
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
        .lines()
        .map(|line| {
            line.trim_matches(|c: char| c.is_whitespace() || c == '\0')
                .to_owned()
        })
        .filter(|line| !line.is_empty() && !line.starts_with("docker-desktop"))
        .collect()
}
