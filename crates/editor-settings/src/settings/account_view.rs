use super::{
    controller::SettingsController,
    ui::{self, Ink},
    view::ViewAction,
};
use aqua_ui::element::{Box as UiBox, Element};
use editor_assets::AssetId;
use editor_core::{AccountLink, AccountPhase, Core, LocalForge};
use editor_ui::{
    kit::{ButtonTone, Finish, IconButton, IconButtonSize, Space, Spaced, TextRole},
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

pub fn forges(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    computers(controller, core, theme, emit, false)
}

#[expect(
    clippy::too_many_lines,
    reason = "the Forges page lays out its enrollment states in one top-to-bottom column"
)]
pub(crate) fn computers(
    controller: &SettingsController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
    settings: bool,
) -> Element {
    let account = core.account();
    let email = account
        .profile()
        .map(|profile| profile.email.as_str())
        .unwrap_or_default();
    let machines = account.machines();
    let mut rows = ui::stack(Space::S12);
    if machines.is_empty() {
        rows = rows.child(text(theme, "No computers on this account yet.", true));
    }
    for machine in machines {
        let id = machine.id.clone();
        let live = core.machine_live(&id);
        let connecting = core.machine_connecting(&id);
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
                if connecting {
                    "Connecting…"
                } else if settings {
                    "Reconnect"
                } else {
                    "Connect"
                },
                ButtonTone::Primary,
                ViewAction::ForgeConnect(id.clone()),
            )
        };
        let seen = if settings {
            if live && core.machine_uses_relay(&id) {
                "Connected via ADR"
            } else if live {
                "Connected directly"
            } else if connecting {
                "Connecting…"
            } else {
                "Disconnected"
            }
            .to_owned()
        } else {
            machine.last_seen_at.as_deref().map_or_else(
                || "Never connected".to_owned(),
                |seen| format!("Last seen {seen}"),
            )
        };
        rows = rows.child(
            line(theme, machine.label, seen)
                .child(
                    ui::actions()
                        .child(button(
                            theme,
                            format!("forges-{kind}-{id}"),
                            label,
                            tone,
                            connecting,
                            action,
                            Rc::clone(emit),
                        ))
                        .child(button(
                            theme,
                            format!("forges-remove-{id}"),
                            if settings {
                                "Delete computer"
                            } else {
                                "Remove"
                            },
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
        line(
            theme,
            if settings {
                "Enrollment"
            } else {
                "This machine"
            },
            detail,
        )
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
    account: Option<&editor_ui::chrome::account::Account>,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let Some(profile) = core.account().profile() else {
        return ui::section(
            theme,
            "Sign in",
            None,
            None,
            ui::card(
                theme,
                vec![ui::block(sign_in(controller, core, theme, emit))],
            ),
        );
    };
    let username = profile
        .username
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let handle = username.map(|name| format!("@{}", name.trim_start_matches('@')));
    let display_name = match profile.display_name.trim() {
        "" => username.unwrap_or("Artisan account"),
        name => name,
    };
    let dashboard = Rc::clone(emit);
    let sign_out = Rc::clone(emit);
    let controls = ui::stack(Space::S2)
        .child(
            IconButton::new(
                "account-dashboard",
                *theme,
                AssetId::TABLER_SETTINGS,
                "Open Artisan dashboard",
                IconButtonSize::Lg,
                Finish::Glass,
            )
            .muted()
            .on_activate(move |_| {
                dashboard(ViewAction::OpenUrl(AccountLink::Dashboard.url()));
            })
            .build(),
        )
        .child(
            IconButton::new(
                "account-sign-out",
                *theme,
                AssetId::TABLER_LOGOUT,
                "Sign out",
                IconButtonSize::Lg,
                Finish::Glass,
            )
            .destructive()
            .on_activate(move |_| sign_out(ViewAction::AccountSignOut))
            .build(),
        )
        .build();
    let identity = UiBox::new()
        .spaced(Space::S16)
        .style(aqua_ui::style::Style::new().align(aqua_ui::AlignItems::CENTER))
        .children(
            account.map(|account| editor_ui::chrome::account::portrait(account, 52.0, 14.0, 28.0)),
        )
        .child(
            ui::stack(Space::S4)
                .child(ui::line(theme, TextRole::Heading, Ink::Plain, display_name))
                .children(handle.map(|handle| ui::line(theme, TextRole::Label, Ink::Muted, handle)))
                .build(),
        )
        .build();
    let body = UiBox::new()
        .spaced(Space::S16)
        .padded(Space::S12)
        .style(
            aqua_ui::style::Style::new()
                .align(aqua_ui::AlignItems::CENTER)
                .justify(aqua_ui::JustifyContent::SPACE_BETWEEN),
        )
        .child(identity)
        .child(controls)
        .build();
    let content = ui::column().child(ui::section(
        theme,
        "Profile",
        None,
        None,
        ui::card(theme, vec![body]),
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
    let refresh = Rc::clone(emit);
    let security = ui::actions()
        .child(
            IconButton::new(
                "account-refresh",
                *theme,
                AssetId::TABLER_REFRESH,
                "Refresh profile",
                IconButtonSize::Md,
                Finish::Glass,
            )
            .muted()
            .on_activate(move |_| refresh(ViewAction::AccountRefresh))
            .build(),
        )
        .child(button(
            theme,
            "account-sign-out-everywhere",
            "Sign out everywhere",
            ButtonTone::Quiet,
            false,
            ViewAction::AccountSignOutEverywhere,
            Rc::clone(emit),
        ))
        .build();
    content
        .child(ui::section(
            theme,
            "Policies",
            None,
            None,
            ui::card(
                theme,
                vec![
                    ui::row(theme, "Legal", "", Some(policies)),
                    ui::row(theme, "Account access", "", Some(security)),
                ],
            ),
        ))
        .build()
}
