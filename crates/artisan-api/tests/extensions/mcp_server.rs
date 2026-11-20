use super::*;

#[test]
fn a_logged_mcp_server_shows_none_of_its_secrets() {
    let server = McpServer {
        name: "linear".to_owned(),
        command: "npx".to_owned(),
        args: vec!["--token".to_owned(), "arg-secret".to_owned()],
        env: [("API_KEY".to_owned(), "env-secret".to_owned())].into(),
        url: Some("https://mcp.example.com/path-secret/mcp?key=query-secret".to_owned()),
        headers: [(
            "Authorization".to_owned(),
            "Bearer header-secret".to_owned(),
        )]
        .into(),
        sse: false,
    };
    let logged = format!("{server:?}");
    for secret in [
        "arg-secret",
        "env-secret",
        "path-secret",
        "query-secret",
        "header-secret",
    ] {
        assert!(!logged.contains(secret), "{secret} in {logged}");
    }
    for shown in [
        "linear",
        "npx",
        "API_KEY",
        "Authorization",
        "https://mcp.example.com",
    ] {
        assert!(logged.contains(shown), "{shown} missing from {logged}");
    }
}
