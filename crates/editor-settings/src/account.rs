//! The AST account page: sign-in with a magic link, sessions, and the
//! Forges on the account.

pub mod forges;
pub mod sign_in;

use crate::Settings;
use crate::page;
use crate::ui::{card, header, row, section_shell};
use editor_core::{Account, SettingsPage};
use editor_ui::button::ButtonVariant;
use editor_ui::theme::ArtisanTheme;
use gpui::{Context, Div, IntoElement, ParentElement as _, Styled as _, div, px};

impl Settings {
    fn account(
        &self,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut Account, &mut Context<Account>),
    ) {
        let account = self.core.read(cx).account().clone();
        account.update(cx, action);
    }
}

pub fn render(settings: &Settings, theme: &ArtisanTheme, cx: &Context<Settings>) -> Div {
    let profile = settings.core.read(cx).account().read(cx).profile().cloned();
    let page = div().flex().flex_col().child(header(
        theme,
        page::label(SettingsPage::Account),
        page::description(SettingsPage::Account),
    ));
    let Some(profile) = profile else {
        return page.child(section_shell(
            theme,
            "Sign in",
            Some("We email you a one-time link. Open it to approve this device, then finish here."),
            None,
            card(theme, vec![settings.sign_in.clone().into_any_element()]),
        ));
    };
    let who = if profile.display_name.is_empty() {
        profile.email
    } else {
        format!("{} ({})", profile.display_name, profile.email)
    };
    let controls = div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .child(settings.button(
            "account-refresh",
            "Refresh",
            ButtonVariant::Ghost,
            false,
            cx.listener(|this, _, _, cx| this.account(cx, Account::refresh)),
        ))
        .child(settings.button(
            "account-sign-out",
            "Sign out",
            ButtonVariant::Ghost,
            false,
            cx.listener(|this, _, _, cx| this.account(cx, Account::sign_out)),
        ))
        .child(settings.button(
            "account-sign-out-everywhere",
            "Sign out everywhere",
            ButtonVariant::Outline,
            false,
            cx.listener(|this, _, _, cx| this.account(cx, Account::sign_out_everywhere)),
        ));
    page.child(section_shell(
        theme,
        "Signed in",
        None,
        None,
        card(
            theme,
            vec![
                row(
                    theme,
                    who,
                    "Signed in on this device.",
                    Some(controls.into_any_element()),
                )
                .into_any_element(),
            ],
        ),
    ))
    .child(section_shell(
        theme,
        "Forges",
        Some("A connected Forge can run anything on its machine."),
        None,
        card(theme, vec![settings.forges.clone().into_any_element()]),
    ))
}
