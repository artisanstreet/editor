use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_threads_gateway_address_stops_acting_for_it() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    fixture
        .hub
        .set_gateway("http://127.0.0.1:9/mcp/".to_owned(), false);
    let pass = fixture.hub.gateway_pass(&fixture.thread_id).await;
    let servers = pass.servers().to_vec();
    let token = servers[0].1.rsplit('/').next().unwrap().to_owned();
    assert_eq!(
        fixture.hub.gateway_thread(&token),
        Some(fixture.thread_id.clone())
    );

    let removed = fixture
        .hub
        .store()
        .remove_inactive_threads(
            artisan_api::now() + 60_000,
            std::collections::HashSet::new(),
            8,
        )
        .await
        .unwrap();
    for thread in removed {
        fixture.hub.forget_removed_thread(thread).await;
    }

    assert_eq!(fixture.hub.gateway_thread(&token), None);
    let reissued = fixture.hub.gateway_pass(&fixture.thread_id).await;
    assert_ne!(reissued.servers()[0].1, servers[0].1);
}
