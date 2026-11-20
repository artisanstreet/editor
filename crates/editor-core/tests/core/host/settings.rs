use artisan_api::{EngineId, MCP_AUTH_KEY, MODEL_CONFIGS_KEY, McpAuthStatus};
use editor_core::{
    DEFAULT_ENGINE_CONFIG_KEY, HostId, HostSettingError, HostSettings, HostSwitch, MCP_SERVERS_KEY,
};
use serde_json::json;

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

    settings.insert(
        MCP_SERVERS_KEY,
        json!([{ "name": "github", "url": "https://mcp.example/github" }]),
    );
    let servers = settings.mcp_servers().expect("a corrected list reads");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "github");

    settings.remove(MCP_SERVERS_KEY);
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

    settings.insert(MCP_AUTH_KEY, json!({ "github": "signed_in" }));
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

    settings.remove(MODEL_CONFIGS_KEY);
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
    settings.insert(
        DEFAULT_ENGINE_CONFIG_KEY,
        json!({ "engine": "claude", "model": "opus", "effort": null }),
    );
    assert_eq!(
        settings
            .default_engine_config()
            .and_then(|config| config.model.as_deref()),
        Some("opus")
    );
    settings.insert(DEFAULT_ENGINE_CONFIG_KEY, json!("opus"));
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

    settings.insert(HostSwitch::SummaryTitles.key(), json!(false));
    assert!(!settings.switch(HostSwitch::SummaryTitles));
    settings.remove(HostSwitch::ContinueAfterRestart.key());
    assert!(settings.switch(HostSwitch::ContinueAfterRestart));
}
