use super::*;
use artisan_api::{EditorCall, EditorReply};

fn pending_calls(hub: &Hub) -> Vec<String> {
    hub.live_snapshot()
        .1
        .into_iter()
        .filter_map(|event| {
            if let Event::EditorCall { id, .. } = event {
                Some(id)
            } else {
                None
            }
        })
        .collect()
}

async fn one_pending_call(hub: &Hub) -> String {
    let mut found = Vec::new();
    test_support::wait::eventually(
        "the call to reach the Editors",
        Duration::from_secs(10),
        || {
            found = pending_calls(hub);
            std::future::ready(found.len() == 1)
        },
    )
    .await;
    found.remove(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_editor_that_reconnects_still_gets_the_call_waiting_for_it() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let calling = tokio::spawn({
        let hub = fixture.hub.clone();
        async move { hub.call_editor(EditorCall::HostsList).await }
    });

    let id = one_pending_call(&fixture.hub).await;
    assert!(
        fixture
            .hub
            .answer_editor_call(&id, EditorReply::Ok(serde_json::json!([])))
    );

    assert_eq!(calling.await.unwrap().unwrap(), serde_json::json!([]));
    assert!(pending_calls(&fixture.hub).is_empty());
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_whose_caller_went_away_is_no_longer_offered() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let calling = tokio::spawn({
        let hub = fixture.hub.clone();
        async move { hub.call_editor(EditorCall::HostsList).await }
    });
    let id = one_pending_call(&fixture.hub).await;

    calling.abort();
    assert!(calling.await.unwrap_err().is_cancelled());

    assert!(pending_calls(&fixture.hub).is_empty());
    assert!(
        !fixture
            .hub
            .answer_editor_call(&id, EditorReply::Ok(serde_json::json!([])))
    );
    fixture.hub.shutdown();
}
