use crate::core::{Core, CoreEvent, EntryGate, Screen};
use crate::shell::workspace::{NotificationEffects, Workspace};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

pub struct Entry {
    workspace: Option<(String, Workspace)>,
    failure: Option<String>,
    landed: bool,
    controller: crate::settings::controller::SettingsController,
    actions: Rc<RefCell<VecDeque<crate::settings::view::ViewAction>>>,
    selections: crate::shell::selection::SelectionAllocator,
}

impl Entry {
    pub fn new(
        failure: Option<String>,
        selections: crate::shell::selection::SelectionAllocator,
    ) -> Self {
        Self {
            workspace: None,
            failure,
            landed: false,
            controller: crate::settings::controller::SettingsController::default(),
            actions: Rc::new(RefCell::new(VecDeque::new())),
            selections,
        }
    }

    pub fn emit(&self) -> Rc<dyn Fn(crate::settings::view::ViewAction)> {
        let actions = Rc::clone(&self.actions);
        Rc::new(move |action| actions.borrow_mut().push_back(action))
    }

    pub fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        ctx.bind_mailbox(window, self.controller.wake_mailbox());
    }

    pub fn dispatch(
        &mut self,
        core: &mut Core,
    ) -> (Vec<crate::shell::workspace::SettingsPlatformEffect>, bool) {
        let mut effects = Vec::new();
        let mut changed = false;
        for action in std::mem::take(&mut *self.actions.borrow_mut()) {
            match &action {
                crate::settings::view::ViewAction::OpenUrl(url) => effects.push(
                    crate::shell::workspace::SettingsPlatformEffect::OpenUrl(url.clone()),
                ),
                crate::settings::view::ViewAction::CopyText(text) => effects.push(
                    crate::shell::workspace::SettingsPlatformEffect::CopyText(text.clone()),
                ),
                _ => {}
            }
            changed |= crate::settings::view::dispatch(&mut self.controller, core, action);
        }
        (effects, changed)
    }

    pub fn drain(&mut self, core: &mut Core) -> bool {
        self.controller.drain(core)
    }

    pub fn view(
        &self,
        core: &Core,
        theme: &crate::ui::theme::ArtisanTheme,
        viewport: crate::shell::layout::Size,
        titlebar: &crate::shell::titlebar::Titlebar,
        window: &aqua::prelude::ObservedWindowState,
    ) -> Option<aqua_ui::element::Element> {
        let (title, body) = match core.entry_gate() {
            EntryGate::Ready { .. } => return None,
            EntryGate::NeedsSetup { .. } => (
                "Connect a Forge",
                crate::settings::account_view::forges(&self.controller, core, theme, self.emit()),
            ),
            EntryGate::Restoring | EntryGate::SignedOut => (
                "Sign in to Artisan",
                crate::settings::account_view::sign_in(&self.controller, core, theme, self.emit()),
            ),
        };
        Some(crate::shell::view::entry(
            theme,
            viewport,
            titlebar.entry(*theme, window),
            title,
            body,
        ))
    }

    pub fn sync_editors(&self, ui: &mut aqua::UiApp) {
        if self.workspace.is_some() {
            return;
        }
        self.controller.sync_editors(ui);
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref().map(|(_, workspace)| workspace)
    }

    #[cfg(feature = "qualification")]
    pub fn qualification_account(&self) -> Option<&str> {
        self.workspace.as_ref().map(|(account, _)| account.as_str())
    }

    pub fn workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.workspace.as_mut().map(|(_, workspace)| workspace)
    }

    pub fn sync(&mut self, core: &Core) -> Option<NotificationEffects> {
        let ready = match core.entry_gate() {
            EntryGate::Ready { account_id } => Some(account_id.clone()),
            EntryGate::Restoring | EntryGate::SignedOut | EntryGate::NeedsSetup { .. } => None,
        };
        if self.workspace.as_ref().map(|(account, _)| account) == ready.as_ref() {
            return None;
        }
        self.workspace = None;
        self.landed = false;
        ready.map(|account| {
            let (workspace, effects) =
                Workspace::new(core, self.failure.clone(), self.selections.workspace());
            self.workspace = Some((account, workspace));
            effects
        })
    }

    pub fn land(&mut self, core: &mut Core) {
        if self.workspace.is_none() || self.landed {
            return;
        }
        match core.screen() {
            Screen::Home => {
                if let Some(host) = core.active_host().map(|host| host.id.clone()) {
                    self.landed = true;
                    core.navigate(Screen::NewThread {
                        host,
                        project: None,
                    });
                }
            }
            _ => self.landed = true,
        }
    }

    pub fn finish_events(&mut self, core: &mut Core, ui: &mut aqua::UiApp) {
        if let Some(workspace) = self.workspace_mut() {
            workspace.finish_events(core, ui);
        }
    }

    pub fn handle_event(
        &mut self,
        core: &mut Core,
        event: &CoreEvent,
        window_active: bool,
        ui: &mut aqua::UiApp,
    ) -> Option<NotificationEffects> {
        self.controller.core_event(core, event);
        let ready = match core.entry_gate() {
            EntryGate::Ready { account_id } => Some(account_id),
            _ => None,
        };
        if self.workspace.as_ref().map(|(account, _)| account) != ready {
            if let Some((_, workspace)) = &mut self.workspace {
                workspace.dispose_composer_popups(ui);
                workspace.sidebar.threads_hover.dispose(ui);
                workspace.settings_sidebar.threads_hover.dispose(ui);
                workspace.navigator.dispose(ui);
                workspace.conversation_selection.clear(ui);
                workspace.markdown.borrow_mut().rebind(ui, None);
                workspace.dispose_terminal_dialog(ui);
                workspace.inspector.release(ui);
            }
        }
        if let CoreEvent::OpenUrl(url) = event {
            if let Err(error) = crate::shell::open_url(url) {
                tracing::warn!(%error, "could not open URL");
            }
        }
        let mounted = self.sync(core);
        self.land(core);
        let effects = self
            .workspace_mut()
            .map(|workspace| workspace.handle_event(core, event, window_active));
        match (mounted, effects) {
            (Some(mut mounted), Some(effects)) => {
                mounted.withdraw.extend(effects.withdraw);
                mounted.show.extend(effects.show);
                if effects.badge.is_some() {
                    mounted.badge = effects.badge;
                }
                Some(mounted)
            }
            (mounted, effects) => mounted.or(effects),
        }
    }
}
