//! The AST account page: sign-in with a magic link, sessions, machines,
//! and pairing this machine's Forge into the account.
//!
//! Plain language throughout: no key material, token IDs, or internal
//! identifiers are ever pasted or displayed. Enrollment codes are the one
//! exception — they are single-use, short-lived, and shown only to claim.

pub mod controller;
pub mod sign_in;

use crate::Settings;
use crate::page;
use crate::ui::{card, error_text, header, note, row, section_shell};
use editor_core::{AccountEvent, SettingsPage};
use editor_ui::button::ButtonVariant;
use editor_ui::text_editor::TextEditor;
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Subscription, div, px,
};

pub struct AccountForm {
    core: Entity<editor_core::Core>,
    sign_in: Entity<sign_in::SignInView>,
    claim_code: Entity<TextEditor>,
    _claim_submit: Subscription,
    account_subscription: Option<Subscription>,
}

impl AccountForm {
    pub(crate) fn new(core: Entity<editor_core::Core>, cx: &mut Context<Settings>) -> Self {
        let sign_in = cx.new(|cx| sign_in::SignInView::new(core.clone(), cx));
        let claim_code = cx.new(|cx| TextEditor::new("", "Code from this Forge", cx).single_line());
        let claim_submit = cx.subscribe(
            &claim_code,
            |this, _, event: &editor_ui::text_editor::TextEditorEvent, cx| {
                if *event == editor_ui::text_editor::TextEditorEvent::Submitted {
                    this.account.claim_code(cx);
                } else {
                    cx.notify();
                }
            },
        );
        Self {
            core,
            sign_in,
            claim_code,
            _claim_submit: claim_submit,
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

    pub(crate) fn sign_out(&self, revoke_all: bool, cx: &mut Context<Settings>) {
        controller::sign_out(&self.core, revoke_all, cx);
    }

    pub(crate) fn refresh(&self, cx: &mut Context<Settings>) {
        controller::refresh(&self.core, cx);
    }

    pub(crate) fn claim_code(&self, cx: &mut Context<Settings>) {
        let code = self.claim_code.read(cx).text().to_owned();
        controller::claim_code(&self.core, &code, cx);
        self.claim_code.update(cx, |editor, cx| {
            editor.set_text("", cx);
        });
    }

    pub(crate) fn enroll_this_forge(&self, cx: &mut Context<Settings>) {
        controller::enroll_this_forge(&self.core, cx);
    }

    pub(crate) fn forge_status(&self, cx: &mut Context<Settings>) {
        controller::forge_status(&self.core, cx);
    }

    pub(crate) fn forge_confirm(&self, cx: &mut Context<Settings>) {
        controller::forge_confirm(&self.core, cx);
    }

    pub(crate) fn connect_machine(&self, machine_id: &str, cx: &mut Context<Settings>) {
        controller::connect_machine(&self.core, machine_id, cx);
    }

    pub(crate) fn remove_machine(&self, machine_id: &str, cx: &mut Context<Settings>) {
        controller::remove_machine(&self.core, machine_id, cx);
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
        AccountPhase::SignedOut
        | AccountPhase::Restoring
        | AccountPhase::Requesting
        | AccountPhase::Error { .. }
        | AccountPhase::Pending { .. }
        | AccountPhase::EmailApproved { .. }
        | AccountPhase::Finalizing => {
            page = page.child(section_shell(
                theme,
                "Account sign-in",
                Some("Sign in with a link sent to your email."),
                None,
                card(
                    theme,
                    vec![
                        row(
                            theme,
                            "Sign in with email",
                            "We email you a one-time sign-in link. Open it to approve this device, then finish signing in here.",
                            None,
                        )
                        .into_any_element(),
                        settings.account.sign_in.clone().into_any_element(),
                    ],
                ),
            ));
        }
        AccountPhase::Authenticated { .. } => {
            page = page.child(render_profile(settings, theme, cx));
        }
    }
    if let Some(error) = account.read_with(cx, |account, _| account.last_error().map(str::to_owned))
    {
        page = page.child(error_text(theme, error));
    }
    page
}

#[expect(
    clippy::too_many_lines,
    reason = "signed-in account, machines and pairing read as one page"
)]
fn render_profile(settings: &Settings, theme: &ArtisanTheme, cx: &Context<Settings>) -> Div {
    let account = settings.core.read(cx).account().clone();
    let (email, display_name, machines) = account
        .read_with(cx, |account, _| {
            account.profile().map(|profile| {
                (
                    profile.email.clone(),
                    profile.display_name.clone(),
                    profile.machines.clone(),
                )
            })
        })
        .unwrap_or_default();
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
            cx.listener(move |this, _, _, cx| this.account.connect_machine(&id, cx)),
        );
        let id = machine.id.clone();
        let remove = settings.button(
            SharedString::from(format!("account-remove-{id}")),
            "Remove",
            ButtonVariant::Ghost,
            machine.revoked_at.is_some(),
            cx.listener(move |this, _, _, cx| this.account.remove_machine(&id, cx)),
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
    let (email, local) = settings
        .core
        .read(cx)
        .account()
        .read_with(cx, |account, _| {
            (
                account
                    .profile()
                    .map(|profile| profile.email.clone())
                    .unwrap_or_default(),
                account.local_forge(),
            )
        });
    let bound_here = matches!(local, editor_core::LocalForge::Enrolled { .. });
    let claimed_here = matches!(local, editor_core::LocalForge::Claimed { .. });
    let claimed_elsewhere = matches!(local, editor_core::LocalForge::OtherAccount);
    let mut blocks: Vec<AnyElement> = vec![
        row(
            theme,
            "Pair this machine",
            "Pairs this Forge into your account, then confirm the exact account shown here.",
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
            "Re-reads this Forge's enrollment state.",
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
    if bound_here {
        blocks.push(note(theme, format!("Paired to {email}.")).into_any_element());
    } else if claimed_here {
        blocks.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(settings.button(
                    SharedString::from("account-forge-confirm"),
                    SharedString::from(format!("Confirm this Forge into {email}")),
                    ButtonVariant::Outline,
                    false,
                    cx.listener(|this, _, _, cx| this.account.forge_confirm(cx)),
                ))
                .into_any_element(),
        );
    } else if claimed_elsewhere {
        blocks.push(
            error_text(
                theme,
                "This Forge is claimed by a different account. Switch accounts, or pair it anew.",
            )
            .into_any_element(),
        );
    }
    section_shell(theme, "Pair a Forge", None, None, card(theme, blocks)).into_any_element()
}
