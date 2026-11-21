use super::{
    controller::SettingsController,
    ui::{self, Ink},
    view::ViewAction,
};
use aqua_ui::element::{Box as UiBox, Element};
use editor_core::{AccountLink, AccountPhase, Core, LocalForge};
use editor_ui::{
    kit::{ButtonTone, Space, TextRole},
    text::{
        Mode,
        mount::{EditorController, EditorEvent},
    },
    theme::ArtisanTheme,
};
use std::rc::Rc;

fn text(theme: &ArtisanTheme, value: impl Into<String>, muted: bool) -> Element {
    let ink = if muted { Ink::Muted } else { Ink::Plain };
    ui::text(theme, TextRole::Label, ink, value)
}
fn button(
    theme: &ArtisanTheme,
    id: impl Into<String>,
    label: &str,
    tone: ButtonTone,
    disabled: bool,
    action: ViewAction,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    ui::button(theme, id, label, tone, disabled, move || {
        emit(action.clone());
    })
}
fn line(theme: &ArtisanTheme, title: impl Into<String>, detail: impl Into<String>) -> UiBox {
    ui::stack(Space::S4)
        .child(text(theme, title, false))
        .child(text(theme, detail, true))
}
fn editor(
    state: Rc<std::cell::RefCell<editor_ui::text::Editor>>,
    key: &'static str,
    placeholder: &'static str,
    submit: ViewAction,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    EditorController::from_shared(state, move |event| match event {
        EditorEvent::Submitted => emit(submit.clone()),
        EditorEvent::Changed => emit(ViewAction::AccountEdited),
        EditorEvent::PresentationChanged => emit(ViewAction::AccountPresentationChanged),
        EditorEvent::SelectionChanged { .. }
        | EditorEvent::List(_)
        | EditorEvent::PastedFiles { .. } => {}
    })
    .element_with_notice_keyed(key, Mode::SingleLine, theme, placeholder, false, false)
}

#[expect(
    clippy::too_many_lines,
    reason = "the sign-in card renders each account phase as one branch of a single layout"
)]
fn sign_in(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let action = |id, label, tone, disabled, action| {
        button(theme, id, label, tone, disabled, action, Rc::clone(emit))
    };
    let start = || {
        action(
            "sign-in-start-over",
            "Start over",
            ButtonTone::Neutral,
            false,
            ViewAction::AccountDismiss,
        )
    };
    let mut body = ui::stack(Space::S8);
    match core.account().phase() {
        AccountPhase::SignedOut
        | AccountPhase::Requesting
        | AccountPhase::Browser { .. }
        | AccountPhase::Error { .. } => {
            let busy = matches!(
                core.account().phase(),
                AccountPhase::Requesting | AccountPhase::Browser { .. }
            );
            body = body
                .child(text(theme, "Email", false))
                .child(editor(
                    Rc::clone(&controller.sign_in.email),
                    "sign-in-email",
                    "you@example.com",
                    ViewAction::AccountRequestLink,
                    theme,
                    Rc::clone(emit),
                ))
                .child(
                    ui::actions()
                        .child(action(
                            "sign-in-submit",
                            if busy {
                                "Sending…"
                            } else {
                                "Email me a sign-in link"
                            },
                            ButtonTone::Primary,
                            busy,
                            ViewAction::AccountRequestLink,
                        ))
                        .build(),
                );
            if let AccountPhase::Error { message } = core.account().phase() {
                body = body.child(
                    ui::problem(
                        theme,
                        "sign-in-error",
                        &editor_core::Problem::new(message.clone()),
                    )
                    .action(start())
                    .build(),
                );
            }
        }
        AccountPhase::Restoring => {
            body = body.child(text(theme, "This device's sign-in was saved by an earlier Editor. Checking it with the account service…", true))
                .child(ui::actions().child(action("sign-in-retry", "Try again", ButtonTone::Neutral, false, ViewAction::AccountRefresh)).child(action("sign-in-use-another", "Use another account", ButtonTone::Neutral, false, ViewAction::AccountSignOut)).build());
        }
        AccountPhase::Pending { email, .. } => {
            body = body
                .child(text(
                    theme,
                    format!("Check {email} for the sign-in link, then open it."),
                    false,
                ))
                .child(text(theme, "Waiting for the email approval…", true))
                .child(ui::actions().child(start()).build());
        }
        AccountPhase::EmailApproved { approval, .. } => {
            let (email, totp_required) = (&approval.email, &approval.totp_required);
            body = body
                .child(text(theme, format!("Approved for {email}"), false))
                .child(text(
                    theme,
                    if *totp_required {
                        "Enter the 6-digit code from your authenticator app, or a recovery code."
                    } else {
                        "Finish signing in on this device."
                    },
                    true,
                ));
            if *totp_required {
                body = body.child(editor(
                    Rc::clone(&controller.sign_in.second_factor),
                    "sign-in-second-factor",
                    "6-digit code",
                    ViewAction::AccountFinish,
                    theme,
                    Rc::clone(emit),
                ));
            }
            body = body.child(
                ui::actions()
                    .child(start())
                    .child(action(
                        "sign-in-finish",
                        "Finish sign-in",
                        ButtonTone::Primary,
                        false,
                        ViewAction::AccountFinish,
                    ))
                    .build(),
            );
        }
        AccountPhase::Finalizing { .. } => {
            body = body.child(text(theme, "Finishing the sign-in…", true));
        }
        AccountPhase::Authenticated { .. } => {}
    }
    body.build()
}

#[expect(
    clippy::too_many_lines,
    reason = "the Forges page lays out its enrollment states in one top-to-bottom column"
)]
pub fn forges(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let account = core.account();
    let email = account
        .profile()
        .map(|profile| profile.email.as_str())
        .unwrap_or_default();
    let machines = account.machines();
    let mut rows = ui::stack(Space::S16);
    if machines.is_empty() {
        rows = rows.child(text(theme, "No Forges on this account yet.", true));
    }
    for machine in machines {
        let id = machine.id.clone();
        let live = core.machine_live(&id);
        let (kind, label, tone, action) = if live {
            (
                "open",
                "Open",
                ButtonTone::Quiet,
                ViewAction::ForgeOpen(id.clone()),
            )
        } else {
            (
                "connect",
                "Connect",
                ButtonTone::Primary,
                ViewAction::ForgeConnect(id.clone()),
            )
        };
        let seen = machine.last_seen_at.as_deref().map_or_else(
            || "Never connected".to_owned(),
            |seen| format!("Last seen {seen}"),
        );
        rows = rows.child(
            line(theme, machine.label, seen)
                .child(
                    ui::actions()
                        .child(button(
                            theme,
                            format!("forges-{kind}-{id}"),
                            label,
                            tone,
                            false,
                            action,
                            Rc::clone(emit),
                        ))
                        .child(button(
                            theme,
                            format!("forges-remove-{id}"),
                            "Remove",
                            ButtonTone::Quiet,
                            false,
                            ViewAction::ForgeRemove(id),
                            Rc::clone(emit),
                        ))
                        .build(),
                )
                .build(),
        );
    }
    let (detail, action) = match account.local_forge() {
        LocalForge::Unknown => ("Its Forge hasn't answered yet.".to_owned(), None),
        LocalForge::Unenrolled => (
            "Its Forge isn't on the account.".to_owned(),
            Some(("forges-pair", "Pair this machine", ViewAction::ForgeEnroll)),
        ),
        LocalForge::Pending => (
            "Its Forge waits for its enrollment code to be claimed below.".to_owned(),
            None,
        ),
        LocalForge::Claimed { email: claimed } => (
            format!(
                "Its Forge is claimed for {}.",
                claimed.as_deref().unwrap_or(email)
            ),
            Some(("forges-confirm", "Confirm", ViewAction::ForgeConfirm)),
        ),
        LocalForge::Enrolled { .. } => (
            "Its Forge is on the account and connects directly.".to_owned(),
            None,
        ),
        LocalForge::Revoked { .. } => (
            "Its Forge was removed from the account.".to_owned(),
            Some(("forges-pair", "Pair it again", ViewAction::ForgeEnroll)),
        ),
        LocalForge::OtherAccount => ("Its Forge belongs to another account.".to_owned(), None),
    };
    let mut local_actions = ui::actions().child(button(
        theme,
        "forges-check",
        "Check again",
        ButtonTone::Neutral,
        false,
        ViewAction::ForgeRefresh,
        Rc::clone(emit),
    ));
    if let Some((id, label, action)) = action {
        local_actions = local_actions.child(button(
            theme,
            id,
            label,
            ButtonTone::Primary,
            false,
            action,
            Rc::clone(emit),
        ));
    }
    rows = rows.child(
        line(theme, "This machine", detail)
            .child(local_actions.build())
            .build(),
    );
    rows = rows.child(
        ui::stack(Space::S4)
            .child(text(theme, "Another machine", false))
            .child(ui::markdown(
                theme,
                TextRole::Label,
                Ink::Muted,
                "Run `ae account enroll` on it, then claim the code it printed.",
            ))
            .child(
                ui::actions()
                    .child(ui::grow(editor(
                        Rc::clone(&controller.forge_code),
                        "forges-code",
                        "Code from another machine",
                        ViewAction::ForgeClaim,
                        theme,
                        Rc::clone(emit),
                    )))
                    .child(button(
                        theme,
                        "forges-claim",
                        "Claim",
                        ButtonTone::Primary,
                        false,
                        ViewAction::ForgeClaim,
                        Rc::clone(emit),
                    ))
                    .build(),
            )
            .build(),
    );
    if let Some(error) = account.last_error() {
        rows = rows.child(ui::problem(theme, "account-error", error).build());
    }
    rows.build()
}

#[expect(
    clippy::too_many_lines,
    reason = "the account page lays out every sign-in phase in one top-to-bottom column"
)]
pub(crate) fn page(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let page = editor_core::SettingsPage::Account;
    let mut content = ui::column().child(ui::header(
        theme,
        page.label(),
        super::page::description(page),
    ));
    let Some(profile) = core.account().profile() else {
        return content.child(ui::section(theme, "Sign in", Some("We email you a one-time link. Open it to approve this device, then finish here."), None, ui::card(theme, vec![ui::block(sign_in(controller, core, theme, emit))]))).build();
    };
    let who = if profile.display_name.is_empty() {
        profile.email.clone()
    } else {
        format!("{} ({})", profile.display_name, profile.email)
    };
    let controls = ui::actions()
        .child(button(
            theme,
            "account-refresh",
            "Refresh",
            ButtonTone::Neutral,
            false,
            ViewAction::AccountRefresh,
            Rc::clone(emit),
        ))
        .child(button(
            theme,
            "account-sign-out",
            "Sign out",
            ButtonTone::Quiet,
            false,
            ViewAction::AccountSignOut,
            Rc::clone(emit),
        ))
        .child(button(
            theme,
            "account-sign-out-everywhere",
            "Sign out everywhere",
            ButtonTone::Neutral,
            false,
            ViewAction::AccountSignOutEverywhere,
            Rc::clone(emit),
        ))
        .build();
    content = content.child(ui::section(
        theme,
        "Signed in",
        None,
        None,
        ui::card(
            theme,
            vec![ui::row(
                theme,
                who,
                "Signed in on this device.",
                Some(controls),
            )],
        ),
    ));
    let policies = ui::actions()
        .child(button(
            theme,
            "account-terms",
            "Terms of Service",
            ButtonTone::Quiet,
            false,
            ViewAction::OpenUrl(AccountLink::Terms.url()),
            Rc::clone(emit),
        ))
        .child(button(
            theme,
            "account-privacy",
            "Privacy Policy",
            ButtonTone::Quiet,
            false,
            ViewAction::OpenUrl(AccountLink::Privacy.url()),
            Rc::clone(emit),
        ))
        .build();
    content
        .child(ui::section(
            theme,
            "Forges",
            Some("A connected Forge can run anything on its machine."),
            None,
            ui::card(
                theme,
                vec![ui::block(forges(controller, core, theme, emit))],
            ),
        ))
        .child(ui::section(
            theme,
            "Policies",
            None,
            None,
            ui::card(
                theme,
                vec![ui::row(
                    theme,
                    "Terms and privacy",
                    "The terms and privacy policy your Artisan account is under.",
                    Some(policies),
                )],
            ),
        ))
        .build()
}
