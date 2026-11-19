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
        test_support::wait::LIVENESS,
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

type ActingDuringTheRun = (String, Option<ThreadId>);

#[tokio::test(flavor = "multi_thread")]
async fn a_runs_gateway_address_stops_acting_for_its_thread_when_the_run_ends() {
    let hub_slot: Arc<std::sync::OnceLock<Hub>> = Arc::new(std::sync::OnceLock::new());
    let during: Arc<Mutex<Option<ActingDuringTheRun>>> = Arc::new(Mutex::new(None));
    let fixture = fixture(Arc::new({
        let hub_slot = Arc::clone(&hub_slot);
        let during = Arc::clone(&during);
        move |turn, _io| {
            let url = turn
                .launch
                .gateway_mcp
                .first()
                .map(|(_, url)| url.clone())
                .expect("the run gets the gateway");
            let token = url.rsplit('/').next().expect("a token").to_owned();
            let acting = hub_slot
                .get()
                .expect("the hub is set before the run")
                .gateway_thread(&token);
            *during.lock().unwrap() = Some((token, acting));
            Box::pin(async { Outcome::done() })
        }
    }))
    .await;
    fixture
        .hub
        .set_gateway("http://127.0.0.1:9/mcp/".to_owned(), false);
    hub_slot.set(fixture.hub.clone()).expect("set once");

    fixture
        .hub
        .send_message(&fixture.thread_id, message("look around"))
        .await
        .unwrap();
    fixture.settled().await;

    let (token, acting) = during.lock().unwrap().clone().expect("the run started");
    assert_eq!(acting, Some(fixture.thread_id.clone()));
    assert_eq!(
        fixture.hub.gateway_thread(&token),
        None,
        "a finished run's gateway address still acts for its thread"
    );
    fixture.hub.shutdown();
}
