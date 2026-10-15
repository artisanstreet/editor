//! Privacy (`privacy.svelte`): the two telemetry choices and what is never
//! collected. The Editor sends no telemetry, so both choices stay
//! undecided and disabled, as the previous Editor painted them.

use crate::Settings;
use crate::page;
use editor_core::SettingsPage;
use editor_ui::page::{card, header, row, section_shell};
use editor_ui::theme::ArtisanTheme;
use gpui::{AnyElement, Div, IntoElement, ParentElement as _, Styled as _, div, px};

pub(crate) fn render(settings: &Settings, theme: &ArtisanTheme) -> Div {
    let telemetry = card(
        theme,
        vec![
            telemetry_row(
                settings,
                theme,
                "settings-switch-usage-analytics",
                "Usage analytics",
                "Sends allowlisted product events to PostHog using a random installation ID. Artisan never sends prompts, responses, source code, diffs, terminal activity, names, or paths.",
            ),
            telemetry_row(
                settings,
                theme,
                "settings-switch-crash-reports",
                "Crash reports",
                "Sends sanitized exceptions, crash reasons, release information, and coarse performance diagnostics to Sentry. Attachments, replay, console logs, request data, environment variables, and process arguments stay off.",
            ),
        ],
    );
    let never = card(
        theme,
        vec![
            row(
                theme,
                "Your work stays local",
                "Prompts, model responses, source code, file contents, diffs, terminal commands and output, repository and project names, paths, credentials, headers, request bodies, process arguments, and environment variables are prohibited from both systems.",
                None,
            )
            .into_any_element(),
        ],
    );
    div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::Privacy),
            page::description(SettingsPage::Privacy),
        ))
        .child(
            section_shell(
                theme,
                "Observability",
                Some("Telemetry choices are not yet configurable in the native app."),
                None,
                telemetry,
            )
            .debug_selector("settings-section-telemetry"),
        )
        .child(
            section_shell(theme, "Never collected", None, None, never)
                .debug_selector("settings-section-never-collected"),
        )
}

/// One category: its caption beside the switch.
fn telemetry_row(
    settings: &Settings,
    theme: &ArtisanTheme,
    id: &'static str,
    title: &str,
    description: &str,
) -> AnyElement {
    let control = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .text_xs()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child("Not decided"),
        )
        .child(settings.switch(id, false, None));
    row(
        theme,
        title.to_owned(),
        description.to_owned(),
        Some(control.into_any_element()),
    )
    .into_any_element()
}
