use super::{
    controller::SettingsController,
    page,
    ui::{self, Ink},
    view::ViewAction,
};
use aqua_ui::element::{Box as UiBox, Element};
use artisan_api::{
    ClaudeAiState, EngineCommand, EngineId, EngineStatus, InstallState, Integrity, SignInState,
};
use editor_core::{HostData, Problem, SettingsPage, SettingsSection};
use editor_ui::{
    kit::{ButtonTone, Space, Spaced, TextRole},
    switch::Switch,
    text::{
        Mode,
        mount::{EditorController, EditorEvent},
    },
    theme::ArtisanTheme,
};
use std::rc::Rc;

fn button(
    theme: &ArtisanTheme,
    id: impl Into<String>,
    label: impl Into<String>,
    tone: ButtonTone,
    disabled: bool,
    action: ViewAction,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    ui::button(theme, id, label, tone, disabled, move || {
        emit(action.clone());
    })
}
fn band(theme: &ArtisanTheme, value: String) -> Element {
    ui::block(ui::text(theme, TextRole::Label, Ink::Plain, value))
}
fn failure(theme: &ArtisanTheme, key: String, headline: String, detail: &str) -> Element {
    let problem = if detail.trim().is_empty() {
        Problem::new(headline)
    } else {
        Problem::detailed(headline, detail)
    };
    ui::block(ui::problem(theme, key, &problem).build())
}
fn refresh(
    theme: &ArtisanTheme,
    id: &str,
    engine: EngineId,
    busy: bool,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    button(
        theme,
        id,
        if busy { "Checking…" } else { "Refresh" },
        ButtonTone::Neutral,
        busy,
        ViewAction::EngineAction(engine, EngineCommand::Refresh {}),
        emit,
    )
}
fn integrity(value: Integrity) -> &'static str {
    match value {
        Integrity::VendorSha256 => "Verified by the vendor's published checksum.",
        Integrity::NpmSha512 => {
            "Verified by the checksum the npm registry publishes for each release."
        }
        Integrity::ArtisanSha256 => {
            "Built by Artisan Street from the vendor's release and verified by its published checksum."
        }
    }
}
fn installation_copy(status: &EngineStatus) -> String {
    let label = status.engine.display_name();
    match &status.install {
        InstallState::Absent => format!("{label} is not installed yet."),
        InstallState::Downloading {
            version,
            received,
            total,
        } => total.filter(|total| *total > 0).map_or_else(
            || format!("Installing {label} {version}…"),
            |total| {
                format!(
                    "Installing {label} {version}… {}%",
                    received.saturating_mul(100) / total
                )
            },
        ),
        InstallState::Verifying { version } => format!("Verifying {label} {version}…"),
        InstallState::Installed { version, .. } => {
            format!("{label} {version} is installed and managed by this Forge.")
        }
        InstallState::Failed { .. } => format!("{label} couldn't be installed."),
    }
}
#[expect(
    clippy::too_many_lines,
    reason = "the engine installation block renders each install state as one branch of a single layout"
)]
fn installation(
    controller: &SettingsController,
    status: &EngineStatus,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let engine = status.engine;
    let busy = controller.engines.busy.contains(&engine);
    let mut blocks = vec![match &status.install {
        InstallState::Failed { message } => failure(
            theme,
            format!("settings-engine-install-error-{engine}"),
            installation_copy(status),
            message,
        ),
        InstallState::Absent
        | InstallState::Downloading { .. }
        | InstallState::Verifying { .. }
        | InstallState::Installed { .. } => band(theme, installation_copy(status)),
    }];
    match &status.install {
        InstallState::Absent | InstallState::Failed { .. } => {
            let control = button(
                theme,
                format!("settings-engine-install-{engine}"),
                if matches!(status.install, InstallState::Absent) {
                    "Install"
                } else {
                    "Try again"
                },
                if matches!(status.install, InstallState::Absent) {
                    ButtonTone::Primary
                } else {
                    ButtonTone::Neutral
                },
                busy,
                ViewAction::EngineAction(engine, EngineCommand::Install {}),
                Rc::clone(&emit),
            );
            blocks.push(ui::row(
                theme,
                "Version",
                format!(
                    "Artisan installs and updates {} in its own home on this Forge.",
                    engine.display_name()
                ),
                Some(control),
            ));
        }
        InstallState::Installed { version, update } => {
            let mut controls = ui::actions();
            if let Some(update) = update {
                controls = controls.child(button(
                    theme,
                    "settings-engine-update",
                    format!("Update to {update}"),
                    ButtonTone::Primary,
                    busy,
                    ViewAction::EngineAction(engine, EngineCommand::Update {}),
                    Rc::clone(&emit),
                ));
            }
            controls = controls
                .child(button(
                    theme,
                    "settings-engine-rollback",
                    "Roll back",
                    ButtonTone::Neutral,
                    busy,
                    ViewAction::EngineAction(engine, EngineCommand::Rollback {}),
                    Rc::clone(&emit),
                ))
                .child(button(
                    theme,
                    "settings-engine-versions",
                    "Choose version",
                    ButtonTone::Neutral,
                    busy,
                    ViewAction::EngineVersions(engine),
                    Rc::clone(&emit),
                ));
            let detail = update.as_ref().map_or_else(
                || format!("Follows the latest release ({version})."),
                |update| format!("Follows the latest release; {update} is available."),
            );
            blocks.push(ui::row(theme, "Version", detail, Some(controls.build())));
        }
        InstallState::Downloading { .. } | InstallState::Verifying { .. } => {}
    }
    if let Some(value) = status.integrity {
        blocks.push(ui::note(theme, integrity(value)));
    }
    if matches!(status.install, InstallState::Installed { .. })
        && controller.engines.choosing_version.contains(&engine)
    {
        if status.versions.is_empty() {
            blocks.push(ui::note(
                theme,
                if busy {
                    "Loading published versions…"
                } else {
                    "The vendor lists no versions."
                },
            ));
        } else {
            let mut list = ui::stack(Space::S4);
            for version in status.versions.iter() {
                let mut marks = Vec::new();
                if version.current {
                    marks.push("current");
                } else if version.installed {
                    marks.push("installed");
                }
                if version.unsupported {
                    marks.push("older than Artisan supports");
                }
                let label = if marks.is_empty() {
                    version.version.clone()
                } else {
                    format!("{} · {}", version.version, marks.join(" · "))
                };
                let ink = if version.unsupported {
                    Ink::Muted
                } else {
                    Ink::Plain
                };
                let mut row =
                    ui::actions().child(ui::grow(ui::text(theme, TextRole::Label, ink, label)));
                if !version.current && !version.unsupported {
                    row = row.child(button(
                        theme,
                        format!("settings-engine-use-{}", version.version),
                        format!("Use {}", version.version),
                        ButtonTone::Neutral,
                        busy,
                        ViewAction::EngineUse(engine, version.version.clone()),
                        Rc::clone(&emit),
                    ));
                }
                list = list.child(row.build());
            }
            blocks.push(ui::block(list.build()));
        }
    }
    ui::section(
        theme,
        "Installation",
        None,
        Some(refresh(
            theme,
            "settings-installation-refresh",
            engine,
            busy,
            emit,
        )),
        ui::card(theme, blocks),
    )
}
fn account_copy(status: &EngineStatus) -> String {
    if status.engine == EngineId::OpenCode {
        match &status.sign_in {
            SignInState::SignedIn {
                account: Some(gateways),
            } => return format!("Signed in to {gateways}."),
            SignInState::SignedOut => {
                return "Not signed in. OpenCode's free models still work.".into();
            }
            SignInState::Unknown
            | SignInState::LoggingIn { .. }
            | SignInState::SignedIn { account: None }
            | SignInState::Failed { .. } => {}
        }
    }
    match &status.sign_in {
        SignInState::SignedIn {
            account: Some(account),
        } => format!("Signed in as {account}."),
        SignInState::SignedIn { account: None } => "Signed in.".into(),
        SignInState::SignedOut => "No account is signed in.".into(),
        SignInState::LoggingIn { .. } => "Signing in…".into(),
        SignInState::Unknown => "Sign-in status unknown.".into(),
        SignInState::Failed { .. } => "Couldn't read the account status.".into(),
    }
}
fn sign_in_copy(engine: EngineId) -> String {
    match engine {
        EngineId::OpenCode => "Sign in to use models beyond OpenCode's free ones.".into(),
        EngineId::Claude | EngineId::Codex => {
            format!("{} can't run until you sign in.", engine.display_name())
        }
    }
}
#[expect(
    clippy::too_many_lines,
    reason = "the engine account block renders each sign-in state as one branch of a single layout"
)]
fn account(
    controller: &SettingsController,
    status: &EngineStatus,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let engine = status.engine;
    let busy = controller.engines.busy.contains(&engine);
    let mut blocks = vec![match &status.sign_in {
        SignInState::Failed { message } => failure(
            theme,
            format!("settings-engine-account-error-{engine}"),
            account_copy(status),
            message,
        ),
        SignInState::SignedIn { .. }
        | SignInState::SignedOut
        | SignInState::LoggingIn { .. }
        | SignInState::Unknown => band(theme, account_copy(status)),
    }];
    match &status.sign_in {
        SignInState::SignedOut | SignInState::Failed { .. }
            if matches!(status.install, InstallState::Installed { .. }) =>
        {
            blocks.push(ui::row(
                theme,
                "Sign in",
                sign_in_copy(engine),
                Some(button(
                    theme,
                    format!("settings-engine-login-{engine}"),
                    "Sign in",
                    ButtonTone::Primary,
                    busy,
                    ViewAction::EngineAction(engine, EngineCommand::Login {}),
                    Rc::clone(&emit),
                )),
            ));
        }
        SignInState::SignedIn { .. } => blocks.push(ui::row(
            theme,
            "Sign out",
            "Signs this Forge out of the account.",
            Some(button(
                theme,
                format!("settings-engine-logout-{engine}"),
                "Sign out",
                ButtonTone::Neutral,
                busy,
                ViewAction::EngineAction(engine, EngineCommand::Logout {}),
                Rc::clone(&emit),
            )),
        )),
        SignInState::LoggingIn {
            url,
            code,
            accepts_code,
        } => {
            let mut content = ui::stack(Space::S8);
            if let Some(url) = url {
                content = content.child(
                    ui::actions()
                        .child(ui::grow(ui::line(
                            theme,
                            TextRole::Caption,
                            Ink::Muted,
                            format!("Open {url}"),
                        )))
                        .child(button(
                            theme,
                            format!("settings-engine-open-{engine}"),
                            "Open link",
                            ButtonTone::Quiet,
                            false,
                            ViewAction::OpenUrl(url.clone()),
                            Rc::clone(&emit),
                        ))
                        .build(),
                );
            }
            if let Some(code) = code {
                content = content.child(
                    UiBox::new()
                        .spaced(Space::S6)
                        .child(ui::line(theme, TextRole::Label, Ink::Muted, "and enter"))
                        .child(ui::line(theme, TextRole::Title, Ink::Plain, code.clone()))
                        .build(),
                );
            }
            if *accepts_code && let Some(field) = controller.engines.login_codes.get(&engine) {
                let send = Rc::clone(&emit);
                let editor =
                    EditorController::from_shared(Rc::clone(field), move |event| match event {
                        EditorEvent::Changed => send(ViewAction::EngineEdited),
                        EditorEvent::PresentationChanged => {
                            send(ViewAction::EnginePresentationChanged);
                        }
                        EditorEvent::Submitted => send(ViewAction::EngineCode(engine)),
                        EditorEvent::SelectionChanged { .. }
                        | EditorEvent::List(_)
                        | EditorEvent::PastedFiles { .. } => {}
                    });
                content = content.child(
                    ui::actions()
                        .child(ui::grow(editor.element_with_notice_keyed(
                            format!("settings-engine-login-code-{engine}"),
                            Mode::SingleLine,
                            theme,
                            "Paste the code",
                            false,
                            false,
                        )))
                        .child(button(
                            theme,
                            format!("settings-engine-code-{engine}"),
                            "Submit code",
                            ButtonTone::Primary,
                            busy,
                            ViewAction::EngineCode(engine),
                            Rc::clone(&emit),
                        ))
                        .build(),
                );
            }
            let cancelling = controller.engines.cancelling.contains(&engine);
            content = content.child(button(
                theme,
                format!("settings-engine-cancel-login-{engine}"),
                if cancelling {
                    "Cancelling…"
                } else {
                    "Cancel"
                },
                ButtonTone::Quiet,
                cancelling,
                ViewAction::EngineAction(engine, EngineCommand::CancelLogin {}),
                Rc::clone(&emit),
            ));
            blocks.push(ui::block(content.build()));
        }
        SignInState::SignedOut | SignInState::Failed { .. } | SignInState::Unknown => {}
    }
    ui::section(
        theme,
        "Account",
        None,
        Some(refresh(theme, "settings-usage-refresh", engine, busy, emit)),
        ui::card(theme, blocks),
    )
}
fn claude(
    controller: &SettingsController,
    host: &HostData,
    status: &EngineStatus,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let busy = controller.engines.busy.contains(&EngineId::Claude);
    let on = artisan_api::transport_of(host.setting(artisan_api::TRANSPORT_KEY))
        == artisan_api::CLAUDE_AI;
    let change = Rc::clone(&emit);
    let host_id = host.id.clone();
    let instance = host.instance();
    let switch = Switch::new(
        "settings-switch-enhanced-claude",
        *theme,
        on,
        "claude.ai mode",
    )
    .on_change(move |checked| {
        change(ViewAction::ClaudeTransport(
            host_id.clone(),
            instance,
            checked,
        ));
    })
    .build();
    let state = status.claude_ai.clone().unwrap_or(ClaudeAiState::SignedOut);
    let (mut description, signed_in) = match &state {
        ClaudeAiState::SignedOut => ("Not signed in. Signing in opens claude.ai in a browser window; it closes by itself once you're in.".to_owned(), false),
        ClaudeAiState::SignedIn { account } => (account.as_ref().map_or_else(|| "Signed in.".into(), |account| format!("Signed in as {account}.")), true),
        ClaudeAiState::NeedsRenewal { .. } => (if claude_ai_browser::can_renew(&host.id.0) { "claude.ai asked for a browser check. Artisan is renewing the session in the background." } else { "claude.ai asked for a browser check. Sign in again on this device, or wait for the device that signed in to renew it." }.to_owned(), false),
        ClaudeAiState::Expired { .. } => ("Sign in again to keep using claude.ai mode.".to_owned(), false),
    };
    if busy {
        description = "Waiting for you to sign in to claude.ai in the browser window…".into();
    }
    let control = if signed_in {
        button(
            theme,
            "settings-claude-ai-sign-out",
            "Sign out",
            ButtonTone::Neutral,
            busy,
            ViewAction::EngineAction(EngineId::Claude, EngineCommand::ClaudeAiSignOut {}),
            emit,
        )
    } else {
        button(
            theme,
            "settings-claude-ai-sign-in",
            if matches!(state, ClaudeAiState::SignedOut) {
                "Sign in"
            } else {
                "Sign in again"
            },
            ButtonTone::Primary,
            busy,
            ViewAction::ClaudeSignIn,
            emit,
        )
    };
    ui::section(
        theme,
        "claude.ai mode",
        None,
        None,
        ui::card(
            theme,
            vec![
                ui::row(
                    theme,
                    "claude.ai mode",
                    "Run Claude through claude.ai with local files, commands, skills, MCP servers and subagents. Turn this off to use Claude Code.",
                    Some(switch),
                ),
                ui::row(theme, "claude.ai session", description, Some(control)),
            ]
            .into_iter()
            .chain(match &state {
                ClaudeAiState::Expired { reason } => Some(failure(
                    theme,
                    "settings-claude-ai-expired".to_owned(),
                    "The claude.ai session stopped working.".to_owned(),
                    reason,
                )),
                ClaudeAiState::SignedOut
                | ClaudeAiState::SignedIn { .. }
                | ClaudeAiState::NeedsRenewal { .. } => None,
            })
            .collect(),
        ),
    )
}

pub(crate) fn page(
    controller: &SettingsController,
    host: &HostData,
    engine: EngineId,
    selected: SettingsPage,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let label = engine.display_name();
    let title = if selected == selected.parent() {
        label.to_owned()
    } else {
        format!("{label} · {}", selected.label())
    };
    let description = if selected == selected.parent() {
        format!("{label}'s installation and sign-in on this Forge.")
    } else {
        page::description(selected).to_owned()
    };
    let mut content = ui::column().child(ui::header(theme, &title, &description));
    let Some(status) = host.engines.get(&engine) else {
        let note = if host.is_live() {
            format!("This Forge reported no {label} engine.")
        } else {
            "Waiting for the Forge to connect…".into()
        };
        return content
            .child(ui::text(theme, TextRole::Label, Ink::Muted, note))
            .build();
    };
    if selected.shows(SettingsSection::Installation(engine)) {
        content = content.child(installation(controller, status, theme, Rc::clone(&emit)));
    }
    if selected.shows(SettingsSection::Account(engine)) {
        content = content.child(account(controller, status, theme, Rc::clone(&emit)));
    }
    if engine == EngineId::Claude && selected.shows(SettingsSection::ClaudeAi) {
        content = content.child(claude(controller, host, status, theme, emit));
    }
    if let Some(problem) = controller.engines.errors.get(&engine) {
        content = content.child(ui::problem(theme, "engine-error", problem).build());
    }
    content.build()
}
