//! The account's Forges this Editor has open.

use crate::Settings;
use crate::page;
use crate::ui::{card, header, row, section_shell};
use editor_core::{ConnectionStatus, SettingsPage};
use editor_ui::theme::ArtisanTheme;
use gpui::{AnyElement, Context, Div, IntoElement, ParentElement as _, Styled as _, div, px};

pub fn render(settings: &Settings, theme: &ArtisanTheme, cx: &Context<Settings>) -> Div {
    let mut rows: Vec<AnyElement> = settings
        .core
        .read(cx)
        .hosts()
        .map(|(config, host)| {
            let host = host.read(cx);
            let (state, colour) = match &host.status {
                ConnectionStatus::Live => ("Connected".to_owned(), theme.colors.green.solid),
                ConnectionStatus::Connecting => ("Connecting…".to_owned(), theme.colors.text_muted),
                ConnectionStatus::Offline { reason, .. } => {
                    (format!("Offline: {reason}"), theme.colors.red.solid)
                }
            };
            let detail = host
                .forge_version
                .as_ref()
                .map_or_else(String::new, |version| format!("Forge {version}"));
            let status = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .max_w(px(240.0))
                .text_size(theme.typography.label_text)
                .text_color(theme.colors.text_muted.to_paint())
                .child(
                    div()
                        .size(px(6.0))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(colour.to_paint()),
                )
                .child(div().truncate().child(state));
            row(
                theme,
                config.name.clone(),
                detail,
                Some(status.into_any_element()),
            )
            .into_any_element()
        })
        .collect();
    if rows.is_empty() {
        rows.push(
            row(
                theme,
                "No Forges open",
                "Add and connect Forges on the Account page.",
                None,
            )
            .into_any_element(),
        );
    }
    div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::Hosts),
            page::description(SettingsPage::Hosts),
        ))
        .child(section_shell(
            theme,
            "Open Forges",
            None,
            None,
            card(theme, rows),
        ))
}
