//! The manual engine settings draft is built by the Forge: saving asks it to
//! resolve the draft, saves what it built, and shows what it refused.

use super::forge_codex_config;
use super::*;
use crate::native_transport_service::ForgeDecisionCommand;
use artisan_protocol::{RegisteredEngineProfilesResult, ThreadEngineSettingsResult};

fn unconfigured_settings(application: &mut NativeApplication, thread_id: &ThreadId) {
    let settings = application.engine_settings_mut();
    settings.select_thread(Some(thread_id));
    settings.on_registry_loaded(RegisteredEngineProfilesResult::RegistryPresent {
        profile_ids: vec![artisan_domain::EngineProfileId::parse("default").expect("profile")],
    });
    let generation = settings.prepare_settings_load().expect("generation");
    assert!(settings.mark_settings_load_admitted(thread_id, generation));
    settings.on_settings_loaded(
        generation,
        ThreadEngineSettingsResult::Unconfigured {
            thread_id: thread_id.clone(),
        },
    );
}

fn resolution_asked(
    commands: &[NativeTransportCommand],
) -> Option<artisan_domain::ResolveEngineConfiguration> {
    commands.iter().find_map(|command| match command {
        NativeTransportCommand::ForgeDecision(
            ForgeDecisionCommand::ResolveEngineConfiguration(query),
        ) => Some(query.clone()),
        _ => None,
    })
}

#[gpui::test]
fn saving_the_manual_draft_saves_what_the_forge_built(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    let thread_id = ThreadId::parse("manual-thread").unwrap();
    let config = forge_codex_config(None);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(application, cx, thread_id.clone(), "", sink);
            unconfigured_settings(application, &thread_id);
            application.engine_settings_mut().draft_mut().clone_from(
                &crate::engine_settings::EngineSettingsDraft::from_config(&config),
            );
            application.save_engine_settings(cx);
            let query = resolution_asked(&commands.borrow()).expect("the Forge builds the draft");
            assert_eq!(query.thread_id, thread_id);
            // No configuration is saved before the Forge answers.
            assert!(!commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::SetThreadEngineConfig(_)
            )));
            application.receive_configuration_resolution(
                &thread_id,
                &query.configuration,
                Ok(Ok(Box::new(config.clone()))),
                cx,
            );
            let saved = commands
                .borrow()
                .iter()
                .find_map(|command| match command {
                    NativeTransportCommand::SetThreadEngineConfig(save) => Some(save.clone()),
                    _ => None,
                })
                .expect("the built configuration is saved");
            assert_eq!(saved.config(), &config);
        });
    });
}

#[gpui::test]
fn a_draft_the_forge_refuses_keeps_its_fields_and_shows_why(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    let thread_id = ThreadId::parse("manual-refused").unwrap();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(application, cx, thread_id.clone(), "", sink);
            unconfigured_settings(application, &thread_id);
            application.engine_settings_mut().draft_mut().profile_id = "default".to_owned();
            application.save_engine_settings(cx);
            let query = resolution_asked(&commands.borrow()).expect("resolution asked");
            let refusal = artisan_domain::SubmissionRefusal::new(
                artisan_domain::SubmissionRefusalKind::InvalidSelection,
                "The model_id value is not a valid identifier. Your settings are preserved; correct it and save again.",
            )
            .expect("refusal");
            application.receive_configuration_resolution(
                &thread_id,
                &query.configuration,
                Ok(Err(refusal)),
                cx,
            );
            assert_eq!(
                application.engine_settings().refusal(),
                Some(
                    "The model_id value is not a valid identifier. Your settings are preserved; correct it and save again."
                )
            );
            assert_eq!(application.engine_settings().draft().profile_id, "default");
            assert!(!commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::SetThreadEngineConfig(_)
            )));
        });
    });
}
