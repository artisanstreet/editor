//! The Marketplace and the global prompt page in the Editor: the sidebar
//! opens the Marketplace, both read the Forge once, pushes reach the
//! screens, and the user's changes are sent as Forge commands that hold the
//! connection until answered.

use super::*;
use crate::marketplace_screen::MarketplaceEvent;
use crate::native_transport_service::{
    ExtensionsCommand, ExtensionsEvent, HoldKind, HostStateEvent,
};
use artisan_domain::{
    Extension, ExtensionCatalog, ExtensionEngineState, ExtensionEngineStatus, ExtensionKind,
    ExtensionLinkSupport, ExtensionOrigin, GlobalPrompt, GlobalPromptEngineState,
    GlobalPromptEngineStatus, GlobalPromptRevision, GlobalPromptSetup,
};

fn catalog(enabled: bool) -> ExtensionCatalog {
    ExtensionCatalog::new(
        vec![Extension {
            id: "artisan.artisan-editor".to_owned(),
            kind: ExtensionKind::Skill,
            origin: ExtensionOrigin::FirstParty,
            name: "artisan-editor".to_owned(),
            description: "Write replies that render richly.".to_owned(),
            digest: "a".repeat(64),
            file_count: 2,
            byte_count: 40_960,
            enabled,
            engines: vec![ExtensionEngineState {
                engine_id: "claude".to_owned(),
                status: if enabled {
                    ExtensionEngineStatus::Synced
                } else {
                    ExtensionEngineStatus::Off
                },
                reason: None,
            }],
        }],
        ExtensionLinkSupport::Available,
    )
    .expect("catalog")
}

fn prompt(revision: u64, text: &str) -> GlobalPrompt {
    GlobalPrompt::new(
        GlobalPromptRevision::new(revision),
        text.to_owned(),
        GlobalPromptSetup::Ready,
        vec![GlobalPromptEngineState {
            engine_id: "claude".to_owned(),
            status: GlobalPromptEngineStatus::Synced,
            file: Some(".claude/CLAUDE.md".to_owned()),
            external_text: None,
            reason: None,
        }],
    )
    .expect("prompt")
}

fn extension_commands(commands: &[NativeTransportCommand]) -> Vec<ExtensionsCommand> {
    commands
        .iter()
        .filter_map(|command| match command {
            NativeTransportCommand::Extensions(command) => Some(command.clone()),
            _ => None,
        })
        .collect()
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario follows the Marketplace from the sidebar through a refused and an accepted change"
)]
fn the_sidebar_opens_the_marketplace_and_switches_reach_the_forge(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, _| {
            application.test_command_sink = Some(sink);
        });
    });
    let row = cx
        .debug_bounds("artisan-marketplace-navigation")
        .expect("marketplace row mounted");
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert_eq!(application.route(), &NativeRoute::Marketplace);
            let sent = extension_commands(&commands.borrow());
            assert!(sent.contains(&ExtensionsCommand::ReadCatalog));
            assert!(sent.contains(&ExtensionsCommand::ReadPrompt));
            for command in &sent {
                assert_eq!(
                    NativeTransportCommand::Extensions(command.clone()).hold_kind(),
                    None,
                    "reads never hold the connection"
                );
            }
            application.handle_service_event(
                NativeTransportEvent::HostState(HostStateEvent::Extensions(catalog(true))),
                cx,
            );
            let screen = application
                .marketplace_screen
                .clone()
                .expect("marketplace mounted");
            assert_eq!(screen.read(cx).catalog(), Some(&catalog(true)));

            application.handle_marketplace_event(
                screen.clone(),
                &MarketplaceEvent::SetEnabled {
                    extension_id: "artisan.artisan-editor".to_owned(),
                    enabled: false,
                },
                cx,
            );
            let change = extension_commands(&commands.borrow())
                .into_iter()
                .find_map(|command| match command {
                    ExtensionsCommand::SetEnabled(change) => Some(change),
                    _ => None,
                })
                .expect("enable change sent");
            assert!(!change.enabled);
            assert_eq!(
                NativeTransportCommand::Extensions(ExtensionsCommand::SetEnabled(change.clone()))
                    .hold_kind(),
                Some(HoldKind::Extension)
            );

            // A refusal names the skill and keeps the engines as they were.
            application.handle_service_event(
                NativeTransportEvent::Extensions(ExtensionsEvent::CatalogChanged {
                    request_id: change.request_id.clone(),
                    result: Err(super::super::invalid_service_failure()),
                }),
                cx,
            );
            assert_eq!(
                screen.read(cx).notice(),
                Some(
                    "Artisan could not update artisan-editor. Its engines are unchanged; try again."
                )
            );

            application.handle_marketplace_event(
                screen.clone(),
                &MarketplaceEvent::SetEnabled {
                    extension_id: "artisan.artisan-editor".to_owned(),
                    enabled: false,
                },
                cx,
            );
            let retry = extension_commands(&commands.borrow())
                .into_iter()
                .filter_map(|command| match command {
                    ExtensionsCommand::SetEnabled(change) => Some(change),
                    _ => None,
                })
                .next_back()
                .expect("retry sent");
            application.handle_service_event(
                NativeTransportEvent::Extensions(ExtensionsEvent::CatalogChanged {
                    request_id: retry.request_id,
                    result: Ok(Box::new(catalog(false))),
                }),
                cx,
            );
            assert_eq!(screen.read(cx).catalog(), Some(&catalog(false)));
            assert_eq!(screen.read(cx).notice(), None);

            application.handle_marketplace_event(screen, &MarketplaceEvent::OpenGlobalPrompt, cx);
            assert!(matches!(
                application.route(),
                NativeRoute::Settings {
                    section: SettingsRoute::GlobalPrompt,
                    ..
                }
            ));
        });
    });
}

#[gpui::test]
fn the_global_prompt_page_saves_from_its_base_and_keeps_unsaved_edits(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.navigate(
                NativeRoute::Settings {
                    section: SettingsRoute::GlobalPrompt,
                    engine: None,
                },
                cx,
            );
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert!(
                extension_commands(&commands.borrow()).contains(&ExtensionsCommand::ReadPrompt)
            );
            application.handle_service_event(
                NativeTransportEvent::Extensions(ExtensionsEvent::Prompt(Ok(Box::new(prompt(
                    3,
                    "Be brief.",
                ))))),
                cx,
            );
            let screen = application
                .settings_screen
                .clone()
                .expect("settings mounted");
            assert_eq!(
                screen.read(cx).global_prompt_view(cx),
                Some(("Be brief.".to_owned(), false, false))
            );

            screen.update(cx, |screen, cx| {
                screen.edit_global_prompt_for_test("Be brief and kind.", cx);
            });
            // A newer version pushed meanwhile never replaces the edit.
            application.handle_service_event(
                NativeTransportEvent::HostState(HostStateEvent::GlobalPrompt(prompt(4, "Other."))),
                cx,
            );
            assert_eq!(
                screen.read(cx).global_prompt_view(cx),
                Some(("Be brief and kind.".to_owned(), true, true))
            );
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let screen = application
                .settings_screen
                .clone()
                .expect("settings mounted");
            // Saving is blocked until the newer version is loaded; once the
            // user writes over the latest, the save names revision 4.
            screen.update(cx, |screen, cx| {
                screen.press_save_global_prompt(cx);
                screen.load_latest_global_prompt(cx);
                screen.edit_global_prompt_for_test("Other, but kind.", cx);
                screen.press_save_global_prompt(cx);
            });
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |_, _| {
            let saves: Vec<_> = extension_commands(&commands.borrow())
                .into_iter()
                .filter_map(|command| match command {
                    ExtensionsCommand::SavePrompt(save) => Some(save),
                    _ => None,
                })
                .collect();
            assert_eq!(saves.len(), 1, "a save over a stale base is never sent");
            assert_eq!(saves[0].base_revision, GlobalPromptRevision::new(4));
            assert_eq!(saves[0].text, "Other, but kind.");
        });
    });
}
