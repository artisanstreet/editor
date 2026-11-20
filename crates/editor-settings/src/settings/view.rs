use std::rc::Rc;

use aqua_ui::{
    JustifyContent,
    element::{Box as UiBox, Element, Scroll},
    style::{Length, Style},
};

use super::{
    about::BuildInfo,
    controller::{Menu, OpenMenus, SettingsController},
    page,
    preferences::SettingKey,
    ui::{self, Ink},
};
use editor_core::{
    ConnectionStatus, Core, HostData, HostSwitch, Problem, Screen, SettingsPage, SettingsSection,
};
use editor_ui::{
    kit::{ButtonTone, Space, Spaced, TextRole, Tone},
    select::{Select, SelectEntry, SelectMenu},
    switch::Switch,
    text::{
        Mode,
        mount::{EditorController, EditorEvent},
    },
    theme::ArtisanTheme,
};

#[derive(Clone, Debug)]
pub enum ViewAction {
    Navigate(Screen),
    EngineEdited,
    EnginePresentationChanged,
    EngineAction(artisan_api::EngineId, artisan_api::EngineCommand),
    EngineVersions(artisan_api::EngineId),
    EngineUse(artisan_api::EngineId, String),
    EngineCode(artisan_api::EngineId),
    ClaudeTransport(editor_core::HostId, editor_core::HostInstance, bool),
    ClaudeSignIn,
    AccountEdited,
    AccountPresentationChanged,
    AccountRequestLink,
    AccountFinish,
    AccountDismiss,
    AccountRefresh,
    AccountSignOut,
    AccountSignOutEverywhere,
    ForgeOpen(String),
    ForgeConnect(String),
    ForgeRemove(String),
    ForgeEnroll,
    ForgeConfirm,
    ForgeRefresh,
    ForgeClaim,
    RetentionMenu(bool),
    SetRetention(editor_core::HostId, editor_core::HostInstance, Option<u32>),
    ArchiveLatest,
    ArchiveOlder,
    Unarchive(artisan_api::ThreadId),
    McpOpen(Option<artisan_api::McpServer>),
    McpRemove(artisan_api::McpServer),
    McpSave,
    McpCancel,
    McpEdited,
    McpPresentationChanged,
    McpSignIn(String),
    McpCancelSignIn(String),
    McpSignOut(String),
    OpenUrl(String),
    OpenNotices,
    CopyText(String),
    PromptChanged,
    PromptPresentationChanged,
    SavePrompt,
    LoadLatestPrompt,
    RetryPrompt,
    ResolvePrompt(artisan_api::ResolvePrompt),
    FontMenu(bool),
    SetFont(&'static str),
    SetDarkMode(bool),
    SetOutlineText(bool),
    SetShowAllModels(bool),
    SetHideBadModelOptions(bool),
    SetHighContrast(Option<bool>),
    TextSizeMenu(bool),
    ContrastMenu(bool),
    SetTextScale(Option<u32>),
    StreamingMenu(bool),
    SetStreaming(&'static str),
    SetFrameRate(bool),
    SetVsync(bool),
    SetNotifications(bool),
    ReloadSettings,
    SetHostSetting(
        editor_core::HostId,
        editor_core::HostInstance,
        &'static str,
        bool,
    ),
    RetryHostSetting(SettingKey),
    DismissHostSetting(SettingKey),
}

const PAGE_WIDTH: f32 = 680.0;
const STATUS_WIDTH: f32 = 240.0;

#[expect(
    clippy::too_many_lines,
    reason = "one arm per settings action; splitting the match would only move arms elsewhere"
)]
pub fn dispatch(controller: &mut SettingsController, core: &mut Core, action: ViewAction) -> bool {
    if matches!(
        action,
        ViewAction::EngineEdited
            | ViewAction::AccountEdited
            | ViewAction::McpEdited
            | ViewAction::OpenUrl(_)
            | ViewAction::OpenNotices
            | ViewAction::CopyText(_)
    ) {
        return false;
    }
    if matches!(
        action,
        ViewAction::PromptPresentationChanged
            | ViewAction::McpPresentationChanged
            | ViewAction::AccountPresentationChanged
            | ViewAction::EnginePresentationChanged
    ) {
        return true;
    }
    if matches!(action, ViewAction::PromptChanged) {
        controller.prompt.changed();
        let dirty = controller.prompt_editor.borrow().text() != controller.prompt.saved;
        let changed = controller.reported_prompt_dirty != dirty;
        controller.reported_prompt_dirty = dirty;
        return changed;
    }
    let before = ViewSnapshot::capture(controller, core);
    match action {
        ViewAction::Navigate(screen) => core.navigate(screen),
        ViewAction::EngineAction(engine, action) => controller.engine_action(core, engine, action),
        ViewAction::EngineVersions(engine) => {
            if !controller.engines.choosing_version.remove(&engine) {
                controller.engines.choosing_version.insert(engine);
                controller.engine_action(core, engine, artisan_api::EngineCommand::Versions {});
            }
        }
        ViewAction::EngineUse(engine, version) => {
            controller.engines.choosing_version.remove(&engine);
            controller.engine_action(core, engine, artisan_api::EngineCommand::Use { version });
        }
        ViewAction::EngineCode(engine) => {
            if let Some(code) = controller.engines.login_code(engine) {
                controller.engine_action(
                    core,
                    engine,
                    artisan_api::EngineCommand::LoginCode { code },
                );
            }
        }
        ViewAction::ClaudeTransport(host, instance, enabled) => controller.set_host_setting_for(
            core,
            host,
            instance,
            artisan_api::TRANSPORT_KEY,
            if enabled {
                artisan_api::CLAUDE_AI
            } else {
                artisan_api::CLAUDE_CODE
            }
            .into(),
        ),
        ViewAction::ClaudeSignIn => controller.sign_in_claude_ai(core),
        ViewAction::AccountRequestLink => controller.sign_in.request_link(core),
        ViewAction::AccountFinish => controller.sign_in.finish_sign_in(core),
        ViewAction::AccountDismiss => core.account_mut().dismiss_error(),
        ViewAction::AccountRefresh => core.account_mut().refresh(),
        ViewAction::AccountSignOut => core.account_mut().sign_out(),
        ViewAction::AccountSignOutEverywhere => core.account_mut().sign_out_everywhere(),
        ViewAction::ForgeOpen(machine) => core.navigate(Screen::NewThread {
            host: editor_core::HostId(machine),
            project: None,
        }),
        ViewAction::ForgeConnect(machine) => core.connect_machine(&machine),
        ViewAction::ForgeRemove(machine) => core.account_mut().remove_machine(machine),
        ViewAction::ForgeEnroll => core.account_mut().enroll_this_forge(),
        ViewAction::ForgeConfirm => core.account_mut().confirm_forge(),
        ViewAction::ForgeRefresh => core.account_mut().refresh_local_forge(),
        ViewAction::ForgeClaim => controller.claim_forge(core),
        ViewAction::RetentionMenu(open) => controller.reported_menus.report(Menu::Retention, open),
        ViewAction::SetRetention(host, instance, days) => controller.set_host_setting_for(
            core,
            host,
            instance,
            artisan_api::THREAD_RETENTION_DAYS_KEY,
            serde_json::json!(days),
        ),
        ViewAction::ArchiveLatest | ViewAction::ArchiveOlder => {
            let direction = if matches!(action, ViewAction::ArchiveLatest) {
                editor_core::ThreadPageDirection::Latest
            } else {
                editor_core::ThreadPageDirection::Older
            };
            if let Some(id) = core.active_host().map(|host| host.id.clone())
                && let Some(host) = core.host_mut(&id)
            {
                host.load_threads(true, direction);
            }
        }
        ViewAction::Unarchive(thread) => controller.unarchive(core, thread),
        ViewAction::McpOpen(server) => controller.open_mcp(server),
        ViewAction::McpRemove(server) => controller.remove_mcp(core, server),
        ViewAction::McpSave => controller.save_mcp(core),
        ViewAction::McpCancel => controller.cancel_mcp(),
        ViewAction::EngineEdited
        | ViewAction::EnginePresentationChanged
        | ViewAction::AccountEdited
        | ViewAction::AccountPresentationChanged
        | ViewAction::McpEdited
        | ViewAction::McpPresentationChanged
        | ViewAction::PromptChanged
        | ViewAction::PromptPresentationChanged
        | ViewAction::OpenUrl(_)
        | ViewAction::OpenNotices
        | ViewAction::CopyText(_) => {}
        ViewAction::McpSignIn(server) => controller.sign_in_mcp(core, &server),
        ViewAction::McpCancelSignIn(server) => controller.cancel_mcp_sign_in(core, &server),
        ViewAction::McpSignOut(server) => controller.sign_out_mcp(core, server),
        ViewAction::SavePrompt => controller.save_prompt(core),
        ViewAction::LoadLatestPrompt => controller.load_latest_prompt(core),
        ViewAction::ResolvePrompt(resolution) => controller.resolve_prompt(core, resolution),
        ViewAction::RetryPrompt => {
            let host = core.active_host().map(|host| host.id.clone());
            if let Some(host) = host.and_then(|id| core.host_mut(&id)) {
                host.load_prompt(true);
            }
        }
        ViewAction::FontMenu(open) => controller.reported_menus.report(Menu::Font, open),
        ViewAction::StreamingMenu(open) => controller.reported_menus.report(Menu::Streaming, open),
        ViewAction::TextSizeMenu(open) => controller.reported_menus.report(Menu::TextSize, open),
        ViewAction::ContrastMenu(open) => controller.reported_menus.report(Menu::Contrast, open),
        ViewAction::SetFont(font) => controller.set_editor_setting(core, "font_set", font.into()),
        ViewAction::SetDarkMode(enabled) => {
            controller.set_editor_setting(core, "dark_mode", enabled.into());
        }
        ViewAction::SetOutlineText(enabled) => {
            controller.set_editor_setting(core, "outline_text", enabled.into());
        }
        ViewAction::SetShowAllModels(enabled) => {
            controller.set_editor_setting(core, artisan_api::SHOW_ALL_MODELS_KEY, enabled.into());
        }
        ViewAction::SetHideBadModelOptions(enabled) => {
            controller.set_editor_setting(
                core,
                artisan_api::HIDE_BAD_MODEL_OPTIONS_KEY,
                enabled.into(),
            );
        }
        ViewAction::SetHighContrast(choice) => {
            controller.set_editor_setting(
                core,
                "high_contrast",
                choice.map_or(serde_json::Value::Null, serde_json::Value::Bool),
            );
        }
        ViewAction::SetTextScale(percent) => {
            controller.set_editor_setting(
                core,
                "text_scale",
                percent.map_or(serde_json::Value::Null, serde_json::Value::from),
            );
        }
        ViewAction::SetStreaming(mode) => {
            controller.set_editor_setting(core, "streaming_mode", mode.into());
        }
        ViewAction::SetFrameRate(enabled) => {
            controller.set_editor_setting(core, "lower_refresh_rate", enabled.into());
        }
        ViewAction::SetVsync(enabled) => {
            controller.set_editor_setting(core, "graphics_vsync", enabled.into());
        }
        ViewAction::SetNotifications(enabled) => {
            controller.set_editor_setting(core, "notifications", enabled.into());
        }
        ViewAction::ReloadSettings => controller.reload_editor_settings(core),
        ViewAction::SetHostSetting(host, instance, key, enabled) => {
            controller.set_host_setting_for(core, host, instance, key, enabled.into());
        }
        ViewAction::RetryHostSetting(target) => controller.retry_setting(core, target),
        ViewAction::DismissHostSetting(target) => controller.dismiss_setting_failure(&target),
    }
    controller.reported_prompt_dirty =
        controller.prompt_editor.borrow().text() != controller.prompt.saved;
    before != ViewSnapshot::capture(controller, core)
}

#[expect(
    clippy::too_many_lines,
    reason = "routes every settings page and wraps it in the shared frame"
)]
pub fn build(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    build_info: &BuildInfo,
    _viewport: aqua_core::Size,
    _visible: bool,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let page = match core.screen() {
        Screen::Settings(page) => *page,
        Screen::Home
        | Screen::Thread { .. }
        | Screen::NewThread { .. }
        | Screen::Marketplace
        | Screen::Usage => SettingsPage::General,
    };
    if page == SettingsPage::GlobalPrompt && core.active_host().is_some() {
        return prompt(controller, core, theme, emit);
    }
    let emit_for_error = Rc::clone(&emit);
    let content = match (page.parent(), core.active_host()) {
        (SettingsPage::General, _) if page == SettingsPage::General => ui::column()
            .child(ui::header(theme, page.label(), page::description(page)))
            .child(appearance(controller, theme, core, Rc::clone(&emit), false))
            .child(streaming(controller, theme, core, Rc::clone(&emit), false))
            .child(notifications(theme, core, Rc::clone(&emit), false))
            .child(performance(theme, core, emit, false))
            .build(),
        (SettingsPage::General, _)
            if page == SettingsPage::Section(SettingsSection::Appearance) =>
        {
            appearance(controller, theme, core, emit, true)
        }
        (SettingsPage::General, _) if page == SettingsPage::Section(SettingsSection::Streaming) => {
            streaming(controller, theme, core, emit, true)
        }
        (SettingsPage::General, _)
            if page == SettingsPage::Section(SettingsSection::Performance) =>
        {
            performance(theme, core, emit, true)
        }
        (SettingsPage::General, _)
            if page == SettingsPage::Section(SettingsSection::Notifications) =>
        {
            notifications(theme, core, emit, true)
        }
        (SettingsPage::Threads, Some(host))
            if page == SettingsPage::Threads
                || page == SettingsPage::Section(SettingsSection::Archive) =>
        {
            let mut content =
                ui::column().child(ui::header(theme, page.label(), page::description(page)));
            if page == SettingsPage::Threads {
                content = content
                    .child(thread_section(
                        theme,
                        host,
                        ThreadSection::Automation,
                        &emit,
                        false,
                    ))
                    .child(thread_section(
                        theme,
                        host,
                        ThreadSection::Assistance,
                        &emit,
                        false,
                    ));
            }
            content
                .child(super::archive_view::cleanup(
                    controller,
                    host,
                    theme,
                    Rc::clone(&emit),
                ))
                .child(super::archive_view::archived(
                    controller, host, theme, &emit,
                ))
                .build()
        }
        (SettingsPage::Threads, Some(host))
            if page == SettingsPage::Section(SettingsSection::Automation) =>
        {
            thread_section(theme, host, ThreadSection::Automation, &emit, true)
        }
        (SettingsPage::Threads, Some(host))
            if page == SettingsPage::Section(SettingsSection::Assistance) =>
        {
            thread_section(theme, host, ThreadSection::Assistance, &emit, true)
        }
        (SettingsPage::Extensions, Some(host))
            if page == SettingsPage::Extensions
                || page == SettingsPage::Section(SettingsSection::McpServers) =>
        {
            let mut content = ui::column()
                .child(ui::header(theme, page.label(), page::description(page)))
                .child(super::mcp_view::section(
                    controller,
                    core,
                    host,
                    theme,
                    Rc::clone(&emit),
                ));
            if page == SettingsPage::Extensions {
                content = content.child(skills_section(theme, emit));
            }
            content.build()
        }
        (SettingsPage::Extensions, Some(_))
            if page == SettingsPage::Section(SettingsSection::Skills) =>
        {
            skills(theme, emit)
        }
        (SettingsPage::Engine(engine), Some(host)) => {
            super::engine_view::page(controller, host, engine, page, theme, emit)
        }
        (SettingsPage::Account, _) => super::account_view::page(controller, core, theme, &emit),
        (SettingsPage::About, _) => about(theme, build_info, emit),
        (SettingsPage::Hosts, _) => hosts(theme, core),
        _ => ui::column()
            .child(ui::header(theme, page.label(), page::description(page)))
            .child(ui::text(
                theme,
                TextRole::Label,
                Ink::Muted,
                "Pair a Forge on the Account page first.",
            ))
            .child(ui::button(
                theme,
                "settings-open-account",
                "Open Account",
                ButtonTone::Neutral,
                false,
                move || {
                    emit(ViewAction::Navigate(Screen::Settings(
                        SettingsPage::Account,
                    )));
                },
            ))
            .build(),
    };
    let host_failures = core
        .active_host()
        .and_then(|host| host_setting_failures(controller, host, theme, &emit_for_error));
    let content = match (
        page.parent(),
        device_settings_error(controller, core, theme, emit_for_error),
        host_failures,
    ) {
        (SettingsPage::General, Some(error), _) => ui::column().child(error).child(content).build(),
        (
            SettingsPage::Threads | SettingsPage::Extensions | SettingsPage::Engine(_),
            _,
            Some(failures),
        ) => ui::column().child(failures).child(content).build(),
        _ => content,
    };
    Scroll::new()
        .preserve_offset(true)
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(Style::new().column())
        .child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(
                    Style::new()
                        .justify(JustifyContent::CENTER)
                        .flex_shrink(0.0),
                )
                .child(
                    UiBox::new()
                        .layout("column")
                        .width(Length::Pct(100.0))
                        .padded(Space::S32)
                        .style(Style::new().max_width(Length::Px(PAGE_WIDTH)))
                        .child(content)
                        .build(),
                )
                .build(),
        )
        .build()
        .with_key(format!("settings-screen-{page:?}"))
}

fn setting_label(key: &'static str) -> &'static str {
    match key {
        artisan_api::TRANSPORT_KEY => "Claude connection",
        artisan_api::THREAD_RETENTION_DAYS_KEY => "Delete inactive threads",
        _ => artisan_api::setting_decl(key).map_or(key, |decl| decl.label),
    }
}

fn host_setting_failures(
    controller: &SettingsController,
    host: &HostData,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Option<Element> {
    let failures = controller.setting_failures(host);
    if failures.is_empty() {
        return None;
    }
    let mut stack = ui::stack(Space::S8);
    for (target, failure) in failures {
        let retry = Rc::clone(emit);
        let dismiss = Rc::clone(emit);
        let retried = target.clone();
        let problem = failure.cause.problem(&format!(
            "Couldn't save “{}” on {}.",
            setting_label(target.key),
            host.name,
        ));
        let key = target.key;
        stack = stack.child(
            ui::problem(theme, format!("settings-error-{key}"), &problem)
                .action(ui::button(
                    theme,
                    format!("settings-retry-{key}"),
                    "Try again",
                    ButtonTone::Neutral,
                    false,
                    move || retry(ViewAction::RetryHostSetting(retried.clone())),
                ))
                .action(ui::button(
                    theme,
                    format!("settings-dismiss-{key}"),
                    "Dismiss",
                    ButtonTone::Quiet,
                    false,
                    move || dismiss(ViewAction::DismissHostSetting(target.clone())),
                ))
                .build(),
        );
    }
    Some(stack.build())
}

fn device_settings_error(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<Element> {
    let problem = match (
        core.settings().unreadable(),
        &controller.editor_setting_error,
    ) {
        (Some(error), _) => Problem::caused_by(
            "Couldn't read settings.json. The file is kept as it is and changes here are not saved until it can be read. Fix or remove the file, then reload.",
            error,
        ),
        (None, Some(problem)) => problem.clone(),
        (None, None) => return None,
    };
    Some(
        ui::problem(theme, "settings-device-error", &problem)
            .action(ui::button(
                theme,
                "settings-reload",
                "Reload settings",
                ButtonTone::Neutral,
                false,
                move || emit(ViewAction::ReloadSettings),
            ))
            .build(),
    )
}

fn about(theme: &ArtisanTheme, build: &BuildInfo, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    let rows = super::about::rows(build)
        .into_iter()
        .map(|(title, value)| ui::row(theme, title, value, None))
        .collect();
    ui::column()
        .child(ui::header(
            theme,
            SettingsPage::About.label(),
            page::description(SettingsPage::About),
        ))
        .child(ui::section(
            theme,
            "Build",
            None,
            None,
            ui::card(theme, rows),
        ))
        .child(ui::section(
            theme,
            "Licences",
            None,
            None,
            ui::card(
                theme,
                vec![ui::row(
                    theme,
                    "Third-party notices",
                    "Licences of the fonts, icons and libraries in the Editor, ae and Forge.",
                    Some(ui::button(
                        theme,
                        "about-open-notices",
                        "Open",
                        ButtonTone::Quiet,
                        false,
                        move || emit(ViewAction::OpenNotices),
                    )),
                )],
            ),
        ))
        .build()
}

fn hosts(theme: &ArtisanTheme, core: &Core) -> Element {
    let mut rows: Vec<Element> = core
        .hosts()
        .map(|(config, host)| {
            let (state, ink) = match &host.status {
                ConnectionStatus::Live => ("Connected".to_owned(), Ink::Success),
                ConnectionStatus::Connecting | ConnectionStatus::Reconnecting { .. } => {
                    ("Connecting…".to_owned(), Ink::Muted)
                }
                ConnectionStatus::Offline { reason, .. } => {
                    (format!("Offline. {}", reason.headline()), Ink::Danger)
                }
            };
            let detail = host
                .forge_version
                .as_ref()
                .map_or_else(String::new, |version| format!("Forge {version}"));
            let status = UiBox::new()
                .style(Style::new().max_width(Length::Px(STATUS_WIDTH)))
                .child(ui::line(theme, TextRole::Label, ink, state))
                .build();
            ui::row(theme, config.name.clone(), detail, Some(status))
        })
        .collect();
    if rows.is_empty() {
        rows.push(ui::row(
            theme,
            "No Forges open",
            "Pair and open Forges on the Account page.",
            None,
        ));
    }
    ui::column()
        .child(ui::header(
            theme,
            SettingsPage::Hosts.label(),
            page::description(SettingsPage::Hosts),
        ))
        .child(ui::section(
            theme,
            "Open Forges",
            None,
            None,
            ui::card(theme, rows),
        ))
        .build()
}

fn notifications(
    theme: &ArtisanTheme,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
    include_header: bool,
) -> Element {
    let enabled = core
        .settings()
        .get("notifications")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let control = Switch::new("settings-switch-notify", *theme, enabled, "Notify me")
        .on_change(move |enabled| emit(ViewAction::SetNotifications(enabled)))
        .build();
    let row = ui::row(
        theme,
        "Notify me",
        "Uses your system's notifications to tell you when a thread finishes, needs attention or fails.",
        Some(control),
    );
    section_page(
        theme,
        SettingsPage::Section(SettingsSection::Notifications),
        ui::section(
            theme,
            "Notifications",
            None,
            None,
            ui::card(theme, vec![row]),
        ),
        include_header,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ThreadSection {
    Automation,
    Assistance,
}

impl ThreadSection {
    const fn page(self) -> SettingsPage {
        SettingsPage::Section(match self {
            Self::Automation => SettingsSection::Automation,
            Self::Assistance => SettingsSection::Assistance,
        })
    }
}

fn thread_section(
    theme: &ArtisanTheme,
    host: &HostData,
    section: ThreadSection,
    emit: &Rc<dyn Fn(ViewAction)>,
    include_header: bool,
) -> Element {
    let (title, entries): (
        &str,
        &[(&'static str, HostSwitch, &'static str, &'static str)],
    ) = match section {
        ThreadSection::Automation => (
            "Recovery & continuation",
            &[
                (
                    "settings-switch-retry-overloaded",
                    HostSwitch::RetryWhenOverloaded,
                    "Keep retrying when the provider is overloaded",
                    "When the model is overloaded, Artisan keeps rescheduling the thread until the turn finishes.",
                ),
                (
                    "settings-switch-continue-usage-limit",
                    HostSwitch::ContinueAfterUsageLimit,
                    "Continue when my usage limit resets",
                    "A thread that stopped at the usage limit continues once the limit resets.",
                ),
                (
                    "settings-switch-continue-restart",
                    HostSwitch::ContinueAfterRestart,
                    "Continue after a restart",
                    "Threads stopped by a shutdown or restart continue automatically afterwards.",
                ),
            ],
        ),
        ThreadSection::Assistance => (
            "Assistance",
            &[
                (
                    "settings-switch-suggestions",
                    HostSwitch::Suggestions,
                    "Suggest my next message",
                    "After each turn, the composer offers what you would most likely send next. It is asked of the thread's own engine and reuses its cached conversation, so each one costs a little of that engine's usage.",
                ),
                (
                    "settings-switch-you-should-know",
                    HostSwitch::YouShouldKnow,
                    "You should know",
                    "After a turn that changed things — edits, migrations, installs, decisions — the engine is asked whether there is something you should know but might miss, and the thread shows it as a note. Turns that only looked around are not checked, and checks that find nothing make the next one rarer.",
                ),
                (
                    "settings-switch-titles",
                    HostSwitch::SummaryTitles,
                    "Summary titles",
                    "A thread takes the title its engine writes as it works. Off, it keeps the first line of its first message.",
                ),
            ],
        ),
    };
    let rows = entries
        .iter()
        .map(|&(id, switch, label, description)| {
            let enabled = host.switch(switch);
            let key = switch.key();
            let send = Rc::clone(emit);
            let host_id = host.id.clone();
            let instance = host.instance();
            let control = Switch::new(
                id,
                *theme,
                enabled,
                id.trim_start_matches("settings-").replace('-', " "),
            )
            .on_change(move |enabled| {
                send(ViewAction::SetHostSetting(
                    host_id.clone(),
                    instance,
                    key,
                    enabled,
                ));
            })
            .build();
            ui::row(theme, label, description, Some(control))
        })
        .collect();
    section_page(
        theme,
        section.page(),
        ui::section(theme, title, None, None, ui::card(theme, rows)),
        include_header,
    )
}

fn skills(theme: &ArtisanTheme, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    let page = SettingsPage::Section(SettingsSection::Skills);
    ui::column()
        .child(ui::header(theme, page.label(), page::description(page)))
        .child(skills_section(theme, emit))
        .build()
}

fn skills_section(theme: &ArtisanTheme, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    let action = UiBox::new()
        .child(ui::button(
            theme,
            "settings-manage-skills",
            "Manage skills in Marketplace",
            ButtonTone::Quiet,
            false,
            move || emit(ViewAction::Navigate(Screen::Marketplace)),
        ))
        .build();
    ui::section(
        theme,
        "Skills",
        Some(
            "Artisan's skills and your own reach every engine on this Forge; the Marketplace turns each on or off.",
        ),
        None,
        action,
    )
}

fn prompt(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let form = &controller.prompt;
    let notify = Rc::clone(&emit);
    let editor = EditorController::from_shared(
        Rc::clone(&controller.prompt_editor),
        move |event| match event {
            EditorEvent::Changed => notify(ViewAction::PromptChanged),
            EditorEvent::PresentationChanged => notify(ViewAction::PromptPresentationChanged),
            EditorEvent::SelectionChanged { .. }
            | EditorEvent::Submitted
            | EditorEvent::List(_)
            | EditorEvent::PastedFiles { .. } => {}
        },
    );
    let mut page = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .spaced(Space::S20)
        .padded(Space::S24)
        .style(Style::new().min_height(Length::Pct(0.0)))
        .child(ui::header(
            theme,
            SettingsPage::GlobalPrompt.label(),
            page::description(SettingsPage::GlobalPrompt),
        ))
        .child(
            Scroll::new()
                .width(Length::Pct(100.0))
                .spaced(Space::S20)
                .style(
                    Style::new()
                        .axis(aqua_ui::style::Axis::Column)
                        .flex_grow(1.0)
                        .min_height(Length::Pct(0.0)),
                )
                .child(prompt_reconciliation(controller, core, theme, &emit))
                .child(editor.element_with_notice_keyed(
                    "settings-global-prompt-editor",
                    Mode::Document,
                    theme,
                    if form.loaded {
                        "Write your global instructions…"
                    } else {
                        "Reading the global prompt…"
                    },
                    false,
                    true,
                ))
                .build()
                .with_key("global-prompt-scroll"),
        );
    if let Some(problem) = form.error.as_ref().or_else(|| {
        core.active_host()
            .and_then(|host| host.prompt_load().error.as_ref())
    }) {
        page = page.child(ui::problem(theme, "settings-global-prompt-error", problem).build());
    }
    page = page.child(ui::callout(
        theme,
        Tone::Neutral,
        form.status(controller.prompt_editor.borrow().text()),
    ));
    let mut actions = ui::actions().style(Style::new().flex_shrink(0.0));
    if core
        .active_host()
        .is_some_and(|host| host.prompt_load().error.is_some())
    {
        let send = Rc::clone(&emit);
        actions = actions.child(ui::button(
            theme,
            "settings-global-prompt-retry",
            "Try again",
            ButtonTone::Neutral,
            false,
            move || send(ViewAction::RetryPrompt),
        ));
    }
    if form.newer {
        let send = Rc::clone(&emit);
        actions = actions.child(ui::button(
            theme,
            "settings-global-prompt-load-latest",
            "Load latest",
            ButtonTone::Neutral,
            form.pending,
            move || send(ViewAction::LoadLatestPrompt),
        ));
    }
    let dirty = controller.prompt_editor.borrow().text() != form.saved;
    actions = actions.child(ui::button(
        theme,
        "settings-global-prompt-save",
        "Save",
        ButtonTone::Primary,
        !form.loaded || !dirty || form.pending || form.newer,
        move || emit(ViewAction::SavePrompt),
    ));
    page.child(actions.build()).build()
}

#[expect(
    clippy::too_many_lines,
    reason = "the prompt reconciliation card renders each engine status as one branch of a single layout"
)]
fn prompt_reconciliation(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let mut controls = ui::stack(Space::S12);
    let Some(prompt) = core
        .active_host()
        .and_then(|host| host.global_prompt.as_ref())
    else {
        return controls.build();
    };
    let disabled = controller.prompt.pending
        || controller.prompt.newer
        || controller.prompt_editor.borrow().text() != controller.prompt.saved;
    for candidate in &prompt.candidates {
        let engines = candidate
            .engines
            .iter()
            .map(|engine| engine.display_name())
            .collect::<Vec<_>>()
            .join(", ");
        let send = Rc::clone(emit);
        let candidate_id = candidate.id.clone();
        controls = controls.child(
            ui::stack(Space::S8)
                .child(ui::callout(
                    theme,
                    Tone::Info,
                    format!("Prompt found in {engines}"),
                ))
                .child(ui::callout(theme, Tone::Neutral, candidate.text.clone()))
                .child(
                    UiBox::new()
                        .child(ui::button(
                            theme,
                            format!("settings-prompt-candidate-{}", candidate.id),
                            "Use this prompt",
                            ButtonTone::Primary,
                            disabled,
                            move || {
                                send(ViewAction::ResolvePrompt(
                                    artisan_api::ResolvePrompt::Choose {
                                        candidate_id: candidate_id.clone(),
                                    },
                                ));
                            },
                        ))
                        .build(),
                )
                .build(),
        );
    }
    for engine in &prompt.engines {
        match engine.status {
            artisan_api::PromptEngineStatus::Drifted => {
                let use_send = Rc::clone(emit);
                let restore_send = Rc::clone(emit);
                let id = engine.engine;
                let mut row = ui::stack(Space::S8).child(ui::callout(
                    theme,
                    Tone::Warning,
                    format!(
                        "{} instructions were edited outside Artisan.",
                        id.display_name()
                    ),
                ));
                if let Some(text) = &engine.external_text {
                    row = row.child(ui::callout(theme, Tone::Neutral, text.clone()));
                }
                controls = controls.child(
                    row.child(
                        UiBox::new()
                            .spaced(Space::S8)
                            .child(ui::button(
                                theme,
                                format!("settings-prompt-import-{id}"),
                                "Use engine's version",
                                ButtonTone::Neutral,
                                disabled,
                                move || {
                                    use_send(ViewAction::ResolvePrompt(
                                        artisan_api::ResolvePrompt::UseEngine { engine: id },
                                    ));
                                },
                            ))
                            .child(ui::button(
                                theme,
                                format!("settings-prompt-restore-{id}"),
                                "Restore Artisan's version",
                                ButtonTone::Neutral,
                                disabled,
                                move || {
                                    restore_send(ViewAction::ResolvePrompt(
                                        artisan_api::ResolvePrompt::RestoreEngine { engine: id },
                                    ));
                                },
                            ))
                            .build(),
                    )
                    .build(),
                );
            }
            artisan_api::PromptEngineStatus::Failed => {
                if let Some(reason) = &engine.reason {
                    let name = engine.engine.display_name();
                    controls = controls.child(
                        ui::problem(
                            theme,
                            format!("settings-global-prompt-engine-error-{name}"),
                            &Problem::detailed(
                                format!("Couldn't apply the global prompt to {name}."),
                                reason.clone(),
                            ),
                        )
                        .build(),
                    );
                }
            }
            artisan_api::PromptEngineStatus::Synced
            | artisan_api::PromptEngineStatus::AwaitingChoice
            | artisan_api::PromptEngineStatus::EngineAbsent
            | artisan_api::PromptEngineStatus::Unavailable => {}
        }
    }
    controls.build()
}

fn performance(
    theme: &ArtisanTheme,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
    include_header: bool,
) -> Element {
    let lowered = core
        .settings()
        .get("lower_refresh_rate")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let control = Switch::new(
        "settings-lower-refresh-rate",
        *theme,
        lowered,
        "Lower refresh rate",
    )
    .on_change({
        let emit = Rc::clone(&emit);
        move |enabled| emit(ViewAction::SetFrameRate(enabled))
    })
    .build();
    let row = ui::row(
        theme,
        "Lower refresh rate",
        "Caps animation and scrolling at 60 frames per second to save power.",
        Some(control),
    );
    let monitor_sync = core
        .settings()
        .get("graphics_vsync")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let monitor_control = Switch::new(
        "settings-monitor-refresh-rate",
        *theme,
        monitor_sync,
        "Match monitor refresh rate",
    )
    .on_change(move |enabled| emit(ViewAction::SetVsync(enabled)))
    .build();
    let monitor_row = ui::row(
        theme,
        "Match monitor refresh rate",
        "Synchronizes presentation to the current monitor. With both limits off, rendering is uncapped.",
        Some(monitor_control),
    );
    let page = SettingsPage::Section(SettingsSection::Performance);
    section_page(
        theme,
        page,
        ui::section(
            theme,
            "Performance",
            None,
            None,
            ui::card(theme, vec![row, monitor_row]),
        ),
        include_header,
    )
}

fn streaming_entries() -> Vec<SelectEntry<&'static str>> {
    vec![
        SelectEntry::item("smooth", "Smooth"),
        SelectEntry::item("swift", "Swift"),
    ]
}

fn streaming_value(core: &Core) -> &'static str {
    if core
        .settings()
        .get("streaming_mode")
        .and_then(serde_json::Value::as_str)
        == Some("swift")
    {
        "swift"
    } else {
        "smooth"
    }
}

pub fn streaming_menu(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<&'static str> {
    let change = Rc::clone(&emit);
    SelectMenu::new(
        *theme,
        Some(streaming_value(core)),
        streaming_entries(),
        Rc::clone(&controller.streaming_select),
        ui::CONTROL_WIDTH,
    )
    .on_change(move |mode| change(ViewAction::SetStreaming(mode)))
    .on_open_change(move |open| emit(ViewAction::StreamingMenu(open)))
}

fn streaming(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
    include_header: bool,
) -> Element {
    let change = Rc::clone(&emit);
    let select = Select::new(
        "settings-streaming-mode",
        *theme,
        Some(streaming_value(core)),
        streaming_entries(),
        Rc::clone(&controller.streaming_select),
    )
    .label("Text Streaming")
    .on_change(move |mode| change(ViewAction::SetStreaming(mode)))
    .on_open_change(move |open| emit(ViewAction::StreamingMenu(open)))
    .build();
    let control = ui::fixed(select);
    let row = ui::row(
        theme,
        "Text Streaming",
        "Smooth fades new text in briefly as it arrives. Swift shows it immediately without reveal animation.",
        Some(control),
    );
    let page = SettingsPage::Section(SettingsSection::Streaming);
    section_page(
        theme,
        page,
        ui::section(
            theme,
            "Text Streaming",
            None,
            None,
            ui::card(theme, vec![row]),
        ),
        include_header,
    )
}

fn font_entries() -> Vec<SelectEntry<&'static str>> {
    vec![
        SelectEntry::item("system", "System"),
        SelectEntry::item("spline", "Spline"),
    ]
}

fn font_value(core: &Core) -> &'static str {
    if core
        .settings()
        .get("font_set")
        .and_then(serde_json::Value::as_str)
        == Some("system")
    {
        "system"
    } else {
        "spline"
    }
}

pub fn font_menu(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<&'static str> {
    let change = Rc::clone(&emit);
    SelectMenu::new(
        *theme,
        Some(font_value(core)),
        font_entries(),
        Rc::clone(&controller.font_select),
        ui::CONTROL_WIDTH,
    )
    .on_change(move |font| change(ViewAction::SetFont(font)))
    .on_open_change(move |open| emit(ViewAction::FontMenu(open)))
}

fn appearance(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
    include_header: bool,
) -> Element {
    let send = Rc::clone(&emit);
    let dark = Switch::new(
        "settings-switch-dark-mode",
        *theme,
        theme.colors.mode.is_dark(),
        "Dark mode",
    )
    .on_change(move |enabled| send(ViewAction::SetDarkMode(enabled)))
    .build();
    let send = Rc::clone(&emit);
    let open = Rc::clone(&emit);
    let fonts = Select::new(
        "settings-font-set",
        *theme,
        Some(font_value(core)),
        font_entries(),
        Rc::clone(&controller.font_select),
    )
    .label("Fonts")
    .on_change(move |font| send(ViewAction::SetFont(font)))
    .on_open_change(move |open_state| open(ViewAction::FontMenu(open_state)))
    .build();
    let fonts = ui::fixed(fonts);
    let show_models = Rc::clone(&emit);
    let hide_options = Rc::clone(&emit);
    let [text_size, high_contrast] = access_rows(controller, theme, core, &emit);
    let hide_bad = Switch::new(
        "settings-switch-hide-bad-model-options",
        *theme,
        core.settings()
            .get(artisan_api::HIDE_BAD_MODEL_OPTIONS_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
        "Hide bad model options",
    )
    .on_change(move |enabled| hide_options(ViewAction::SetHideBadModelOptions(enabled)))
    .build();
    let all_models = Switch::new(
        "settings-switch-show-all-models",
        *theme,
        core.settings()
            .get(artisan_api::SHOW_ALL_MODELS_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        "Show all models",
    )
    .on_change(move |enabled| show_models(ViewAction::SetShowAllModels(enabled)))
    .build();
    let outline = Switch::new(
        "settings-switch-outline-text",
        *theme,
        core.settings()
            .get("outline_text")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
        "Outline text rendering",
    )
    .on_change(move |enabled| emit(ViewAction::SetOutlineText(enabled)))
    .build();
    let rows = vec![
        ui::row(
            theme,
            "Dark mode",
            "Whether to draw Artisan Editor in a dark theme.",
            Some(dark),
        ),
        ui::row(
            theme,
            "Fonts",
            "Choose the fonts Artisan Editor uses.",
            Some(fonts),
        ),
        text_size,
        high_contrast,
        ui::row(
            theme,
            "Outline text rendering",
            "Unhinted grayscale glyphs with no gamma or contrast correction, so letters keep their designed shapes. Turn off if text looks too soft or thin.",
            Some(outline),
        ),
        ui::row(
            theme,
            "Show all models",
            "List every Claude and Codex model in the model picker, not only the latest of each class.",
            Some(all_models),
        ),
        ui::row(
            theme,
            "Hide bad model options",
            "Hide Max and Ultra for Claude and Codex, plus Light and Extra High for Opus 5.5. Your current selection is kept.",
            Some(hide_bad),
        ),
    ];
    let page = SettingsPage::Section(SettingsSection::Appearance);
    section_page(
        theme,
        page,
        ui::section(theme, "Appearance", None, None, ui::card(theme, rows)),
        include_header,
    )
}

fn access_rows(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    core: &Core,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> [Element; 2] {
    let preferences = editor_ui::preferences::Preferences::from_core(
        core,
        editor_ui::preferences::SystemAccess::default(),
    );
    [
        ui::row(
            theme,
            "Text size",
            "How large the Editor draws its text. Match system follows the system text size.",
            Some(ui::fixed(text_size(
                controller,
                theme,
                preferences.text_scale,
                Rc::clone(emit),
            ))),
        ),
        ui::row(
            theme,
            "High contrast",
            "Draw text, borders and focus rings with stronger contrast. Match system follows the system's contrast setting.",
            Some(ui::fixed(high_contrast(
                controller,
                theme,
                preferences.high_contrast,
                Rc::clone(emit),
            ))),
        ),
    ]
}

const TEXT_SIZES: [u32; 6] = [100, 125, 150, 175, 200, 225];

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the stored scale is clamped to 1..=2.25"
)]
fn percent(scale: f32) -> u32 {
    (scale * 100.0).round() as u32
}

fn text_size_entries(chosen: Option<u32>) -> Vec<SelectEntry<Option<u32>>> {
    let mut sizes: Vec<u32> = TEXT_SIZES.into_iter().chain(chosen).collect();
    sizes.sort_unstable();
    sizes.dedup();
    std::iter::once(SelectEntry::item(None, "Match system"))
        .chain(
            sizes
                .into_iter()
                .map(|percent| SelectEntry::item(Some(percent), format!("{percent}%"))),
        )
        .collect()
}

pub fn text_size_menu(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<Option<u32>> {
    let chosen = editor_ui::preferences::Preferences::from_core(
        core,
        editor_ui::preferences::SystemAccess::default(),
    )
    .text_scale
    .map(percent);
    let change = Rc::clone(&emit);
    SelectMenu::new(
        *theme,
        Some(chosen),
        text_size_entries(chosen),
        Rc::clone(&controller.text_size_select),
        ui::CONTROL_WIDTH,
    )
    .on_change(move |percent| change(ViewAction::SetTextScale(percent)))
    .on_open_change(move |open| emit(ViewAction::TextSizeMenu(open)))
}

fn text_size(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    scale: Option<f32>,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let chosen = scale.map(percent);
    let change = Rc::clone(&emit);
    Select::new(
        "settings-text-size",
        *theme,
        Some(chosen),
        text_size_entries(chosen),
        Rc::clone(&controller.text_size_select),
    )
    .label("Text size")
    .on_change(move |percent| change(ViewAction::SetTextScale(percent)))
    .on_open_change(move |open| emit(ViewAction::TextSizeMenu(open)))
    .build()
}

fn high_contrast_entries() -> Vec<SelectEntry<Option<bool>>> {
    vec![
        SelectEntry::item(None, "Match system"),
        SelectEntry::item(Some(true), "On"),
        SelectEntry::item(Some(false), "Off"),
    ]
}

pub fn high_contrast_menu(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<Option<bool>> {
    let chosen = editor_ui::preferences::Preferences::from_core(
        core,
        editor_ui::preferences::SystemAccess::default(),
    )
    .high_contrast;
    let change = Rc::clone(&emit);
    SelectMenu::new(
        *theme,
        Some(chosen),
        high_contrast_entries(),
        Rc::clone(&controller.contrast_select),
        ui::CONTROL_WIDTH,
    )
    .on_change(move |choice| change(ViewAction::SetHighContrast(choice)))
    .on_open_change(move |open| emit(ViewAction::ContrastMenu(open)))
}

fn high_contrast(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    chosen: Option<bool>,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let change = Rc::clone(&emit);
    Select::new(
        "settings-high-contrast",
        *theme,
        Some(chosen),
        high_contrast_entries(),
        Rc::clone(&controller.contrast_select),
    )
    .label("High contrast")
    .on_change(move |choice| change(ViewAction::SetHighContrast(choice)))
    .on_open_change(move |open| emit(ViewAction::ContrastMenu(open)))
    .build()
}

fn section_page(
    theme: &ArtisanTheme,
    page: SettingsPage,
    section: Element,
    include_header: bool,
) -> Element {
    if !include_header {
        return section;
    }
    ui::column()
        .child(ui::header(theme, page.label(), page::description(page)))
        .child(section)
        .build()
}

#[derive(PartialEq)]
struct ViewSnapshot {
    screen: Screen,
    account: editor_core::AccountPhase,
    account_error: Option<Problem>,
    local_forge: editor_core::LocalForge,
    device_settings: Vec<Option<serde_json::Value>>,
    device_settings_error: (bool, Option<Problem>),
    host_settings: Vec<Option<serde_json::Value>>,
    setting_failures: Vec<(SettingKey, super::preferences::SettingFailure)>,
    host_instance: Option<editor_core::HostInstance>,
    archive_loading: Option<(bool, bool, bool, Option<Problem>)>,
    archive_restoring: std::collections::HashSet<artisan_api::ThreadId>,
    archive_error: Option<Problem>,
    engine_busy: std::collections::BTreeSet<artisan_api::EngineId>,
    engine_cancelling: std::collections::BTreeSet<artisan_api::EngineId>,
    engine_errors: std::collections::HashMap<artisan_api::EngineId, Problem>,
    engine_versions: std::collections::HashSet<artisan_api::EngineId>,
    engine_codes: std::collections::HashMap<artisan_api::EngineId, String>,
    mcp_editing: Option<super::extensions::McpEditing>,
    mcp_saving: bool,
    mcp_sign_ins: Vec<(String, editor_core::McpSignInPhase, Option<String>)>,
    mcp_error: Option<Problem>,
    mcp_fields: Vec<String>,
    prompt: (bool, bool, bool, bool, Option<Problem>),
    forge_code: String,
    second_factor: String,
    menus: OpenMenus,
}

impl ViewSnapshot {
    #[expect(
        clippy::too_many_lines,
        reason = "the snapshot reads every field the settings view renders, one line each"
    )]
    fn capture(controller: &SettingsController, core: &Core) -> Self {
        let host = core.active_host();
        let preferences = editor_ui::preferences::Preferences::from_core(
            core,
            editor_ui::preferences::SystemAccess::default(),
        );
        let host_keys = [
            "retry_when_overloaded",
            "continue_after_usage_limit",
            "continue_after_restart",
            "suggestions",
            "you_should_know",
            "summary_titles",
            artisan_api::THREAD_RETENTION_DAYS_KEY,
            artisan_api::TRANSPORT_KEY,
        ];
        Self {
            screen: core.screen().clone(),
            account: core.account().phase().clone(),
            account_error: core.account().last_error().cloned(),
            local_forge: core.account().local_forge(),
            device_settings: vec![
                Some(serde_json::json!(preferences.theme.is_dark())),
                Some(serde_json::json!(preferences.fonts.key())),
                Some(serde_json::json!(preferences.outline_text)),
                Some(serde_json::json!(preferences.text_scale)),
                Some(serde_json::json!(preferences.high_contrast)),
                Some(serde_json::json!(matches!(
                    preferences.streaming,
                    editor_ui::preferences::StreamingMode::Swift
                ))),
                Some(serde_json::json!(preferences.lower_frame_rate)),
                Some(serde_json::json!(preferences.vsync)),
                Some(serde_json::json!(
                    core.settings()
                        .get("notifications")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true)
                )),
            ],
            device_settings_error: (
                core.settings().unreadable().is_some(),
                controller.editor_setting_error.clone(),
            ),
            host_settings: host_keys
                .iter()
                .map(|key| {
                    host.and_then(|host| {
                        if *key == artisan_api::TRANSPORT_KEY {
                            Some(serde_json::json!(artisan_api::transport_of(
                                host.setting(key)
                            )))
                        } else {
                            host.setting(key).cloned().or_else(|| {
                                artisan_api::setting_decl(key)
                                    .map(artisan_api::SettingDecl::default_value)
                            })
                        }
                    })
                })
                .collect(),
            setting_failures: host.map_or_else(Vec::new, |host| controller.setting_failures(host)),
            host_instance: host.map(HostData::instance),
            archive_loading: host.map(|host| {
                let page = host.thread_page(true);
                (
                    page.is_loading(),
                    page.at_latest(),
                    page.has_older(),
                    page.error().cloned(),
                )
            }),
            archive_restoring: controller.archive.restoring.clone(),
            archive_error: controller.archive.error.clone(),
            engine_busy: controller.engines.busy.clone(),
            engine_cancelling: controller.engines.cancelling.clone(),
            engine_errors: controller.engines.errors.clone(),
            engine_versions: controller.engines.choosing_version.clone(),
            engine_codes: controller
                .engines
                .login_codes
                .iter()
                .map(|(engine, editor)| (*engine, editor.borrow().text().to_owned()))
                .collect(),
            mcp_editing: controller.extensions.editing.clone(),
            mcp_saving: controller.extensions.saving,
            mcp_sign_ins: host
                .into_iter()
                .flat_map(|host| {
                    let servers = match host.mcp_servers() {
                        Ok(servers) => servers,
                        Err(_shown_in_the_mcp_section) => &[],
                    };
                    servers.iter().filter_map(move |server| {
                        let name = server.name.clone();
                        core.mcp_sign_in(&host.id, &name).map(|sign_in| {
                            (
                                name.clone(),
                                sign_in.phase().clone(),
                                sign_in.authorize_url().map(str::to_owned),
                            )
                        })
                    })
                })
                .collect(),
            mcp_error: controller.extensions.error.clone(),
            mcp_fields: controller
                .mcp_editors
                .iter()
                .map(|editor| editor.borrow().text().to_owned())
                .collect(),
            prompt: (
                controller.prompt.loaded,
                controller.prompt.pending,
                controller.prompt.newer,
                controller.reported_prompt_dirty,
                controller.prompt.error.clone(),
            ),
            forge_code: controller.forge_code.borrow().text().to_owned(),
            second_factor: controller.sign_in.second_factor.borrow().text().to_owned(),
            menus: controller.reported_menus.clone(),
        }
    }
}
