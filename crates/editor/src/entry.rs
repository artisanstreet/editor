use editor_core::{Core, EntryGate};
use editor_settings::account::forges::ForgesView;
use editor_settings::account::sign_in::SignInView;
use editor_ui::theme::{ActiveTheme as _, RadiusStep, RadiusTokens};
use editor_ui::window_controls::WindowControls;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, WindowControlArea, div, px,
};

use crate::workspace::Workspace;

#[cfg(all(test, feature = "capture"))]
#[path = "../tests/entry.rs"]
pub(crate) mod behavior;

pub const KEY_CONTEXT: &str = "Entry";

pub struct EntryRoot {
    core: Entity<Core>,
    /// The workspace and the account it was mounted for.
    workspace: Option<(String, Entity<Workspace>)>,
    sign_in: Entity<SignInView>,
    forges: Entity<ForgesView>,
    failure: Option<String>,
    focus: FocusHandle,
    _core: Subscription,
}

impl EntryRoot {
    pub fn new(
        core: Entity<Core>,
        failure: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sign_in = cx.new(|cx| SignInView::new(core.clone(), cx));
        let forges = cx.new(|cx| ForgesView::new(core.clone(), cx));
        let subscription = cx.subscribe_in(&core, window, |this, _, _, window, cx| {
            this.sync(window, cx);
        });
        let mut this = Self {
            core,
            workspace: None,
            sign_in,
            forges,
            failure,
            focus: cx.focus_handle(),
            _core: subscription,
        };
        this.sync(window, cx);
        window.focus(&this.focus, cx);
        this
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ready = match self.core.read(cx).entry_gate(cx) {
            EntryGate::Ready { account_id } => Some(account_id),
            EntryGate::Restoring | EntryGate::SignedOut | EntryGate::NeedsSetup { .. } => None,
        };
        let mounted = self.workspace.as_ref().map(|(account_id, _)| account_id);
        if mounted == ready.as_ref() {
            if ready.is_none() {
                cx.notify();
            }
            return;
        }
        self.workspace = ready.map(|account_id| {
            let (core, failure) = (self.core.clone(), self.failure.clone());
            let workspace = cx.new(|cx| Workspace::new(core, failure, window, cx));
            (account_id, workspace)
        });
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
        if let Some((_, workspace)) = &self.workspace {
            return workspace.clone().into_any_element();
        }
        let (title, body) = match self.core.read(cx).entry_gate(cx) {
            EntryGate::Restoring | EntryGate::SignedOut | EntryGate::Ready { .. } => (
                "Sign in to Artisan",
                self.sign_in.clone().into_any_element(),
            ),
            EntryGate::NeedsSetup { .. } => {
                ("Connect a Forge", self.forges.clone().into_any_element())
            }
        };
        self.shell(title, body, window, cx)
    }
}
