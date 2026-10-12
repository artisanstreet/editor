//! Models (`models.svelte`, `compaction-model.svelte`), as the previous
//! Editor painted it: the compaction choice and the favorites list, neither
//! configurable yet.

use crate::Settings;
use crate::chrome::{card, header, row, section_shell};
use crate::page;
use editor_core::SettingsPage;
use editor_ui::button::ButtonVariant;
use editor_ui::theme::ArtisanTheme;
use gpui::{Div, IntoElement, ParentElement as _, Styled as _, div, px};

pub(crate) fn render(settings: &Settings, theme: &ArtisanTheme) -> Div {
    let compaction = card(
        theme,
        vec![
            row(
                theme,
                "Cross-transfer compaction model",
                "Choose who writes a hand-off summary when a thread moves to another engine or model.",
                Some(settings.static_button(
                    "settings-compaction-trigger",
                    "Curated \u{25be}",
                    ButtonVariant::Outline,
                )),
            )
            .into_any_element(),
        ],
    );
    let favorites = card(
        theme,
        vec![
            div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .px(px(16.0))
                .py(px(28.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(
                            "No favorites yet. Starred models float to the top of every model picker; star one from the composer's picker or an engine page.",
                        ),
                )
                .into_any_element(),
        ],
    );
    div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::Models),
            page::description(SettingsPage::Models),
        ))
        .child(section_shell(
            theme,
            "compaction",
            "Compaction",
            Some("Compaction choices are not yet configurable in the native app."),
            None,
            compaction,
        ))
        .child(section_shell(
            theme,
            "favorites",
            "Favorites",
            None,
            None,
            favorites,
        ))
}
