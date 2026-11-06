use super::{controller::SettingsController, ui, view::ViewAction};
use crate::{
    core::{AccountPhase, Core, EntryGate, LocalForge},
    ui::{
        button::{Button, ButtonVariant},
        text::{
            Mode,
            mount::{EditorController, EditorEvent},
        },
        theme::ArtisanTheme,
    },
};
use aqua_core::Insets;
use aqua_text::{FontFamily, TextStyle};
use aqua_ui::{
    JustifyContent,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};
use std::rc::Rc;

fn text(theme: &ArtisanTheme, value: impl Into<String>, muted: bool) -> Element {
    Text::new(value.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            ..TextStyle::default()
        })
        .gpui_source_color(if muted {
            theme.colors.text_muted.to_reference_text_color()
        } else {
            theme.colors.text.to_reference_text_color()
        })
        .wrap(true)
        .build()
}
fn button(
    theme: &ArtisanTheme,
    id: impl Into<String>,
    label: &str,
    variant: ButtonVariant,
    disabled: bool,
    action: ViewAction,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    Button::text(id.into(), *theme, label)
        .variant(variant)
        .disabled(disabled)
        .on_activate(move |_| emit(action.clone()))
        .build()
}
fn actions() -> UiBox {
    UiBox::new()
        .gap(8.0)
        .style(Style::new().justify(JustifyContent::END))
}
fn line(theme: &ArtisanTheme, title: impl Into<String>, detail: impl Into<String>) -> UiBox {
    UiBox::new()
        .layout("column")
        .gap(4.0)
        .style(Style::new().padding_insets(Insets {
            top: 8.0,
            bottom: 8.0,
            ..Insets::default()
        }))
        .child(text(theme, title, false))
        .child(text(theme, detail, true))
}
fn editor(
    state: Rc<std::cell::RefCell<crate::ui::text::Editor>>,
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
        _ => {}
    })
    .element_with_notice_keyed(key, Mode::SingleLine, theme, placeholder, false, false)
}

pub fn sign_in(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let action = |id, label, variant, disabled, action| {
        button(theme, id, label, variant, disabled, action, emit.clone())
    };
    let start = || {
        action(
            "sign-in-start-over",
            "Start over",
            ButtonVariant::Ghost,
            false,
            ViewAction::AccountDismiss,
        )
    };
    let mut body = UiBox::new()
        .layout("column")
        .gap(8.0)
        .style(Style::new().gpui_source_text_color(theme.colors.text.to_reference_text_color()));
    match core.account().phase() {
        AccountPhase::SignedOut | AccountPhase::Requesting | AccountPhase::Error { .. } => {
            let busy = matches!(core.account().phase(), AccountPhase::Requesting);
            body = body
                .child(text(theme, "Email", false))
                .child(editor(
                    controller.sign_in.email.clone(),
                    "sign-in-email",
                    "you@example.com",
                    ViewAction::AccountRequestLink,
                    theme,
                    emit.clone(),
                ))
                .child(
                    actions()
                        .child(action(
                            "sign-in-submit",
                            if busy {
                                "Sending…"
                            } else {
                                "Email me a sign-in link"
                            },
                            ButtonVariant::Default,
                            busy,
                            ViewAction::AccountRequestLink,
                        ))
                        .build(),
                );
            if let AccountPhase::Error { message } = core.account().phase() {
                body = body.child(
                    UiBox::new()
                        .layout("column")
                        .gap(8.0)
                        .child(
                            Text::new(message.clone())
                                .gpui_source_color(
                                    theme.colors.destructive_text.to_reference_text_color(),
                                )
                                .build(),
                        )
                        .child(actions().child(start()).build())
                        .build(),
                );
            }
        }
        AccountPhase::Restoring => {
            body = body.child(text(theme, "This device's sign-in was saved by an earlier Editor. Checking it with the account service…", true))
                .child(actions().child(action("sign-in-retry", "Retry", ButtonVariant::Default, false, ViewAction::AccountRefresh)).child(action("sign-in-use-another", "Use another account", ButtonVariant::Ghost, false, ViewAction::AccountSignOut)).build());
        }
        AccountPhase::Pending { email, .. } => {
            body = body
                .child(text(
                    theme,
                    format!("Check {email} for the sign-in link, then open it."),
                    false,
                ))
                .child(text(theme, "Waiting for the email approval…", true))
                .child(actions().child(start()).build());
        }
        AccountPhase::EmailApproved {
            email,
            totp_required,
            ..
        } => {
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
                    controller.sign_in.second_factor.clone(),
                    "sign-in-second-factor",
                    "6-digit code",
                    ViewAction::AccountFinish,
                    theme,
                    emit.clone(),
                ));
            }
            body = body.child(
                actions()
                    .child(start())
                    .child(action(
                        "sign-in-finish",
                        "Finish sign-in",
                        ButtonVariant::Default,
                        false,
                        ViewAction::AccountFinish,
                    ))
                    .build(),
            );
        }
        AccountPhase::Finalizing => {
            body = body.child(text(theme, "Finishing the sign-in…", true));
        }
        AccountPhase::Authenticated { .. } => {}
    }
    body.build()
}

pub fn forges(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let setup = matches!(core.entry_gate(), EntryGate::NeedsSetup { .. });
    let account = core.account();
    let email = account
        .profile()
        .map(|profile| profile.email.as_str())
        .unwrap_or_default();
    let machines = account.machines();
    let mut rows = UiBox::new()
        .layout("column")
        .gap(8.0)
        .style(Style::new().gpui_source_text_color(theme.colors.text.to_reference_text_color()));
    if machines.is_empty() {
        rows = rows.child(
            UiBox::new()
                .style(Style::new().padding_insets(Insets {
                    top: 8.0,
                    bottom: 8.0,
                    ..Insets::default()
                }))
                .child(text(theme, "No Forges on this account yet.", true))
                .build(),
        );
    }
    for machine in machines {
        let id = machine.id.clone();
        let live = core.machine_live(&id);
        let (kind, label, variant, action) = if setup {
            (
                "use",
                "Use this Forge",
                ButtonVariant::Default,
                ViewAction::ForgeUse(id.clone()),
            )
        } else if live {
            (
                "open",
                "Open",
                ButtonVariant::Outline,
                ViewAction::ForgeOpen(id.clone()),
            )
        } else {
            (
                "connect",
                "Connect",
                ButtonVariant::Default,
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
                    actions()
                        .child(button(
                            theme,
                            format!("forges-{kind}-{id}"),
                            label,
                            variant,
                            false,
                            action,
                            emit.clone(),
                        ))
                        .child(button(
                            theme,
                            format!("forges-remove-{id}"),
                            "Remove",
                            ButtonVariant::Ghost,
                            false,
                            ViewAction::ForgeRemove(id),
                            emit.clone(),
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
    let mut local_actions = actions().child(button(
        theme,
        "forges-check",
        "Check again",
        ButtonVariant::Ghost,
        false,
        ViewAction::ForgeRefresh,
        emit.clone(),
    ));
    if let Some((id, label, action)) = action {
        local_actions = local_actions.child(button(
            theme,
            id,
            label,
            ButtonVariant::Default,
            false,
            action,
            emit.clone(),
        ));
    }
    rows = rows.child(
        line(theme, "This machine", detail)
            .child(local_actions.build())
            .build(),
    );
    rows = rows.child(
        line(
            theme,
            "Another machine",
            "Claim the code its `ae account enroll` printed.",
        )
        .child(
            UiBox::new()
                .gap(8.0)
                .child(
                    UiBox::new()
                        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                        .child(editor(
                            controller.forge_code.clone(),
                            "forges-code",
                            "Code from another machine",
                            ViewAction::ForgeClaim,
                            theme,
                            emit.clone(),
                        ))
                        .build(),
                )
                .child(button(
                    theme,
                    "forges-claim",
                    "Claim",
                    ButtonVariant::Default,
                    false,
                    ViewAction::ForgeClaim,
                    emit.clone(),
                ))
                .build(),
        )
        .build(),
    );
    if let Some(error) = controller
        .forge_error
        .as_deref()
        .or_else(|| account.last_error())
    {
        rows = rows.child(
            Text::new(error)
                .gpui_source_color(theme.colors.destructive_text.to_reference_text_color())
                .build(),
        );
    }
    if setup {
        rows = rows.child(
            actions()
                .child(button(
                    theme,
                    "forges-use-another",
                    "Use another account",
                    ButtonVariant::Ghost,
                    false,
                    ViewAction::AccountSignOut,
                    emit,
                ))
                .build(),
        );
    }
    rows.build()
}

pub fn page(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let page = crate::core::SettingsPage::Account;
    let mut content = UiBox::new().layout("column").child(ui::header(
        theme,
        page.label(),
        super::page::description(page),
    ));
    let Some(profile) = core.account().profile() else {
        return content.child(ui::section(theme, "Sign in", Some("We email you a one-time link. Open it to approve this device, then finish here."), None, ui::card(theme, vec![sign_in(controller, core, theme, emit)]))).build();
    };
    let who = if profile.display_name.is_empty() {
        profile.email.clone()
    } else {
        format!("{} ({})", profile.display_name, profile.email)
    };
    let controls = UiBox::new()
        .gap(8.0)
        .child(button(
            theme,
            "account-refresh",
            "Refresh",
            ButtonVariant::Ghost,
            false,
            ViewAction::AccountRefresh,
            emit.clone(),
        ))
        .child(button(
            theme,
            "account-sign-out",
            "Sign out",
            ButtonVariant::Ghost,
            false,
            ViewAction::AccountSignOut,
            emit.clone(),
        ))
        .child(button(
            theme,
            "account-sign-out-everywhere",
            "Sign out everywhere",
            ButtonVariant::Outline,
            false,
            ViewAction::AccountSignOutEverywhere,
            emit.clone(),
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
    content
        .child(ui::section(
            theme,
            "Forges",
            Some("A connected Forge can run anything on its machine."),
            None,
            ui::card(theme, vec![forges(controller, core, theme, emit)]),
        ))
        .build()
}
