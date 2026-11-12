use super::{
    controller::SettingsController,
    page,
    ui::{self, Ink},
    view::ViewAction,
};
use editor_core::{Core, HostData, SettingsPage, SettingsSection};
use editor_ui::{
    kit::{ButtonTone, Space, Spaced, TextRole},
    switch::Switch,
    text::{
        Mode,
        mount::{EditorController, EditorEvent},
    },
    theme::ArtisanTheme,
};
use aqua_ui::element::{Box as UiBox, Element};
use artisan_api::{
    ClaudeAiState, EngineCommand, EngineId, EngineStatus, InstallState, Integrity, SignInState,
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
    ui::button(theme, id, label, tone, disabled, move || emit(action.clone()))
}
fn band(theme: &ArtisanTheme, value: String) -> Element {
    ui::block(ui::text(theme, TextRole::Label, Ink::Plain, value))
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
        InstallState::Failed { message } if message.is_empty() => {
            format!("{label} could not be installed.")
        }
        InstallState::Failed { message } => message.clone(),
    }
}
fn installation(
    controller: &SettingsController,
    status: &EngineStatus,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let engine = status.engine;
    let busy = controller.engines.busy.contains(&engine);
    let mut blocks = vec![band(theme, installation_copy(status))];
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
                emit.clone(),
            );
            blocks.push(ui::row(
                theme,
                "Version",
                format!(
                    "Artisan installs and updates {} in its own home on this host.",
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
                    emit.clone(),
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
                    emit.clone(),
                ))
                .child(button(
                    theme,
                    "settings-engine-versions",
                    "Choose version",
                    ButtonTone::Neutral,
                    busy,
                    ViewAction::EngineVersions(engine),
                    emit.clone(),
                ));
            let detail = update.as_ref().map_or_else(
                || format!("Follows the latest release ({version})."),
                |update| format!("Follows the latest release; {update} is available."),
            );
            blocks.push(ui::row(theme, "Version", detail, Some(controls.build())));
        }
        _ => {}
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
                let mut row = ui::actions().child(ui::grow(ui::text(
                    theme,
                    TextRole::Label,
                    ink,
                    label,
                )));
                if !version.current && !version.unsupported {
                    row = row.child(button(
                        theme,
                        format!("settings-engine-use-{}", version.version),
                        format!("Use {}", version.version),
                        ButtonTone::Neutral,
                        busy,
                        ViewAction::EngineUse(engine, version.version.clone()),
                        emit.clone(),
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
            _ => {}
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
        SignInState::Failed { message } => format!("Account status unavailable: {message}."),
    }
}
fn account(
    controller: &SettingsController,
    status: &EngineStatus,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let engine = status.engine;
    let busy = controller.engines.busy.contains(&engine);
    let mut blocks = vec![band(theme, account_copy(status))];
    match &status.sign_in {
        SignInState::SignedOut | SignInState::Failed { .. }
            if matches!(status.install, InstallState::Installed { .. }) =>
        {
            blocks.push(ui::row(
                theme,
                "Sign in",
                format!("{} can't run until you sign in.", engine.display_name()),
                Some(button(
                    theme,
                    format!("settings-engine-login-{engine}"),
                    "Sign in",
                    ButtonTone::Primary,
                    busy,
                    ViewAction::EngineAction(engine, EngineCommand::Login {}),
                    emit.clone(),
                )),
            ));
        }
        SignInState::SignedIn { .. } => blocks.push(ui::row(
            theme,
            "Sign out",
            "Signs this host out of the account.",
            Some(button(
                theme,
                format!("settings-engine-logout-{engine}"),
                "Sign out",
                ButtonTone::Neutral,
                busy,
                ViewAction::EngineAction(engine, EngineCommand::Logout {}),
                emit.clone(),
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
                            emit.clone(),
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
            if *accepts_code {
                if let Some(field) = controller.engines.login_codes.get(&engine) {
                    let send = emit.clone();
                    let editor =
                        EditorController::from_shared(field.clone(), move |event| match event {
                            EditorEvent::Changed => send(ViewAction::EngineEdited),
                            EditorEvent::PresentationChanged => {
                                send(ViewAction::EnginePresentationChanged)
                            }
                            _ => {}
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
                                emit.clone(),
                            ))
                            .build(),
                    );
                }
            }
            content = content.child(button(
                theme,
                format!("settings-engine-cancel-login-{engine}"),
                "Cancel",
                ButtonTone::Quiet,
                false,
                ViewAction::EngineAction(engine, EngineCommand::CancelLogin {}),
                emit.clone(),
            ));
            blocks.push(ui::block(content.build()));
        }
        _ => {}
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
    let change = emit.clone();
    let host_id = host.id.clone();
    let instance = host.instance();
    let switch = Switch::new(
        "settings-switch-enhanced-claude",
        *theme,
        on,
        "switch enhanced claude",
    )
    .on_change(move |checked| {
        change(ViewAction::ClaudeTransport(
            host_id.clone(),
            instance,
            checked,
        ))
    })
    .build();
    let state = status.claude_ai.clone().unwrap_or(ClaudeAiState::SignedOut);
    let (mut description, signed_in) = match &state {
        ClaudeAiState::SignedOut => ("Not signed in. Signing in opens claude.ai in a browser window; it closes by itself once you're in.".to_owned(), false),
        ClaudeAiState::SignedIn { account } => (account.as_ref().map_or_else(|| "Signed in.".into(), |account| format!("Signed in as {account}.")), true),
        ClaudeAiState::NeedsRenewal { .. } => (if claude_ai_browser::can_renew() { "claude.ai asked for a browser check. Artisan is renewing the session in the background." } else { "claude.ai asked for a browser check. Sign in again on this device, or wait for the device that signed in to renew it." }.to_owned(), false),
        ClaudeAiState::Expired { reason } => (format!("The session stopped working: {reason}"), false),
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
            ],
        ),
    )
}

pub fn page(
    controller: &SettingsController,
    core: &Core,
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
        format!("{label}'s installation and sign-in on this host.")
    } else {
        page::description(selected).to_owned()
    };
    let mut content = ui::column().child(ui::header(theme, &title, &description));
    let Some(status) = host.engines.get(&engine) else {
        let note = if host.is_live() {
            format!("This host reported no {label} engine.")
        } else {
            "Waiting for the host to connect…".into()
        };
        return content
            .child(ui::text(theme, TextRole::Label, Ink::Muted, note))
            .build();
    };
    if selected.shows(SettingsSection::Installation(engine)) {
        content = content.child(installation(controller, status, theme, emit.clone()));
    }
    if selected.shows(SettingsSection::Account(engine)) {
        content = content.child(account(controller, status, theme, emit.clone()));
    }
    if engine == EngineId::Claude && selected.shows(SettingsSection::ClaudeAi) {
        content = content.child(claude(controller, host, status, theme, emit));
    }
    if let Some(error) = controller.engines.errors.get(&engine) {
        content = content.child(ui::error_text(theme, error));
    }
    let _ = core;
    content.build()
}
