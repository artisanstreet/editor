use super::{
    controller::{Gear, SettingsController},
    ui::{self, Ink},
    view::ViewAction,
};
use aqua_ui::element::Element;
use artisan_api::{EngineCommand, EngineId, EngineStatus, InstallState, SignInState};
use editor_core::{HostData, Problem, SettingsPage, SettingsSection};
use editor_ui::{
    carved::{
        Carve,
        button::{Content, Tone},
        glyph::{Icon, glyph},
        menu::{Entry, MenuItem},
    },
    kit::{ButtonTone, IconSize},
    select::{Select, SelectEntry, SelectMenu},
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
fn failure(theme: &ArtisanTheme, key: String, headline: String, detail: &str) -> Element {
    let problem = if detail.trim().is_empty() {
        Problem::new(headline)
    } else {
        Problem::detailed(headline, detail)
    };
    ui::block(ui::problem(theme, key, &problem).build())
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
fn version_entries(status: &EngineStatus) -> Vec<SelectEntry<String>> {
    let InstallState::Installed { version, update } = &status.install else {
        return Vec::new();
    };
    let mut versions: Vec<String> = status
        .versions
        .iter()
        .filter(|published| !published.unsupported || published.version == *version)
        .map(|published| published.version.clone())
        .collect();
    if let Some(update) = update
        && !versions.contains(update)
    {
        versions.insert(0, update.clone());
    }
    if !versions.contains(version) {
        versions.push(version.clone());
    }
    versions
        .into_iter()
        .map(|version| SelectEntry::item(version.clone(), version))
        .collect()
}

fn choose_version(status: &EngineStatus, chosen: String) -> Option<ViewAction> {
    let InstallState::Installed { version, update } = &status.install else {
        return None;
    };
    let engine = status.engine;
    (chosen != *version).then(|| {
        if update.as_deref() == Some(chosen.as_str()) {
            ViewAction::EngineAction(engine, EngineCommand::Update {})
        } else {
            ViewAction::EngineUse(engine, chosen)
        }
    })
}

pub(crate) fn version_menu(
    controller: &SettingsController,
    status: &EngineStatus,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<String> {
    let current = match &status.install {
        InstallState::Installed { version, .. } => Some(version.clone()),
        InstallState::Absent
        | InstallState::Downloading { .. }
        | InstallState::Verifying { .. }
        | InstallState::Failed { .. } => None,
    };
    let engine = status.engine;
    let change = Rc::clone(&emit);
    let picked = status.clone();
    SelectMenu::new(
        *theme,
        current,
        version_entries(status),
        Rc::clone(&controller.version_select),
        ui::CONTROL_WIDTH,
    )
    .on_change(move |chosen| {
        if let Some(action) = choose_version(&picked, chosen) {
            change(action);
        }
    })
    .on_open_change(move |open| emit(ViewAction::VersionMenu(engine, open)))
}

fn spinner(theme: &ArtisanTheme) -> Element {
    glyph(Icon::Spinner, 16.0, Carve::of(theme).ink_dim)
}

fn installation(
    controller: &SettingsController,
    host: &HostData,
    status: &EngineStatus,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let engine = status.engine;
    let busy = controller.engines.busy.contains(&engine);
    let key = engine.enabled_setting();
    let enabled = artisan_api::switch_decl(key).switch(host.setting(key)).on();
    let host_id = host.id.clone();
    let instance = host.instance();
    let toggle = Rc::clone(emit);
    let enabled = Switch::new(
        format!("settings-engine-enabled-{engine}"),
        *theme,
        enabled,
        engine.display_name(),
    )
    .recessed()
    .on_change(move |enabled| {
        toggle(ViewAction::SetHostSetting(
            host_id.clone(),
            instance,
            key,
            enabled,
        ));
    })
    .build();
    let control = match &status.install {
        InstallState::Absent | InstallState::Failed { .. } => button(
            theme,
            format!("settings-engine-install-{engine}"),
            if matches!(status.install, InstallState::Absent) {
                "Install"
            } else {
                "Try again"
            },
            ButtonTone::Primary,
            busy,
            ViewAction::EngineAction(engine, EngineCommand::Install {}),
            Rc::clone(emit),
        ),
        InstallState::Installed { version, .. } => {
            let change = Rc::clone(emit);
            let open = Rc::clone(emit);
            let picked = status.clone();
            ui::fixed(
                Select::new(
                    "settings-engine-version",
                    *theme,
                    Some(version.clone()),
                    version_entries(status),
                    Rc::clone(&controller.version_select),
                )
                .label(format!("{} version", engine.display_name()))
                .on_change(move |chosen| {
                    if let Some(action) = choose_version(&picked, chosen) {
                        change(action);
                    }
                })
                .on_open_change(move |state| open(ViewAction::VersionMenu(engine, state)))
                .build(),
            )
        }
        InstallState::Downloading { .. } | InstallState::Verifying { .. } => {
            ui::desc(theme, installation_copy(status))
        }
    };
    let working = busy
        || matches!(
            status.install,
            InstallState::Downloading { .. } | InstallState::Verifying { .. }
        );
    let name = ui::inline(8.0)
        .child(ui::label(theme, engine.display_name()))
        .children(working.then(|| spinner(theme)))
        .build();
    let mark = IconSize::Md.glyph(
        editor_ui::engine::engine_icon(theme, engine),
        Carve::of(theme).ink.to_reference_paint(),
    );
    let mut blocks = vec![ui::machine_row(
        Some(ui::tile(mark)),
        name,
        Some(ui::actions().child(enabled).child(control).build()),
        false,
    )];
    if let InstallState::Failed { message } = &status.install {
        blocks.push(failure(
            theme,
            format!("settings-engine-install-error-{engine}"),
            installation_copy(status),
            message,
        ));
    }
    ui::bare_section(ui::card(theme, blocks))
}

fn account_copy(status: &EngineStatus) -> String {
    if status.engine == EngineId::OpenCode {
        match &status.sign_in {
            SignInState::SignedIn {
                account: Some(gateways),
            } => return format!("Signed in to {gateways}."),
            SignInState::SignedOut => {
                return "No accounts connected".into();
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
        SignInState::SignedOut => "No accounts connected".into(),
        SignInState::LoggingIn { .. } => "Signing in…".into(),
        SignInState::Unknown => "Checking accounts…".into(),
        SignInState::Failed { .. } => "Couldn't read the account status.".into(),
    }
}
fn account_row(
    theme: &ArtisanTheme,
    status: &EngineStatus,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let engine = status.engine;
    let (title, sub) = match &status.sign_in {
        SignInState::SignedIn {
            account: Some(account),
        } if account == "API Key" => ("API Key".to_owned(), "API Key".to_owned()),
        SignInState::SignedIn {
            account: Some(account),
        } if engine == EngineId::OpenCode => (account.clone(), "Providers".to_owned()),
        SignInState::SignedIn {
            account: Some(account),
        } => (account.clone(), "Subscription".to_owned()),
        SignInState::SignedIn { account: None }
        | SignInState::Unknown
        | SignInState::SignedOut
        | SignInState::LoggingIn { .. }
        | SignInState::Failed { .. } => ("Signed in".to_owned(), engine.display_name().to_owned()),
    };
    let heading = ui::inline(8.0)
        .child(ui::label(theme, title.clone()))
        .child(ui::inset_tag(theme, "Active"))
        .build();
    let gear = Gear::Account(engine);
    let send = Rc::clone(emit);
    let open = gear.clone();
    ui::machine_row(
        None,
        ui::titled(theme, heading, Some(&sub)),
        Some(ui::carved(
            theme,
            gear.key(),
            Tone::Secondary,
            Content::Icon(Icon::Gear, format!("{title} options")),
            false,
            move || send(ViewAction::Gear(Some(open.clone()))),
        )),
        true,
    )
}

fn logging_in(
    controller: &SettingsController,
    status: &EngineStatus,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Option<Element> {
    let SignInState::LoggingIn {
        url,
        code,
        accepts_code,
    } = &status.sign_in
    else {
        return None;
    };
    let engine = status.engine;
    let busy = controller.engines.busy.contains(&engine);
    let mut content = Vec::new();
    if let Some(url) = url {
        content.push(ui::row_of(
            theme,
            ui::desc(theme, format!("Open {url}")),
            Some(button(
                theme,
                format!("settings-engine-open-{engine}"),
                "Open link",
                ButtonTone::Neutral,
                false,
                ViewAction::OpenUrl(url.clone()),
                Rc::clone(emit),
            )),
            false,
        ));
    }
    if let Some(code) = code {
        content.push(
            ui::inline(6.0)
                .child(ui::desc(theme, "Enter"))
                .child(ui::inset_tag(theme, code))
                .build(),
        );
    }
    let mut actions = ui::actions();
    if *accepts_code && let Some(field) = controller.engines.login_codes.get(&engine) {
        let send = Rc::clone(emit);
        let editor = EditorController::from_shared(Rc::clone(field), move |event| match event {
            EditorEvent::Changed => send(ViewAction::EngineEdited),
            EditorEvent::PresentationChanged => send(ViewAction::EnginePresentationChanged),
            EditorEvent::Submitted => send(ViewAction::EngineCode(engine)),
            EditorEvent::SelectionChanged { .. }
            | EditorEvent::List(_)
            | EditorEvent::PastedFiles { .. } => {}
        });
        content.push(ui::field(
            theme,
            "Code",
            ui::input(
                theme,
                editor.element_with_notice_keyed(
                    format!("settings-engine-login-code-{engine}"),
                    Mode::SingleLine,
                    theme,
                    "Paste the code",
                    true,
                    false,
                ),
            ),
        ));
        actions = actions.child(button(
            theme,
            format!("settings-engine-code-{engine}"),
            "Submit code",
            ButtonTone::Primary,
            busy,
            ViewAction::EngineCode(engine),
            Rc::clone(emit),
        ));
    }
    let cancelling = controller.engines.cancelling.contains(&engine);
    actions = actions.child(button(
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
        Rc::clone(emit),
    ));
    content.push(actions.build());
    Some(ui::form_block(theme, content))
}

fn api_key_form(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Option<Element> {
    let field = controller.engines.api_key.as_ref()?;
    let send = Rc::clone(emit);
    let editor = EditorController::from_shared(Rc::clone(field), move |event| match event {
        EditorEvent::Submitted => send(ViewAction::SaveApiKey),
        EditorEvent::Changed | EditorEvent::PresentationChanged => {
            send(ViewAction::EnginePresentationChanged);
        }
        EditorEvent::SelectionChanged { .. }
        | EditorEvent::List(_)
        | EditorEvent::PastedFiles { .. } => {}
    })
    .password();
    let key = ui::field(
        theme,
        "API Key",
        ui::input(
            theme,
            editor.element_with_notice_keyed(
                "settings-codex-api-key",
                Mode::SingleLine,
                theme,
                "sk-…",
                true,
                false,
            ),
        ),
    );
    let actions = ui::actions()
        .child(button(
            theme,
            "settings-codex-api-key-cancel",
            "Cancel",
            ButtonTone::Quiet,
            false,
            ViewAction::CancelApiKey,
            Rc::clone(emit),
        ))
        .child(button(
            theme,
            "settings-codex-api-key-save",
            "Save",
            ButtonTone::Primary,
            false,
            ViewAction::SaveApiKey,
            Rc::clone(emit),
        ))
        .build();
    Some(ui::form_block(theme, vec![key, actions]))
}

fn add_account(
    controller: &SettingsController,
    status: &EngineStatus,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Option<Element> {
    if !matches!(status.install, InstallState::Installed { .. })
        || matches!(status.sign_in, SignInState::LoggingIn { .. })
    {
        return None;
    }
    let engine = status.engine;
    let busy = controller.engines.busy.contains(&engine);
    let send = Rc::clone(emit);
    let gear = Gear::AddAccount(engine);
    let action = if engine == EngineId::Codex {
        ViewAction::Gear(Some(gear.clone()))
    } else {
        ViewAction::EngineAction(engine, EngineCommand::Login {})
    };
    Some(ui::carved(
        theme,
        gear.key(),
        Tone::Horizon,
        Content::Label("Add new...".to_owned()),
        busy,
        move || send(action.clone()),
    ))
}

fn account(
    controller: &SettingsController,
    status: &EngineStatus,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let engine = status.engine;
    let mut blocks = Vec::new();
    match &status.sign_in {
        SignInState::SignedIn { .. } => blocks.push(account_row(theme, status, emit)),
        SignInState::Failed { message } => blocks.push(failure(
            theme,
            format!("settings-engine-account-error-{engine}"),
            account_copy(status),
            message,
        )),
        SignInState::Unknown | SignInState::SignedOut | SignInState::LoggingIn { .. } => {
            if controller.engines.api_key.is_none() || engine != EngineId::Codex {
                blocks.push(ui::row_of(
                    theme,
                    ui::typed(theme, ui::LABEL, Ink::Muted, account_copy(status)),
                    None,
                    false,
                ));
            }
        }
    }
    blocks.extend(logging_in(controller, status, theme, emit));
    if engine == EngineId::Codex {
        blocks.extend(api_key_form(controller, theme, emit));
    }
    ui::section(
        theme,
        "Accounts",
        add_account(controller, status, theme, emit),
        ui::card(theme, blocks),
    )
}

pub(crate) fn gear_entries(gear: &Gear) -> Vec<Entry<ViewAction>> {
    match gear {
        Gear::Account(engine) => vec![Entry::Item(
            MenuItem::new(
                ViewAction::EngineAction(*engine, EngineCommand::Logout {}),
                "Remove account",
                Icon::Trash,
            )
            .danger(),
        )],
        Gear::AddAccount(engine) => vec![
            Entry::Item(MenuItem::new(
                ViewAction::EngineAction(*engine, EngineCommand::Login {}),
                "Account",
                Icon::User,
            )),
            Entry::Item(MenuItem::new(ViewAction::OpenApiKey, "API Key", Icon::Key)),
        ],
        Gear::Computer(_) | Gear::Mcp(_) | Gear::Skill(_) => Vec::new(),
    }
}

pub(crate) fn page(
    controller: &SettingsController,
    host: &HostData,
    engine: EngineId,
    selected: SettingsPage,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let label = engine.display_name();
    let mut content = ui::column();
    let Some(status) = host.engines.get(&engine) else {
        let note = if host.is_live() {
            format!("This Forge reported no {label} engine.")
        } else {
            "Waiting for the Forge to connect…".into()
        };
        return content
            .child(ui::card(theme, vec![ui::note(theme, &note)]))
            .build();
    };
    if engine == EngineId::Claude
        && artisan_api::transport_of(host.setting(artisan_api::TRANSPORT_KEY))
            == artisan_api::CLAUDE_AI
    {
        content = content.child(ui::card(
            theme,
            vec![ui::row(
                theme,
                "Claude connection",
                "This Forge still uses the retired claude.ai mode.",
                Some(button(
                    theme,
                    "settings-use-claude-code",
                    "Use Claude Code",
                    ButtonTone::Primary,
                    false,
                    ViewAction::ClaudeTransport(host.id.clone(), host.instance(), false),
                    Rc::clone(emit),
                )),
            )],
        ));
    }
    if selected.shows(SettingsSection::Installation(engine)) {
        content = content.child(installation(controller, host, status, theme, emit));
    }
    if selected.shows(SettingsSection::Account(engine)) {
        content = content.child(account(controller, status, theme, emit));
    }
    if let Some(problem) = controller.engines.errors.get(&engine) {
        content = content.child(ui::problem(theme, "engine-error", problem).build());
    }
    content.build()
}
