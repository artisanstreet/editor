//! The live engine Installation card: the Forge-managed status, the version
//! selection with its actions, and the vendor version picker.

use artisan_domain::EngineVersionEntry;

use super::chrome::{settings_card, settings_row};
use super::*;

impl SettingsScreen {
    /// Builds the Installation card for one live engine.
    pub(super) fn installation_card(
        theme: &artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let status = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .py(px(20.0))
            .child(
                div()
                    .debug_selector(|| "settings-installation-status".to_owned())
                    .text_sm()
                    .text_color(theme.colors.foreground.to_paint())
                    .child(snapshot.installation_state()),
            );
        let Some(install) = snapshot
            .install
            .as_ref()
            .filter(|install| install.manageable())
        else {
            return settings_card(theme, vec![status]);
        };
        let engine_id = snapshot.engine_id.clone();
        let mut actions = div().flex_shrink_0().flex().flex_row().gap(px(8.0));
        if install.status.held_version.is_some() {
            actions = actions.child(Self::install_action(
                "settings-engine-use-latest".to_owned(),
                "Use latest".to_owned(),
                SettingsScreenEvent::SelectEngineVersion {
                    engine_id: engine_id.clone(),
                    version: None,
                },
                cx,
            ));
        }
        if let Some(previous) = &install.status.rollback_version {
            actions = actions.child(Self::install_action(
                "settings-engine-rollback".to_owned(),
                format!("Roll back to {previous}"),
                SettingsScreenEvent::RollbackEngine {
                    engine_id: engine_id.clone(),
                },
                cx,
            ));
        }
        actions = actions.child(Self::install_action(
            "settings-engine-versions".to_owned(),
            "Choose version".to_owned(),
            SettingsScreenEvent::LoadEngineVersions {
                engine_id: engine_id.clone(),
            },
            cx,
        ));
        let mut blocks = vec![
            status,
            settings_row(
                theme,
                "Version",
                &install.selection_copy(),
                Some(actions.into_any_element()),
            ),
        ];
        if let Some(failure) = &install.request_failure {
            blocks.push(Self::install_note(theme, failure.clone()));
        }
        match &install.versions {
            SettingsEngineVersions::NotLoaded => {}
            SettingsEngineVersions::Loading => {
                blocks.push(Self::install_note(
                    theme,
                    "Loading published versions…".to_owned(),
                ));
            }
            SettingsEngineVersions::Failed(failure) => {
                blocks.push(Self::install_note(theme, failure.clone()));
            }
            SettingsEngineVersions::Loaded(versions) => {
                blocks.push(Self::version_list(theme, &engine_id, versions, cx));
            }
        }
        settings_card(theme, blocks)
    }

    fn version_list(
        theme: &artisan_ui::theme::ArtisanTheme,
        engine_id: &str,
        versions: &[EngineVersionEntry],
        cx: &mut Context<Self>,
    ) -> Div {
        let mut list = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(12.0))
            .debug_selector(|| "settings-engine-version-list".to_owned());
        for entry in versions {
            let mut marks = Vec::new();
            if entry.active {
                marks.push("current");
            } else if entry.installed {
                marks.push("installed");
            }
            if entry.below_floor {
                marks.push("older than Artisan supports");
            }
            let label = if marks.is_empty() {
                entry.version.clone()
            } else {
                format!("{} · {}", entry.version, marks.join(" · "))
            };
            let mut row = div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .text_color(if entry.below_floor {
                            theme.colors.muted_foreground.to_paint()
                        } else {
                            theme.colors.foreground.to_paint()
                        })
                        .child(label),
                );
            if !entry.active && !entry.below_floor {
                row = row.child(Self::install_action(
                    format!("settings-engine-use-{}", entry.version),
                    format!("Use {}", entry.version),
                    SettingsScreenEvent::SelectEngineVersion {
                        engine_id: engine_id.to_owned(),
                        version: Some(entry.version.clone()),
                    },
                    cx,
                ));
            }
            list = list.child(row);
        }
        list
    }

    fn install_note(theme: &artisan_ui::theme::ArtisanTheme, copy: String) -> Div {
        div()
            .w_full()
            .py(px(12.0))
            .text_sm()
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(copy)
    }

    /// One ghost-style action emitting a screen event, with a dynamic label.
    fn install_action(
        selector: String,
        label: String,
        event: SettingsScreenEvent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(selector.clone())
            .debug_selector(move || selector.clone())
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(event.clone());
            }))
            .p(px(4.0))
            .text_sm()
            .child(label)
            .into_any_element()
    }
}
