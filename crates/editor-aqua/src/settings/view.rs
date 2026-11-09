use std::rc::Rc;

use aqua_core::Insets;
use aqua_text::{FontFamily, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Scroll, Text},
    style::{Length, Style},
};

use super::{about::BuildInfo, controller::SettingsController, page, ui};
use crate::core::{ConnectionStatus, Core, Screen, SettingsPage, SettingsSection};
use crate::ui::{
    button::{Button, ButtonShape, ButtonVariant},
    select::{Select, SelectEntry, SelectMenu},
    switch::{Switch, SwitchSize},
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
    ClaudeTransport(crate::core::HostId, crate::core::HostInstance, bool),
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
    SetRetention(crate::core::HostId, crate::core::HostInstance, Option<u32>),
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
    CopyText(String),
    PromptChanged,
    PromptPresentationChanged,
    SavePrompt,
    LoadLatestPrompt,
    FontMenu(bool),
    SetFont(&'static str),
    SetDarkMode(bool),
    SetOutlineText(bool),
    SetShowAllModels(bool),
    SetHideBadModelOptions(bool),
    StreamingMenu(bool),
    SetStreaming(&'static str),
    SetFrameRate(bool),
    SetVsync(bool),
    SetNotifications(bool),
    SetHostSetting(
        crate::core::HostId,
        crate::core::HostInstance,
        &'static str,
        bool,
    ),
}

pub fn dispatch(_controller: &mut SettingsController, core: &mut Core, action: ViewAction) -> bool {
    match &action {
        ViewAction::EngineEdited
        | ViewAction::AccountEdited
        | ViewAction::McpEdited
        | ViewAction::OpenUrl(_)
        | ViewAction::CopyText(_) => return false,
        ViewAction::PromptPresentationChanged
        | ViewAction::McpPresentationChanged
        | ViewAction::AccountPresentationChanged
        | ViewAction::EnginePresentationChanged => return true,
        ViewAction::PromptChanged => {
            _controller.prompt.changed();
            let dirty = _controller.prompt_editor.borrow().text() != _controller.prompt.saved;
            let changed = _controller.reported_prompt_dirty != dirty;
            _controller.reported_prompt_dirty = dirty;
            return changed;
        }
        _ => {}
    }
    let before = ViewSnapshot::capture(_controller, core);
    match action {
        ViewAction::Navigate(screen) => core.navigate(screen),
        ViewAction::EngineEdited | ViewAction::EnginePresentationChanged => {}
        ViewAction::EngineAction(engine, action) => _controller.engine_action(core, engine, action),
        ViewAction::EngineVersions(engine) => {
            if !_controller.engines.choosing_version.remove(&engine) {
                _controller.engines.choosing_version.insert(engine);
                _controller.engine_action(core, engine, artisan_api::EngineCommand::Versions {});
            }
        }
        ViewAction::EngineUse(engine, version) => {
            _controller.engines.choosing_version.remove(&engine);
            _controller.engine_action(core, engine, artisan_api::EngineCommand::Use { version });
        }
        ViewAction::EngineCode(engine) => {
            if let Some(code) = _controller.engines.login_code(engine) {
                _controller.engine_action(
                    core,
                    engine,
                    artisan_api::EngineCommand::LoginCode { code },
                );
            }
        }
        ViewAction::ClaudeTransport(host, instance, enabled) => _controller.set_host_setting_for(
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
        ViewAction::ClaudeSignIn => _controller.sign_in_claude_ai(core),
        ViewAction::AccountEdited | ViewAction::AccountPresentationChanged => {}
        ViewAction::AccountRequestLink => _controller.sign_in.request_link(core),
        ViewAction::AccountFinish => _controller.sign_in.finish_sign_in(core),
        ViewAction::AccountDismiss => core.account_mut().dismiss_error(),
        ViewAction::AccountRefresh => core.account_mut().refresh(),
        ViewAction::AccountSignOut => core.account_mut().sign_out(),
        ViewAction::AccountSignOutEverywhere => core.account_mut().sign_out_everywhere(),
        ViewAction::ForgeOpen(machine) => core.navigate(Screen::NewThread {
            host: crate::core::HostId(machine),
            project: None,
        }),
        ViewAction::ForgeConnect(machine) => core.connect_machine(&machine),
        ViewAction::ForgeRemove(machine) => core.account_mut().remove_machine(machine),
        ViewAction::ForgeEnroll => core.account_mut().enroll_this_forge(),
        ViewAction::ForgeConfirm => core.account_mut().confirm_forge(),
        ViewAction::ForgeRefresh => core.account_mut().refresh_local_forge(),
        ViewAction::ForgeClaim => _controller.claim_forge(core),
        ViewAction::RetentionMenu(open) => _controller.reported_retention_menu = open,
        ViewAction::SetRetention(host, instance, days) => _controller.set_host_setting_for(
            core,
            host,
            instance,
            artisan_api::THREAD_RETENTION_DAYS_KEY,
            serde_json::json!(days),
        ),
        ViewAction::ArchiveLatest | ViewAction::ArchiveOlder => {
            let direction = if matches!(action, ViewAction::ArchiveLatest) {
                crate::core::ThreadPageDirection::Latest
            } else {
                crate::core::ThreadPageDirection::Older
            };
            if let Some(id) = core.active_host().map(|host| host.id.clone()) {
                if let Some(host) = core.host_mut(&id) {
                    host.load_threads(true, direction);
                }
            }
        }
        ViewAction::Unarchive(thread) => _controller.unarchive(core, thread),
        ViewAction::McpOpen(server) => _controller.open_mcp(server),
        ViewAction::McpRemove(server) => _controller.remove_mcp(core, server),
        ViewAction::McpSave => _controller.save_mcp(core),
        ViewAction::McpCancel => _controller.extensions.cancel(),
        ViewAction::McpEdited
        | ViewAction::McpPresentationChanged
        | ViewAction::OpenUrl(_)
        | ViewAction::CopyText(_) => {}
        ViewAction::McpSignIn(server) => _controller.sign_in_mcp(core, &server),
        ViewAction::McpCancelSignIn(server) => _controller.cancel_mcp_sign_in(core, &server),
        ViewAction::McpSignOut(server) => _controller.sign_out_mcp(core, server),
        ViewAction::PromptChanged => {
            _controller.prompt.changed();
            _controller.reported_prompt_dirty =
                _controller.prompt_editor.borrow().text() != _controller.prompt.saved;
        }
        ViewAction::PromptPresentationChanged => {}
        ViewAction::SavePrompt => _controller.save_prompt(core),
        ViewAction::LoadLatestPrompt => _controller.load_latest_prompt(core),
        ViewAction::FontMenu(open) => _controller.reported_font_menu = open,
        ViewAction::StreamingMenu(open) => _controller.reported_streaming_menu = open,
        ViewAction::SetFont(font) => core.set_setting("font_set", font.into()),
        ViewAction::SetDarkMode(enabled) => core.set_setting("dark_mode", enabled.into()),
        ViewAction::SetOutlineText(enabled) => core.set_setting("outline_text", enabled.into()),
        ViewAction::SetShowAllModels(enabled) => {
            core.set_setting(artisan_api::SHOW_ALL_MODELS_KEY, enabled.into());
        }
        ViewAction::SetHideBadModelOptions(enabled) => {
            core.set_setting(artisan_api::HIDE_BAD_MODEL_OPTIONS_KEY, enabled.into());
        }
        ViewAction::SetStreaming(mode) => core.set_setting("streaming_mode", mode.into()),
        ViewAction::SetFrameRate(enabled) => core.set_setting("lower_refresh_rate", enabled.into()),
        ViewAction::SetVsync(enabled) => core.set_setting("graphics_vsync", enabled.into()),
        ViewAction::SetNotifications(enabled) => core.set_setting("notifications", enabled.into()),
        ViewAction::SetHostSetting(host, instance, key, enabled) => {
            _controller.set_host_setting_for(core, host, instance, key, enabled.into())
        }
    }
    _controller.reported_prompt_dirty =
        _controller.prompt_editor.borrow().text() != _controller.prompt.saved;
    before != ViewSnapshot::capture(_controller, core)
}

pub fn build(
    controller: &SettingsController,
    core: &Core,
    theme: ArtisanTheme,
    build_info: &BuildInfo,
    _viewport: aqua_core::Size,
    _visible: bool,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let page = match core.screen() {
        Screen::Settings(page) => *page,
        _ => SettingsPage::General,
    };
    if page == SettingsPage::GlobalPrompt && core.active_host().is_some() {
        return prompt(controller, core, &theme, emit);
    }
    let content = match page.parent() {
        SettingsPage::General if page == SettingsPage::General => UiBox::new()
            .layout("column")
            .child(ui::header(&theme, page.label(), page::description(page)))
            .child(appearance(controller, &theme, core, emit.clone(), false))
            .child(streaming(controller, &theme, core, emit.clone(), false))
            .child(notifications(&theme, core, emit.clone(), false))
            .child(performance(&theme, core, emit, false))
            .build(),
        SettingsPage::General if page == SettingsPage::Section(SettingsSection::Appearance) => {
            appearance(controller, &theme, core, emit, true)
        }
        SettingsPage::General if page == SettingsPage::Section(SettingsSection::Streaming) => {
            streaming(controller, &theme, core, emit, true)
        }
        SettingsPage::General if page == SettingsPage::Section(SettingsSection::Performance) => {
            performance(&theme, core, emit, true)
        }
        SettingsPage::General if page == SettingsPage::Section(SettingsSection::Notifications) => {
            notifications(&theme, core, emit, true)
        }
        SettingsPage::Threads
            if core.active_host().is_some()
                && (page == SettingsPage::Threads
                    || page == SettingsPage::Section(SettingsSection::Archive)) =>
        {
            let host = core.active_host().unwrap();
            let mut content = UiBox::new().layout("column").child(ui::header(
                &theme,
                page.label(),
                page::description(page),
            ));
            if page == SettingsPage::Threads {
                content = content
                    .child(thread_section(
                        &theme,
                        core,
                        SettingsPage::Section(SettingsSection::Automation),
                        emit.clone(),
                        false,
                    ))
                    .child(thread_section(
                        &theme,
                        core,
                        SettingsPage::Section(SettingsSection::Assistance),
                        emit.clone(),
                        false,
                    ));
            }
            content
                .child(super::archive_view::cleanup(
                    controller,
                    host,
                    &theme,
                    emit.clone(),
                ))
                .child(super::archive_view::archived(
                    controller, host, &theme, emit,
                ))
                .build()
        }
        SettingsPage::Threads
            if core.active_host().is_some()
                && (page == SettingsPage::Section(SettingsSection::Automation)
                    || page == SettingsPage::Section(SettingsSection::Assistance)) =>
        {
            thread_section(&theme, core, page, emit, true)
        }
        SettingsPage::Extensions
            if core.active_host().is_some()
                && (page == SettingsPage::Extensions
                    || page == SettingsPage::Section(SettingsSection::McpServers)) =>
        {
            let host = core.active_host().unwrap();
            let mut content = UiBox::new()
                .layout("column")
                .child(ui::header(&theme, page.label(), page::description(page)))
                .child(super::mcp_view::section(
                    controller,
                    core,
                    host,
                    &theme,
                    emit.clone(),
                ));
            if page == SettingsPage::Extensions {
                content = content.child(skills_section(&theme, emit));
            }
            content.build()
        }
        SettingsPage::Extensions
            if core.active_host().is_some()
                && page == SettingsPage::Section(SettingsSection::Skills) =>
        {
            skills(&theme, emit)
        }
        SettingsPage::Engine(engine) if core.active_host().is_some() => super::engine_view::page(
            controller,
            core,
            core.active_host().unwrap(),
            engine,
            page,
            &theme,
            emit,
        ),
        SettingsPage::Account => super::account_view::page(controller, core, &theme, emit),
        SettingsPage::About => about(&theme, build_info),
        SettingsPage::Hosts => hosts(&theme, core),
        _ => UiBox::new()
            .layout("column")
            .child(ui::header(&theme, page.label(), page::description(page)))
            .child(
                UiBox::new()
                    .style(Style::new().margin_insets(Insets {
                        top: 32.0,
                        ..Insets::default()
                    }))
                    .child(ui::note(&theme, "Pair a host on the Hosts page first."))
                    .build(),
            )
            .build(),
    };
    Scroll::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(Style::new().justify(JustifyContent::CENTER))
                .child(
                    UiBox::new()
                        .layout("column")
                        .width(Length::Pct(100.0))
                        .style(
                            Style::new()
                                .max_width(Length::Px(680.0))
                                .padding_insets(Insets {
                                    left: 40.0,
                                    right: 40.0,
                                    top: 64.0,
                                    bottom: 120.0,
                                }),
                        )
                        .child(content)
                        .build(),
                )
                .build(),
        )
        .build()
        .with_key(format!("settings-screen-{page:?}"))
}

fn about(theme: &ArtisanTheme, build: &BuildInfo) -> Element {
    let executable = std::env::current_exe().ok();
    let rows = super::about::rows(build, executable.as_deref())
        .into_iter()
        .map(|(title, value)| ui::row(theme, title, value, None))
        .collect();
    UiBox::new()
        .layout("column")
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
        .build()
}

fn hosts(theme: &ArtisanTheme, core: &Core) -> Element {
    let mut rows: Vec<Element> = core
        .hosts()
        .map(|(config, host)| {
            let (state, colour) = match &host.status {
                ConnectionStatus::Live => ("Connected".to_owned(), theme.colors.green.solid),
                ConnectionStatus::Connecting => ("Connecting…".to_owned(), theme.colors.text_muted),
                ConnectionStatus::Offline { reason, .. } => {
                    (format!("Offline: {reason}"), theme.colors.red.solid)
                }
            };
            let detail = host
                .forge_version
                .as_ref()
                .map_or_else(String::new, |version| format!("Forge {version}"));
            let status = UiBox::new()
                .gap(6.0)
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .max_width(Length::Px(240.0)),
                )
                .child(
                    UiBox::new()
                        .width(6.0)
                        .height(6.0)
                        .radius(3.0)
                        .fill(colour.to_paint())
                        .style(Style::new().flex_shrink(0.0))
                        .build(),
                )
                .child(
                    Text::new(state)
                        .text_style(TextStyle {
                            families: vec![FontFamily::Named(
                                theme.typography.sans.family.to_owned(),
                            )],
                            size_px: theme.typography.label_text,
                            ..TextStyle::default()
                        })
                        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                        .wrap(false)
                        .build(),
                )
                .build();
            ui::row(theme, config.name.clone(), detail, Some(status))
        })
        .collect();
    if rows.is_empty() {
        rows.push(ui::row(
            theme,
            "No Forges open",
            "Add and connect Forges on the Account page.",
            None,
        ));
    }
    UiBox::new()
        .layout("column")
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
        .settings
        .get("notifications")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let control = Switch::new(
        "settings-switch-notify",
        *theme,
        SwitchSize::Default,
        enabled,
        "switch notify",
    )
    .on_change(move |enabled| emit(ViewAction::SetNotifications(enabled)))
    .build();
    let row = ui::row(
        theme,
        "Notify Me",
        "Uses your operating systems notification scheduler to alert you when threads finish, is stuck or out right failed.",
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

fn thread_section(
    theme: &ArtisanTheme,
    core: &Core,
    page: SettingsPage,
    emit: Rc<dyn Fn(ViewAction)>,
    include_header: bool,
) -> Element {
    let Some(host) = core.active_host() else {
        unreachable!()
    };
    let (title, entries): (
        &str,
        &[(&'static str, &'static str, &'static str, &'static str)],
    ) = match page {
        SettingsPage::Section(SettingsSection::Automation) => (
            "Recovery & continuation",
            &[
                (
                    "settings-switch-retry-overloaded",
                    "retry_when_overloaded",
                    "Keep retrying when the provider is overloaded",
                    "Artisan Editor indefintely reschedules a thread which its model's capacityis overloaded  to continue until theturn finishes.",
                ),
                (
                    "settings-switch-continue-usage-limit",
                    "continue_after_usage_limit",
                    "Continue when my usage limit resets",
                    "Artisan Editor marks a thread that stopped because of session depletage to continue when its session is replenished.",
                ),
                (
                    "settings-switch-continue-restart",
                    "continue_after_restart",
                    "Continue after a restart",
                    "Artisan Editor automatically continues threads that were stopped because of a system shutdown, restart or similiar.",
                ),
            ],
        ),
        SettingsPage::Section(SettingsSection::Assistance) => (
            "Assistance",
            &[
                (
                    "settings-switch-suggestions",
                    "suggestions",
                    "Suggest my next message",
                    "After each turn, the composer offers what you would most likely send next. It is asked of the thread's own engine and reuses its cached conversation, so each one costs a little of that engine's usage.",
                ),
                (
                    "settings-switch-you-should-know",
                    "you_should_know",
                    "You should know",
                    "After a turn that changed things — edits, migrations, installs, decisions — the engine is asked whether there is something you should know but might miss, and the thread shows it as a note. Turns that only looked around are not checked, and checks that find nothing make the next one rarer.",
                ),
                (
                    "settings-switch-titles",
                    "summary_titles",
                    "Summary titles",
                    "A thread takes the title its engine writes as it works. Off, it keeps the first line of its first message.",
                ),
            ],
        ),
        _ => unreachable!(),
    };
    let rows = entries
        .iter()
        .map(|&(id, key, label, description)| {
            let enabled = host
                .setting(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or_else(|| {
                    if page == SettingsPage::Section(SettingsSection::Automation) {
                        artisan_api::setting_decl(key)
                            .and_then(|decl| decl.default_value().as_bool())
                            .unwrap_or(true)
                    } else {
                        true
                    }
                });
            let send = emit.clone();
            let host_id = host.id.clone();
            let instance = host.instance();
            let control = Switch::new(
                id,
                *theme,
                SwitchSize::Default,
                enabled,
                id.trim_start_matches("settings-").replace('-', " "),
            )
            .on_change(move |enabled| {
                send(ViewAction::SetHostSetting(
                    host_id.clone(),
                    instance,
                    key,
                    enabled,
                ))
            })
            .build();
            ui::row(theme, label, description, Some(control))
        })
        .collect();
    section_page(
        theme,
        page,
        ui::section(theme, title, None, None, ui::card(theme, rows)),
        include_header,
    )
}

fn skills(theme: &ArtisanTheme, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    let page = SettingsPage::Section(SettingsSection::Skills);
    UiBox::new()
        .layout("column")
        .child(ui::header(theme, page.label(), page::description(page)))
        .child(skills_section(theme, emit))
        .build()
}

fn skills_section(theme: &ArtisanTheme, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    let action = Button::text(
        "settings-manage-skills",
        *theme,
        "Manage skills in Marketplace",
    )
    .variant(ButtonVariant::Outline)
    .on_activate(move |_| emit(ViewAction::Navigate(Screen::Marketplace)))
    .build();
    ui::section(
        theme,
        "Skills",
        Some(
            "Artisan's skills and your own reach every engine on this host; the Marketplace turns each on or off.",
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
    let notify = emit.clone();
    let editor =
        EditorController::from_shared(controller.prompt_editor.clone(), move |event| match event {
            EditorEvent::Changed => notify(ViewAction::PromptChanged),
            EditorEvent::PresentationChanged => notify(ViewAction::PromptPresentationChanged),
            _ => {}
        });
    let mut page = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .gap(20.0)
        .style(
            Style::new()
                .min_height(Length::Px(0.0))
                .padding_insets(Insets {
                    left: 24.0,
                    right: 24.0,
                    top: 24.0,
                    bottom: 24.0,
                }),
        )
        .child(ui::header(
            theme,
            SettingsPage::GlobalPrompt.label(),
            "Instructions shared by every engine, in every thread.",
        ))
        .child(
            Scroll::new()
                .width(Length::Pct(100.0))
                .style(Style::new().flex_grow(1.0).min_height(Length::Px(0.0)))
                .child(editor.element_with_notice_keyed(
                    "settings-global-prompt-editor",
                    Mode::Document,
                    theme,
                    "Write your global instructions…",
                    false,
                    true,
                ))
                .build()
                .with_key("global-prompt-scroll"),
        );
    if let Some(error) = form.error.as_deref().or_else(|| {
        core.active_host()
            .and_then(|host| host.prompt_load().error.as_deref())
    }) {
        page = page.child(crate::ui::page::notice(theme, error).build());
    }
    let mut actions = UiBox::new()
        .gap(8.0)
        .style(Style::new().justify(JustifyContent::END).flex_shrink(0.0));
    if form.newer {
        let send = emit.clone();
        actions = actions.child(
            Button::text("settings-global-prompt-load-latest", *theme, "Load latest")
                .variant(ButtonVariant::Outline)
                .disabled(form.pending)
                .on_activate(move |_| send(ViewAction::LoadLatestPrompt))
                .build(),
        );
    }
    let dirty = controller.prompt_editor.borrow().text() != form.saved;
    actions = actions.child(
        Button::text("settings-global-prompt-save", *theme, "Save")
            .variant(ButtonVariant::Accent)
            .shape(ButtonShape::Rounded)
            .disabled(!form.loaded || !dirty || form.pending || form.newer)
            .on_activate(move |_| emit(ViewAction::SavePrompt))
            .build(),
    );
    page.child(actions.build()).build()
}

fn performance(
    theme: &ArtisanTheme,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
    include_header: bool,
) -> Element {
    let lowered = core
        .settings
        .get("lower_refresh_rate")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let control = Switch::new(
        "settings-lower-refresh-rate",
        *theme,
        SwitchSize::Default,
        lowered,
        "lower refresh rate",
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
        .settings
        .get("graphics_vsync")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let monitor_control = Switch::new(
        "settings-monitor-refresh-rate",
        *theme,
        SwitchSize::Default,
        monitor_sync,
        "match monitor refresh rate",
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
        .settings
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
    theme: ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<&'static str> {
    let change = emit.clone();
    SelectMenu::new(
        theme,
        Some(streaming_value(core)),
        streaming_entries(),
        controller.streaming_select.clone(),
        144.0,
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
    let change = emit.clone();
    let select = Select::new(
        "settings-streaming-mode",
        *theme,
        Some(streaming_value(core)),
        streaming_entries(),
        controller.streaming_select.clone(),
    )
    .label("Text Streaming")
    .on_change(move |mode| change(ViewAction::SetStreaming(mode)))
    .on_open_change(move |open| emit(ViewAction::StreamingMenu(open)))
    .build();
    let control = UiBox::new()
        .width(144.0)
        .style(Style::new().flex_shrink(0.0))
        .child(select)
        .build();
    let row = ui::row(
        theme,
        "Text Streaming",
        "How conversations in Artisan Editor streams. We recommend you leave it at smooth, but for models with higher TPS you should try swift.",
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
        .settings
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
    theme: ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<&'static str> {
    let change = emit.clone();
    SelectMenu::new(
        theme,
        Some(font_value(core)),
        font_entries(),
        controller.font_select.clone(),
        144.0,
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
    let send = emit.clone();
    let dark = Switch::new(
        "settings-switch-dark-mode",
        *theme,
        SwitchSize::Default,
        theme.colors.mode.is_dark(),
        "switch dark mode",
    )
    .on_change(move |enabled| send(ViewAction::SetDarkMode(enabled)))
    .build();
    let send = emit.clone();
    let open = emit.clone();
    let fonts = Select::new(
        "settings-font-set",
        *theme,
        Some(font_value(core)),
        font_entries(),
        controller.font_select.clone(),
    )
    .label("Fontset")
    .on_change(move |font| send(ViewAction::SetFont(font)))
    .on_open_change(move |open_state| open(ViewAction::FontMenu(open_state)))
    .build();
    let fonts = UiBox::new()
        .width(144.0)
        .style(Style::new().flex_shrink(0.0))
        .child(fonts)
        .build();
    let checked = core
        .settings
        .get("outline_text")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let show_models = emit.clone();
    let hide_options = emit.clone();
    let hide_bad = Switch::new(
        "settings-switch-hide-bad-model-options",
        *theme,
        SwitchSize::Default,
        core.settings.get(artisan_api::HIDE_BAD_MODEL_OPTIONS_KEY)
            .and_then(serde_json::Value::as_bool).unwrap_or(true),
        "Hide bad model options",
    )
    .on_change(move |enabled| hide_options(ViewAction::SetHideBadModelOptions(enabled)))
    .build();
    let all_models = Switch::new(
        "settings-switch-show-all-models",
        *theme,
        SwitchSize::Default,
        core.settings
            .get(artisan_api::SHOW_ALL_MODELS_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        "switch show all models",
    )
    .on_change(move |enabled| show_models(ViewAction::SetShowAllModels(enabled)))
    .build();
    let outline = Switch::new(
        "settings-switch-outline-text",
        *theme,
        SwitchSize::Default,
        checked,
        "switch outline text",
    )
    .on_change(move |enabled| emit(ViewAction::SetOutlineText(enabled)))
    .build();
    let rows = vec![
        ui::row(
            theme,
            "Dark Mode",
            "Whether to draw Artisan Editor in a dark theme.",
            Some(dark),
        ),
        ui::row(
            theme,
            "Fontset",
            "Choose what set of fonts to Artisan Editor will render in.",
            Some(fonts),
        ),
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

fn section_page(
    theme: &ArtisanTheme,
    page: SettingsPage,
    section: Element,
    include_header: bool,
) -> Element {
    if !include_header {
        return section;
    }
    UiBox::new()
        .layout("column")
        .child(ui::header(theme, page.label(), page::description(page)))
        .child(section)
        .build()
}

#[derive(PartialEq)]
struct ViewSnapshot {
    screen: Screen,
    account: crate::core::AccountPhase,
    account_error: Option<String>,
    local_forge: crate::core::LocalForge,
    device_settings: Vec<Option<serde_json::Value>>,
    host_settings: Vec<Option<serde_json::Value>>,
    host_instance: Option<crate::core::HostInstance>,
    archive_loading: Option<(bool, bool, bool, Option<String>)>,
    archive_restoring: std::collections::HashSet<artisan_api::ThreadId>,
    archive_error: Option<String>,
    engine_busy: std::collections::BTreeSet<artisan_api::EngineId>,
    engine_errors: std::collections::HashMap<artisan_api::EngineId, String>,
    engine_versions: std::collections::HashSet<artisan_api::EngineId>,
    engine_codes: std::collections::HashMap<artisan_api::EngineId, String>,
    mcp_editing: Option<Option<artisan_api::McpServer>>,
    mcp_saving: bool,
    mcp_sign_ins: Vec<(String, crate::core::McpSignInPhase, Option<String>)>,
    mcp_error: Option<String>,
    mcp_fields: Vec<String>,
    prompt: (bool, bool, bool, bool, Option<String>),
    forge_error: Option<String>,
    forge_code: String,
    second_factor: String,
    menus: (bool, bool, bool),
}

impl ViewSnapshot {
    fn capture(controller: &SettingsController, core: &Core) -> Self {
        let host = core.active_host();
        let preferences = crate::shell::preferences::Preferences::from_core(core);
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
            account_error: core.account().last_error().map(str::to_owned),
            local_forge: core.account().local_forge(),
            device_settings: vec![
                Some(serde_json::json!(preferences.theme.is_dark())),
                Some(serde_json::json!(preferences.fonts.key())),
                Some(serde_json::json!(preferences.outline_text)),
                Some(serde_json::json!(matches!(
                    preferences.streaming,
                    crate::shell::preferences::StreamingMode::Swift
                ))),
                Some(serde_json::json!(preferences.lower_frame_rate)),
                Some(serde_json::json!(preferences.vsync)),
                Some(serde_json::json!(
                    core.settings
                        .get("notifications")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true)
                )),
            ],
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
            host_instance: host.map(|host| host.instance()),
            archive_loading: host.map(|host| {
                let page = host.thread_page(true);
                (
                    page.loading,
                    page.at_latest,
                    page.next.is_some(),
                    page.error.clone(),
                )
            }),
            archive_restoring: controller.archive.restoring.clone(),
            archive_error: controller.archive.error.clone(),
            engine_busy: controller.engines.busy.clone(),
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
                    let servers: Vec<artisan_api::McpServer> = host
                        .setting(super::extensions::MCP_SERVERS_KEY)
                        .and_then(|value| serde_json::from_value(value.clone()).ok())
                        .unwrap_or_default();
                    servers.into_iter().filter_map(move |server| {
                        let name = server.name;
                        core.mcp_sign_in(&host.id, &name).map(|sign_in| {
                            (
                                name.clone(),
                                sign_in.phase.clone(),
                                sign_in.authorize_url.clone(),
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
            forge_error: controller.forge_error.clone(),
            forge_code: controller.forge_code.borrow().text().to_owned(),
            second_factor: controller.sign_in.second_factor.borrow().text().to_owned(),
            menus: (
                controller.reported_font_menu,
                controller.reported_streaming_menu,
                controller.reported_retention_menu,
            ),
        }
    }
}
