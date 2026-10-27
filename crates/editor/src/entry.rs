use editor_core::{Core, CoreEvent, EntryGate};
use editor_settings::account::{controller, sign_in::SignInView};
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::theme::{ActiveTheme as _, RadiusStep, RadiusTokens};
use editor_ui::window_controls::WindowControls;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, WindowControlArea, div, px,
};
use gpui::prelude::FluentBuilder as _;

use crate::workspace::Workspace;

pub mod setup;
use setup::SetupView;

#[cfg(all(test, feature = "capture"))]
#[path = "../tests/entry.rs"]
pub(crate) mod behavior;

pub const KEY_CONTEXT: &str = "Entry";

pub struct EntryRoot {
    core: Entity<Core>,
    workspace: Option<Entity<Workspace>>,
    mounted: Option<(String, u64)>,
    sign_in: Entity<SignInView>,
    setup: Entity<SetupView>,
    failure: Option<String>,
    focus: FocusHandle,
    _core: Subscription,
    _account: Subscription,
}

impl EntryRoot {
    pub fn new(
        core: Entity<Core>,
        failure: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sign_in = cx.new(|cx| SignInView::new(core.clone(), cx));
        let setup = cx.new(|cx| SetupView::new(core.clone(), true, cx));
        let core_subscription = cx.subscribe_in(&core, window, |this, _, event: &CoreEvent, window, cx| {
            if matches!(
                event,
                CoreEvent::ScreenChanged
                    | CoreEvent::HostsChanged
                    | CoreEvent::SettingsChanged
                    | CoreEvent::AccessChanged
                    | CoreEvent::McpSignInChanged
            ) {
                this.sync(window, cx);
            }
        });
        let account = core.read(cx).account().clone();
        let account_subscription = cx.subscribe_in(
            &account,
            window,
            |this, _, _: &editor_core::AccountEvent, window, cx| {
                this.sync(window, cx);
            },
        );
        let mut this = Self {
            core,
            workspace: None,
            mounted: None,
            sign_in,
            setup,
            failure,
            focus: cx.focus_handle(),
            _core: core_subscription,
            _account: account_subscription,
        };
        this.sync(window, cx);
        window.focus(&this.focus, cx);
        this
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = match self.core.read(cx).entry_gate(cx) {
            EntryGate::Ready { account_id } => {
                let generation = self
                    .core
                    .read(cx)
                    .account()
                    .read(cx)
                    .generation();
                Some((account_id, generation))
            }
            EntryGate::Restoring
            | EntryGate::SignedOut
            | EntryGate::NeedsSetup { .. } => None,
        };
        if key != self.mounted {
            self.workspace = None;
            self.mounted = None;
        }
        if let Some((account_id, generation)) = key
            && self.workspace.is_none()
        {
            let core = self.core.clone();
            let failure = self.failure.clone();
            self.workspace = Some(cx.new(|cx| Workspace::new(core, failure, window, cx)));
            self.mounted = Some((account_id, generation));
        }
        cx.notify();
    }

    fn titlebar(window: &Window, cx: &App) -> AnyElement {
        let theme = *cx.theme();
        div()
            .h(px(48.0))
            .w_full()
            .flex()
            .flex_shrink_0()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(WindowControls::new(theme, window.is_maximized()))
            .into_any_element()
    }

    fn shell(&self, title: &str, body: AnyElement, window: &Window, cx: &App) -> AnyElement {
        let theme = *cx.theme();
        let narrow = window.viewport_size().width < px(640.0);
        // Tall setup content must stay reachable on short windows: cap the
        // card below the viewport and let it scroll internally.
        let max_h = (window.viewport_size().height - px(96.0)).max(px(200.0));
        let column = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .w_full()
            .child(
                div()
                    .text_size(px(20.0))
                    .text_color(theme.colors.text.to_paint())
                    .child(title.to_owned()),
            )
            .child(body);
        div()
            .id("entry-root")
            .debug_selector(|| "entry-root".to_owned())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.frame.to_paint())
            .text_color(theme.colors.text.to_paint())
            .font_family(cx.theme().typography.sans.family)
            .text_size(px(14.0))
            .tracking(cx.theme().typography.tracking_em)
            .child(Self::titlebar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("entry-card")
                            .bg(theme.colors.background.to_paint())
                            .border_1()
                            .border_color(theme.colors.line.to_paint())
                            .rounded(RadiusTokens::value(RadiusStep::X2l))
                            .p(px(24.0))
                            .max_h(max_h)
                            .overflow_y_scroll()
                            .when(!narrow, |card| card.w(px(560.0)))
                            .when(narrow, |card| card.w_full().mx(px(16.0)))
                            .child(column),
                    ),
            )
            .into_any_element()
    }
}

impl Render for EntryRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(workspace) = self.workspace.clone() {
            return workspace.into_any_element();
        }
        match self.core.read(cx).entry_gate(cx) {
            EntryGate::Ready { .. } => div().into_any_element(),
            EntryGate::Restoring => {
                let theme = *cx.theme();
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_color(theme.colors.text_muted.to_paint())
                            .child(
                                "Checking the saved sign-in with the account service.",
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                Button::text("entry-retry", theme, "Retry")
                                    .variant(ButtonVariant::Default)
                                    .debug_selector("entry-retry")
                                    .on_activate(cx.listener(|this, _, _, cx| {
                                        controller::refresh(&this.core, cx);
                                    }))
                                    .into_any_element(),
                            )
                            .child(
                                Button::text(
                                    "entry-use-another",
                                    theme,
                                    "Use another account",
                                )
                                .variant(ButtonVariant::Ghost)
                                .debug_selector("entry-use-another")
                                .on_activate(cx.listener(|this, _, _, cx| {
                                    controller::sign_out(&this.core, false, cx);
                                }))
                                .into_any_element(),
                            ),
                    )
                    .into_any_element();
                self.shell("Restoring the session", body, window, cx)
            }
            EntryGate::SignedOut => {
                let form = self.sign_in.clone().into_any_element();
                self.shell("Sign in to Artisan", form, window, cx)
            }
            EntryGate::NeedsSetup { .. } => {
                let body = self.setup.clone().into_any_element();
                self.shell("Connect a Forge", body, window, cx)
            }
        }
    }
}
