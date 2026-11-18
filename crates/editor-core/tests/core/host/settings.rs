use editor_core::{HostId, HostSettings, MCP_SERVERS_KEY};
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
        MCP_SERVERS_KEY.to_owned(),
        json!([{ "name": "github", "url": "https://mcp.example/github" }]),
    );
    let servers = settings.mcp_servers().expect("a corrected list reads");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "github");

    settings.remove(MCP_SERVERS_KEY);
    assert_eq!(settings.mcp_servers().map(<[_]>::len).ok(), Some(0));
}
