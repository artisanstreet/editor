//! The AST account page: sign-in with a magic link, sessions, machines,
//! and pairing this machine's Forge into the account.
//!
//! Plain language throughout: no key material, token IDs, or internal
//! identifiers are ever pasted or displayed. Enrollment codes are the one
//! exception — they are single-use, short-lived, and shown only to claim.

use crate::Settings;
use crate::page;
use crate::ui::{card, error_text, header, note, row, section_shell};
use editor_core::{AccountEvent, AccountPhase, SettingsPage};
use editor_ui::button::ButtonVariant;
use editor_ui::text_editor::{TextEditor, TextEditorEvent};
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Subscription, div, px,
};

pub struct AccountForm {
    core: Entity<editor_core::Core>,
    email: Entity<TextEditor>,
    second_factor: Entity<TextEditor>,
    claim_code: Entity<TextEditor>,
    _subscriptions: Vec<Subscription>,
    account_subscription: Option<Subscription>,
}

impl AccountForm {
    pub(crate) fn new(core: Entity<editor_core::Core>, cx: &mut Context<Settings>) -> Self {
        let email = cx.new(|cx| TextEditor::new("", "you@example.com", cx).single_line());
        let second_factor = cx.new(|cx| TextEditor::new("", "6-digit code", cx).single_line());
        let claim_code = cx.new(|cx| TextEditor::new("", "Code from this Forge", cx).single_line());
        let email_submit =
            cx.subscribe(&email, |this, _, event: &TextEditorEvent, cx| match event {
                TextEditorEvent::Submitted => this.account.request_link(cx),
                TextEditorEvent::Changed => cx.notify(),
            });
        let factor_submit =
            cx.subscribe(
                &second_factor,
                |this, _, event: &TextEditorEvent, cx| match event {
                    TextEditorEvent::Submitted => this.account.finish_sign_in(cx),
                    TextEditorEvent::Changed => cx.notify(),
                },
            );
        let claim_submit =
            cx.subscribe(
                &claim_code,
                |this, _, event: &TextEditorEvent, cx| match event {
                    TextEditorEvent::Submitted => this.account.claim_code(cx),
                    TextEditorEvent::Changed => cx.notify(),
                },
            );
        Self {
            core,
            email,
            second_factor,
            claim_code,
            _subscriptions: vec![email_submit, factor_submit, claim_submit],
            account_subscription: None,
        }
    }

    pub(crate) fn watch_account(&mut self, cx: &mut Context<Settings>) {
        if self.account_subscription.is_some() {
            return;
        }
        let account = self.core.read(cx).account().clone();
        self.account_subscription =
            Some(cx.subscribe(&account, |this, _, _: &AccountEvent, cx| {
                let _ = this;
                cx.notify();
            }));
    }

    pub(crate) fn request_link(&self, cx: &mut Context<Settings>) {
        let email = self.email.read(cx).text().to_owned();
        let core_entity = self.core.clone();
        core_entity.update(cx, |core, cx| {
            core.account()
                .update(cx, |account, cx| account.request_link(email, cx));
        });
    }

    pub(crate) fn cancel(&self, cx: &mut Context<Settings>) {
        let core_entity = self.core.clone();
        core_entity.update(cx, |core, cx| {
            core.account().update(cx, |account, cx| {
                account.dismiss_error(cx);
            });
        });
    }

    pub(crate) fn finish_sign_in(&self, cx: &mut Context<Settings>) {
        let code = self.second_factor.read(cx).text().trim().to_owned();
        let (totp, recovery) = if code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) {
            (Some(code), None)
        } else if code.is_empty() {
            (None, None)
        } else {
            (None, Some(code))
        };
        let core_entity = self.core.clone();
        core_entity.update(cx, |core, cx| {
            core.account()
                .update(cx, |account, cx| account.finish_sign_in(totp, recovery, cx));
        });
        self.second_factor.update(cx, |editor, cx| {
            editor.set_text("", cx);
        });
    }

    pub(crate) fn sign_out(&self, revoke_all: bool, cx: &mut Context<Settings>) {
        let core_entity = self.core.clone();
        core_entity.update(cx, |core, cx| {
            core.account().update(cx, |account, cx| {
                if revoke_all {
                    account.sign_out_everywhere(cx);
                } else {
                    account.sign_out(cx);
                }
            });
        });
    }

    pub(crate) fn refresh(&self, cx: &mut Context<Settings>) {
        let core_entity = self.core.clone();
        core_entity.update(cx, |core, cx| {
            core.account().update(cx, editor_core::Account::refresh);
        });
    }

    pub(crate) fn claim_code(&self, cx: &mut Context<Settings>) {
        let code = self.claim_code.read(cx).text().to_owned();
        let core_entity = self.core.clone();
        core_entity.update(cx, |core, cx| {
            core.account()
                .update(cx, |account, cx| account.claim_code(code, cx));
        });
        self.claim_code.update(cx, |editor, cx| {
            editor.set_text("", cx);
        });
    }

    pub(crate) fn enroll_this_forge(&self, cx: &mut Context<Settings>) {
        let core_entity = self.core.clone();
        core_entity.update(cx, |core, cx| {
            core.account()
                .update(cx, editor_core::Account::enroll_this_forge);
        });
    }

    pub(crate) fn forge_status(&self, cx: &mut Context<Settings>) {
        let core_entity = self.core.clone();
        core_entity.update(cx, |core, cx| {
            core.account()
                .update(cx, editor_core::Account::forge_status);
        });
    }

    pub(crate) fn forge_confirm(&self, cx: &mut Context<Settings>) {
        let core_entity = self.core.clone();
        core_entity.update(cx, |core, cx| {
            core.account()
                .update(cx, editor_core::Account::forge_confirm);
        });
    }

    pub(crate) fn connect_machine(&self, machine_id: String, cx: &mut Context<Settings>) {
        let core_entity = self.core.clone();
        let account = core_entity.read_with(cx, |core, _| core.account().clone());
        account.update(cx, |account, cx| {
            account.connect_machine(machine_id, &core_entity, cx);
        });
    }

    pub(crate) fn remove_machine(&self, machine_id: String, cx: &mut Context<Settings>) {
        let core_entity = self.core.clone();
        core_entity.update(cx, |core, cx| {
            core.account()
                .update(cx, |account, cx| account.remove_machine(machine_id, cx));
        });
    }
}

/// Renders the account page for the current sign-in state.
pub fn render(settings: &Settings, theme: &ArtisanTheme, cx: &Context<Settings>) -> Div {
    use editor_core::AccountPhase;
    let account = settings.core.read(cx).account().clone();
    let phase = account.read_with(cx, |account, _| account.phase().clone());
    let mut page = div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::Account),
            page::description(SettingsPage::Account),
        ))
        .debug_selector(|| "settings-account".to_owned());
    match phase {
        AccountPhase::SignedOut | AccountPhase::Requesting | AccountPhase::Error { .. } => {
            page = page.child(render_sign_in(settings, theme, &phase, cx));
        }
        AccountPhase::Pending { ref email, .. } => {
            page = page.child(render_waiting(
                settings,
                theme,
                &format!("Check {email} for the sign-in link, then open it."),
                "Waiting for the email approval…",
                cx,
            ));
        }
        AccountPhase::EmailApproved {
            ref email,
            totp_required,
            ..
        } => {
            page = page.child(render_finish(settings, theme, email, totp_required, cx));
        }
        AccountPhase::Finalizing => {
            page = page.child(render_waiting(
                settings,
                theme,
                "Finishing the sign-in…",
                "Talking to the account service.",
                cx,
            ));
        }
        AccountPhase::Authenticated { .. } => {
            page = page.child(render_profile(settings, theme, cx));
        }
    }
    if let Some(notice) = account.read_with(cx, |account, _| account.notice().map(str::to_owned)) {
        page = page.child(note(theme, notice));
    }
    if let Some(error) =
        account.read_with(cx, |account, _| account.relay_error().map(str::to_owned))
    {
        page = page.child(error_text(theme, error));
    }
    page
}

fn render_sign_in(
    settings: &Settings,
    theme: &ArtisanTheme,
    phase: &AccountPhase,
    cx: &Context<Settings>,
) -> AnyElement {
    let busy = matches!(phase, AccountPhase::Requesting);
    let form = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(settings.account.email.clone()),
        )
        .child(div().flex().flex_row().justify_end().child(settings.button(
            "account-sign-in",
            if busy {
                "Sending…"
            } else {
                "Email me a sign-in link"
            },
            ButtonVariant::Default,
            busy || settings.account.email.read(cx).text().trim().is_empty(),
            cx.listener(|this, _, _, cx| this.account.request_link(cx)),
        )));
    let mut children = vec![
        row(
            theme,
            "Sign in with email",
            "We email you a one-time sign-in link. Open it to approve this device, then finish signing in here.",
            None,
        )
        .into_any_element(),
        form.into_any_element(),
    ];
    if let AccountPhase::Error { message } = phase {
        children.push(error_text(theme, message.clone()).into_any_element());
        children.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(settings.button(
                    "account-retry",
                    "Try again",
                    ButtonVariant::Ghost,
                    false,
                    cx.listener(|this, _, _, cx| this.account.cancel(cx)),
                ))
                .into_any_element(),
        );
    }
    section_shell(
        theme,
        "Account sign-in",
        Some("Sign in with a link sent to your email."),
        None,
        card(theme, children),
    )
    .into_any_element()
}

fn render_waiting(
    settings: &Settings,
    theme: &ArtisanTheme,
    title: &str,
    detail: &str,
    cx: &Context<Settings>,
) -> AnyElement {
    section_shell(
        theme,
        "Waiting for the email",
        None,
        None,
        card(
            theme,
            vec![
                row(theme, title, detail, None).into_any_element(),
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .child(settings.button(
                        "account-cancel",
                        "Start over",
                        ButtonVariant::Ghost,
                        false,
                        cx.listener(|this, _, _, cx| this.account.cancel(cx)),
                    ))
                    .into_any_element(),
            ],
        ),
    )
    .into_any_element()
}

fn render_finish(
    settings: &Settings,
    theme: &ArtisanTheme,
    email: &str,
    totp_required: bool,
    cx: &Context<Settings>,
) -> AnyElement {
    let mut rows: Vec<AnyElement> = vec![
        row(
            theme,
            format!("Approved for {email}"),
            if totp_required {
                "Enter the 6-digit code from the authenticator app, or a recovery code instead."
            } else {
                "Finish signing in on this device."
            },
            None,
        )
        .into_any_element(),
    ];
    if totp_required {
        rows.push(
            div()
                .flex_1()
                .min_w_0()
                .child(settings.account.second_factor.clone())
                .into_any_element(),
        );
    }
    rows.push(
        div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(px(8.0))
            .child(settings.button(
                "account-back",
                "Back",
                ButtonVariant::Ghost,
                false,
                cx.listener(|this, _, _, cx| this.account.cancel(cx)),
            ))
            .child(settings.button(
                "account-finish",
                "Finish sign-in",
                ButtonVariant::Default,
                false,
                cx.listener(|this, _, _, cx| this.account.finish_sign_in(cx)),
            ))
            .into_any_element(),
    );
    section_shell(
        theme,
        "Finish signing in",
        Some("This step binds the sign-in to this device."),
        None,
        card(theme, rows),
    )
    .into_any_element()
}

#[expect(
        clippy::too_many_lines,
        reason = "signed-in account, machines and pairing read as one page"
    )]
fn render_profile(settings: &Settings, theme: &ArtisanTheme, cx: &Context<Settings>) -> Div {
    let account = settings.core.read(cx).account().clone();
    let (email, display_name, machines, notice_pending) = account
        .read_with(cx, |account, _| {
            account.profile().map(|profile| {
                (
                    profile.email.clone(),
                    profile.display_name.clone(),
                    profile.machines.clone(),
                    account.notice().map(str::to_owned),
                )
            })
        })
        .unwrap_or_default();
    let _ = notice_pending;
    let mut page = div().flex().flex_col().gap(px(10.0));
    let who = if display_name.is_empty() {
        email
    } else {
        format!("{display_name} ({email})")
    };
    page = page.child(section_shell(
        theme,
        "Signed in",
        None,
        None,
        card(theme, vec![
            row(
                theme,
                who,
                "Signed in on this device. Your Forges below belong to this account, not to any single Editor window.",
                Some(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(8.0))
                        .child(settings.button(
                            "account-refresh",
                            "Refresh",
                            ButtonVariant::Ghost,
                            false,
                            cx.listener(|this, _, _, cx| this.account.refresh(cx)),
                        ))
                        .child(settings.button(
                            "account-sign-out",
                            "Sign out",
                            ButtonVariant::Ghost,
                            false,
                            cx.listener(|this, _, _, cx| this.account.sign_out(false, cx)),
                        ))
                        .child(settings.button(
                            "account-sign-out-everywhere",
                            "Sign out everywhere",
                            ButtonVariant::Outline,
                            false,
                            cx.listener(|this, _, _, cx| this.account.sign_out(true, cx)),
                        ))
                        .into_any_element(),
                ),
            )
            .into_any_element(),
        ]),
    ));
    let mut machine_rows: Vec<AnyElement> = Vec::new();
    for machine in &machines {
        let short = machine.id.chars().take(13).collect::<String>();
        let state = if machine.revoked_at.is_some() {
            "Revoked".to_owned()
        } else if let Some(seen) = machine.last_seen_at.as_deref() {
            format!("Last seen {seen}")
        } else {
            "Enrolled · never connected".to_owned()
        };
        let id = machine.id.clone();
        let connect = settings.button(
            SharedString::from(format!("account-connect-{id}")),
            "Connect",
            ButtonVariant::Default,
            machine.revoked_at.is_some(),
            cx.listener(move |this, _, _, cx| this.account.connect_machine(id.clone(), cx)),
        );
        let id = machine.id.clone();
        let remove = settings.button(
            SharedString::from(format!("account-remove-{id}")),
            "Remove",
            ButtonVariant::Ghost,
            machine.revoked_at.is_some(),
            cx.listener(move |this, _, _, cx| this.account.remove_machine(id.clone(), cx)),
        );
        machine_rows.push(
            row(
                theme,
                machine.label.clone(),
                format!("{short} · {state}"),
                Some(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(connect)
                        .child(remove)
                        .into_any_element(),
                ),
            )
            .into_any_element(),
        );
    }
    if machine_rows.is_empty() {
        machine_rows.push(
            row(
                theme,
                "No Forges yet",
                "Pair this machine below, or claim a pairing code from another Forge.",
                None,
            )
            .into_any_element(),
        );
    }
    page = page.child(section_shell(
        theme,
        "Forges on this account",
        Some("Full access once connected: a paired Forge can run everything on its machine."),
        None,
        card(theme, machine_rows),
    ));
    page = page.child(render_pair_this_machine(settings, theme, cx));
    page
}

#[expect(
        clippy::too_many_lines,
        reason = "pairing status, code entry and confirmation read as one flow"
    )]
fn render_pair_this_machine(
    settings: &Settings,
    theme: &ArtisanTheme,
    cx: &Context<Settings>,
) -> AnyElement {
    let output: Option<String> = settings
        .core
        .read(cx)
        .account()
        .read_with(cx, |account, _| {
            account.forge_status_output().map(str::to_owned)
        });
    let claimed_here = output
        .as_deref()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("Claimed by account: ")
                    .map(str::trim)
                    .filter(|account| !account.is_empty())
                    .map(str::to_owned)
            })
        })
        .or_else(|| {
            output.as_deref().and_then(|text| {
                text.lines().find_map(|line| {
                    line.strip_prefix("Enrolled: ").map(|rest| {
                        rest.split_whitespace()
                            .next()
                            .unwrap_or_default()
                            .to_owned()
                    })
                })
            })
        });
    let mut blocks: Vec<AnyElement> = vec![
        row(
            theme,
            "Pair this machine",
            "Runs `ae account enroll` on this Forge, then claims the printed code into this account. You still confirm the exact account on the Forge itself.",
            Some(
                settings
                    .button(
                        "account-pair-this",
                        "Pair this machine",
                        ButtonVariant::Default,
                        false,
                        cx.listener(|this, _, _, cx| this.account.enroll_this_forge(cx)),
                    )
                    .into_any_element(),
            ),
        )
        .into_any_element(),
        row(
            theme,
            "Claim a pairing code",
            "Paste the code another Forge printed, to pull it into this account.",
            None,
        )
        .into_any_element(),
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .pb(px(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(settings.account.claim_code.clone()),
            )
            .child(settings.button(
                "account-claim",
                "Claim",
                ButtonVariant::Default,
                false,
                cx.listener(|this, _, _, cx| this.account.claim_code(cx)),
            ))
            .into_any_element(),
        row(
            theme,
            "Check this Forge",
            "Shows what this Forge thinks: pending, claimed by which account, or already enrolled.",
            Some(
                settings
                    .button(
                        "account-forge-status",
                        "Check",
                        ButtonVariant::Ghost,
                        false,
                        cx.listener(|this, _, _, cx| this.account.forge_status(cx)),
                    )
                    .into_any_element(),
            ),
        )
        .into_any_element(),
    ];
    if let Some(output) = output {
        blocks.push(
            div()
                .text_size(editor_ui::theme::TypographyTokens::size(
                    editor_ui::theme::TextStep::Terminal,
                ))
                .text_color(theme.colors.text_muted.to_paint())
                .child(SharedString::from(output))
                .into_any_element(),
        );
    }
    if let Some(account) = claimed_here {
        blocks.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(settings.button(
                    SharedString::from("account-forge-confirm"),
                    SharedString::from(format!("Confirm enrollment into {account}")),
                    ButtonVariant::Outline,
                    false,
                    cx.listener(|this, _, _, cx| this.account.forge_confirm(cx)),
                ))
                .into_any_element(),
        );
    }
    section_shell(theme, "Pair a Forge", None, None, card(theme, blocks)).into_any_element()
}
