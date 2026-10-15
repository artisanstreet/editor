//! Engine capabilities, installation, sign-in and model defaults reported by Forge.

use crate::Settings;
use crate::chrome::{card, error_text, header, row, section_shell};
use crate::page;
use artisan_api::{
    EngineCommand, EngineConfig, EngineId, EngineStatus, InstallState, Integrity, SignInState,
};
use editor_core::{HostData, HostId};
use editor_ui::badge::Badge;
use editor_ui::button::ButtonVariant;
use editor_ui::text_editor::TextEditor;
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, Div, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
use std::collections::{BTreeSet, HashMap, HashSet};

const DEFAULT_CONFIG_KEY: &str = "default_engine_config";

pub(crate) struct EnginesForm {
    host: Option<HostId>,
    generation: u64,
    /// Pasted sign-in responses for engines advertising that capability.
    login_codes: HashMap<EngineId, Entity<TextEditor>>,
    /// Engines with a request in flight.
    busy: BTreeSet<EngineId>,
    errors: HashMap<EngineId, String>,
    /// Engines whose published versions are open ("Choose version").
    choosing_version: HashSet<EngineId>,
}

impl EnginesForm {
    pub(crate) fn new(_cx: &mut Context<Settings>) -> Self {
        Self {
            host: None,
            generation: 0,
            login_codes: HashMap::new(),
            busy: BTreeSet::new(),
            errors: HashMap::new(),
            choosing_version: HashSet::new(),
        }
    }

    pub(crate) fn sync(&mut self, host: Option<HostId>) {
        if self.host == host {
            return;
        }
        self.host = host;
        self.generation += 1;
        self.login_codes.clear();
        self.busy.clear();
        self.errors.clear();
        self.choosing_version.clear();
    }

    pub(crate) fn prepare(&mut self, engine: EngineId, cx: &mut Context<Settings>) {
        self.login_codes.entry(engine).or_insert_with(|| {
            cx.new(|cx| TextEditor::new("", "Paste the code", cx).single_line())
        });
    }

    pub(crate) fn clear_errors(&mut self) {
        self.errors.clear();
    }
}

impl Settings {
    fn engine_action(&mut self, engine: EngineId, action: EngineCommand, cx: &mut Context<Self>) {
        let Some(host) = self.selected_host(cx) else {
            return;
        };
        if self.engines.busy.contains(&engine) {
            return;
        }
        let host_id = host.read(cx).id.clone();
        let generation = self.engines.generation;
        let client = host.read(cx).client.clone();
        self.engines.errors.remove(&engine);
        self.engines.busy.insert(engine);
        cx.notify();
        let action_label = action.as_str();
        let action_verb = verb(&action);
        #[cfg(test)]
        let completion_gate = self.completion_gate.clone();
        cx.spawn(async move |this, cx| {
            let result = client.engine_action(engine, &action).await;
            #[cfg(test)]
            if let Some(gate) = completion_gate { gate.wait().await; }
            let _ = this.update(cx, |this, cx| {
                if this.engines.host.as_ref() != Some(&host_id) || this.engines.generation != generation { return; }
                this.engines.busy.remove(&engine);
                if let Err(error) = result {
                    tracing::warn!(%engine, action = action_label, "engine action failed: {error:#}");
                    this.engines.errors.insert(
                        engine,
                        format!(
                            "Couldn't {} {}: {error:#}",
                            action_verb,
                            page::engine_label(engine)
                        ),
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn submit_login_code(&mut self, engine: EngineId, cx: &mut Context<Self>) {
        let Some(editor) = self.engines.login_codes.get(&engine).cloned() else {
            return;
        };
        let code = editor.read(cx).text().trim().to_owned();
        if code.is_empty() {
            self.engines.errors.insert(
                engine,
                "Paste the code the sign-in page showed you first.".to_owned(),
            );
            cx.notify();
            return;
        }
        editor.update(cx, |editor, cx| editor.set_text("", cx));
        self.engine_action(engine, EngineCommand::LoginCode { code }, cx);
    }

    fn use_version(&mut self, engine: EngineId, version: &str, cx: &mut Context<Self>) {
        self.engines.choosing_version.remove(&engine);
        self.engine_action(
            engine,
            EngineCommand::Use {
                version: version.to_owned(),
            },
            cx,
        );
    }

    /// Opens or closes "Choose version"; opening asks Forge for the
    /// vendor's versions, which arrive in the engine's status.
    fn toggle_versions(&mut self, engine: EngineId, cx: &mut Context<Self>) {
        if self.engines.choosing_version.remove(&engine) {
            cx.notify();
            return;
        }
        self.engines.choosing_version.insert(engine);
        self.engine_action(engine, EngineCommand::Versions {}, cx);
    }

    /// Makes `model` the model new threads on this host start with.
    fn set_default_model(
        &mut self,
        host: &Entity<HostData>,
        engine: EngineId,
        model: String,
        cx: &mut Context<Self>,
    ) {
        if self.engines.busy.contains(&engine) {
            return;
        }
        let data = host.read(cx);
        let (client, host_id) = (data.client.clone(), data.id.clone());
        let generation = self.engines.generation;
        self.engines.busy.insert(engine);
        self.engines.errors.remove(&engine);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.set_default_model(engine, &model).await;
            let _ = this.update(cx, |this, cx| {
                if this.engines.host.as_ref() != Some(&host_id)
                    || this.engines.generation != generation
                {
                    return;
                }
                this.engines.busy.remove(&engine);
                if let Err(error) = result {
                    this.engines.errors.insert(
                        engine,
                        format!("Couldn't save the default model: {error:#}"),
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn verb(action: &EngineCommand) -> &'static str {
    match action {
        EngineCommand::Install {} => "install",
        EngineCommand::Update {} => "update",
        EngineCommand::Rollback {} => "roll back",
        EngineCommand::Use { .. } => "switch the version of",
        EngineCommand::Versions {} => "list the published versions of",
        EngineCommand::Login {} => "start signing in to",
        EngineCommand::LoginCode { .. } => "finish signing in to",
        EngineCommand::CancelLogin {} => "cancel signing in to",
        EngineCommand::Logout {} => "sign out of",
        EngineCommand::Refresh {} => "refresh",
    }
}

/// The engine's page.
pub(crate) fn render(
    settings: &Settings,
    host: &Entity<HostData>,
    engine: EngineId,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> Div {
    let label = page::engine_label(engine);
    let mut page = div().flex().flex_col().child(header(
        theme,
        label,
        format!(
            "Choose where {label} appears, manage its installation, and inspect its account and models."
        ),
    ));
    let data = host.read(cx);
    let Some(status) = data.engines.get(&engine).cloned() else {
        let note = if data.is_live() {
            format!("This host reported no {label} engine.")
        } else {
            "Waiting for the host to connect\u{2026}".to_owned()
        };
        return page.child(
            div()
                .mt(px(32.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(note),
        );
    };
    let default_model = data
        .setting(DEFAULT_CONFIG_KEY)
        .and_then(|value| serde_json::from_value::<EngineConfig>(value.clone()).ok())
        .filter(|config| config.engine == engine)
        .and_then(|config| config.model);
    let busy = settings.engines.busy.contains(&engine);
    let availability = card(
        theme,
        vec![
            row(
                theme,
                "Availability",
                "Whether this engine can run models. Native engines follow their installed CLI and account state; there is no separate switch.",
                Some(
                    div()
                        .flex_shrink_0()
                        .child(Badge::new(
                            *theme,
                            availability_badge(&status),
                        ))
                        .into_any_element(),
                ),
            )
            .into_any_element(),
        ],
    );
    page = page
        .child(section_shell(
            theme,
            "availability",
            "Availability",
            None,
            None,
            availability,
        ))
        .child(render_installation(settings, &status, busy, theme, cx))
        .child(render_account(settings, &status, busy, theme, cx));
    if let Some(error) = settings.engines.errors.get(&engine) {
        page = page.child(div().mt(px(12.0)).child(error_text(theme, error.clone())));
    }
    page.child(render_models(
        host,
        &status,
        default_model.as_deref(),
        theme,
        cx,
    ))
}

/// The badge beside Availability.
fn availability_badge(status: &EngineStatus) -> &'static str {
    match (&status.install, &status.sign_in) {
        (InstallState::Installed { .. }, SignInState::SignedIn { .. }) => "Available",
        (InstallState::Installed { .. }, SignInState::SignedOut | SignInState::Failed { .. }) => {
            "Sign-in required"
        }
        (InstallState::Absent | InstallState::Failed { .. }, _) => "Unavailable",
        _ => "Checking",
    }
}

/// A plain-text action at a section heading or in a row (`live_action`).
fn text_action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    disabled: bool,
    theme: &ArtisanTheme,
    on_click: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let id = id.into();
    editor_ui::button::Button::text(id.clone(), *theme, label)
        .variant(ButtonVariant::Ghost)
        .disabled(disabled)
        .debug_selector(id)
        .on_activate(on_click)
        .into_any_element()
}

/// The Refresh action of Installation and Account.
fn refresh_action(
    engine: EngineId,
    selector: &'static str,
    busy: bool,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> AnyElement {
    text_action(
        selector,
        if busy { "Checking\u{2026}" } else { "Refresh" },
        busy,
        theme,
        cx.listener(move |this, _, _, cx| {
            this.engine_action(engine, EngineCommand::Refresh {}, cx)
        }),
    )
}

/// A band of muted copy inside a card.
fn note(theme: &ArtisanTheme, text: impl Into<SharedString>) -> AnyElement {
    editor_ui::page::note(theme, text).into_any_element()
}

/// The first band: the state in one foreground sentence.
fn state_band(theme: &ArtisanTheme, selector: &'static str, text: String) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .py(px(20.0))
        .child(
            div()
                .debug_selector(move || selector.to_owned())
                .text_sm()
                .text_color(theme.colors.foreground.to_paint())
                .child(text),
        )
        .into_any_element()
}

fn installation_copy(status: &EngineStatus) -> String {
    let label = page::engine_label(status.engine);
    match &status.install {
        InstallState::Absent => format!("{label} is not installed yet."),
        InstallState::Downloading {
            version,
            received,
            total,
        } => match total.filter(|total| *total > 0) {
            Some(total) => format!(
                "Installing {label} {version}\u{2026} {}%",
                received.saturating_mul(100) / total
            ),
            None => format!("Installing {label} {version}\u{2026}"),
        },
        InstallState::Verifying { version } => format!("Verifying {label} {version}\u{2026}"),
        InstallState::Installed { version, .. } => {
            format!("{label} {version} is installed and managed by this Forge.")
        }
        InstallState::Failed { message } => {
            if message.is_empty() {
                format!("{label} could not be installed.")
            } else {
                message.clone()
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
fn render_installation(
    settings: &Settings,
    status: &EngineStatus,
    busy: bool,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> Div {
    let engine = status.engine;
    let label = page::engine_label(engine);
    let mut blocks = vec![state_band(
        theme,
        "settings-installation-status",
        installation_copy(status),
    )];
    match &status.install {
        InstallState::Absent | InstallState::Failed { .. } => {
            let install = settings.button(
                SharedString::from(format!("settings-engine-install-{engine}")),
                if matches!(status.install, InstallState::Absent) {
                    "Install"
                } else {
                    "Try again"
                },
                ButtonVariant::Default,
                busy,
                cx.listener(move |this, _, _, cx| {
                    this.engine_action(engine, EngineCommand::Install {}, cx);
                }),
            );
            blocks.push(
                row(
                    theme,
                    "Version",
                    format!("Artisan installs and updates {label} in its own home on this host."),
                    Some(install),
                )
                .into_any_element(),
            );
            if let Some(integrity) = status.integrity {
                blocks.push(note(theme, integrity_copy(integrity)));
            }
        }
        InstallState::Installed { version, update } => {
            let mut actions = div().flex_shrink_0().flex().flex_row().gap(px(8.0));
            if let Some(update) = update {
                actions = actions.child(text_action(
                    "settings-engine-update",
                    format!("Update to {update}"),
                    busy,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.engine_action(engine, EngineCommand::Update {}, cx);
                    }),
                ));
            }
            actions = actions
                .child(text_action(
                    "settings-engine-rollback",
                    "Roll back",
                    busy,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.engine_action(engine, EngineCommand::Rollback {}, cx);
                    }),
                ))
                .child(text_action(
                    "settings-engine-versions",
                    "Choose version",
                    busy,
                    theme,
                    cx.listener(move |this, _, _, cx| this.toggle_versions(engine, cx)),
                ));
            let selection = match update {
                Some(update) => format!("Follows the latest release; {update} is available."),
                None => format!("Follows the latest release ({version})."),
            };
            blocks.push(
                row(
                    theme,
                    "Version",
                    selection,
                    Some(actions.into_any_element()),
                )
                .into_any_element(),
            );
            if let Some(integrity) = status.integrity {
                blocks.push(note(theme, integrity_copy(integrity)));
            }
            if settings.engines.choosing_version.contains(&engine) {
                blocks.push(if status.versions.is_empty() {
                    note(
                        theme,
                        if busy {
                            "Loading published versions\u{2026}"
                        } else {
                            "The vendor lists no versions."
                        },
                    )
                } else {
                    version_list(status, busy, theme, cx)
                });
            }
        }
        InstallState::Downloading { .. } | InstallState::Verifying { .. } => {
            if let Some(integrity) = status.integrity {
                blocks.push(note(theme, integrity_copy(integrity)));
            }
        }
    }
    section_shell(
        theme,
        "installation",
        "Installation",
        None,
        Some(refresh_action(
            engine,
            "settings-installation-refresh",
            busy,
            theme,
            cx,
        )),
        card(theme, blocks),
    )
}

/// How this engine's downloads are verified, the weaker guarantee said
/// plainly (`integrity_copy`).
fn integrity_copy(integrity: Integrity) -> &'static str {
    match integrity {
        Integrity::VendorSha256 => "Verified by the vendor's published checksum.",
        Integrity::NpmSha512 => {
            "Verified by the checksum the npm registry publishes for each release."
        }
        Integrity::HttpsOnly => {
            "Downloaded over HTTPS from the vendor, unverified: the vendor publishes no checksum. The vendor publishes no version list either, so Artisan offers the latest release and the versions this Forge has installed."
        }
    }
}

/// The published versions, newest first, each with where it stands here
/// and a Use action (`version_list`).
fn version_list(
    status: &EngineStatus,
    busy: bool,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> AnyElement {
    let engine = status.engine;
    let mut list = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .py(px(12.0))
        .debug_selector(|| "settings-engine-version-list".to_owned());
    for entry in status.versions.iter() {
        let mut marks = Vec::new();
        if entry.current {
            marks.push("current");
        } else if entry.installed {
            marks.push("installed");
        }
        if entry.unsupported {
            marks.push("older than Artisan supports");
        }
        let label = if marks.is_empty() {
            entry.version.clone()
        } else {
            format!("{} \u{b7} {}", entry.version, marks.join(" \u{b7} "))
        };
        let mut row = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_sm()
                    .text_color(if entry.unsupported {
                        theme.colors.muted_foreground.to_paint()
                    } else {
                        theme.colors.foreground.to_paint()
                    })
                    .child(label),
            );
        if !entry.current && !entry.unsupported {
            let version = entry.version.clone();
            row = row.child(text_action(
                format!("settings-engine-use-{}", entry.version),
                format!("Use {}", entry.version),
                busy,
                theme,
                cx.listener(move |this, _, _, cx| this.use_version(engine, &version, cx)),
            ));
        }
        list = list.child(row);
    }
    list.into_any_element()
}

fn account_copy(status: &EngineStatus) -> String {
    match &status.sign_in {
        SignInState::SignedIn {
            account: Some(account),
        } => format!("Signed in as {account}."),
        SignInState::SignedIn { account: None } => "Signed in.".to_owned(),
        SignInState::SignedOut => "No account is signed in.".to_owned(),
        SignInState::LoggingIn { .. } => "Signing in\u{2026}".to_owned(),
        SignInState::Unknown => "Sign-in status unknown.".to_owned(),
        SignInState::Failed { message } => format!("Account status unavailable: {message}."),
    }
}

#[allow(clippy::too_many_lines)]
fn render_account(
    settings: &Settings,
    status: &EngineStatus,
    busy: bool,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> Div {
    let engine = status.engine;
    let mut blocks = vec![state_band(
        theme,
        "settings-account-status",
        account_copy(status),
    )];
    let installed = matches!(status.install, InstallState::Installed { .. });
    match &status.sign_in {
        SignInState::SignedOut | SignInState::Failed { .. } if installed => {
            blocks.push(
                row(
                    theme,
                    "Sign in",
                    format!(
                        "{} can't run until you sign in.",
                        page::engine_label(engine)
                    ),
                    Some(settings.button(
                        SharedString::from(format!("settings-engine-login-{engine}")),
                        "Sign in",
                        ButtonVariant::Default,
                        busy,
                        cx.listener(move |this, _, _, cx| {
                            this.engine_action(engine, EngineCommand::Login {}, cx);
                        }),
                    )),
                )
                .into_any_element(),
            );
        }
        SignInState::SignedIn { .. } => {
            blocks.push(
                row(
                    theme,
                    "Sign out",
                    "Signs this host out of the account.",
                    Some(settings.button(
                        SharedString::from(format!("settings-engine-logout-{engine}")),
                        "Sign out",
                        ButtonVariant::Outline,
                        busy,
                        cx.listener(move |this, _, _, cx| {
                            this.engine_action(engine, EngineCommand::Logout {}, cx);
                        }),
                    )),
                )
                .into_any_element(),
            );
        }
        SignInState::LoggingIn {
            url,
            code,
            accepts_code,
        } => {
            let mut block = div().flex().flex_col().gap(px(8.0)).py(px(14.0));
            if let Some(url) = url {
                let link = url.clone();
                block = block.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child(format!("Open {url}")),
                        )
                        .child(settings.button(
                            SharedString::from(format!("settings-engine-open-{engine}")),
                            "Open link",
                            ButtonVariant::Outline,
                            false,
                            move |_, _, cx| cx.open_url(&link),
                        )),
                );
            }
            if let Some(code) = code {
                block = block.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(6.0))
                        .text_sm()
                        .child(
                            div()
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child("and enter"),
                        )
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(code.clone())),
                );
            }
            // The adapter declares whether this login accepts a returned code.
            if *accepts_code && let Some(field) = settings.engines.login_codes.get(&engine).cloned()
            {
                block = block.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(div().flex_1().child(field))
                        .child(settings.button(
                            SharedString::from(format!("settings-engine-code-{engine}")),
                            "Submit code",
                            ButtonVariant::Default,
                            busy,
                            cx.listener(move |this, _, _, cx| this.submit_login_code(engine, cx)),
                        )),
                );
            }
            block = block.child(div().child(settings.button(
                SharedString::from(format!("settings-engine-cancel-login-{engine}")),
                "Cancel",
                ButtonVariant::Ghost,
                false,
                cx.listener(move |this, _, _, cx| {
                    this.engine_action(engine, EngineCommand::CancelLogin {}, cx);
                }),
            )));
            blocks.push(block.into_any_element());
        }
        _ => {}
    }
    section_shell(
        theme,
        "account",
        "Account",
        None,
        Some(refresh_action(
            engine,
            "settings-usage-refresh",
            busy,
            theme,
            cx,
        )),
        card(theme, blocks),
    )
}

fn render_models(
    host: &Entity<HostData>,
    status: &EngineStatus,
    default_model: Option<&str>,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> Div {
    let engine = status.engine;
    let label = page::engine_label(engine);
    let mut blocks = Vec::new();
    // Signed-out engines say "sign in" before any "no models".
    if !matches!(status.sign_in, SignInState::SignedIn { .. }) {
        blocks.push(note(
            theme,
            format!("Sign in to {label} to see its models."),
        ));
    } else if status.models.is_empty() {
        blocks.push(
            div()
                .w_full()
                .py(px(20.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(format!("No models are listed for {label} yet."))
                .into_any_element(),
        );
    } else {
        for model in status.models.iter() {
            let is_default = default_model == Some(model.id.as_str());
            let selector = format!("settings-engine-model-{}", model.id);
            let mut model_row = div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone())
                .role(gpui::Role::Button)
                .aria_label(model.id.clone())
                .aria_selected(is_default)
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                .py(px(10.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.colors.foreground.to_paint())
                                .child(model.id.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child(label),
                        ),
                );
            if is_default {
                model_row =
                    model_row.child(div().flex_shrink_0().child(Badge::new(*theme, "Default")));
            } else {
                let (host, id) = (host.clone(), model.id.clone());
                model_row = model_row
                    .focusable()
                    .tab_index(0)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_default_model(&host, engine, id.clone(), cx);
                    }));
            }
            blocks.push(model_row.into_any_element());
        }
        blocks.push(
            div()
                .w_full()
                .py(px(12.0))
                .text_xs()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(format!(
                    "Choosing a model makes it the default for new {label} threads on this host."
                ))
                .into_any_element(),
        );
    }
    section_shell(theme, "models", "Models", None, None, card(theme, blocks))
}

#[cfg(test)]
mod delayed_action_tests {
    use super::*;
    use crate::tests::{CompletionGate, SettingsFixture, finish_gate, wait_gate};
    use gpui::TestAppContext;
    #[gpui::test]
    fn old_engine_failure_cannot_clear_new_hosts_busy_or_error(cx: &mut TestAppContext) {
        let fixture = SettingsFixture::new(cx);
        let (settings, cx) = fixture.open(cx, editor_core::SettingsPage::Engines);
        let gate = CompletionGate::new();
        settings.update(cx, |settings, cx| {
            settings.completion_gate = Some(gate.clone());
            settings.engine_action(
                EngineId::Claude,
                EngineCommand::Use {
                    version: "missing-test-version".into(),
                },
                cx,
            );
        });
        wait_gate(cx, &gate);
        fixture.select(1, editor_core::SettingsPage::Engines, cx);
        settings.update(cx, |settings, _| {
            settings.engines.busy.insert(EngineId::Claude);
            settings
                .engines
                .errors
                .insert(EngineId::Claude, "B owns this error".into());
        });
        finish_gate(cx, &gate);
        settings.read_with(cx, |settings, _| {
            assert_eq!(settings.engines.host.as_ref(), Some(&fixture.hosts[1]));
            assert!(settings.engines.busy.contains(&EngineId::Claude));
            assert_eq!(
                settings
                    .engines
                    .errors
                    .get(&EngineId::Claude)
                    .map(String::as_str),
                Some("B owns this error")
            );
        });
    }
}
