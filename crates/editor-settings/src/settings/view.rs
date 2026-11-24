use std::rc::Rc;

use aqua_ui::{
    JustifyContent,
    element::{Box as UiBox, Element, Scroll},
    style::{Length, Style},
};

use super::{
    about::BuildInfo,
    controller::{Gear, Menu, OpenMenus, SettingsController},
    preferences::SettingKey,
    ui::{self, Ink},
};
use editor_core::{Core, HostData, HostSwitch, Problem, Screen, SettingsPage, SettingsSection};
use editor_ui::{
    carved::{
        Carve, RADIUS_MD, Type,
        avatar::{Avatar, avatar},
        button::{Content, Tone as CarvedTone},
        glyph::Icon,
        menu::{ActionMenu, Entry, MenuItem},
    },
    kit::{ButtonTone, Space, Spaced, TextRole, Tone},
    segmented_control::{Segment, SegmentedControl},
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
    SearchChanged,
    EngineEdited,
    EnginePresentationChanged,
    EngineAction(artisan_api::EngineId, artisan_api::EngineCommand),
    VersionMenu(artisan_api::EngineId, bool),
    Gear(Option<Gear>),
    EngineUse(artisan_api::EngineId, String),
    EngineCode(artisan_api::EngineId),
    OpenApiKey,
    CancelApiKey,
    SaveApiKey,
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
    McpToggle(editor_core::HostInstance, artisan_api::McpServer, bool),
    SkillToggle(editor_core::HostInstance, String, bool),
    ReloadSkills,
    McpSave,
    McpCancel,
    McpEdited,
    McpPresentationChanged,
    McpSignIn(String),
    McpCancelSignIn(String),
    McpSignOut(String),
    OpenUrl(String),
    ToggleLicence(usize),
    ExportDiagnostics,
    DiagnosticsExported(Result<(), Problem>),
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
    SetTheme(editor_core::ThemePreference),
    SetOutlineText(bool),
    SetShowAllModels(bool),
    SetHideBadModelOptions(bool),
    SetCuratedModels(bool),
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
            | ViewAction::ExportDiagnostics
            | ViewAction::CopyText(_)
    ) {
        return false;
    }
    if matches!(action, ViewAction::SearchChanged) {
        if !controller.search_editor.borrow().text().trim().is_empty()
            && let Some(host) = core.active_host().map(|host| host.id.clone())
            && let Some(host) = core.host_mut(&host)
        {
            host.load_skill_catalog(false);
        }
        return true;
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
        ViewAction::Navigate(screen) => {
            let cleared = controller.clear_search();
            core.navigate(screen);
            if cleared {
                return true;
            }
        }
        ViewAction::EngineAction(engine, action) => controller.engine_action(core, engine, action),
        ViewAction::VersionMenu(engine, open) => {
            controller.reported_menus.report(Menu::Version, open);
            if open && controller.engines.choosing_version.insert(engine) {
                controller.engine_action(core, engine, artisan_api::EngineCommand::Versions {});
            }
            if !open {
                controller.engines.choosing_version.remove(&engine);
            }
        }
        ViewAction::Gear(gear) => {
            controller.gear = match (&controller.gear, gear) {
                (Some(open), Some(next)) if *open == next => None,
                (_, next) => next,
            };
        }
        ViewAction::EngineUse(engine, version) => {
            controller.engines.choosing_version.remove(&engine);
            controller.engine_action(core, engine, artisan_api::EngineCommand::Use { version });
        }
        ViewAction::ToggleLicence(index) => {
            controller.open_licence = (controller.open_licence != Some(index)).then_some(index);
            return true;
        }
        ViewAction::DiagnosticsExported(result) => {
            controller.diagnostics_result = Some(result);
            return true;
        }
        ViewAction::OpenApiKey => controller.engines.open_api_key(),
        ViewAction::CancelApiKey => controller.engines.api_key = None,
        ViewAction::SaveApiKey => {
            if let Some(key) = controller.engines.take_api_key() {
                controller.engine_action(
                    core,
                    artisan_api::EngineId::Codex,
                    artisan_api::EngineCommand::LoginApiKey { key },
                );
            }
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
        ViewAction::SkillToggle(instance, skill, enabled) => {
            controller.toggle_skill(core, instance, skill, enabled);
        }
        ViewAction::ReloadSkills => {
            if let Some(host) = core
                .active_host()
                .map(|host| host.id.clone())
                .and_then(|id| core.host_mut(&id))
            {
                host.load_skill_catalog(true);
            }
        }
        ViewAction::McpToggle(instance, server, enabled) => {
            controller.toggle_mcp(core, instance, server, enabled);
        }
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
        | ViewAction::SearchChanged
        | ViewAction::ExportDiagnostics
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
        ViewAction::SetTheme(choice) => {
            controller.set_editor_setting(core, "theme", choice.as_str().into());
        }
        ViewAction::SetCuratedModels(curated) => {
            controller.set_editor_setting(core, "curated_models", curated.into());
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

pub fn search_editor(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    EditorController::from_shared(
        Rc::clone(&controller.search_editor),
        move |event| match event {
            EditorEvent::Changed | EditorEvent::PresentationChanged => {
                emit(ViewAction::SearchChanged);
            }
            EditorEvent::Submitted
            | EditorEvent::SelectionChanged { .. }
            | EditorEvent::List(_)
            | EditorEvent::PastedFiles { .. } => {}
        },
    )
    .element_with_chrome(Mode::SingleLine, theme, "Search settings…", true, false)
    .with_key("settings-search")
}

pub fn build(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    build_info: &BuildInfo,
    _viewport: aqua_core::Size,
    _visible: bool,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    build_with_account(controller, core, theme, build_info, None, emit)
}

#[expect(
    clippy::too_many_lines,
    reason = "routes every settings page and wraps it in the shared frame"
)]
pub fn build_with_account(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    build_info: &BuildInfo,
    account: Option<&editor_ui::chrome::account::Account>,
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
    let search = controller
        .search_editor
        .borrow()
        .text()
        .trim()
        .to_lowercase();
    if !search.is_empty() {
        let results = settings_search(controller, core, theme, &search, &emit);
        return frame(
            controller,
            theme,
            results,
            "settings-search-results".to_owned(),
        );
    }
    if page == SettingsPage::GlobalPrompt && core.active_host().is_some() {
        return prompt(controller, core, theme, emit);
    }
    let emit_for_error = Rc::clone(&emit);
    let content = match (page.parent(), core.active_host()) {
        (SettingsPage::General, _) if page == SettingsPage::General => ui::column()
            .child(appearance(controller, theme, core, Rc::clone(&emit)))
            .child(interface(controller, theme, core, Rc::clone(&emit)))
            .child(performance(theme, core, emit))
            .build(),
        (SettingsPage::General, _)
            if page == SettingsPage::Section(SettingsSection::Appearance) =>
        {
            appearance(controller, theme, core, emit)
        }
        (SettingsPage::General, _) if page == SettingsPage::Section(SettingsSection::Streaming) => {
            streaming(controller, theme, core, emit)
        }
        (SettingsPage::General, _)
            if page == SettingsPage::Section(SettingsSection::Performance) =>
        {
            performance(theme, core, emit)
        }
        (SettingsPage::General, _)
            if page == SettingsPage::Section(SettingsSection::Notifications) =>
        {
            notifications(theme, core, emit)
        }
        (SettingsPage::Threads, Some(host))
            if page == SettingsPage::Threads
                || page == SettingsPage::Section(SettingsSection::Archive) =>
        {
            let mut content = ui::column();
            if page == SettingsPage::Threads {
                content = content
                    .child(thread_section(
                        theme,
                        host,
                        ThreadSection::Automation,
                        &emit,
                    ))
                    .child(thread_section(
                        theme,
                        host,
                        ThreadSection::Assistance,
                        &emit,
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
            thread_section(theme, host, ThreadSection::Automation, &emit)
        }
        (SettingsPage::Threads, Some(host))
            if page == SettingsPage::Section(SettingsSection::Assistance) =>
        {
            thread_section(theme, host, ThreadSection::Assistance, &emit)
        }
        (SettingsPage::Extensions, Some(host))
            if page == SettingsPage::Extensions
                || page == SettingsPage::Section(SettingsSection::McpServers) =>
        {
            let mut content = ui::column().child(super::mcp_view::section(
                controller,
                core,
                host,
                theme,
                Rc::clone(&emit),
            ));
            if page == SettingsPage::Extensions {
                content = content.child(skills_section(controller, host, theme, &emit));
            }
            content.build()
        }
        (SettingsPage::Extensions, Some(host))
            if page == SettingsPage::Section(SettingsSection::Skills) =>
        {
            skills_section(controller, host, theme, &emit)
        }
        (SettingsPage::Engine(engine), Some(host)) => {
            super::engine_view::page(controller, host, engine, page, theme, &emit)
        }
        (SettingsPage::Account, _) => {
            super::account_view::page(controller, core, theme, account, &emit)
        }
        (SettingsPage::About, _) => about(controller, theme, build_info, &emit),
        (SettingsPage::Hosts, _) => hosts(controller, theme, core, &emit),
        _ => ui::section(
            theme,
            page.label(),
            None,
            ui::card(
                theme,
                vec![ui::row_of(
                    theme,
                    ui::desc(theme, "Connect a computer first."),
                    Some(ui::button(
                        theme,
                        "settings-open-account",
                        "Open Computers",
                        ButtonTone::Neutral,
                        false,
                        move || {
                            emit(ViewAction::Navigate(Screen::Settings(SettingsPage::Hosts)));
                        },
                    )),
                    false,
                )],
            ),
        ),
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
    frame(
        controller,
        theme,
        content,
        format!("settings-screen-{page:?}"),
    )
}

fn frame(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    content: Element,
    key: String,
) -> Element {
    let scroll = Scroll::new()
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
                .child(ui::page_body().child(content).build())
                .build(),
        )
        .build()
        .with_key("settings-main-scroll")
        .debug_selector("settings-main-scroll");
    let edges = &controller.scroll_edges;
    UiBox::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .child(scroll)
                .build()
                .with_key(key),
        )
        .child(edges.band(
            "settings-edge-top",
            editor_ui::scroll_edge::Edge::Top,
            theme.colors.background,
        ))
        .child(edges.band(
            "settings-edge-bottom",
            editor_ui::scroll_edge::Edge::Bottom,
            theme.colors.background,
        ))
        .build()
}

#[expect(
    clippy::too_many_lines,
    reason = "the search builds live controls and links from one settings catalog"
)]
fn settings_search(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    search: &str,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let matches = |terms: &str| {
        search
            .split_whitespace()
            .all(|word| terms.to_lowercase().contains(word))
    };
    let mut results = Vec::new();
    for (group, rows) in [
        (
            "Appearance",
            Vec::from(appearance_rows(controller, theme, core, Rc::clone(emit))),
        ),
        (
            "Interface & Models",
            Vec::from(interface_rows(controller, theme, core, Rc::clone(emit))),
        ),
        (
            "Performance",
            Vec::from(performance_rows(theme, core, Rc::clone(emit))),
        ),
    ] {
        let rows: Vec<_> = rows
            .into_iter()
            .filter_map(|(terms, row)| matches(&format!("general {group} {terms}")).then_some(row))
            .collect();
        if !rows.is_empty() {
            results.push(ui::section(theme, group, None, ui::card(theme, rows)));
        }
    }
    if let Some(host) = core.active_host() {
        results.extend(search_host_controls(
            controller, host, theme, emit, &matches,
        ));
    }
    let mut links = Vec::new();
    for (terms, label, target) in [
        (
            "profile identity account avatar",
            "Account",
            SettingsPage::Account,
        ),
        (
            "computers forges machines pairing relay",
            "Computers",
            SettingsPage::Hosts,
        ),
        (
            "threads restore archive retention cleanup",
            "Threads",
            SettingsPage::Threads,
        ),
        (
            "system global prompt instructions",
            "Global Prompt",
            SettingsPage::GlobalPrompt,
        ),
        (
            "mcp servers tools extensions",
            "MCPs",
            SettingsPage::Section(SettingsSection::McpServers),
        ),
        (
            "skills extensions marketplace",
            "Skills",
            SettingsPage::Section(SettingsSection::Skills),
        ),
        (
            "about build version diagnostics licences licenses",
            "About",
            SettingsPage::About,
        ),
    ] {
        if matches(terms) {
            let send = Rc::clone(emit);
            links.push(ui::row(
                theme,
                label,
                "",
                Some(ui::button(
                    theme,
                    format!("settings-search-{target:?}"),
                    "Open",
                    ButtonTone::Quiet,
                    false,
                    move || send(ViewAction::Navigate(Screen::Settings(target))),
                )),
            ));
        }
    }
    if !links.is_empty() {
        results.push(ui::section(theme, "Pages", None, ui::card(theme, links)));
    }
    if let Some(host) = core.active_host() {
        let mut engines = Vec::new();
        for status in host.engines.values() {
            let name = status.engine.display_name();
            let terms = if status.engine == artisan_api::EngineId::Codex {
                format!("{name} engine account version install API key")
            } else {
                format!("{name} engine account version install")
            };
            if matches(&terms) {
                let send = Rc::clone(emit);
                let engine = status.engine;
                engines.push(ui::row(
                    theme,
                    name,
                    "",
                    Some(ui::button(
                        theme,
                        format!("settings-search-{engine}"),
                        "Open",
                        ButtonTone::Quiet,
                        false,
                        move || {
                            send(ViewAction::Navigate(Screen::Settings(
                                SettingsPage::Engine(engine),
                            )));
                        },
                    )),
                ));
            }
        }
        if !engines.is_empty() {
            results.push(ui::section(
                theme,
                "Engines",
                None,
                ui::card(theme, engines),
            ));
        }
    }
    if results.is_empty() {
        results.push(ui::card(
            theme,
            vec![ui::note(
                theme,
                &format!("No settings matching “{}”.", search.trim()),
            )],
        ));
    }
    ui::column().children(results).build()
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

const LICENCE_ROW: f32 = 56.0;
const LICENCE_ROWS: u8 = 10;

#[derive(Clone)]
struct LicenceRow {
    index: usize,
    licence: &'static super::licences::Licence,
    open: bool,
    edges: ui::Edges,
}

fn licence_list(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    build: &BuildInfo,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let licences = match build.licences.load() {
        Ok(licences) => licences,
        Err(error) => {
            return ui::card(
                theme,
                vec![ui::block(
                    ui::problem(
                        theme,
                        "settings-licences-error",
                        &Problem::caused_by("Couldn't read the bundled licences.", error),
                    )
                    .build(),
                )],
            );
        }
    };
    if licences.is_empty() {
        return ui::card(
            theme,
            vec![ui::note(theme, "This build carries no licences.")],
        );
    }
    let final_row = licences.len() - 1;
    let rows: Vec<LicenceRow> = licences
        .iter()
        .enumerate()
        .map(|(index, licence)| LicenceRow {
            index,
            licence,
            open: controller.open_licence == Some(index),
            edges: ui::Edges {
                first: index == 0,
                last: index == final_row,
            },
        })
        .collect();
    let visible = u8::try_from(rows.len()).map_or(LICENCE_ROWS, |rows| rows.min(LICENCE_ROWS));
    let theme = *theme;
    let emit = Rc::clone(emit);
    let list = aqua_ui::collection::List::new()
        .items(rows)
        .key(|row| format!("settings-licence-{}", row.index))
        .row(move |row| {
            let send = Rc::clone(&emit);
            let index = row.index;
            let item = ui::licence(
                &theme,
                format!("settings-licence-{index}"),
                &row.licence.name,
                row.open.then_some(row.licence.text.as_str()),
                row.edges,
                move || send(ViewAction::ToggleLicence(index)),
            );
            if row.edges.first {
                item
            } else {
                ui::ruled(&theme, item)
            }
        })
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Px(f32::from(visible) * LICENCE_ROW)),
        )
        .estimated_row_height(Length::Px(LICENCE_ROW))
        .build()
        .with_key("settings-licences");
    ui::card(&theme, vec![list])
}

const ABOUT_TITLE: Type = Type::sans(17.0, 22.0, 500);

fn about(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    build: &BuildInfo,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let export = Rc::clone(emit);
    let mark = ui::raised(
        avatar(
            theme,
            None,
            "A",
            Avatar {
                edge: 44.0,
                radius: RADIUS_MD,
                initial: Type::sans(18.4, 22.0, 600),
                ring: false,
            },
        ),
        RADIUS_MD,
        theme,
    );
    let carve = Carve::of(theme);
    let heading = ui::stack_gap(3.0)
        .child(
            ui::inline(8.0)
                .child(ui::typed(theme, ABOUT_TITLE, Ink::Plain, "Artisan Editor"))
                .child(ui::inset_tag(
                    theme,
                    if build.official() {
                        "Built by Artisan Street"
                    } else {
                        "Local build"
                    },
                ))
                .build(),
        )
        .child(
            Type::mono(13.0, 19.0, 400)
                .text(theme, carve.ink_dim, build.identifier())
                .wrap(false)
                .build(),
        )
        .build();
    let identity = ui::inline(14.0).child(mark).child(heading).build();
    let hero = ui::about_hero(
        identity,
        ui::typed(theme, ui::DESC, Ink::Muted, build.description()),
    );
    let footer = ui::row_of(
        theme,
        ui::carved(
            theme,
            "settings-export-diagnostics",
            CarvedTone::Horizon,
            Content::IconLabel(Icon::Download, "Export diagnostics".to_owned()),
            false,
            move || export(ViewAction::ExportDiagnostics),
        ),
        None,
        true,
    );
    let mut build_rows = vec![hero, footer];
    if let Some(result) = &controller.diagnostics_result {
        build_rows.push(match result {
            Ok(()) => ui::note(theme, "Diagnostics exported."),
            Err(problem) => {
                ui::block(ui::problem(theme, "settings-export-diagnostics-error", problem).build())
            }
        });
    }
    let licences = licence_list(controller, theme, build, emit);
    ui::column()
        .child(ui::section(
            theme,
            "Build",
            None,
            ui::card(theme, build_rows),
        ))
        .child(ui::section(theme, "Licences", None, licences))
        .build()
}

fn hosts(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    core: &Core,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let relay = ui::section(
        theme,
        "Artisan Datagram Relay",
        None,
        ui::card(
            theme,
            vec![ui::row(
                theme,
                "Artisan Datagram Relay",
                "Artisan Street offers this service as a relay and broker for pairing machines and connecting any host without hassle, so you can talk to your machines anywhere in the world, at any time. Artisan Datagram Relay is, to the best of its capabilities, end-to-end encrypted. Artisan Street does not claim this service is trustless.",
                None,
            )],
        ),
    );
    let enrolled = core.account().machines();
    let open: Vec<_> = core
        .hosts()
        .filter(|(config, _)| !enrolled.iter().any(|machine| machine.id == config.id.0))
        .map(|(config, host)| {
            let (status, ink) = match &host.status {
                editor_core::ConnectionStatus::Live => ("Connected", Ink::Success),
                editor_core::ConnectionStatus::Connecting
                | editor_core::ConnectionStatus::Reconnecting { .. } => ("Connecting…", Ink::Muted),
                editor_core::ConnectionStatus::Offline { .. } => ("Disconnected", Ink::Danger),
            };
            ui::row(
                theme,
                config.name.clone(),
                "",
                Some(ui::line(theme, TextRole::Label, ink, status)),
            )
        })
        .collect();
    let mut content = ui::column()
        .child(relay)
        .children(super::account_view::computers(
            controller, core, theme, emit,
        ));
    if !open.is_empty() {
        content = content.child(ui::section(
            theme,
            "Open Forges",
            None,
            ui::card(theme, open),
        ));
    }
    content.build()
}

fn notifications(theme: &ArtisanTheme, core: &Core, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    let enabled = core.settings().preferences().notifications.on();
    let control = Switch::new("settings-switch-notify", *theme, enabled, "Notify me")
        .on_change(move |enabled| emit(ViewAction::SetNotifications(enabled)))
        .build();
    let row = ui::row(
        theme,
        "Notify me",
        "Uses your system's notifications to tell you when a thread finishes, needs attention or fails.",
        Some(control),
    );
    ui::section(theme, "Notifications", None, ui::card(theme, vec![row]))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ThreadSection {
    Automation,
    Assistance,
}

type ThreadControl = (&'static str, HostSwitch, &'static str, &'static str);

fn thread_controls(section: ThreadSection) -> (&'static str, &'static [ThreadControl]) {
    match section {
        ThreadSection::Automation => (
            "Recovery & Continuation",
            &[
                (
                    "settings-switch-retry-overloaded",
                    HostSwitch::RetryWhenOverloaded,
                    "Keep retrying when overloaded",
                    "When the model is overloaded, Artisan keeps rescheduling the thread until the turn finishes.",
                ),
                (
                    "settings-switch-continue-usage-limit",
                    HostSwitch::ContinueAfterUsageLimit,
                    "Continue when usage limit resets",
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
                    "Suggest next message",
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
    }
}

fn thread_section(
    theme: &ArtisanTheme,
    host: &HostData,
    section: ThreadSection,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let (title, entries) = thread_controls(section);
    let rows = entries
        .iter()
        .map(|&(id, switch, label, _)| host_switch(theme, host, switch, id, label, emit))
        .collect();
    ui::section(theme, title, None, ui::card(theme, rows))
}

fn host_switch(
    theme: &ArtisanTheme,
    host: &HostData,
    switch: HostSwitch,
    id: &str,
    label: &str,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let send = Rc::clone(emit);
    let host_id = host.id.clone();
    let instance = host.instance();
    let control = Switch::new(id.to_owned(), *theme, host.switch(switch), label.to_owned())
        .recessed()
        .on_change(move |enabled| {
            send(ViewAction::SetHostSetting(
                host_id.clone(),
                instance,
                switch.key(),
                enabled,
            ));
        })
        .build();
    ui::row(theme, label, "", Some(control))
}

fn search_host_controls(
    controller: &SettingsController,
    host: &HostData,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
    matches: &impl Fn(&str) -> bool,
) -> Vec<Element> {
    let mut sections = Vec::new();
    for section in [ThreadSection::Automation, ThreadSection::Assistance] {
        let (title, entries) = thread_controls(section);
        let rows: Vec<_> = entries
            .iter()
            .filter(|&&(_, _, label, description)| {
                matches(&format!("threads {title} {label} {description}"))
            })
            .map(|&(id, switch, label, _)| host_switch(theme, host, switch, id, label, emit))
            .collect();
        if !rows.is_empty() {
            sections.push(ui::section(theme, title, None, ui::card(theme, rows)));
        }
    }
    if matches("threads delete inactive threads retention cleanup archive days") {
        sections.push(super::archive_view::cleanup(
            controller,
            host,
            theme,
            Rc::clone(emit),
        ));
    }
    let prompt_label = "System prompt for Artisan Editor";
    if matches(&format!(
        "{prompt_label} forge global prompt built-in instructions"
    )) {
        let row = host_switch(
            theme,
            host,
            HostSwitch::ArtisanSystemPrompt,
            "settings-artisan-system-prompt",
            prompt_label,
            emit,
        );
        sections.push(ui::section(
            theme,
            "Global Prompt",
            None,
            ui::card(theme, vec![row]),
        ));
    }
    if let Ok(servers) = host.mcp_servers() {
        let rows: Vec<_> = servers
            .iter()
            .filter(|server| matches(&format!("mcp server tool {}", server.name)))
            .map(|server| {
                ui::row(
                    theme,
                    server.name.clone(),
                    "",
                    Some(super::mcp_view::toggle(
                        controller, host, server, theme, emit,
                    )),
                )
            })
            .collect();
        if !rows.is_empty() {
            sections.push(ui::section(theme, "MCPs", None, ui::card(theme, rows)));
        }
    }
    if let Some(catalog) = host.skill_catalog() {
        let rows: Vec<_> = catalog
            .iter()
            .filter(|skill| {
                skill.origin != artisan_api::SkillOrigin::Discovered
                    && matches(&format!("skills marketplace {}", skill.name))
            })
            .map(|skill| skill_row(controller, host, skill, theme, emit))
            .collect();
        if !rows.is_empty() {
            sections.push(ui::section(theme, "Skills", None, ui::card(theme, rows)));
        }
    }
    sections
}

fn skill_row(
    controller: &SettingsController,
    host: &HostData,
    skill: &artisan_api::CatalogSkill,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let instance = host.instance();
    let send = Rc::clone(emit);
    let id = skill.id.clone();
    let control = Switch::new(
        format!("settings-skill-{}", skill.id),
        *theme,
        skill.enabled,
        skill.name.clone(),
    )
    .recessed()
    .disabled(controller.skill_pending.contains(&skill.id))
    .on_change(move |enabled| send(ViewAction::SkillToggle(instance, id.clone(), enabled)))
    .build();
    let gear = Gear::Skill(skill.id.clone());
    let open = Rc::clone(emit);
    let chosen = gear.clone();
    let controls = ui::actions()
        .child(control)
        .child(ui::gear(
            theme,
            gear.key(),
            format!("{} options", skill.name),
            move || open(ViewAction::Gear(Some(chosen.clone()))),
        ))
        .build();
    ui::row(theme, skill.name.clone(), "", Some(controls))
}

fn skills_section(
    controller: &SettingsController,
    host: &HostData,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let open = Rc::clone(emit);
    let action = ui::button(
        theme,
        "settings-manage-skills",
        "Open Marketplace",
        ButtonTone::Primary,
        false,
        move || open(ViewAction::Navigate(Screen::Marketplace)),
    );
    let mut rows = Vec::new();
    if let Some(catalog) = host.skill_catalog() {
        rows.extend(
            catalog
                .iter()
                .filter(|skill| skill.origin != artisan_api::SkillOrigin::Discovered)
                .map(|skill| skill_row(controller, host, skill, theme, emit)),
        );
    }
    if let Some(problem) = host.catalog_load().error.as_ref() {
        let retry = Rc::clone(emit);
        rows.push(ui::problem(theme, "settings-skills-load", problem).build());
        rows.push(ui::button(
            theme,
            "settings-skills-retry",
            "Try again",
            ButtonTone::Quiet,
            false,
            move || retry(ViewAction::ReloadSkills),
        ));
    } else if rows.is_empty() {
        rows.push(ui::note(
            theme,
            if host.catalog_load().loading {
                "Loading skills…"
            } else {
                "No skills installed."
            },
        ));
    }
    if let Some(problem) = &controller.skill_error {
        rows.push(ui::problem(theme, "settings-skills-mutation", problem).build());
    }
    ui::section(theme, "Skills", Some(action), ui::card(theme, rows))
}

#[expect(
    clippy::too_many_lines,
    reason = "the prompt page coordinates load and save controls with the editor"
)]
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
    let built_in = core.active_host().map(|host| {
        host_switch(
            theme,
            host,
            HostSwitch::ArtisanSystemPrompt,
            "settings-artisan-system-prompt",
            "System prompt for Artisan Editor",
            &emit,
        )
    });
    let setting_failures = core
        .active_host()
        .and_then(|host| host_setting_failures(controller, host, theme, &emit));
    let page = ui::prompt_page().children(setting_failures).children(
        built_in.map(|row| ui::section(theme, "Built-in", None, ui::card(theme, vec![row]))),
    );
    let field = Scroll::new()
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .axis(aqua_ui::style::Axis::Column)
                .flex_grow(1.0)
                .min_height(Length::Pct(0.0)),
        )
        .child(editor.element_with_notice_keyed(
            "settings-global-prompt-editor",
            Mode::Source,
            theme,
            if form.loaded {
                "Instructions shared across every thread…"
            } else {
                "Reading the global prompt…"
            },
            true,
            false,
        ))
        .build()
        .with_key("global-prompt-scroll");
    let mut above = ui::stack_gap(12.0)
        .width(Length::Pct(100.0))
        .child(prompt_reconciliation(controller, core, theme, &emit));
    if let Some(problem) = form.error.as_ref().or_else(|| {
        core.active_host()
            .and_then(|host| host.prompt_load().error.as_ref())
    }) {
        above = above.child(ui::problem(theme, "settings-global-prompt-error", problem).build());
    }
    let body = ui::prompt_body(theme, above.build(), field);
    let mut actions = ui::actions();
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
    let card = ui::card_box(
        theme,
        vec![
            body,
            ui::row_of(theme, UiBox::new().build(), Some(actions.build()), true),
        ],
    )
    .style(Style::new().flex_grow(1.0).min_height(Length::Pct(0.0)))
    .build();
    page.child(
        ui::section(theme, "Global Prompt", None, card)
            .style(Style::new().flex_grow(1.0).min_height(Length::Pct(0.0))),
    )
    .build()
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

fn performance(theme: &ArtisanTheme, core: &Core, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    ui::section(
        theme,
        "Performance",
        None,
        ui::card(
            theme,
            performance_rows(theme, core, emit)
                .into_iter()
                .map(|(_, row)| row)
                .collect(),
        ),
    )
}

fn performance_rows(
    theme: &ArtisanTheme,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
) -> [(&'static str, Element); 2] {
    let preferences = core.settings().preferences();
    let refresh = Rc::clone(&emit);
    let rates = SegmentedControl::new(
        "settings-refresh-rate",
        *theme,
        "Refresh rate",
        preferences.lower_refresh_rate.on(),
        Segment::new(false, "Display"),
    )
    .options([Segment::new(true, "60 Hz")])
    .carved()
    .on_change(move |limited| refresh(ViewAction::SetFrameRate(limited)))
    .build();
    let vsync = Switch::new(
        "settings-switch-vsync",
        *theme,
        preferences.graphics_vsync.on(),
        "V-Sync",
    )
    .recessed()
    .on_change(move |on| emit(ViewAction::SetVsync(on)))
    .build();
    [
        (
            "refresh rate display monitor fps 60 hz",
            ui::row(theme, "Refresh rate", "", Some(rates)),
        ),
        (
            "vsync v-sync vertical sync gpu",
            ui::row(theme, "V-Sync", "", Some(vsync)),
        ),
    ]
}

fn streaming_entries() -> Vec<SelectEntry<&'static str>> {
    vec![
        SelectEntry::item("smooth", "Smooth"),
        SelectEntry::item("swift", "Swift"),
    ]
}

fn streaming_value(core: &Core) -> &'static str {
    match core.settings().preferences().streaming {
        editor_core::Streaming::Smooth => "smooth",
        editor_core::Streaming::Swift => "swift",
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
) -> Element {
    ui::section(
        theme,
        "Text streaming",
        None,
        ui::card(theme, vec![streaming_row(controller, theme, core, emit)]),
    )
}

fn streaming_row(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let change = Rc::clone(&emit);
    let select = Select::new(
        "settings-streaming-mode",
        *theme,
        Some(streaming_value(core)),
        streaming_entries(),
        Rc::clone(&controller.streaming_select),
    )
    .label("Text streaming")
    .on_change(move |mode| change(ViewAction::SetStreaming(mode)))
    .on_open_change(move |open| emit(ViewAction::StreamingMenu(open)))
    .build();
    ui::row(theme, "Text streaming", "", Some(ui::fixed(select)))
}

fn interface(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let rows = interface_rows(controller, theme, core, emit);
    ui::section(
        theme,
        "Interface & Models",
        None,
        ui::card(theme, rows.into_iter().map(|(_, row)| row).collect()),
    )
}

fn interface_rows(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
) -> [(&'static str, Element); 3] {
    let preferences = core.settings().preferences();
    let notify = Rc::clone(&emit);
    let notifications = Switch::new(
        "settings-switch-notify",
        *theme,
        preferences.notifications.on(),
        "System notifications",
    )
    .recessed()
    .on_change(move |value| notify(ViewAction::SetNotifications(value)))
    .build();
    let curated = Switch::new(
        "settings-switch-models",
        *theme,
        !preferences.show_all_models.on() && preferences.hide_bad_model_options.on(),
        "Curated models",
    )
    .recessed()
    .on_change({
        let emit = Rc::clone(&emit);
        move |value| emit(ViewAction::SetCuratedModels(value))
    })
    .build();
    [
        (
            "text streaming smooth swift",
            streaming_row(controller, theme, core, emit),
        ),
        (
            "system notifications notify alerts",
            ui::row(theme, "System notifications", "", Some(notifications)),
        ),
        (
            "curated models model picker show all",
            ui::row(theme, "Curated models", "", Some(curated)),
        ),
    ]
}

fn font_entries() -> Vec<SelectEntry<&'static str>> {
    vec![
        SelectEntry::item("system", "System"),
        SelectEntry::item("spline", "Spline"),
    ]
}

fn font_value(core: &Core) -> &'static str {
    match core.settings().preferences().fonts {
        editor_core::Fonts::Spline => "spline",
        editor_core::Fonts::System => "system",
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
) -> Element {
    let rows = appearance_rows(controller, theme, core, emit);
    ui::section(
        theme,
        "Appearance",
        None,
        ui::card(theme, rows.into_iter().map(|(_, row)| row).collect()),
    )
}

fn appearance_rows(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
) -> [(&'static str, Element); 5] {
    let preferences = core.settings().preferences();
    let send = Rc::clone(&emit);
    let appearance = SegmentedControl::new(
        "settings-theme",
        *theme,
        "Theme",
        preferences.theme,
        Segment::new(editor_core::ThemePreference::System, "System").glyph(Icon::Laptop),
    )
    .options([
        Segment::new(editor_core::ThemePreference::Light, "Light").glyph(Icon::Sun),
        Segment::new(editor_core::ThemePreference::Dark, "Dark").glyph(Icon::Moon),
    ])
    .carved()
    .on_change(move |value| send(ViewAction::SetTheme(value)))
    .build();
    let font_send = Rc::clone(&emit);
    let font_open = Rc::clone(&emit);
    let fonts = Select::new(
        "settings-font-set",
        *theme,
        Some(font_value(core)),
        font_entries(),
        Rc::clone(&controller.font_select),
    )
    .label("Fonts")
    .on_change(move |font| font_send(ViewAction::SetFont(font)))
    .on_open_change(move |open| font_open(ViewAction::FontMenu(open)))
    .build();
    let contrast_controls = contrast_switch(theme, preferences.high_contrast, &emit);
    let render_emit = Rc::clone(&emit);
    let render_menu = Rc::clone(&emit);
    let render = Select::new(
        "settings-text-rendering",
        *theme,
        Some(Some(preferences.outline_text.on())),
        render_entries(),
        Rc::clone(&controller.contrast_select),
    )
    .label("Text rendering")
    .on_change(move |choice| render_emit(ViewAction::SetOutlineText(choice == Some(true))))
    .on_open_change(move |open| render_menu(ViewAction::ContrastMenu(open)))
    .build();
    [
        (
            "theme dark light color",
            ui::row(theme, "Theme", "", Some(appearance)),
        ),
        (
            "fonts font family spline typography",
            ui::row(theme, "Fonts", "", Some(ui::fixed(fonts))),
        ),
        (
            "text size font scale zoom bigger accessibility",
            ui::row(
                theme,
                "Text size",
                "",
                Some(ui::fixed(text_size(
                    controller,
                    theme,
                    preferences.text_percent,
                    emit,
                ))),
            ),
        ),
        (
            "text rendering outline raster vector system",
            ui::row(theme, "Text rendering", "", Some(ui::fixed(render))),
        ),
        (
            "higher contrast borders accessibility",
            ui::row(theme, "Higher contrast", "", Some(contrast_controls)),
        ),
    ]
}

fn contrast_switch(
    theme: &ArtisanTheme,
    chosen: Option<bool>,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let contrast_emit = Rc::clone(emit);
    let higher = Switch::new(
        "settings-higher-contrast",
        *theme,
        chosen.unwrap_or(theme.colors.contrast == editor_ui::theme::Contrast::High),
        "Higher contrast",
    )
    .recessed()
    .on_change(move |enabled| contrast_emit(ViewAction::SetHighContrast(Some(enabled))))
    .build();
    if chosen.is_some() {
        let follow_system = Rc::clone(emit);
        ui::actions()
            .child(ui::button(
                theme,
                "settings-contrast-follow-system",
                "Match system",
                ButtonTone::Quiet,
                false,
                move || follow_system(ViewAction::SetHighContrast(None)),
            ))
            .child(higher)
            .build()
    } else {
        higher
    }
}

fn text_size_entries() -> Vec<SelectEntry<Option<u32>>> {
    let sizes = artisan_api::choice_values("text_scale");
    sizes
        .iter()
        .map(|size| match size {
            None => SelectEntry::item(None, "Match system"),
            Some(80) => SelectEntry::item(Some(80), "Smallest"),
            Some(90) => SelectEntry::item(Some(90), "Small"),
            Some(100) => SelectEntry::item(Some(100), "Balanced"),
            Some(125) => SelectEntry::item(Some(125), "Bigger"),
            Some(150) => SelectEntry::item(Some(150), "Biggest"),
            Some(percent) => SelectEntry::item(Some(*percent), format!("{percent}%")),
        })
        .collect()
}

pub fn text_size_menu(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<Option<u32>> {
    let chosen = core.settings().preferences().text_percent;
    let change = Rc::clone(&emit);
    SelectMenu::new(
        *theme,
        Some(chosen),
        text_size_entries(),
        Rc::clone(&controller.text_size_select),
        ui::CONTROL_WIDTH,
    )
    .on_change(move |percent| change(ViewAction::SetTextScale(percent)))
    .on_open_change(move |open| emit(ViewAction::TextSizeMenu(open)))
}

fn text_size(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    chosen: Option<u32>,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let change = Rc::clone(&emit);
    Select::new(
        "settings-text-size",
        *theme,
        Some(chosen),
        text_size_entries(),
        Rc::clone(&controller.text_size_select),
    )
    .label("Text size")
    .on_change(move |percent| change(ViewAction::SetTextScale(percent)))
    .on_open_change(move |open| emit(ViewAction::TextSizeMenu(open)))
    .build()
}

fn render_entries() -> Vec<SelectEntry<Option<bool>>> {
    vec![
        SelectEntry::item(Some(false), "System"),
        SelectEntry::item(Some(true), "Vector"),
    ]
}

pub fn text_rendering_menu(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<Option<bool>> {
    let send = Rc::clone(&emit);
    SelectMenu::new(
        *theme,
        Some(Some(core.settings().preferences().outline_text.on())),
        render_entries(),
        Rc::clone(&controller.contrast_select),
        ui::CONTROL_WIDTH,
    )
    .on_change(move |choice| send(ViewAction::SetOutlineText(choice == Some(true))))
    .on_open_change(move |open| emit(ViewAction::ContrastMenu(open)))
}

#[derive(PartialEq)]
struct ViewSnapshot {
    screen: Screen,
    account: editor_core::AccountPhase,
    account_error: Option<Problem>,
    local_forge: editor_core::LocalForge,
    machine_connections: Vec<(String, bool, bool)>,
    device_settings: ShownPreferences,
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
    api_key_open: bool,
    skill_pending: std::collections::HashSet<String>,
    skill_error: Option<Problem>,
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
    gear: Option<Gear>,
}

type ShownPreferences = (
    [bool; 6],
    [Option<bool>; 2],
    editor_core::ThemePreference,
    Option<u32>,
    editor_core::Fonts,
    editor_core::Streaming,
    editor_core::Browser,
);

fn shown(preferences: &editor_core::EditorPreferences) -> ShownPreferences {
    (
        [
            preferences.outline_text.on(),
            preferences.notifications.on(),
            preferences.lower_refresh_rate.on(),
            preferences.graphics_vsync.on(),
            preferences.hide_bad_model_options.on(),
            preferences.show_all_models.on(),
        ],
        [preferences.reduced_motion, preferences.high_contrast],
        preferences.theme,
        preferences.text_percent,
        preferences.fonts,
        preferences.streaming,
        preferences.browser.clone(),
    )
}

impl ViewSnapshot {
    #[expect(
        clippy::too_many_lines,
        reason = "the snapshot compares all settings dispatch effects in one state"
    )]
    fn capture(controller: &SettingsController, core: &Core) -> Self {
        let host = core.active_host();
        let host_keys = [
            "retry_when_overloaded",
            "continue_after_usage_limit",
            "continue_after_restart",
            "suggestions",
            "you_should_know",
            "summary_titles",
            "artisan_system_prompt",
            artisan_api::THREAD_RETENTION_DAYS_KEY,
            artisan_api::TRANSPORT_KEY,
        ];
        let host_keys = host_keys
            .into_iter()
            .chain(artisan_api::EngineId::ALL.map(artisan_api::EngineId::enabled_setting));
        Self {
            screen: core.screen().clone(),
            account: core.account().phase().clone(),
            account_error: core.account().last_error().cloned(),
            local_forge: core.account().local_forge(),
            machine_connections: if matches!(core.screen(), Screen::Settings(SettingsPage::Hosts)) {
                core.account()
                    .machines()
                    .into_iter()
                    .map(|machine| {
                        let live = core.machine_live(&machine.id);
                        let connecting = core.machine_connecting(&machine.id);
                        (machine.id, live, connecting)
                    })
                    .collect()
            } else {
                Vec::new()
            },
            device_settings: shown(core.settings().preferences()),
            device_settings_error: (
                core.settings().unreadable().is_some(),
                controller.editor_setting_error.clone(),
            ),
            host_settings: host_keys
                .map(|key| {
                    host.and_then(|host| {
                        if key == artisan_api::TRANSPORT_KEY {
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
            api_key_open: controller.engines.api_key.is_some(),
            skill_pending: controller.skill_pending.clone(),
            skill_error: controller.skill_error.clone(),
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
            gear: controller.gear.clone(),
        }
    }
}

pub fn gear_menu(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<(String, ActionMenu<ViewAction>)> {
    let gear = controller.gear.clone()?;
    let entries = match &gear {
        Gear::Computer(id) => {
            let open = core.machine_live(id).then(|| {
                [
                    Entry::Item(MenuItem::new(
                        ViewAction::ForgeOpen(id.clone()),
                        "Open computer",
                        Icon::Laptop,
                    )),
                    Entry::Separator,
                ]
            });
            open.into_iter()
                .flatten()
                .chain([Entry::Item(
                    MenuItem::new(
                        ViewAction::ForgeRemove(id.clone()),
                        "Delete computer",
                        Icon::Trash,
                    )
                    .danger(),
                )])
                .collect()
        }
        Gear::Mcp(server) => {
            let signed_in = core.active_host().is_some_and(|host| {
                host.settings.mcp_auth().is_ok_and(|auth| {
                    auth.get(&server.name) == Some(&artisan_api::McpAuthStatus::SignedIn)
                })
            });
            let mut entries = Vec::new();
            if signed_in {
                entries.push(Entry::Item(MenuItem::new(
                    ViewAction::McpSignOut(server.name.clone()),
                    "Sign out",
                    Icon::Logout,
                )));
            }
            entries.push(Entry::Item(MenuItem::new(
                ViewAction::McpOpen(Some(server.clone())),
                "Edit server",
                Icon::Pencil,
            )));
            entries.push(Entry::Separator);
            entries.push(Entry::Item(
                MenuItem::new(
                    ViewAction::McpRemove(server.clone()),
                    "Delete server",
                    Icon::Trash,
                )
                .danger(),
            ));
            entries
        }
        Gear::Skill(_) => vec![Entry::Item(MenuItem::new(
            ViewAction::Navigate(Screen::Marketplace),
            "Open Marketplace",
            Icon::FileText,
        ))],
        Gear::Account(_) | Gear::AddAccount(_) => super::engine_view::gear_entries(&gear),
    };
    let pick = Rc::clone(&emit);
    Some((
        gear.key(),
        ActionMenu::new(
            *theme,
            entries,
            move |action| pick(action),
            move || emit(ViewAction::Gear(None)),
        ),
    ))
}

pub fn engine_version_menu(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<SelectMenu<String>> {
    let Screen::Settings(SettingsPage::Engine(engine)) = core.screen() else {
        return None;
    };
    let status = core.active_host()?.engines.get(engine)?;
    Some(super::engine_view::version_menu(
        controller, status, theme, emit,
    ))
}
