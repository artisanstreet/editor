use artisan_api::Event;
use artisan_api::{EngineId, MCP_AUTH_KEY, MODEL_CONFIGS_KEY, McpAuthStatus};
use editor_core::client::SettingAck;
use editor_core::{
    DEFAULT_ENGINE_CONFIG_KEY, HostData, HostId, HostSettingError, HostSettings, HostSwitch,
    MCP_SERVERS_KEY, Restaged,
};
use serde_json::json;
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

#[test]
fn unreadable_mcp_servers_read_as_a_failure_not_as_none_configured() {
    let host = HostId("forge".into());
    let mut settings = HostSettings::from_values(
        host.clone(),
        serde_json::Map::from_iter([(MCP_SERVERS_KEY.to_owned(), json!({ "name": "github" }))]),
    );
    let error = settings
        .mcp_servers()
        .expect_err("a server object where a list belongs is not an empty list");
    assert_eq!(error.host, host);

    settings.replace(
        serde_json::Map::from_iter([(
            MCP_SERVERS_KEY.to_owned(),
            json!([{ "name": "github", "url": "https://mcp.example/github" }]),
        )]),
        1,
    );
    let servers = settings.mcp_servers().expect("a corrected list reads");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "github");

    settings.replace(serde_json::Map::new(), 2);
    assert_eq!(settings.mcp_servers().map(<[_]>::len).ok(), Some(0));
}

#[test]
fn unreadable_mcp_sign_in_states_read_as_a_failure_not_as_signed_out() {
    let host = HostId("forge".into());
    let mut settings = HostSettings::from_values(
        host.clone(),
        serde_json::Map::from_iter([(MCP_AUTH_KEY.to_owned(), json!(["github"]))]),
    );
    let error = settings
        .mcp_auth()
        .expect_err("a list where a map of states belongs is not an empty map");
    assert!(matches!(error, HostSettingError::Decode { host: failed, .. } if *failed == host));

    settings.replace(
        serde_json::Map::from_iter([(MCP_AUTH_KEY.to_owned(), json!({ "github": "signed_in" }))]),
        1,
    );
    let auth = settings.mcp_auth().expect("a corrected map reads");
    assert_eq!(auth.get("github"), Some(&McpAuthStatus::SignedIn));
}

#[test]
fn a_model_keeps_its_saved_config_beside_a_damaged_entry() {
    let mut settings = HostSettings::from_values(
        HostId("forge".into()),
        serde_json::Map::from_iter([(
            MODEL_CONFIGS_KEY.to_owned(),
            json!({
                "claude/opus": { "engine": "claude", "model": "opus", "effort": "high" },
                "claude/broken": { "engine": 7 },
                "renamed": { "engine": "codex", "model": "gpt", "effort": "low" },
            }),
        )]),
    );
    let opus = settings
        .saved_model_config(EngineId::Claude, "opus")
        .expect("opus was saved under its own name");
    assert_eq!(opus.effort.as_deref(), Some("high"));
    let gpt = settings
        .saved_model_config(EngineId::Codex, "gpt")
        .expect("an entry under another name still belongs to its model");
    assert_eq!(gpt.effort.as_deref(), Some("low"));
    assert!(
        settings
            .saved_model_config(EngineId::Claude, "gpt")
            .is_none()
    );

    settings.replace(
        serde_json::Map::from_iter([(MODEL_CONFIGS_KEY.to_owned(), json!({}))]),
        1,
    );
    assert!(
        settings
            .saved_model_config(EngineId::Claude, "opus")
            .is_none()
    );
}

#[test]
fn the_remembered_default_config_follows_its_setting() {
    let mut settings = HostSettings::new(HostId("forge".into()));
    assert!(settings.default_engine_config().is_none());
    settings.replace(
        serde_json::Map::from_iter([(
            DEFAULT_ENGINE_CONFIG_KEY.to_owned(),
            json!({ "engine": "claude", "model": "opus", "effort": null }),
        )]),
        1,
    );
    assert_eq!(
        settings
            .default_engine_config()
            .and_then(|config| config.model.as_deref()),
        Some("opus")
    );
    settings.replace(
        serde_json::Map::from_iter([(DEFAULT_ENGINE_CONFIG_KEY.to_owned(), json!("opus"))]),
        2,
    );
    assert!(settings.default_engine_config().is_none());
}

#[test]
fn a_host_switch_reads_its_value_its_declared_default_and_never_a_damaged_value() {
    let host = HostId("forge".into());
    let mut settings = HostSettings::from_values(
        host,
        serde_json::Map::from_iter([
            (
                HostSwitch::ContinueAfterRestart.key().to_owned(),
                json!(false),
            ),
            (HostSwitch::Suggestions.key().to_owned(), json!("false")),
        ]),
    );
    assert!(!settings.switch(HostSwitch::ContinueAfterRestart));
    assert!(settings.switch(HostSwitch::Suggestions));
    assert!(settings.switch(HostSwitch::SummaryTitles));

    settings.replace(
        serde_json::Map::from_iter([
            (
                HostSwitch::ContinueAfterRestart.key().to_owned(),
                json!(false),
            ),
            (HostSwitch::SummaryTitles.key().to_owned(), json!(false)),
        ]),
        1,
    );
    assert!(!settings.switch(HostSwitch::SummaryTitles));
    assert!(!settings.switch(HostSwitch::ContinueAfterRestart));
    settings.replace(serde_json::Map::new(), 2);
    assert!(settings.switch(HostSwitch::ContinueAfterRestart));
}

fn suggestions(host: &HostData) -> bool {
    host.settings.switch(HostSwitch::Suggestions)
}

const KEY: &str = "suggestions";

fn forge_with_suggestions(on: bool) -> StubbedHost {
    let mut state = empty_state();
    state.head = 5;
    state.settings = serde_json::Map::from_iter([(KEY.to_owned(), json!(on))]);
    StubbedHost::start(state, axum::Router::new())
}

fn changed_on_forge(stubbed: &mut StubbedHost, on: bool) -> artisan_api::Seq {
    let seq = stubbed.forge.publish(Event::Setting {
        key: KEY.into(),
        value: json!(on),
    });
    stubbed.apply_published();
    seq
}

const fn saved(revision: artisan_api::Seq) -> SettingAck {
    SettingAck::Revision(revision)
}

#[test]
fn a_value_being_saved_survives_a_reconnect_snapshot() {
    let mut stubbed = forge_with_suggestions(true);
    let host = stubbed.host_mut();
    let staging = host.stage_setting(KEY, json!(false));
    host.settings.replace(
        serde_json::Map::from_iter([
            (KEY.to_owned(), json!(true)),
            (HostSwitch::SummaryTitles.key().to_owned(), json!(false)),
        ]),
        6,
    );
    assert!(
        !suggestions(host),
        "the snapshot taken while saving keeps the user's choice"
    );
    assert!(
        !host.settings.switch(HostSwitch::SummaryTitles),
        "the snapshot's other values show"
    );
    staging.release(host);
    assert!(suggestions(host), "a refused save shows what Forge holds");
}

#[test]
fn a_snapshot_taken_before_a_save_committed_does_not_undo_it() {
    let mut stubbed = forge_with_suggestions(true);
    let host = stubbed.host_mut();
    let staging = host.stage_setting(KEY, json!(false));
    staging.accepted(host, json!(false), saved(10));
    changed_on_forge(&mut stubbed, true);
    let host = stubbed.host_mut();
    host.settings.replace(
        serde_json::Map::from_iter([(KEY.to_owned(), json!(true))]),
        8,
    );
    staging.release(host);
    assert!(
        !suggestions(host),
        "neither a change Forge streamed before the save nor a snapshot older than it brings the old value back"
    );

    let staging = host.stage_setting(KEY, json!(true));
    staging.accepted(host, json!(true), saved(11));
    host.settings.replace(
        serde_json::Map::from_iter([(KEY.to_owned(), json!(false))]),
        12,
    );
    staging.release(host);
    assert!(
        !suggestions(host),
        "a snapshot taken after the save shows the change made elsewhere since"
    );
}

#[test]
fn a_save_answered_after_a_newer_change_elsewhere_does_not_hide_that_change() {
    let mut stubbed = forge_with_suggestions(true);
    let staging = stubbed.host_mut().stage_setting(KEY, json!(false));
    let ours = changed_on_forge(&mut stubbed, false);
    changed_on_forge(&mut stubbed, true);
    let host = stubbed.host_mut();
    staging.accepted(host, json!(false), saved(ours));
    assert!(
        !suggestions(host),
        "the newer choice still shows while it saves"
    );
    staging.release(host);
    assert!(
        suggestions(host),
        "the other Editor's newer value shows, not our older save Forge answered late"
    );
}

#[test]
fn a_save_without_an_echo_yet_shows_the_saved_value() {
    let mut stubbed = forge_with_suggestions(true);
    let ours = stubbed.forge.head() + 1;
    let host = stubbed.host_mut();
    let staging = host.stage_setting(KEY, json!(false));
    staging.accepted(host, json!(false), saved(ours));
    staging.release(host);
    assert!(!suggestions(host));
    assert_eq!(changed_on_forge(&mut stubbed, false), ours);
    changed_on_forge(&mut stubbed, true);
    assert!(
        suggestions(stubbed.host()),
        "once saved, later changes from Forge show"
    );
}

#[test]
fn unversioned_saves_answered_before_their_echoes_hold_the_last_choice_until_each_echo_arrives() {
    let mut stubbed = forge_with_suggestions(true);
    let host = stubbed.host_mut();
    let staging = host.stage_setting(KEY, json!(false));
    staging.accepted(host, json!(false), SettingAck::Unversioned);
    assert_eq!(staging.restage(host, json!(true)), Restaged::Shown);
    staging.dispatched(host, json!(true));
    staging.accepted(host, json!(true), SettingAck::Unversioned);
    staging.release(host);
    assert!(
        suggestions(host),
        "the last choice shows once both saves are answered"
    );
    changed_on_forge(&mut stubbed, false);
    assert!(
        suggestions(stubbed.host()),
        "the first save's late echo does not flip the switch back"
    );
    changed_on_forge(&mut stubbed, true);
    assert!(suggestions(stubbed.host()));
    changed_on_forge(&mut stubbed, false);
    assert!(
        !suggestions(stubbed.host()),
        "once every echo arrived, a change made elsewhere shows"
    );
}

#[test]
fn an_unversioned_save_answered_after_a_change_elsewhere_shows_whichever_forge_committed_last() {
    let mut stubbed = forge_with_suggestions(false);
    let staging = stubbed.host_mut().stage_setting(KEY, json!(true));
    changed_on_forge(&mut stubbed, false);
    let host = stubbed.host_mut();
    staging.accepted(host, json!(true), SettingAck::Unversioned);
    staging.release(host);
    assert!(
        suggestions(host),
        "the saved choice shows until Forge streams where it landed"
    );
    changed_on_forge(&mut stubbed, true);
    assert!(
        suggestions(stubbed.host()),
        "our save committed after the other change, so it stays"
    );

    let staging = stubbed.host_mut().stage_setting(KEY, json!(false));
    changed_on_forge(&mut stubbed, false);
    changed_on_forge(&mut stubbed, true);
    let host = stubbed.host_mut();
    staging.accepted(host, json!(false), SettingAck::Unversioned);
    staging.release(host);
    assert!(
        suggestions(host),
        "our echo arrived before the empty answer, so the later change elsewhere shows"
    );
}

#[test]
fn an_unversioned_save_answered_after_a_resnapshot_shows_the_snapshot() {
    const NOTE: &str = "note";
    let mut stubbed = forge_with_suggestions(true);
    let host = stubbed.host_mut();
    let staging = host.stage_setting(NOTE, json!("ours"));
    host.settings.replace(
        serde_json::Map::from_iter([(NOTE.to_owned(), json!("theirs"))]),
        5,
    );
    staging.accepted(host, json!("ours"), SettingAck::Unversioned);
    staging.release(host);
    assert_eq!(
        host.setting(NOTE),
        Some(&json!("theirs")),
        "a snapshot taken while the save was answered holds Forge's value, even under the same head"
    );
}

#[test]
fn a_failed_last_write_falls_back_to_the_earlier_write_forge_accepted() {
    let mut stubbed = forge_with_suggestions(true);
    let host = stubbed.host_mut();
    let staging = host.stage_setting(KEY, json!(false));
    assert_eq!(staging.restage(host, json!(true)), Restaged::Shown);
    staging.accepted(host, json!(false), saved(10));
    assert!(suggestions(host), "the newer choice shows while it saves");
    staging.release(host);
    assert!(
        !suggestions(host),
        "the refused write falls back to the write Forge accepted, not the older value"
    );
}

#[test]
fn a_released_writer_cannot_take_back_a_setting_another_writer_staged() {
    let mut stubbed = forge_with_suggestions(true);
    let host = stubbed.host_mut();
    let first = host.stage_setting(KEY, json!(false));
    let second = host.stage_setting(KEY, json!(false));
    assert_eq!(
        first.restage(host, json!(true)),
        Restaged::Superseded,
        "a superseded writer cannot take the setting back by restaging"
    );
    assert!(!suggestions(host), "the newer writer's choice still shows");
    first.release(host);
    assert!(!suggestions(host), "the newer writer's choice still shows");
    second.release(host);
    assert!(suggestions(host));
}

#[test]
#[should_panic(expected = "is changed only through that host")]
fn a_staged_setting_cannot_be_released_through_another_host() {
    let mut first = forge_with_suggestions(true);
    let mut second = forge_with_suggestions(true);
    let staging = first.host_mut().stage_setting(KEY, json!(false));
    let _other = second.host_mut().stage_setting(KEY, json!(false));
    staging.release(second.host_mut());
}

#[test]
fn a_dropped_staging_token_lets_go_of_the_setting() {
    let mut stubbed = StubbedHost::start(empty_state(), axum::Router::new());
    let staging = stubbed.host_mut().stage_setting(KEY, json!(false));
    assert!(!suggestions(stubbed.host()));
    drop(staging);
    stubbed.settle("the dropped token to release the setting", |core, host| {
        core.host(host).is_some_and(suggestions)
    });
    let staging = stubbed.host_mut().stage_setting(KEY, json!(false));
    assert!(
        !suggestions(stubbed.host()),
        "the setting can be staged again after its writer went away"
    );
    staging.release(stubbed.host_mut());
    assert!(suggestions(stubbed.host()));
}
