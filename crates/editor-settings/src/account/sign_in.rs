use editor_core::{Account, AccountEvent, AccountPhase, Core};
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::text_editor::{TextEditor, TextEditorEvent};
use editor_ui::theme::{ActiveTheme as _, ArtisanTheme, TextStep, TypographyTokens};
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px,
};

pub struct SignInView {
    core: Entity<Core>,
    email: Entity<TextEditor>,
    second_factor: Entity<TextEditor>,
    _subscriptions: [Subscription; 3],
}

impl SignInView {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        let email = cx.new(|cx| TextEditor::new("", "you@example.com", cx).single_line());
        let second_factor = cx.new(|cx| TextEditor::new("", "6-digit code", cx).single_line());
        let account = core.read(cx).account().clone();
        let subscriptions = [
            cx.subscribe(&email, |this, _, event: &TextEditorEvent, cx| {
                if *event == TextEditorEvent::Submitted {
                    this.request_link(cx);
                }
            }),
            cx.subscribe(&second_factor, |this, _, event: &TextEditorEvent, cx| {
                if *event == TextEditorEvent::Submitted {
                    this.finish_sign_in(cx);
                }
            }),
            cx.subscribe(&account, |_, _, _: &AccountEvent, cx| cx.notify()),
        ];
        Self {
            core,
            email,
            second_factor,
            _subscriptions: subscriptions,
        }
    }

    fn account(
        &self,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut Account, &mut Context<Account>),
    ) {
        let account = self.core.read(cx).account().clone();
        account.update(cx, action);
    }

    fn request_link(&mut self, cx: &mut Context<Self>) {
        let email = self.email.read(cx).text().to_owned();
        self.account(cx, |account, cx| account.request_link(&email, cx));
    }

    fn finish_sign_in(&mut self, cx: &mut Context<Self>) {
        let (totp, recovery) = second_factor(self.second_factor.read(cx).text());
        self.account(cx, |account, cx| account.finish_sign_in(totp, recovery, cx));
        self.second_factor
            .update(cx, |editor, cx| editor.set_text("", cx));
    }

    fn button(
        id: &'static str,
        label: &'static str,
        variant: ButtonVariant,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Button {
        Button::text(id, *theme, label)
            .variant(variant)
            .debug_selector(id)
            .on_activate(cx.listener(move |this, _, _, cx| action(this, cx)))
    }

    fn start_over(theme: &ArtisanTheme, cx: &Context<Self>) -> Button {
        Self::button(
            "sign-in-start-over",
            "Start over",
            ButtonVariant::Ghost,
            theme,
            cx,
            |this, cx| this.account(cx, Account::dismiss_error),
        )
    }

    fn email_form(
        &self,
        error: Option<String>,
        busy: bool,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
    ) -> Div {
        column()
            .child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Sm))
                    .child("Email"),
            )
            .child(
                div()
                    .id("sign-in-email")
                    .min_w_0()
                    .child(self.email.clone()),
            )
            .child(
                actions().child(
                    Self::button(
                        "sign-in-submit",
                        if busy {
                            "Sending…"
                        } else {
                            "Email me a sign-in link"
                        },
                        ButtonVariant::Default,
                        theme,
                        cx,
                        Self::request_link,
                    )
                    .disabled(busy),
                ),
            )
            .children(error.map(|error| {
                column()
                    .child(
                        div()
                            .text_color(theme.colors.destructive_text.to_paint())
                            .child(error),
                    )
                    .child(actions().child(Self::start_over(theme, cx)))
            }))
    }

    fn approved(
        &self,
        email: &str,
        totp_required: bool,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
    ) -> Div {
        let prompt = if totp_required {
            "Enter the 6-digit code from your authenticator app, or a recovery code."
        } else {
            "Finish signing in on this device."
        };
        column()
            .child(format!("Approved for {email}"))
            .child(muted(theme, prompt))
            .children(totp_required.then(|| {
                div()
                    .id("sign-in-second-factor")
                    .min_w_0()
                    .child(self.second_factor.clone())
            }))
            .child(
                actions()
                    .child(Self::start_over(theme, cx))
                    .child(Self::button(
                        "sign-in-finish",
                        "Finish sign-in",
                        ButtonVariant::Default,
                        theme,
                        cx,
                        Self::finish_sign_in,
                    )),
            )
    }

    fn restoring(theme: &ArtisanTheme, cx: &Context<Self>) -> Div {
        column()
            .child(muted(
                theme,
                "This device's sign-in was saved by an earlier Editor. Checking it with the account service…",
            ))
            .child(
                actions()
                    .child(Self::button(
                        "sign-in-retry",
                        "Retry",
                        ButtonVariant::Default,
                        theme,
                        cx,
                        |this, cx| this.account(cx, Account::refresh),
                    ))
                    .child(Self::button(
                        "sign-in-use-another",
                        "Use another account",
                        ButtonVariant::Ghost,
                        theme,
                        cx,
                        |this, cx| this.account(cx, Account::sign_out),
                    )),
            )
    }
}

impl Render for SignInView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &{ *cx.theme() };
        let phase = self.core.read(cx).account().read(cx).phase().clone();
        let body: AnyElement = match phase {
            AccountPhase::SignedOut => self.email_form(None, false, theme, cx).into_any_element(),
            AccountPhase::Requesting => self.email_form(None, true, theme, cx).into_any_element(),
            AccountPhase::Error { message } => self
                .email_form(Some(message), false, theme, cx)
                .into_any_element(),
            AccountPhase::Restoring => Self::restoring(theme, cx).into_any_element(),
            AccountPhase::Pending { email, .. } => column()
                .child(format!("Check {email} for the sign-in link, then open it."))
                .child(muted(theme, "Waiting for the email approval…"))
                .child(actions().child(Self::start_over(theme, cx)))
                .into_any_element(),
            AccountPhase::EmailApproved {
                email,
                totp_required,
                ..
            } => self
                .approved(&email, totp_required, theme, cx)
                .into_any_element(),
            AccountPhase::Finalizing => muted(theme, "Finishing the sign-in…").into_any_element(),
            AccountPhase::Authenticated { .. } => div().into_any_element(),
        };
        div().text_color(theme.colors.text.to_paint()).child(body)
    }
}

/// A six-digit answer is a TOTP code; anything else is a recovery code.
fn second_factor(input: &str) -> (Option<String>, Option<String>) {
    let code = input.trim();
    match (
        code.is_empty(),
        code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()),
    ) {
        (true, _) => (None, None),
        (false, true) => (Some(code.to_owned()), None),
        (false, false) => (None, Some(code.to_owned())),
    }
}

fn column() -> Div {
    div().flex().flex_col().gap(px(8.0))
}

fn actions() -> Div {
    div().flex().flex_row().justify_end().gap(px(8.0))
}

fn muted(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_color(theme.colors.text_muted.to_paint())
        .child(text.into())
}
