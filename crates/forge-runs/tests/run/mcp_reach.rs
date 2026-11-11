use super::*;

async fn turn_servers(fixture: &Fixture, seen: &Mutex<Vec<String>>) -> Vec<String> {
    seen.lock().unwrap().clear();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("go"))
        .await
        .unwrap();
    fixture.settled().await;
    let mut names = seen.lock().unwrap().clone();
    names.retain(|name| ["up", "down", "local"].contains(&name.as_str()));
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_starts_without_servers_that_do_not_answer() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let fixture = fixture(Arc::new(move |turn, _| {
        recorded.lock().unwrap().extend(
            turn.launch
                .mcp_servers
                .iter()
                .map(|server| server.name.clone()),
        );
        Box::pin(async { Outcome::done() })
    }))
    .await;
    let up = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vacated = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let down = vacated.local_addr().unwrap();
    drop(vacated);
    fixture
        .hub
        .store()
        .set_setting(
            "mcp_servers".to_owned(),
            serde_json::json!([
                { "name": "up", "url": format!("http://{}/mcp", up.local_addr().unwrap()) },
                { "name": "down", "url": format!("http://{down}/mcp") },
                { "name": "local", "command": "server" },
            ]),
        )
        .await
        .unwrap();

    assert_eq!(
        turn_servers(&fixture, &seen).await,
        ["down", "local", "up"],
        "before anything is known every server is offered"
    );

    fixture.hub.probe_mcp_servers().await.unwrap();
    assert_eq!(
        turn_servers(&fixture, &seen).await,
        ["local", "up"],
        "a server nobody listens on is left out; command servers always start"
    );

    let _revived = tokio::net::TcpListener::bind(down).await.unwrap();
    fixture.hub.probe_mcp_servers().await.unwrap();
    assert_eq!(
        turn_servers(&fixture, &seen).await,
        ["down", "local", "up"],
        "a server that answers again is back on the next turn"
    );
}
