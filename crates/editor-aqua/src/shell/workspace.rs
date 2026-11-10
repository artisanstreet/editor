use crate::core::{Attention, Core, CoreEvent, HostEvent, Screen, SettingsPage};
use crate::shell::notifications::Notifications;
use crate::shell::palette::{HostSnapshot, Palette, SearchHost};
use aqua::prelude::Notification;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

#[derive(Default)]
pub struct NotificationEffects {
    pub withdraw: Vec<String>,
    pub show: Vec<Notification>,
    pub badge: Option<Attention>,
}

pub enum SettingsPlatformEffect {
    OpenUrl(String),
    CopyText(String),
}

#[derive(PartialEq, Eq)]
struct QuestionSourceIdentity {
    host: crate::core::HostId,
    instance: crate::core::HostInstance,
    item: artisan_api::ItemId,
    step: usize,
    source: artisan_api::ContentRef,
    revision: u64,
    document: aqua_text::document::SelectionDocumentId,
    selection: aqua_text::document::SelectionSourceId,
}

pub struct Workspace {
    pub selections: crate::shell::selection::SelectionScope,
    pub inspector: crate::inspector::InspectorOwner,
    pub navigator: crate::conversation::view::navigator::Navigator,
    pub conversation_selection: crate::conversation::presentation::SelectionOwners,
    pub markdown: Rc<RefCell<crate::conversation::presentation::MarkdownOwners>>,
    pub status_shimmers: Rc<RefCell<crate::conversation::view::status::StatusShimmers>>,
    pub images: Rc<crate::ui::media::image::Images>,
    code_actions: Rc<RefCell<VecDeque<crate::conversation::presentation::ScopedCodeAction>>>,
    code_frame_pending: bool,
    code_deadline: Option<aqua::Instant>,
    inspector_actions: Rc<RefCell<VecDeque<crate::inspector::OwnerAction>>>,
    terminal_dialog: Option<crate::ui::dialog::DialogHost>,
    terminal_content: Option<aqua_ui::element::Element>,
    terminal_dismissed: Rc<Cell<bool>>,
    pub sidebar: crate::sidebar::controller::Sidebar,
    pub settings_sidebar: crate::sidebar::controller::Sidebar,
    pub usage: crate::sidebar::usage_controller::UsagePage,
    usage_actions: Rc<RefCell<VecDeque<crate::sidebar::usage_page::Action>>>,
    pub conversation: crate::conversation::controller::Conversation,
    pub composer: Option<crate::composer::view::ComposerOwner>,
    composer_target: Option<(
        crate::composer::controller::ComposerTarget,
        Option<crate::core::HostInstance>,
    )>,
    draft_sessions: crate::composer::persistence::DraftSessions,
    composer_actions: Rc<RefCell<VecDeque<crate::composer::view::ComposerAction>>>,
    composer_effects: Vec<crate::composer::view::ComposerEffect>,
    composer_editor_dirty: bool,
    composer_focus_pending: bool,
    composer_was_focused: bool,
    composer_mentions_dirty: bool,
    composer_tooltip: Option<(
        crate::composer::picker::TooltipContent,
        aqua_core::NodeId,
        crate::ui::hover_tooltip::TooltipHost,
    )>,
    question_option_actions: Rc<RefCell<VecDeque<crate::conversation::questions::OptionAction>>>,
    question_source_actions: Rc<RefCell<VecDeque<crate::sources::ContentAction>>>,
    question_source: Option<(QuestionSourceIdentity, crate::shell::source::SourceView)>,
    question_source_revision: u64,
    question_option_detail: Option<((u64, u64), crate::shell::source::SourceView)>,
    question_option_detail_actions:
        Rc<RefCell<VecDeque<((u64, u64), crate::sources::ContentAction)>>>,
    pub marketplace: crate::marketplace::controller::MarketplaceController,
    marketplace_focus_pending: bool,
    marketplace_reveal_pending: bool,
    pub settings: crate::settings::controller::SettingsController,
    pub home: crate::settings::controller::SettingsController,
    home_actions: Rc<RefCell<VecDeque<crate::settings::view::ViewAction>>>,
    sidebar_actions: Rc<RefCell<VecDeque<crate::sidebar::view::ViewAction>>>,
    settings_sidebar_actions: Rc<RefCell<VecDeque<crate::sidebar::view::ViewAction>>>,
    marketplace_actions: Rc<RefCell<VecDeque<crate::marketplace::view::ViewAction>>>,
    settings_actions: Rc<RefCell<VecDeque<crate::settings::view::ViewAction>>>,
    theme_transition: Option<bool>,
    settings_platform_effects: Vec<SettingsPlatformEffect>,
    pub palette: Option<Palette>,
    pub resource_preview: Option<crate::shell::resource_preview::ResourcePreview>,
    resource_preview_generation: u64,
    pub show_sidebar: bool,
    pub show_inspector: bool,
    pub failure: Option<String>,
    notifications: Notifications,
    repository: crate::shell::repository::RepositoryContext,
    start_pending: bool,
    repository_pending: bool,
}

impl Workspace {
    pub fn new(
        core: &Core,
        failure: Option<String>,
        selections: crate::shell::selection::SelectionScope,
    ) -> (Self, NotificationEffects) {
        let marketplace_actions = Rc::new(RefCell::new(VecDeque::new()));
        let sink = Rc::clone(&marketplace_actions);
        let mut workspace = Self {
            inspector: crate::inspector::InspectorOwner::new(core, selections.inspector),
            conversation_selection: crate::conversation::presentation::SelectionOwners::new(
                selections.conversation,
            ),
            markdown: Rc::new(RefCell::new(
                crate::conversation::presentation::MarkdownOwners::new(selections.conversation),
            )),
            status_shimmers: Rc::new(RefCell::new(
                crate::conversation::view::status::StatusShimmers::default(),
            )),
            images: Rc::new(crate::ui::media::image::Images::new()),
            code_actions: Rc::new(RefCell::new(VecDeque::new())),
            code_frame_pending: false,
            code_deadline: None,
            selections,
            navigator: crate::conversation::view::navigator::Navigator::default(),
            inspector_actions: Rc::new(RefCell::new(VecDeque::new())),
            terminal_dialog: None,
            terminal_content: None,
            terminal_dismissed: Rc::new(Cell::new(false)),
            sidebar: crate::sidebar::controller::Sidebar::new(true),
            settings_sidebar: crate::sidebar::controller::Sidebar::new(false),
            usage: crate::sidebar::usage_controller::UsagePage::default(),
            usage_actions: Rc::new(RefCell::new(VecDeque::new())),
            sidebar_actions: Rc::new(RefCell::new(VecDeque::new())),
            settings_sidebar_actions: Rc::new(RefCell::new(VecDeque::new())),
            conversation: crate::conversation::controller::Conversation::new(core),
            composer: None,
            composer_target: None,
            draft_sessions: crate::composer::persistence::DraftSessions::default(),
            composer_actions: Rc::new(RefCell::new(VecDeque::new())),
            composer_effects: Vec::new(),
            composer_editor_dirty: false,
            composer_focus_pending: false,
            composer_was_focused: false,
            composer_mentions_dirty: true,
            composer_tooltip: None,
            question_option_actions: Rc::new(RefCell::new(VecDeque::new())),
            question_source_actions: Rc::new(RefCell::new(VecDeque::new())),
            question_source: None,
            question_source_revision: 0,
            question_option_detail: None,
            question_option_detail_actions: Rc::new(RefCell::new(VecDeque::new())),
            marketplace: crate::marketplace::controller::MarketplaceController::new(Rc::new(
                move |action| sink.borrow_mut().push_back(action),
            )),
            settings: crate::settings::controller::SettingsController::default(),
            home: crate::settings::controller::SettingsController::default(),
            home_actions: Rc::new(RefCell::new(VecDeque::new())),
            marketplace_focus_pending: false,
            marketplace_reveal_pending: false,
            marketplace_actions,
            settings_actions: Rc::new(RefCell::new(VecDeque::new())),
            theme_transition: None,
            settings_platform_effects: Vec::new(),
            palette: None,
            resource_preview: None,
            resource_preview_generation: 0,
            show_sidebar: true,
            show_inspector: true,
            failure,
            notifications: Notifications::default(),
            repository: crate::shell::repository::RepositoryContext::default(),
            start_pending: true,
            repository_pending: true,
        };
        let effects = workspace.follow_hosts(core);
        (workspace, effects)
    }

    fn attention(core: &Core) -> Attention {
        core.hosts()
            .map(|(_, host)| host.attention())
            .fold(Attention::default(), |total, host| total + host)
    }

    fn follow_hosts(&mut self, core: &Core) -> NotificationEffects {
        self.draft_sessions.retain_hosts(core);
        let hosts: Vec<_> = core.hosts().map(|(_, host)| host).collect();
        let ids: Vec<_> = hosts.iter().map(|host| host.id.clone()).collect();
        let withdraw = self.notifications.retain_hosts(&ids);
        for host in hosts {
            self.notifications.reset(host);
        }
        NotificationEffects {
            withdraw,
            show: Vec::new(),
            badge: Some(Self::attention(core)),
        }
    }

    pub fn handle_event(
        &mut self,
        core: &Core,
        event: &CoreEvent,
        window_active: bool,
    ) -> NotificationEffects {
        let mut effects = NotificationEffects::default();
        match event {
            CoreEvent::HostsChanged => effects = self.follow_hosts(core),
            CoreEvent::Host { host: id, event } => {
                if let Some(host) = core.host(id) {
                    match event {
                        HostEvent::ThreadChanged { thread_id } => {
                            let watching = match core.screen() {
                                Screen::Thread { host, thread } if window_active && host == id => {
                                    Some(thread)
                                }
                                _ => None,
                            };
                            let enabled = core
                                .settings
                                .get("notifications")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(true);
                            let changes = self
                                .notifications
                                .thread_changed(host, thread_id, watching, enabled);
                            effects.withdraw = changes.withdraw;
                            effects.show.extend(changes.show);
                            effects.badge = Some(Self::attention(core));
                        }
                        HostEvent::Reset => {
                            effects.withdraw = self.notifications.host_reset(host);
                            effects.badge = Some(Self::attention(core));
                        }
                        HostEvent::Changed => {
                            self.notifications.sync_connection(host);
                            effects.badge = Some(Self::attention(core));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if matches!(
            event,
            CoreEvent::HostsChanged
                | CoreEvent::Host {
                    event: HostEvent::Changed | HostEvent::Reset | HostEvent::ThreadChanged { .. },
                    ..
                }
        ) {
            self.refresh_palette(core);
        }
        match event {
            CoreEvent::ScreenChanged | CoreEvent::HostsChanged => self.repository_pending = true,
            CoreEvent::Host {
                host,
                event: HostEvent::Reset,
            } => {
                if core.screen().host() == Some(host) {
                    self.repository.reset();
                    self.repository_pending = true;
                }
            }
            CoreEvent::Host {
                host,
                event: HostEvent::Changed,
            } => {
                if core.screen().host() == Some(host) {
                    self.repository_pending = true;
                }
            }
            CoreEvent::Host {
                host,
                event: HostEvent::ThreadChanged { thread_id },
            } => {
                if core.screen().host() == Some(host) && core.screen().thread() == Some(thread_id) {
                    self.repository_pending = true;
                }
            }
            _ => {}
        }
        effects
    }

    pub fn finish_events(&mut self, core: &mut Core, ui: &mut aqua::UiApp) {
        if self.start_pending {
            match core.screen() {
                Screen::Home => {
                    if let Some(host) = core.active_host().map(|host| host.id.clone()) {
                        self.start_pending = false;
                        core.navigate(Screen::NewThread {
                            host,
                            project: None,
                        });
                    }
                }
                _ => self.start_pending = false,
            }
        }
        if self.repository_pending {
            self.repository_pending = false;
            self.repository.sync(core);
            self.sidebar.sync(core, artisan_api::now());
            self.settings_sidebar.sync(core, artisan_api::now());
            self.conversation.sync_screen(core);
            self.sync_composer(core, ui);
            self.marketplace.sync(core);
            self.usage.sync(core);
            self.settings.sync(core);
        }
    }

    pub fn handle_domain_event(
        &mut self,
        core: &mut Core,
        event: &CoreEvent,
        window_active: bool,
        ui: &mut aqua::UiApp,
    ) {
        if matches!(
            event,
            CoreEvent::Host {
                event: HostEvent::Live { .. },
                ..
            }
        ) {
            self.conversation.core_event(core, event, window_active);
            self.inspector
                .on_core_event(core, &self.conversation, ui, event);
            return;
        }
        self.sidebar.sync(core, artisan_api::now());
        self.settings_sidebar.sync(core, artisan_api::now());
        if matches!(
            event,
            CoreEvent::SettingsChanged | CoreEvent::HostsChanged | CoreEvent::ScreenChanged
        ) || matches!(event, CoreEvent::Host { host, event: HostEvent::Changed | HostEvent::Reset | HostEvent::ThreadChanged { .. } } if self.composer_target.as_ref().is_some_and(|(target, _)| target.host() == host))
        {
            self.composer_mentions_dirty = true;
        }
        self.conversation.core_event(core, event, window_active);
        self.inspector
            .on_core_event(core, &self.conversation, ui, event);
        if matches!(event, CoreEvent::ScreenChanged | CoreEvent::HostsChanged) {
            self.sync_composer(core, ui);
        }
        self.marketplace.core_event(core, event);
        self.usage.sync(core);
        self.settings.core_event(core, event);
        self.home.core_event(core, event);
        self.sync_question_pages(core);
    }

    pub fn dispose_composer_popups(&mut self, ui: &mut aqua::UiApp) {
        if let Some((_, _, tooltip)) = self.composer_tooltip.take() {
            tooltip.dispose(ui);
        }
        self.release_question_source(ui);
        self.release_question_options(ui);
        if let Some(owner) = &mut self.composer {
            owner.dispose_popups(ui);
        }
    }

    pub fn sync_composer_tooltip(
        &mut self,
        ui: &mut aqua::UiApp,
        theme: crate::ui::theme::ArtisanTheme,
        now: std::time::Instant,
    ) -> bool {
        use crate::composer::view::PopupKind;
        let Some(owner) = &self.composer else {
            return false;
        };
        let mut changed = owner.advance_tooltip(now);
        let shown = owner.shown_tooltip();
        let node = shown.as_ref().and_then(|shown| {
            [
                PopupKind::Model,
                PopupKind::Project,
                PopupKind::Mentions,
            ]
            .into_iter()
            .filter_map(|kind| owner.popup_root(kind, ui))
            .find_map(|root| keyed_descendant(ui, root, shown.anchor.clone().into()))
            .or_else(|| keyed_node(ui, shown.anchor.clone().into()))
        });
        if self
            .composer_tooltip
            .as_ref()
            .is_some_and(|(content, anchor, host)| {
                shown.as_ref() != Some(content) || node != Some(*anchor) || !host.is_open(ui)
            })
        {
            if let Some((_, _, tooltip)) = self.composer_tooltip.take() {
                tooltip.dispose(ui);
            }
            changed = true;
        }
        if self.composer_tooltip.is_none() {
            if let (Some(content), Some(node), Some(factory)) =
                (shown, node, owner.tooltip_surface_factory(theme))
            {
                let surface = factory.surface;
                match crate::ui::hover_tooltip::TooltipHost::open(
                    ui,
                    node,
                    factory.placement,
                    move |width, fixed| surface(width, fixed),
                ) {
                    Ok(host) => {
                        self.composer_tooltip = Some((content, node, host));
                        changed = true;
                    }
                    Err(error) => tracing::warn!(%error, "could not open composer tooltip"),
                }
            }
        } else if let Some((_, _, host)) = &mut self.composer_tooltip {
            match host.sync(ui) {
                Ok(updated) => changed |= updated,
                Err(error) => tracing::warn!(%error, "could not synchronize composer tooltip"),
            }
        }
        changed
    }

    pub fn code_action_emit(
        &self,
    ) -> Rc<dyn Fn(crate::conversation::presentation::ScopedCodeAction)> {
        let actions = Rc::clone(&self.code_actions);
        Rc::new(move |action| actions.borrow_mut().push_back(action))
    }

    pub fn drain_code_actions(
        &mut self,
        core: &Core,
        ctx: &mut aqua::AppCtx<'_>,
        window: aqua::prelude::AquaWindowId,
    ) -> bool {
        use crate::conversation::presentation::CodeAction;
        use crate::ui::code_copy::CopyEffect;
        let actions = std::mem::take(&mut *self.code_actions.borrow_mut());
        let mut changed = false;
        for action in actions {
            let Some(action) = action.accept(&self.conversation, core, &self.markdown.borrow())
            else {
                continue;
            };
            match action {
                CodeAction::Copy(CopyEffect::Clipboard(text)) => {
                    if let Err(error) = ctx.clipboard().set_text(text) {
                        tracing::warn!(%error, "could not copy code");
                    }
                }
                CodeAction::Copy(CopyEffect::RequestFrame) => {
                    self.code_frame_pending = true;
                    ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
                }
                CodeAction::Copy(CopyEffect::WakeAt(deadline)) => {
                    let delay = deadline.saturating_duration_since(std::time::Instant::now());
                    let at = ctx.now() + delay;
                    self.code_deadline =
                        Some(self.code_deadline.map_or(at, |previous| previous.min(at)));
                    ctx.request_animation_deadline(window, at);
                }
                CodeAction::Reference(reference) => {
                    let scope = reference
                        .scope
                        .map(|scope| (crate::core::HostId(scope.host), scope.thread));
                    self.open_reference(core, reference.path, reference.line, scope);
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn take_code_frame(&mut self, now: aqua::Instant) -> bool {
        let due = self.code_deadline.is_some_and(|at| now >= at);
        if due {
            self.code_deadline = None;
        }
        std::mem::take(&mut self.code_frame_pending) || due
    }

    pub fn sync_composer_popups(
        &mut self,
        core: &Core,
        ui: &mut aqua::UiApp,
        theme: crate::ui::theme::ArtisanTheme,
        assets: &aqua::images::ImageAssets,
        now: f64,
    ) -> bool {
        use crate::composer::{
            picker::Menu,
            view::{self, PopupKind},
        };
        let Some(owner) = &mut self.composer else {
            return false;
        };
        let (host_id, instance) = {
            let composer = owner.controller.borrow();
            (composer.target.host().clone(), composer.host_instance())
        };
        let Some(host) = instance.and_then(|instance| core.host_if_instance(&host_id, instance))
        else {
            return false;
        };
        let viewport = ui.viewport();
        let actions = Rc::clone(&self.composer_actions);
        let emit: view::Dispatch = Rc::new(move |action| actions.borrow_mut().push_back(action));
        let (menu, mentions) = {
            let composer = owner.controller.borrow();
            (
                composer.picker.menu,
                !composer.mentions.rows.is_empty(),
            )
        };
        for kind in [
            PopupKind::Model,
            PopupKind::Project,
            PopupKind::Mentions,
        ] {
            let desired = match kind {
                PopupKind::Model => menu == Some(Menu::Model),
                PopupKind::Project => menu == Some(Menu::Project),
                PopupKind::Mentions => mentions,
            };
            if !desired {
                if owner.popup_is_open(kind, ui) {
                    if let Err(error) = owner.close_popup(ui, kind, now) {
                        tracing::warn!(%error, "could not close composer popup");
                    }
                }
                continue;
            }
            let anchor_key = match kind {
                PopupKind::Model => "composer-model".to_owned(),
                PopupKind::Project => "composer-project".to_owned(),
                PopupKind::Mentions => "composer-prompt".to_owned(),
            };
            let content = match kind {
                PopupKind::Model => {
                    match owner.model_menu(
                        theme,
                        host,
                        viewport,
                        crate::shell::wheel::kept_offset(ui, crate::shell::wheel::MODEL_MENU).y,
                        assets,
                        std::time::Instant::now(),
                    ) {
                        Ok(content) => Some(content),
                        Err(error) => {
                            tracing::warn!(%error, "could not build composer model menu");
                            None
                        }
                    }
                }
                PopupKind::Project => {
                    let composer = owner.controller.borrow();
                    Some(view::project_menu(
                        theme,
                        host,
                        &composer.picker,
                        composer.project(host).as_ref(),
                        viewport,
                        owner.project_hover(),
                        Rc::clone(&emit),
                    ))
                }
                PopupKind::Mentions => {
                    let composer = owner.controller.borrow();
                    let width = keyed_node(ui, aqua_ui::element::Key::from("composer-prompt"))
                        .and_then(|node| ui.tree().rect(node))
                        .map_or(viewport.w, |rect| rect.w);
                    view::mention_menu(
                        theme,
                        &composer.mentions,
                        viewport,
                        width,
                        Rc::clone(&emit),
                    )
                }
            };
            let Some(content) = content else { continue };
            let anchor = keyed_node(ui, anchor_key.into());
            let result = if owner.popup_is_open(kind, ui) {
                owner.refresh_popup_content(kind, ui, content, now)
            } else if let Some(anchor) = anchor {
                owner.open_popup_content(ui, anchor, kind, content, now)
            } else {
                continue;
            };
            if let Err(error) = result {
                tracing::warn!(%error, "could not synchronize composer popup");
            }
        }
        let roots = [PopupKind::Model, PopupKind::Project]
            .map(|kind| owner.popup_root(kind, ui));
        match owner.advance_picker_hover(ui, &theme, now, roots) {
            Ok(changed) => changed,
            Err(error) => {
                tracing::warn!(%error, "could not advance composer picker hover");
                false
            }
        }
    }

    #[must_use]
    pub fn inspector_emit(&self) -> Rc<dyn Fn(crate::inspector::InspectorAction)> {
        let actions = Rc::clone(&self.inspector_actions);
        let scope = self.inspector.scope();
        Rc::new(move |action| {
            actions
                .borrow_mut()
                .push_back(crate::inspector::OwnerAction::Inspector { scope, action });
        })
    }

    pub fn inspector_elements(
        &mut self,
        core: &Core,
        ui: &mut aqua::UiApp,
        theme: crate::ui::theme::ArtisanTheme,
        viewport_height: f32,
        reduced_motion: bool,
        now: aqua::Instant,
    ) -> anyhow::Result<aqua_ui::element::Element> {
        let actions = Rc::clone(&self.inspector_actions);
        let elements = self.inspector.build(
            core,
            ui,
            theme,
            viewport_height,
            reduced_motion,
            now,
            Rc::new(move |action| actions.borrow_mut().push_back(action)),
        )?;
        self.terminal_content = elements.terminal;
        Ok(elements.panel)
    }

    pub fn sync_terminal_dialog(
        &mut self,
        ui: &mut aqua::UiApp,
        theme: crate::ui::theme::ArtisanTheme,
    ) {
        if self.terminal_dismissed.replace(false) {
            self.inspector.close_terminal(ui);
            self.terminal_content = None;
        }
        let Some(content) = self.terminal_content.take() else {
            if let Some(dialog) = self.terminal_dialog.take() {
                dialog.dispose(ui);
            }
            return;
        };
        if let Some(dialog) = &self.terminal_dialog {
            if dialog.is_open(ui) {
                if let Err(error) = dialog.refresh_content(ui, content) {
                    tracing::warn!(%error, "could not refresh terminal dialog");
                }
                return;
            }
            self.terminal_dialog = None;
        }
        let Some(owner) = ui.find_debug_selector("workspace-root") else {
            self.terminal_content = Some(content);
            return;
        };
        let dismissed = Rc::clone(&self.terminal_dismissed);
        let dialog = crate::ui::dialog::Dialog::new(
            "inspector-terminal-dialog",
            theme,
            "Terminal output",
            content,
        )
        .bare()
        .viewport_max_width(96.0, 1040.0)
        .on_dismiss(move |_| dismissed.set(true));
        match crate::ui::dialog::DialogHost::open(ui, owner, dialog) {
            Ok(dialog) => self.terminal_dialog = Some(dialog),
            Err(error) => tracing::warn!(%error, "could not open terminal dialog"),
        }
    }

    pub fn sync_terminal_input(&mut self, ui: &mut aqua::UiApp) -> bool {
        if let Some(dialog) = &self.terminal_dialog {
            dialog.sync(ui);
        }
        if self.terminal_dismissed.replace(false) {
            self.inspector.close_terminal(ui);
            return true;
        }
        false
    }

    pub fn dispose_terminal_dialog(&mut self, ui: &mut aqua::UiApp) {
        if let Some(dialog) = self.terminal_dialog.take() {
            dialog.dispose(ui);
        }
        self.terminal_content = None;
        self.terminal_dismissed.set(false);
        self.inspector.close_terminal(ui);
    }

    pub fn drain_inspector(
        &mut self,
        core: &mut Core,
        routes: &crate::shell::dialogs::DialogRoutes,
        ui: &mut aqua::UiApp,
        ctx: &mut aqua::AppCtx<'_>,
        window: aqua::prelude::AquaWindowId,
    ) -> bool {
        let mut changed = false;
        for action in std::mem::take(&mut *self.inspector_actions.borrow_mut()) {
            changed |= self.inspector.dispatch_action(
                core,
                &mut self.conversation,
                ui,
                ctx,
                routes,
                window,
                action,
            );
        }
        changed | self.inspector.drain(core, ui, ctx)
    }

    pub fn sync_composer(&mut self, core: &Core, ui: &mut aqua::UiApp) {
        let target = match core.workspace_screen() {
            Screen::Thread { host, thread } => {
                Some(crate::composer::controller::ComposerTarget::Thread {
                    host: host.clone(),
                    thread: thread.clone(),
                })
            }
            Screen::NewThread { host, project } => {
                Some(crate::composer::controller::ComposerTarget::NewThread {
                    host: host.clone(),
                    project: project.clone(),
                })
            }
            _ => return,
        };
        let identity = target.map(|target| {
            let instance = core.host(target.host()).map(|host| host.instance());
            (target, instance)
        });
        let prompt_key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("composer-prompt"));
        let focused_now = ui
            .tree()
            .focused()
            .and_then(|node| ui.tree().get(node))
            .is_some_and(|record| record.key == Some(prompt_key));
        if self.composer_target == identity {
            self.composer_was_focused = focused_now;
            return;
        }
        let keep_focus = self.composer_was_focused || focused_now;
        self.composer_was_focused = false;
        self.dispose_composer_popups(ui);
        self.images.clear();
        self.composer_actions.borrow_mut().clear();
        self.composer_editor_dirty = true;
        self.composer_focus_pending = keep_focus;
        self.composer_mentions_dirty = true;
        self.composer_target = identity;
        if let Some((target, _)) = &self.composer_target {
            if let Some(owner) = &self.composer {
                owner.set_target(core, &mut self.draft_sessions, target.clone());
            } else {
                let actions = Rc::clone(&self.composer_actions);
                self.composer = Some(crate::composer::view::ComposerOwner::new(
                    core,
                    &mut self.draft_sessions,
                    target.clone(),
                    Rc::new(move |action| actions.borrow_mut().push_back(action)),
                ));
            }
        }
    }

    pub fn take_composer_actions(&mut self) -> VecDeque<crate::composer::view::ComposerAction> {
        std::mem::take(&mut *self.composer_actions.borrow_mut())
    }

    pub fn refresh_composer_mentions(
        &mut self,
        core: &mut Core,
        theme: &crate::ui::theme::ArtisanTheme,
    ) {
        if let (Some(_), Some((target, Some(instance)))) = (&self.composer, &self.composer_target) {
            if let Some(host) = core.host_mut_if_instance(target.host(), *instance) {
                host.load_skill_catalog(false);
            }
        }
        if !self.composer_mentions_dirty {
            return;
        }
        if let (Some(owner), Some((target, Some(instance)))) =
            (&self.composer, &self.composer_target)
        {
            if let Some(host) = core.host_if_instance(target.host(), *instance) {
                owner.refresh_mentions(host, theme);
                self.composer_mentions_dirty = false;
            }
        }
    }

    pub fn sync_question_pages(&mut self, core: &Core) {
        if let Some(owner) = &self.composer {
            let mut composer = owner.controller.borrow_mut();
            let before = composer.editor.borrow().text().to_owned();
            crate::conversation::adapter::sync_questionnaires(
                &self.conversation,
                core,
                &mut composer,
            );
            if composer.editor.borrow().text() != before {
                self.composer_editor_dirty = true;
            }
            self.conversation
                .sync_question_pages(core, &composer.questionnaire);
        }
    }

    fn route_composer_effects(
        &mut self,
        core: &Core,
        effects: Vec<crate::composer::view::ComposerEffect>,
    ) {
        use crate::composer::view::ComposerEffect;
        for effect in effects {
            match effect {
                ComposerEffect::EditorChanged => self.composer_editor_dirty = true,
                ComposerEffect::FocusEditor => self.composer_focus_pending = true,
                ComposerEffect::NewProject => self.sidebar.add_project(core),
                ComposerEffect::RetryQuestionPage => {
                    self.conversation.question_pages.retry_question(core)
                }
                ComposerEffect::OpenQuestionSource(source) => {
                    if let (Some(owner), Some((target, Some(instance)))) =
                        (&self.composer, &self.composer_target)
                    {
                        let composer = owner.controller.borrow();
                        if self.conversation.question_pages.open_source(
                            core,
                            target.host(),
                            *instance,
                            &composer.questionnaire,
                            source,
                        ) {
                            self.question_source_revision =
                                self.question_source_revision.wrapping_add(1);
                        }
                    }
                }
                effect => self.composer_effects.push(effect),
            }
        }
    }

    pub fn take_composer_effects(&mut self) -> Vec<crate::composer::view::ComposerEffect> {
        std::mem::take(&mut self.composer_effects)
    }

    pub fn dispatch_composer_actions(&mut self, core: &mut Core) -> bool {
        let mut changed = false;
        for action in self.take_composer_actions() {
            self.sync_question_pages(core);
            let question = self.composer.as_ref().and_then(|owner| {
                let composer = owner.controller.borrow();
                self.conversation
                    .question_pages
                    .current_question(&composer.questionnaire)
                    .cloned()
            });
            let (effects, action_changed) = self.composer.as_mut().map_or_else(
                || (Vec::new(), false),
                |owner| owner.dispatch_with_change(core, action, question.as_ref()),
            );
            changed |= action_changed;
            self.route_composer_effects(core, effects);
        }
        self.sync_question_pages(core);
        changed
    }

    pub fn handle_question_option(
        &mut self,
        core: &mut Core,
        action: crate::conversation::questions::OptionAction,
    ) -> bool {
        let Some(owner) = &self.composer else {
            return false;
        };
        let before = self
            .conversation
            .question_pages
            .options_browser()
            .map(|browser| browser.borrow().view_state());
        let composer = owner.controller.borrow();
        let action = self.conversation.question_pages.handle_option_action(
            core,
            &composer.questionnaire,
            action,
        );
        drop(composer);
        if let Some(action) = action {
            self.composer_actions.borrow_mut().push_back(action);
        }
        let changed = self.dispatch_composer_actions(core);
        changed
            || before
                != self
                    .conversation
                    .question_pages
                    .options_browser()
                    .map(|browser| browser.borrow().view_state())
    }

    pub fn question_source_view(
        &mut self,
        ui: &mut aqua::UiApp,
        theme: crate::ui::theme::ArtisanTheme,
        document: aqua_text::document::SelectionDocumentId,
        selection: aqua_text::document::SelectionSourceId,
    ) -> Result<Option<aqua_ui::element::Element>, aqua_text::document::SelectionSourceError> {
        let identity = self
            .composer_target
            .as_ref()
            .and_then(|(target, instance)| {
                let owner = self.composer.as_ref()?;
                let composer = owner.controller.borrow();
                let item = composer.questionnaire.active()?.item.clone();
                let source = self
                    .conversation
                    .question_pages
                    .source_view
                    .as_ref()?
                    .window()
                    .source()
                    .clone();
                Some(QuestionSourceIdentity {
                    host: target.host().clone(),
                    instance: (*instance)?,
                    item,
                    step: composer.questionnaire.step,
                    source,
                    revision: self.question_source_revision,
                    document,
                    selection,
                })
            });
        if self.question_source.as_ref().map(|(current, _)| current) != identity.as_ref() {
            if let Some((_, old)) = self.question_source.take() {
                old.release(ui);
            }
            self.question_source_actions.borrow_mut().clear();
            self.question_source = identity.map(|identity| {
                (
                    identity,
                    crate::shell::source::SourceView::new(document, selection),
                )
            });
        }
        let Some((_, service)) = &self.question_source else {
            return Ok(None);
        };
        let Some(content) = self.conversation.question_pages.source_view.as_ref() else {
            return Ok(None);
        };
        let actions = Rc::clone(&self.question_source_actions);
        service
            .render(
                ui,
                content,
                theme,
                Rc::new(move |action| actions.borrow_mut().push_back(action)),
            )
            .map(Some)
    }

    pub fn composer_content(
        &mut self,
        ui: &mut aqua::UiApp,
        theme: crate::ui::theme::ArtisanTheme,
        reduced_motion: bool,
    ) -> anyhow::Result<(
        Option<aqua_ui::element::Element>,
        Option<aqua_ui::element::Element>,
    )> {
        let (document, source) = self.selections.question;
        let source_view = self.question_source_view(ui, theme, document, source)?;
        let browser = self.conversation.question_pages.options_browser();
        let identity = browser.as_ref().and_then(|browser| {
            let browser = browser.borrow();
            Some((browser.identity(), browser.detail.as_ref()?.identity()))
        });
        if self
            .question_option_detail
            .as_ref()
            .map(|(identity, _)| identity)
            != identity.as_ref()
        {
            if let Some((_, service)) = self.question_option_detail.take() {
                service.release(ui);
            }
            self.question_option_detail_actions.borrow_mut().clear();
            self.question_option_detail = identity.map(|identity| {
                let (document, source) = self.selections.question_option_detail;
                (
                    identity,
                    crate::shell::source::SourceView::new(document, source),
                )
            });
        }
        let stored_detail = if let (Some(browser), Some((identity, service))) =
            (&browser, &self.question_option_detail)
        {
            let browser = browser.borrow();
            if let Some(content) = browser.detail.as_ref() {
                let identity = identity.clone();
                let actions = Rc::clone(&self.question_option_detail_actions);
                Some(service.render(
                    ui,
                    content,
                    theme,
                    Rc::new(move |action| {
                        actions.borrow_mut().push_back((identity.clone(), action))
                    }),
                )?)
            } else {
                None
            }
        } else {
            None
        };
        let owner = self
            .composer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("conversation composer is not mounted"))?;
        let composer = owner.controller.borrow();
        let actions = Rc::clone(&self.composer_actions);
        let emit: crate::composer::view::Dispatch =
            Rc::new(move |action| actions.borrow_mut().push_back(action));
        let (options_document, options_source) = self.selections.question_options;
        let options = crate::conversation::questions::stored_options(
            &self.conversation.question_pages,
            &composer.questionnaire,
            theme,
            options_document,
            options_source,
            stored_detail,
            self.question_option_emit(),
        )
        .map_err(anyhow::Error::msg)?;
        let options = if let Some(options) = options {
            if let Some(selection) = options.selection {
                ui.register_selection_source_in(options_document, selection)?;
            } else {
                ui.unregister_selection_source_in(options_document, options_source);
            }
            Some(options.element)
        } else {
            ui.unregister_selection_source_in(options_document, options_source);
            None
        };
        let question = self
            .conversation
            .question_pages
            .current_question(&composer.questionnaire);
        let questionnaires = crate::composer::view::questionnaire(
            theme,
            &composer.questionnaire,
            question,
            options,
            self.conversation.question_pages.question_error(),
            source_view,
            Rc::clone(&emit),
        );
        let attachments = crate::composer::view::attachments(
            theme,
            &composer,
            std::time::Instant::now(),
            reduced_motion,
            emit,
        );
        Ok((questionnaires, attachments))
    }

    pub fn bind_question_source(
        &self,
        ctx: &aqua::AppCtx<'_>,
        window: aqua::prelude::AquaWindowId,
    ) {
        if let (Some((_, service)), Some(content)) = (
            &self.question_source,
            &self.conversation.question_pages.source_view,
        ) {
            service.bind(content, ctx, window);
        }
        if let (Some((identity, service)), Some(browser)) = (
            &self.question_option_detail,
            self.conversation.question_pages.options_browser(),
        ) {
            let browser = browser.borrow();
            if browser.identity() == identity.0 {
                if let Some(content) = browser
                    .detail
                    .as_ref()
                    .filter(|content| content.identity() == identity.1)
                {
                    service.bind(content, ctx, window);
                }
            }
        }
    }

    pub fn drain_question_source(
        &mut self,
        core: &Core,
        routes: &crate::shell::dialogs::DialogRoutes,
        ctx: &mut aqua::AppCtx<'_>,
        window: aqua::prelude::AquaWindowId,
    ) -> bool {
        let mut changed = false;
        let actions = std::mem::take(&mut *self.question_source_actions.borrow_mut());
        if let (Some((_, service)), Some(content)) = (
            &mut self.question_source,
            &mut self.conversation.question_pages.source_view,
        ) {
            for action in actions {
                changed |= service.action(core, content, ctx, routes, window, action);
            }
            changed |= service.drain(core, content, ctx);
        }
        let actions = std::mem::take(&mut *self.question_option_detail_actions.borrow_mut());
        if let (Some((identity, service)), Some(browser)) = (
            &mut self.question_option_detail,
            self.conversation.question_pages.options_browser(),
        ) {
            let mut browser = browser.borrow_mut();
            if browser.identity() == identity.0 {
                if let Some(content) = browser
                    .detail
                    .as_mut()
                    .filter(|content| content.identity() == identity.1)
                {
                    for (origin, action) in actions {
                        if &origin == identity {
                            changed |= service.action(core, content, ctx, routes, window, action);
                        }
                    }
                    changed |= service.drain(core, content, ctx);
                }
            }
        }
        changed
    }

    pub fn release_question_source(&mut self, ui: &mut aqua::UiApp) {
        if let Some((_, service)) = self.question_source.take() {
            service.release(ui);
        }
        self.question_source_actions.borrow_mut().clear();
    }

    pub fn release_question_options(&mut self, ui: &mut aqua::UiApp) {
        if let Some((_, service)) = self.question_option_detail.take() {
            service.release(ui);
        }
        self.question_option_detail_actions.borrow_mut().clear();
        ui.unregister_selection_source_in(
            self.selections.question_options.0,
            self.selections.question_options.1,
        );
    }

    pub fn question_option_emit(&self) -> Rc<dyn Fn(crate::conversation::questions::OptionAction)> {
        let actions = Rc::clone(&self.question_option_actions);
        Rc::new(move |action| actions.borrow_mut().push_back(action))
    }

    pub fn sidebar_emit(&self, settings: bool) -> Rc<dyn Fn(crate::sidebar::view::ViewAction)> {
        let actions = Rc::clone(if settings {
            &self.settings_sidebar_actions
        } else {
            &self.sidebar_actions
        });
        Rc::new(move |action| actions.borrow_mut().push_back(action))
    }

    pub fn usage_emit(&self) -> Rc<dyn Fn(crate::sidebar::usage_page::Action)> {
        let actions = Rc::clone(&self.usage_actions);
        Rc::new(move |action| actions.borrow_mut().push_back(action))
    }

    pub fn sidebar_view(
        &self,
        core: &Core,
        theme: crate::ui::theme::ArtisanTheme,
        viewport: aqua_core::Size,
        visible: bool,
        reduced_motion: bool,
        now: std::time::Instant,
        settings: bool,
    ) -> aqua_ui::element::Element {
        let sidebar = if settings {
            &self.settings_sidebar
        } else {
            &self.sidebar
        };
        crate::sidebar::view::build(
            sidebar,
            core,
            theme,
            viewport,
            visible,
            reduced_motion,
            now,
            sidebar.thread_visual_factory(theme, reduced_motion),
            self.sidebar_emit(settings),
        )
    }

    pub fn usage_view(
        &self,
        core: &Core,
        theme: crate::ui::theme::ArtisanTheme,
        viewport: aqua_core::Size,
    ) -> aqua_ui::element::Element {
        crate::sidebar::usage_page::build(
            &self.usage,
            core,
            theme,
            artisan_api::now(),
            viewport,
            self.usage_emit(),
        )
    }

    pub fn marketplace_emit(&self) -> Rc<dyn Fn(crate::marketplace::view::ViewAction)> {
        let actions = Rc::clone(&self.marketplace_actions);
        Rc::new(move |action| actions.borrow_mut().push_back(action))
    }

    pub fn home_emit(&self) -> Rc<dyn Fn(crate::settings::view::ViewAction)> {
        let actions = Rc::clone(&self.home_actions);
        Rc::new(move |action| actions.borrow_mut().push_back(action))
    }

    pub fn home_view(
        &self,
        core: &Core,
        theme: &crate::ui::theme::ArtisanTheme,
    ) -> aqua_ui::element::Element {
        crate::shell::view::home(
            theme,
            crate::settings::account_view::forges(&self.home, core, theme, self.home_emit()),
        )
    }

    pub fn settings_emit(&self) -> Rc<dyn Fn(crate::settings::view::ViewAction)> {
        let actions = Rc::clone(&self.settings_actions);
        Rc::new(move |action| actions.borrow_mut().push_back(action))
    }

    pub fn dispatch_sidebar_actions(&mut self, core: &mut Core) -> bool {
        let mut changed = false;
        let sidebar = std::mem::take(&mut *self.sidebar_actions.borrow_mut());
        for action in sidebar {
            if let crate::sidebar::view::ViewAction::SetDarkMode(dark) = &action {
                self.theme_transition = Some(*dark);
            }
            changed |= crate::sidebar::view::dispatch(&mut self.sidebar, core, action);
        }
        let settings_sidebar = std::mem::take(&mut *self.settings_sidebar_actions.borrow_mut());
        for action in settings_sidebar {
            if let crate::sidebar::view::ViewAction::SetDarkMode(dark) = &action {
                self.theme_transition = Some(*dark);
            }
            changed |= crate::sidebar::view::dispatch(&mut self.settings_sidebar, core, action);
        }
        changed
    }

    pub fn dispatch_actions(&mut self, core: &mut Core) -> bool {
        let mut changed = false;
        let options = std::mem::take(&mut *self.question_option_actions.borrow_mut());
        for action in options {
            changed |= self.handle_question_option(core, action);
        }
        changed |= self.dispatch_composer_actions(core);
        changed |= self.dispatch_sidebar_actions(core);
        let usage = std::mem::take(&mut *self.usage_actions.borrow_mut());
        for action in usage {
            changed |= crate::sidebar::usage_page::dispatch(&mut self.usage, core, action);
        }
        let marketplace = std::mem::take(&mut *self.marketplace_actions.borrow_mut());
        for action in marketplace {
            if matches!(
                action,
                crate::marketplace::view::ViewAction::SearchKey(
                    crate::ui::text::ListKey::Up | crate::ui::text::ListKey::Down
                )
            ) {
                self.marketplace_reveal_pending = true;
            }
            changed |= crate::marketplace::view::dispatch(&mut self.marketplace, core, action);
        }
        let home = std::mem::take(&mut *self.home_actions.borrow_mut());
        for action in home {
            match &action {
                crate::settings::view::ViewAction::OpenUrl(url) => self
                    .settings_platform_effects
                    .push(SettingsPlatformEffect::OpenUrl(url.clone())),
                crate::settings::view::ViewAction::CopyText(text) => self
                    .settings_platform_effects
                    .push(SettingsPlatformEffect::CopyText(text.clone())),
                _ => {}
            }
            changed |= crate::settings::view::dispatch(&mut self.home, core, action);
        }
        let settings = std::mem::take(&mut *self.settings_actions.borrow_mut());
        for action in settings {
            match &action {
                crate::settings::view::ViewAction::SetDarkMode(dark) => {
                    self.theme_transition = Some(*dark)
                }
                crate::settings::view::ViewAction::OpenUrl(url) => self
                    .settings_platform_effects
                    .push(SettingsPlatformEffect::OpenUrl(url.clone())),
                crate::settings::view::ViewAction::CopyText(text) => self
                    .settings_platform_effects
                    .push(SettingsPlatformEffect::CopyText(text.clone())),
                _ => {}
            }
            changed |= crate::settings::view::dispatch(&mut self.settings, core, action);
        }
        changed
    }

    pub fn take_settings_platform_effects(&mut self) -> Vec<SettingsPlatformEffect> {
        std::mem::take(&mut self.settings_platform_effects)
    }

    pub fn take_theme_transition(&mut self) -> Option<bool> {
        self.theme_transition.take()
    }

    pub fn drain_domains(&mut self, core: &mut Core, assets: &aqua::images::ImageAssets) -> bool {
        let tm_1 = crate::shell::application::Tm::new("dd.conversation");
        let conversation_changed = self.conversation.drain(core);
        drop(tm_1);
        let tm_2 = crate::shell::application::Tm::new("dd.questions");
        let question_changed = self.conversation.question_pages.drain(core);
        self.sync_question_pages(core);
        drop(tm_2);
        let tm_3 = crate::shell::application::Tm::new("dd.composer");
        let mut accepted = None;
        let mut composer_changed = false;
        if let Some(owner) = &mut self.composer {
            let target = owner.controller.borrow().target.clone();
            let result = owner.drain_async(core, assets);
            composer_changed = result.changed;
            for event in result.events {
                match event {
                    crate::composer::persistence::SessionEvent::Accepted { thread, .. } => {
                        accepted = Some((target.clone(), thread));
                        self.composer_editor_dirty = true;
                    }
                    crate::composer::persistence::SessionEvent::Reloaded
                    | crate::composer::persistence::SessionEvent::Changed => {
                        self.composer_editor_dirty = true;
                    }
                }
            }
            self.route_composer_effects(core, result.effects);
        }
        drop(tm_3);
        if let Some((target, thread)) = accepted {
            if matches!(
                target,
                crate::composer::controller::ComposerTarget::NewThread { .. }
            ) {
                core.navigate(Screen::Thread {
                    host: target.host().clone(),
                    thread: thread.clone(),
                });
            }
            if core.workspace_screen().thread() == Some(&thread) {
                self.conversation
                    .jump(core, crate::conversation::viewport::Jump::Latest);
            }
        }
        let tm_4 = crate::shell::application::Tm::new("dd.qpages2");
        self.sync_question_pages(core);
        drop(tm_4);
        let now = artisan_api::now();
        let tm_5 = crate::shell::application::Tm::new("dd.sidebar");
        let sidebar = self.sidebar.drain(core, now);
        drop(tm_5);
        let tm_6 = crate::shell::application::Tm::new("dd.settings_sidebar");
        let settings_sidebar = self.settings_sidebar.drain(core, now);
        drop(tm_6);
        let tm_7 = crate::shell::application::Tm::new("dd.others");
        let usage = self.usage.drain(core);
        let marketplace = self.marketplace.drain(core);
        let settings = self.settings.drain(core);
        let home = self.home.drain(core);
        drop(tm_7);
        let tm_8 = crate::shell::application::Tm::new("dd.images");
        let images_changed = self.images.drain(assets) | self.images.poll_assets();
        drop(tm_8);
        conversation_changed
            || images_changed
            || composer_changed
            || question_changed
            || sidebar
            || settings_sidebar
            || usage
            || marketplace
            || settings
            || home
    }

    pub fn sync_editor_ui(&mut self, ui: &mut aqua::UiApp, settings_visible: bool) {
        if self.composer_editor_dirty {
            if let Some(owner) = &self.composer {
                if let Some(key) = owner.editor.text_key() {
                    if let Some(node) = keyed_node(ui, key) {
                        owner.editor.sync(ui.tree_mut(), node);
                        self.composer_editor_dirty = false;
                    }
                }
            }
        }
        if !self.composer_editor_dirty
            && self.composer_focus_pending
            && ui.focus_key("composer-prompt")
        {
            self.composer_focus_pending = false;
        }
        if settings_visible {
            self.settings.sync_editors(ui);
        } else {
            self.home.sync_editors(ui);
        }
    }

    pub fn sync_marketplace_ui(&mut self, ui: &mut aqua::UiApp) {
        self.marketplace_focus_pending |= self.marketplace.take_focus_request();
        if self.marketplace_focus_pending && ui.focus_key("marketplace-search") {
            self.marketplace_focus_pending = false;
        }
        if self.marketplace_reveal_pending {
            if let Some(index) = self.marketplace.selected_entry_index() {
                if ui.reveal_collection_item("marketplace-list", index) {
                    self.marketplace_reveal_pending = false;
                }
            } else {
                self.marketplace_reveal_pending = false;
            }
        }
    }

    pub fn marketplace_view(
        &self,
        core: &Core,
        theme: crate::ui::theme::ArtisanTheme,
    ) -> aqua_ui::element::Element {
        crate::marketplace::view::build(&self.marketplace, core, theme, self.marketplace_emit())
    }

    pub fn settings_view(
        &self,
        core: &Core,
        theme: crate::ui::theme::ArtisanTheme,
        build: &crate::settings::about::BuildInfo,
        viewport: aqua_core::Size,
        visible: bool,
    ) -> aqua_ui::element::Element {
        crate::settings::view::build(
            &self.settings,
            core,
            theme,
            build,
            viewport,
            visible,
            self.settings_emit(),
        )
    }

    pub fn open_reference(
        &mut self,
        core: &Core,
        path: String,
        line: Option<u32>,
        scope: Option<(crate::core::HostId, artisan_api::ThreadId)>,
    ) {
        self.resource_preview_generation = self.resource_preview_generation.wrapping_add(1);
        let source = scope.and_then(|(id, thread)| {
            core.host(&id)
                .map(|host| (id.clone(), host.instance(), host.client.clone(), thread))
        });
        self.resource_preview = Some(crate::shell::resource_preview::ResourcePreview::new(
            path, line, source,
        ));
    }

    pub fn dismiss_reference(&mut self) {
        self.resource_preview = None;
        self.resource_preview_generation = self.resource_preview_generation.wrapping_add(1);
    }

    pub fn resource_preview_generation(&self) -> u64 {
        self.resource_preview_generation
    }

    pub fn drain_reference(&mut self, core: &Core) -> bool {
        self.resource_preview
            .as_mut()
            .is_some_and(|preview| preview.drain(core))
    }

    pub fn open_palette(&mut self, core: &Core) {
        if self.palette.is_none() {
            self.palette = Some(Palette::default());
            self.refresh_palette(core);
        }
    }

    pub fn dismiss_palette(&mut self) {
        self.palette = None;
    }

    pub fn search_palette(&mut self, core: &Core, query: String) {
        if let Some(palette) = &mut self.palette {
            let hosts = core
                .hosts()
                .filter(|(_, host)| host.is_live())
                .map(|(config, host)| SearchHost {
                    id: config.id.clone(),
                    name: config.name.clone(),
                    client: host.client.clone(),
                    instance: host.instance(),
                })
                .collect();
            palette.start_search(query, hosts);
        }
        self.refresh_palette(core);
    }

    pub fn drain_palette(&mut self, core: &Core) -> bool {
        let changed = self
            .palette
            .as_mut()
            .is_some_and(|palette| palette.drain(core));
        if changed {
            self.refresh_palette(core);
        }
        changed
    }

    pub fn refresh_palette(&mut self, core: &Core) {
        if let Some(palette) = &mut self.palette {
            let active = core.active_host().map(|host| &host.id);
            let hosts: Vec<_> = core
                .hosts()
                .map(|(config, host)| HostSnapshot {
                    id: &config.id,
                    name: &config.name,
                    instance: host.instance(),
                    threads: &host.threads,
                    projects: &host.projects,
                })
                .collect();
            palette.refresh(active, &hosts);
        }
    }

    pub fn activate_palette(&mut self, core: &mut Core) {
        if let Some(palette) = self.palette.take() {
            if let Some(screen) = palette.selected() {
                core.navigate(screen.clone());
            }
        }
    }

    pub fn new_thread(core: &mut Core) {
        let screen = core
            .active_host()
            .map(|host| Screen::NewThread {
                host: host.id.clone(),
                project: None,
            })
            .unwrap_or(Screen::Home);
        core.navigate(screen);
    }

    pub fn open_settings(core: &mut Core) {
        core.navigate(Screen::Settings(SettingsPage::General));
    }
}

fn keyed_node(ui: &aqua::UiApp, key: aqua_ui::element::Key) -> Option<aqua_core::NodeId> {
    let hash = aqua_ui::tree::hash_key(key);
    ui.tree().sole_node_with_key(hash)
}

fn keyed_descendant(
    ui: &aqua::UiApp,
    root: aqua_core::NodeId,
    key: aqua_ui::element::Key,
) -> Option<aqua_core::NodeId> {
    let hash = aqua_ui::tree::hash_key(key);
    let mut pending = vec![root];
    let mut found = None;
    while let Some(node) = pending.pop() {
        let record = ui.tree().get(node)?;
        if record.key == Some(hash) && found.replace(node).is_some() {
            return None;
        }
        pending.extend(record.children.iter().copied());
    }
    found
}

impl Workspace {
    pub fn acknowledge_painted(
        &self,
        core: &mut Core,
        painted_row: &crate::conversation::adapter::RowModel,
        bounds: aqua_core::Rect,
        clip: aqua_core::Rect,
        window_active: bool,
        revealed: bool,
        prepared: impl Fn(&crate::conversation::fold::Row, &artisan_api::Item) -> bool,
    ) -> bool {
        if !crate::conversation::read::painted(bounds, clip, window_active, revealed) {
            return false;
        }
        let row = &painted_row.row;
        let current = |row: &crate::conversation::fold::Row, item: &artisan_api::Item| {
            let key = painted_row.row.key();
            row.key() == key
                && self
                    .conversation
                    .rows
                    .iter()
                    .any(|(row, version)| row.key() == key && *version == painted_row.version)
                && painted_row
                    .items
                    .get(&item.id)
                    .is_some_and(|painted| painted.seq == item.seq)
                && prepared(row, item)
        };
        let Some(candidate) =
            crate::conversation::read::capture(&self.conversation, core, row, &current)
        else {
            return false;
        };
        crate::conversation::read::acknowledge(candidate, &self.conversation, core, current)
    }
}
