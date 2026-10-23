//! An engine's installation and sign-in, as Forge reports them.

use crate::Settings;
use crate::page;
use crate::ui::{PageBlock, card, error_text, header, row, section_shell};
use artisan_api::{EngineCommand, EngineId, EngineStatus, InstallState, Integrity, SignInState};
use editor_core::{HostData, HostId};
use editor_ui::button::ButtonVariant;
use editor_ui::text_editor::TextEditor;
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, Div, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
};
use std::collections::{BTreeSet, HashMap, HashSet};

pub struct EnginesForm {
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
    /// Signs the host's Claude in to claude.ai: a browser on this device
    /// captures the session, and the host's Forge checks and keeps it.
    pub(crate) fn sign_in_to_claude_ai(&mut self, cx: &mut Context<Self>) {
        let engine = EngineId::Claude;
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
        let signing_in = editor_core::runtime::runtime().spawn(async move {
            let session = claude_ai_browser::capture(claude_ai_browser::Mode::SignIn).await?;
            client
                .engine_action(engine, &EngineCommand::ClaudeAiSignIn { session })
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = match signing_in.await {
                Ok(result) => result,
                Err(error) => Err(anyhow::anyhow!("the sign-in stopped: {error}")),
            };
            this.update(cx, |this, cx| {
                if this.engines.host.as_ref() != Some(&host_id)
                    || this.engines.generation != generation
                {
                    return;
                }
                this.engines.busy.remove(&engine);
                if let Err(error) = result {
                    tracing::warn!("claude.ai sign-in failed: {error:#}");
                    this.engines
                        .errors
                        .insert(engine, format!("Couldn't sign in to claude.ai: {error:#}"));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn engine_action(
        &mut self,
        engine: EngineId,
        action: EngineCommand,
        cx: &mut Context<Self>,
    ) {
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
            this.update(cx, |this, cx| {
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
            }).ok();
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
}

const fn verb(action: &EngineCommand) -> &'static str {
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
        EngineCommand::ClaudeAiSignIn { .. } => "sign in to claude.ai for",
        EngineCommand::ClaudeAiSignOut {} => "sign out of claude.ai for",
    }
}

/// The engine's page.
pub fn render(
    settings: &Settings,
    host: &Entity<HostData>,
    engine: EngineId,
    theme: &ArtisanTheme,
    cx: &Context<Settings>,
) -> Div {
    let label = page::engine_label(engine);
    let mut page = div().flex().flex_col().child(header(
        theme,
        label,
        format!("{label}'s installation and sign-in on this host."),
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
                .text_color(theme.colors.text_muted.to_paint())
                .child(note),
        );
    };
    let busy = settings.engines.busy.contains(&engine);
    page = page
        .child(render_installation(settings, &status, busy, theme, cx))
        .child(render_account(settings, &status, busy, theme, cx));
    if let Some(error) = settings.engines.errors.get(&engine) {
        page = page.child(div().mt(px(12.0)).child(error_text(theme, error.clone())));
    }
    page
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
    cx: &Context<Settings>,
) -> AnyElement {
    text_action(
        selector,
        if busy { "Checking\u{2026}" } else { "Refresh" },
        busy,
        theme,
        cx.listener(move |this, _, _, cx| {
            this.engine_action(engine, EngineCommand::Refresh {}, cx);
        }),
    )
}

/// A band of muted copy inside a card.
fn note(theme: &ArtisanTheme, text: impl Into<SharedString>) -> AnyElement {
    crate::ui::note(theme, text).into_any_element()
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
                .text_color(theme.colors.text.to_paint())
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
        } => total.filter(|total| *total > 0).map_or_else(
            || format!("Installing {label} {version}\u{2026}"),
            |total| {
                format!(
                    "Installing {label} {version}\u{2026} {}%",
                    received.saturating_mul(100) / total
                )
            },
        ),
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

#[expect(
    clippy::too_many_lines,
    reason = "one card that lays out every installation state in order"
)]
fn render_installation(
    settings: &Settings,
    status: &EngineStatus,
    busy: bool,
    theme: &ArtisanTheme,
    cx: &Context<Settings>,
) -> PageBlock {
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
            let selection = update.as_ref().map_or_else(
                || format!("Follows the latest release ({version})."),
                |update| format!("Follows the latest release; {update} is available."),
            );
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
    .debug_selector("settings-section-installation")
}

/// How this engine's downloads are verified, the weaker guarantee said
/// plainly (`integrity_copy`).
const fn integrity_copy(integrity: Integrity) -> &'static str {
    match integrity {
        Integrity::VendorSha256 => "Verified by the vendor's published checksum.",
        Integrity::NpmSha512 => {
            "Verified by the checksum the npm registry publishes for each release."
        }
        Integrity::ArtisanSha256 => {
            "Built by Artisan Street from the vendor's release and verified by its published checksum."
        }
    }
}

/// The published versions, newest first, each with where it stands here
/// and a Use action (`version_list`).
fn version_list(
    status: &EngineStatus,
    busy: bool,
    theme: &ArtisanTheme,
    cx: &Context<Settings>,
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
                        theme.colors.text_muted.to_paint()
                    } else {
                        theme.colors.text.to_paint()
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
    if status.engine == EngineId::OpenCode {
        match &status.sign_in {
            SignInState::SignedIn {
                account: Some(gateways),
            } => return format!("Signed in to {gateways}."),
            SignInState::SignedOut => {
                return "Not signed in. OpenCode's free models still work.".to_owned();
            }
            SignInState::Unknown
            | SignInState::LoggingIn { .. }
            | SignInState::SignedIn { .. }
            | SignInState::Failed { .. } => {}
        }
    }
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

#[expect(
    clippy::too_many_lines,
    reason = "one card that lays out every sign-in state in order"
)]
fn render_account(
    settings: &Settings,
    status: &EngineStatus,
    busy: bool,
    theme: &ArtisanTheme,
    cx: &Context<Settings>,
) -> PageBlock {
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
                                .text_color(theme.colors.text_muted.to_paint())
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
                                .text_color(theme.colors.text_muted.to_paint())
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
        SignInState::Unknown | SignInState::SignedOut | SignInState::Failed { .. } => {}
    }
    if engine == EngineId::Claude {
        blocks.push(claude_ai_session(settings, status, busy, theme, cx));
    }
    section_shell(
        theme,
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
    .debug_selector("settings-section-account")
}

/// The claude.ai session Native Claude Mode runs on: where it stands, and
/// signing in or out. Signing in opens a browser on this device.
fn claude_ai_session(
    settings: &Settings,
    status: &EngineStatus,
    busy: bool,
    theme: &ArtisanTheme,
    cx: &Context<Settings>,
) -> AnyElement {
    use artisan_api::ClaudeAiState;
    let state = status.claude_ai.clone().unwrap_or(ClaudeAiState::SignedOut);
    let (mut description, signed_in) = match &state {
        ClaudeAiState::SignedOut => (
            "Not signed in. Signing in opens claude.ai in a browser window; it closes by itself once you're in.".to_owned(),
            false,
        ),
        ClaudeAiState::SignedIn { account } => (
            account.as_ref().map_or_else(
                || "Signed in.".to_owned(),
                |account| format!("Signed in as {account}."),
            ),
            true,
        ),
        ClaudeAiState::NeedsRenewal { .. } => (
            if claude_ai_browser::can_renew() {
                "claude.ai asked for a browser check. Artisan is renewing the session in the background.".to_owned()
            } else {
                "claude.ai asked for a browser check. Sign in again on this device, or wait for the device that signed in to renew it.".to_owned()
            },
            false,
        ),
        ClaudeAiState::Expired { reason } => {
            (format!("The session stopped working: {reason}"), false)
        }
    };
    if busy {
        "Waiting for you to sign in to claude.ai in the browser window\u{2026}"
            .clone_into(&mut description);
    }
    let button = if signed_in {
        settings.button(
            "settings-claude-ai-sign-out",
            "Sign out",
            ButtonVariant::Outline,
            busy,
            cx.listener(|this, _, _, cx| {
                this.engine_action(EngineId::Claude, EngineCommand::ClaudeAiSignOut {}, cx);
            }),
        )
    } else {
        settings.button(
            "settings-claude-ai-sign-in",
            if matches!(state, ClaudeAiState::SignedOut) {
                "Sign in"
            } else {
                "Sign in again"
            },
            ButtonVariant::Default,
            busy,
            cx.listener(|this, _, _, cx| this.sign_in_to_claude_ai(cx)),
        )
    };
    row(theme, "claude.ai session (Native Claude Mode)", description, Some(button))
        .into_any_element()
}

#[cfg(test)]
mod delayed_action_tests {
    use super::*;
    use crate::tests::{CompletionGate, SettingsFixture, finish_gate, wait_gate};
    use gpui::TestAppContext;
    #[gpui::test]
    fn old_engine_failure_cannot_clear_new_hosts_busy_or_error(cx: &mut TestAppContext) {
        let fixture = SettingsFixture::new(cx);
        let (settings, cx) = fixture.open(cx, editor_core::SettingsPage::Engine(EngineId::Claude));
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
        fixture.select(1, editor_core::SettingsPage::Engine(EngineId::Claude), cx);
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
