use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_threads_gateway_address_stops_acting_for_it() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    fixture
        .hub
        .set_gateway("http://127.0.0.1:9/mcp/".to_owned(), false);
    let pass = fixture.hub.gateway_pass(&fixture.thread_id).await.unwrap();
    let servers = pass.servers().to_vec();
    let token = servers[0].1.rsplit('/').next().unwrap().to_owned();
    let admitted = fixture
        .hub
        .gateway_caller(&token)
        .expect("the pass acts for its thread");
    assert_eq!(admitted.thread, fixture.thread_id);

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

    assert!(matches!(
        fixture.hub.gateway_caller(&token),
        Err(GatewayRefusal::Unknown)
    ));
    assert!(
        admitted.ended.is_cancelled(),
        "calls admitted before the thread was removed keep acting for it"
    );
    let reissued = fixture.hub.gateway_pass(&fixture.thread_id).await.unwrap();
    assert_ne!(reissued.servers()[0].1, servers[0].1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retention_setting_that_is_not_a_number_of_days_is_reported_not_ignored() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    fixture
        .hub
        .store()
        .set_setting(
            artisan_api::THREAD_RETENTION_DAYS_KEY.to_owned(),
            serde_json::json!("30"),
        )
        .await
        .unwrap();

    let removed = fixture.hub.remove_inactive_threads().await;

    assert!(
        matches!(removed, Err(RemoveInactiveThreadsError::NotDays)),
        "{removed:?}"
    );
}
