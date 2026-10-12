//! Notifications (`notifications.svelte`): whether the Editor posts a
//! system notification, kept in this device's `settings.json`.

use crate::Settings;
use crate::chrome::{card, header, row, section_shell};
use crate::page;
use editor_core::SettingsPage;
use gpui::{Context, Div, IntoElement, ParentElement as _, Styled as _, div};
use serde_json::Value;

/// System notifications when a thread finishes, fails or asks; default on.
pub const NOTIFICATIONS_KEY: &str = "notifications";

pub(crate) fn render(
    settings: &Settings,
    theme: &editor_ui::theme::ArtisanTheme,
    cx: &mut Context<Settings>,
) -> Div {
    let enabled = settings
        .core
        .read(cx)
        .settings
        .get(NOTIFICATIONS_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let core = settings.core.clone();
    let switch = settings.switch(
        "settings-switch-notify",
        enabled,
        Some(Box::new(move |checked, _, cx| {
            core.update(cx, |core, cx| {
                core.set_setting(NOTIFICATIONS_KEY, Value::Bool(checked), cx);
            });
        })),
    );
    let blocks = vec![
        row(
            theme,
            "Notify me",
            "Posts a system notification when a thread finishes, fails, or needs an answer from you. Artisan appears in your operating system's notification settings, where you can silence or restyle it.",
            Some(switch),
        )
        .into_any_element(),
        row(
            theme,
            "Clears itself",
            "A notification you don't answer disappears after a few seconds. Nothing is lost by letting it go \u{2014} an approval or a question keeps waiting in its thread \u{2014} so a notification can never pile up into something you have to go and dismiss.",
            None,
        )
        .into_any_element(),
    ];
    div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::Notifications),
            page::description(SettingsPage::Notifications),
        ))
        .child(section_shell(
            theme,
            "system",
            "System",
            None,
            None,
            card(theme, blocks),
        ))
}
