use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_check_that_changes_no_status_writes_nothing() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let store = fixture.hub.store();
    let head = store.head();
    fixture.hub.check_mcp_auth().await;
    assert_eq!(store.head(), head);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_records_a_changed_status_once() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let store = fixture.hub.store();
    store
        .set_setting(
            artisan_api::MCP_AUTH_KEY.to_owned(),
            serde_json::json!({ "removed": "needs_sign_in" }),
        )
        .await
        .unwrap();
    let head = store.head();
    fixture.hub.check_mcp_auth().await;
    assert_eq!(store.head(), head + 1);
    assert!(fixture.hub.mcp_auth().await.unwrap().is_empty());
    fixture.hub.check_mcp_auth().await;
    assert_eq!(store.head(), head + 1);
}
