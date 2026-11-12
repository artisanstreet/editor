use crate::core::{AccountPhase, Core, CoreEvent, EntryGate, Screen};

const AUTH_ORIGIN: &str = "https://auth.artisan.st";
use crate::shell::workspace::{NotificationEffects, Workspace};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

pub struct Entry {
    workspace: Option<(String, Workspace)>,
    failure: Option<String>,
    landed: bool,
    controller: crate::settings::controller::SettingsController,
    actions: Rc<RefCell<VecDeque<crate::settings::view::ViewAction>>>,
    selections: crate::shell::selection::SelectionAllocator,
    pub sign_in: crate::frame::sign_in::SignIn,
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
            sign_in: crate::frame::sign_in::SignIn::default(),
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
        changed |= self.dispatch_sign_in(core, &mut effects);
        (effects, changed)
    }

    fn dispatch_sign_in(
        &mut self,
        core: &mut Core,
        effects: &mut Vec<crate::shell::workspace::SettingsPlatformEffect>,
    ) -> bool {
        use crate::core::account::SecondFactor;
        use crate::frame::sign_in::{Event, model};
        let mut changed = false;
        for event in self.sign_in.take_events() {
            let view = model::view_of(core.account().phase(), self.sign_in.problem);
            match event {
                Event::SubmitEmail if !model::locked(&view) => {
                    let typed = self.controller.sign_in.email.borrow().text().to_owned();
                    match model::checked_email(&typed) {
                        Err(problem) => self.sign_in.problem = Some(problem),
                        Ok(email) => {
                            self.sign_in.problem = None;
                            let device = core.device_name.clone();
                            core.account_mut().request_link(email, device);
                        }
                    }
                }
                Event::SubmitCode
                    if matches!(view, model::View::SecondFactor { .. }) && !model::locked(&view) =>
                {
                    let typed = self.controller.sign_in.second_factor.borrow().text().to_owned();
                    core.account_mut().finish_sign_in(SecondFactor::parse(&typed));
                }
                Event::Provider(provider) if !model::locked(&view) => {
                    self.sign_in.problem = None;
                    if let Some(url) = core.account_mut().sign_in_with(provider, AUTH_ORIGIN) {
                        effects.push(crate::shell::workspace::SettingsPlatformEffect::OpenUrl(url));
                    }
                }
                Event::SubmitEmail | Event::SubmitCode | Event::Provider(_) => {}
                Event::Edited => {
                    self.sign_in.problem = None;
                    match core.account().phase() {
                        AccountPhase::Error { .. } => core.account_mut().dismiss_error(),
                        AccountPhase::EmailApproved {
                            rejected: Some(_), ..
                        } => core.account_mut().clear_rejection(),
                        _ => {}
                    }
                }
                Event::Open(link) => effects.push(
                    crate::shell::workspace::SettingsPlatformEffect::OpenUrl(format!(
                        "{AUTH_ORIGIN}{}",
                        link.path()
                    )),
                ),
                Event::Cancel if model::cancellable(&view) => {
                    self.sign_in.problem = None;
                    core.account_mut().dismiss_error();
                    self.controller.sign_in.second_factor.borrow_mut().set_text("");
                }
                Event::Cancel => {}
            }
            changed = true;
        }
        if let AccountPhase::EmailApproved {
            approval,
            rejected: None,
        } = core.account().phase()
        {
            if !approval.totp_required {
                core.account_mut().finish_sign_in(None);
                changed = true;
            }
        }
        let view = model::view_of(core.account().phase(), self.sign_in.problem);
        let feedback = model::feedback_of(&view);
        let none = model::Feedback::None;
        let slots = match model::page_of(&view) {
            model::Page::Email => (&feedback, &none),
            model::Page::Code => (&none, &feedback),
        };
        let reduced = self.sign_in.reduced;
        changed
            | self.sign_in.present(
                model::page_of(&view),
                model::locked(&view),
                slots,
                std::time::Instant::now(),
                reduced,
            )
    }

    pub fn drain(&mut self, core: &mut Core) -> bool {
        self.controller.drain(core)
    }

    pub fn view(
        &self,
        core: &Core,
        theme: &crate::ui::theme::ArtisanTheme,
        titlebar: &crate::shell::titlebar::Titlebar,
        window: &aqua::prelude::ObservedWindowState,
    ) -> Option<aqua_ui::element::Element> {
        match core.entry_gate() {
            EntryGate::Ready { .. } => None,
            EntryGate::Restoring | EntryGate::SignedOut => {
                Some(self.sign_in_screen(core, theme, titlebar, window))
            }
        }
    }

    fn sign_in_screen(
        &self,
        core: &Core,
        theme: &crate::ui::theme::ArtisanTheme,
        titlebar: &crate::shell::titlebar::Titlebar,
        window: &aqua::prelude::ObservedWindowState,
    ) -> aqua_ui::element::Element {
        use crate::frame::sign_in::{Event, card, model};
        use crate::ui::text::{
            Mode,
            mount::{EditorController, EditorEvent},
        };
        let view = model::view_of(core.account().phase(), self.sign_in.problem);
        let emit = self.sign_in.emit();
        let settings = self.emit();
        let editor = |state, key: &'static str, placeholder: &str, submit: Event| {
            let emit = Rc::clone(&emit);
            let settings = Rc::clone(&settings);
            EditorController::from_shared(state, move |event| match event {
                EditorEvent::Submitted => emit(submit),
                EditorEvent::Changed => emit(Event::Edited),
                EditorEvent::PresentationChanged => {
                    settings(crate::settings::view::ViewAction::AccountPresentationChanged);
                }
                _ => {}
            })
            .element_with_notice_keyed(key, Mode::SingleLine, theme, placeholder, true, false)
        };
        let email = editor(
            self.controller.sign_in.email.clone(),
            "artisan-sign-in-email",
            "you@artisan.email",
            Event::SubmitEmail,
        );
        let code = editor(
            self.controller.sign_in.second_factor.clone(),
            "artisan-sign-in-code",
            "123456",
            Event::SubmitCode,
        );
        let card = card::card(
            theme,
            card::Props {
                view: &view,
                sign_in: &self.sign_in,
                email,
                code,
                busy: model::locked(&view),
                width: card::WIDTH,
                now: std::time::Instant::now(),
                emit,
            },
        );
        card::screen(theme, titlebar.entry(*theme, window), card)
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
            EntryGate::Restoring | EntryGate::SignedOut => None,
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
