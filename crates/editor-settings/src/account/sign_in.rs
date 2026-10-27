use super::controller;
use editor_core::{AccountEvent, AccountPhase, Core};
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::text_editor::{TextEditor, TextEditorEvent};
use editor_ui::theme::{ActiveTheme as _, TextStep, TypographyTokens};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, div, px,
};

pub struct SignInView {
    core: Entity<Core>,
    email: Entity<TextEditor>,
    second_factor: Entity<TextEditor>,
    _edits: Vec<Subscription>,
    account_subscription: Option<Subscription>,
}

impl SignInView {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        let email = cx.new(|cx| TextEditor::new("", "you@example.com", cx).single_line());
        let second_factor = cx.new(|cx| TextEditor::new("", "6-digit code", cx).single_line());
        let request = cx.subscribe(&email, |this, _, event: &TextEditorEvent, cx| {
            if *event == TextEditorEvent::Submitted {
                this.request_link(cx);
            } else {
                cx.notify();
            }
        });
        let finish = cx.subscribe(&second_factor, |this, _, event: &TextEditorEvent, cx| {
            if *event == TextEditorEvent::Submitted {
                this.finish_sign_in(cx);
            } else {
                cx.notify();
            }
        });
        let mut this = Self {
            core,
            email,
            second_factor,
            _edits: vec![request, finish],
            account_subscription: None,
        };
        this.watch(cx);
        this
    }

    fn watch(&mut self, cx: &mut Context<Self>) {
        if self.account_subscription.is_some() {
            return;
        }
        let account = self.core.read(cx).account().clone();
        self.account_subscription = Some(cx.subscribe(&account, |_, _, _: &AccountEvent, cx| {
            cx.notify();
        }));
    }

    fn phase(&self, cx: &Context<Self>) -> AccountPhase {
        self.core
            .read(cx)
            .account()
            .read(cx)
            .phase()
            .clone()
    }

    pub fn request_link(&self, cx: &mut Context<Self>) {
        let email = self.email.read(cx).text().to_owned();
        let core = self.core.clone();
        core.update(cx, |core, cx| {
            core.account()
                .update(cx, |account, cx| account.request_link(email, cx));
        });
    }

    pub fn cancel(&self, cx: &mut Context<Self>) {
        let core = self.core.clone();
        core.update(cx, |core, cx| {
            core.account()
                .update(cx, editor_core::Account::dismiss_error);
        });
    }

    pub fn finish_sign_in(&self, cx: &mut Context<Self>) {
        let input = self.second_factor.read(cx).text().trim().to_owned();
        let (totp, recovery) = controller::split_second_factor(&input);
        let core = self.core.clone();
        core.update(cx, |core, cx| {
            core.account()
                .update(cx, |account, cx| account.finish_sign_in(totp, recovery, cx));
        });
        self.second_factor.update(cx, |editor, cx| {
            editor.set_text("", cx);
        });
    }

    fn button(
        id: &'static str,
        label: &str,
        variant: ButtonVariant,
        disabled: bool,
        cx: &Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let theme = *cx.theme();
        Button::text(id, theme, label)
            .variant(variant)
            .disabled(disabled)
            .debug_selector(id)
            .on_activate(cx.listener(move |this, _, _, cx| action(this, cx)))
            .into_any_element()
    }
}

impl Render for SignInView {
    #[expect(
        clippy::too_many_lines,
        reason = "one match over every sign-in state builds one element tree"
    )]
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let phase = self.phase(cx);
        let busy = matches!(phase, AccountPhase::Requesting);
        match phase {
            AccountPhase::SignedOut | AccountPhase::Requesting | AccountPhase::Error { .. } => {
                let message = match &phase {
                    AccountPhase::Error { message } => Some(message.clone()),
                    AccountPhase::SignedOut
                    | AccountPhase::Restoring
                    | AccountPhase::Requesting
                    | AccountPhase::Pending { .. }
                    | AccountPhase::EmailApproved { .. }
                    | AccountPhase::Finalizing
                    | AccountPhase::Authenticated { .. } => None,
                };
                let email_empty = self.email.read(cx).text().trim().is_empty();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(TypographyTokens::size(TextStep::Sm))
                            .text_color(theme.colors.text.to_paint())
                            .child("Email"),
                    )
                    .child(
                        div()
                            .id("sign-in-email")
                            .flex_1()
                            .min_w_0()
                            .child(self.email.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .child(Self::button(
                                "sign-in-submit",
                                if busy { "Sending…" } else { "Email me a sign-in link" },
                                ButtonVariant::Default,
                                busy || email_empty,
                                cx,
                                |this, cx| this.request_link(cx),
                            )),
                    )
                    .children(message.map(|message| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(TypographyTokens::size(TextStep::Sm))
                                    .text_color(theme.colors.destructive_text.to_paint())
                                    .child(message),
                            )
                            .child(
                                div().flex().flex_row().justify_end().child(Self::button(
                                    "sign-in-retry",
                                    "Try again",
                                    ButtonVariant::Ghost,
                                    false,
                                    cx,
                                    |this, cx| this.cancel(cx),
                                )),
                            )
                    }))
                    .into_any_element()
            }
            AccountPhase::Restoring => div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_color(theme.colors.text.to_paint())
                        .child("Restoring the session…"),
                )
                .child(
                    div()
                        .text_color(theme.colors.text_muted.to_paint())
                        .child("Checking the saved sign-in with the account service."),
                )
                .into_any_element(),
            AccountPhase::Pending { email, .. } => div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_color(theme.colors.text.to_paint())
                        .child(format!("Check {email} for the sign-in link, then open it.")),
                )
                .child(
                    div()
                        .text_color(theme.colors.text_muted.to_paint())
                        .child("Waiting for the email approval…"),
                )
                .child(
                    div().flex().flex_row().justify_end().child(Self::button(
                        "sign-in-cancel",
                        "Start over",
                        ButtonVariant::Ghost,
                        false,
                        cx,
                        |this, cx| this.cancel(cx),
                    )),
                )
                .into_any_element(),
            AccountPhase::EmailApproved {
                email, totp_required, ..
            } => div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_color(theme.colors.text.to_paint())
                        .child(format!("Approved for {email}")),
                )
                .children(totp_required.then(|| {
                    div()
                        .id("sign-in-second-factor")
                        .flex_1()
                        .min_w_0()
                        .child(self.second_factor.clone())
                        .into_any_element()
                }))
                .children((!totp_required).then(|| {
                    div()
                        .text_color(theme.colors.text_muted.to_paint())
                        .child("Finish signing in on this device.")
                        .into_any_element()
                }))
                .children(totp_required.then(|| {
                    div()
                        .text_color(theme.colors.text_muted.to_paint())
                        .child("Enter the 6-digit code from the authenticator app, or a recovery code instead.")
                        .into_any_element()
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(px(8.0))
                        .child(Self::button(
                            "sign-in-back",
                            "Back",
                            ButtonVariant::Ghost,
                            false,
                            cx,
                            |this, cx| this.cancel(cx),
                        ))
                        .child(Self::button(
                            "sign-in-finish",
                            "Finish sign-in",
                            ButtonVariant::Default,
                            false,
                            cx,
                            |this, cx| this.finish_sign_in(cx),
                        )),
                )
                .into_any_element(),
            AccountPhase::Finalizing => div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_color(theme.colors.text.to_paint())
                        .child("Finishing the sign-in…"),
                )
                .child(
                    div()
                        .text_color(theme.colors.text_muted.to_paint())
                        .child("Talking to the account service."),
                )
                .into_any_element(),
            AccountPhase::Authenticated { .. } => div().into_any_element(),
        }
    }
}
