use super::RootViews;
use crate::shell::host::Shell;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};
use std::rc::Rc;

impl RootViews {
    pub(super) fn sync_sign_in(
        &mut self,
        shell: &mut Shell,
        ui: &UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        use crate::frame::sign_in::{CODE_FEEDBACK, CODE_PAGE, EMAIL_FEEDBACK, EMAIL_PAGE, Sizes};
        if !matches!(
            shell.core.entry_gate(),
            crate::core::EntryGate::Restoring | crate::core::EntryGate::SignedOut
        ) {
            return;
        }
        let reduced = Self::reduced(ui);
        let images = shell.images().clone();
        let sign_in = &mut shell.entry.sign_in;
        sign_in.reduced = reduced;
        let painted = sign_in.art().is_some();
        sign_in.ensure_art(&images);
        let height = |selector| {
            ui.find_debug_selector(selector)
                .and_then(|node| ui.tree().rect(node))
                .map_or(0.0, |rect| rect.h)
        };
        let sizes = Sizes {
            email_page: height(EMAIL_PAGE),
            code_page: height(CODE_PAGE),
            email_feedback: height(EMAIL_FEEDBACK),
            code_feedback: height(CODE_FEEDBACK),
        };
        let measured = sign_in.measure(sizes, shell.core.clock().now(), reduced);
        let dirty = measured || painted != sign_in.art().is_some();
        if dirty || sign_in.animating() {
            self.sign_in_dirty |= dirty;
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
    }

    pub(super) fn apply_account_effects(
        &mut self,
        shell: &mut Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        reduced: bool,
    ) {
        use crate::ui::chrome::menu::Effect;
        for effect in self.sidebar_views.sidebar.take_effects() {
            match effect {
                Effect::FlipTheme => {
                    let dark = !shell.preferences.theme().colors.mode.is_dark();
                    crate::shell::application::begin_theme_transition(ctx, window, dark, reduced);
                    if let Err(error) = shell.core.set_setting("dark_mode", dark.into()) {
                        tracing::error!(
                            error = %::diagnostics::chain(&error),
                            "saving the theme"
                        );
                    }
                }
                Effect::OpenSettings => shell.core.navigate(crate::core::Screen::Settings(
                    crate::core::SettingsPage::General,
                )),
                Effect::OpenAccount => shell.core.navigate(crate::core::Screen::Settings(
                    crate::core::SettingsPage::Account,
                )),
                Effect::SignOut => shell.core.account_mut().sign_out(),
            }
        }
        if self.apply_navigation(shell, reduced) {
            self.navigation_views.navigation_invalidated.set(true);
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
    }

    pub(super) fn account(&mut self, shell: &Shell) -> Option<crate::ui::chrome::account::Account> {
        let url = if let crate::core::AccountPhase::Authenticated { profile, .. } =
            shell.core.account().phase()
        {
            profile.avatar_url.as_deref()
        } else {
            None
        };
        self.sidebar_views
            .avatar
            .want(url, shell.core.clock().now());
        self.account_snapshot(shell)
    }

    pub(super) fn account_snapshot(
        &self,
        shell: &Shell,
    ) -> Option<crate::ui::chrome::account::Account> {
        let crate::core::AccountPhase::Authenticated { profile, .. } = shell.core.account().phase()
        else {
            return None;
        };
        Some(crate::ui::chrome::account::Account {
            identity: crate::ui::chrome::account::Identity::of(
                &profile.display_name,
                &profile.email,
                profile.username.as_deref(),
            ),
            avatar: self.sidebar_views.avatar.image(),
            limits: crate::ui::chrome::limits::resolve(
                shell.core.active_host().into_iter().flat_map(|host| {
                    host.engines.values().filter_map(|status| {
                        status.usage.as_ref().map(|usage| (status.engine, usage))
                    })
                }),
            ),
        })
    }

    pub(super) fn sync_account_menu(
        &mut self,
        shell: &Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        if let Some(mailbox) = self.sidebar_views.avatar.mailbox() {
            ctx.bind_mailbox(window, mailbox);
        }
        let theme = shell.preferences.theme();
        let Some(account) = self.account(shell) else {
            self.sidebar_views.account_popup.dispose(ui);
            return;
        };
        let menu = *self.sidebar_views.sidebar.menu();
        let emit = self.sidebar_views.sidebar.emit();
        let dismissed = Rc::clone(&emit);
        let width = crate::frame::sidebar::OPEN_WIDTH
            - crate::frame::view::FOOTER_LEFT
            - crate::frame::view::SHEET_PADDING;
        let content = move || {
            crate::ui::chrome::account::menu(
                &theme,
                &crate::ui::chrome::account::Sheet {
                    account: &account,
                    menu: &menu,
                    width,
                    now: shell.core.clock().now(),
                    emit: Rc::new(move |event| emit(crate::frame::sidebar::Event::Account(event))),
                },
            )
        };
        if let Err(error) = self.sidebar_views.account_popup.sync(
            ui,
            menu.open(),
            content,
            ctx.now().as_secs_f64(),
            Rc::new(move || {
                dismissed(crate::frame::sidebar::Event::Account(
                    crate::ui::chrome::menu::Event::Dismissed,
                ));
            }),
        ) {
            tracing::warn!(error = %diagnostics::chain(&error), "could not open the account menu");
        }
        match self
            .sidebar_views
            .account_popup
            .advance(ui, ctx.now().as_secs_f64())
        {
            Ok(true) => ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(error = %diagnostics::chain(&error), "could not animate the account menu");
            }
        }
    }
}
