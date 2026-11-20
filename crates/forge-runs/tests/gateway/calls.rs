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

#[tokio::test(flavor = "multi_thread")]
async fn reconnecting_editors_receive_waiting_calls_in_one_order() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let mut offered_live = fixture.hub.store().subscribe_local_live();
    let mut callers = Vec::new();
    for _ in 0..16 {
        callers.push(tokio::spawn({
            let hub = fixture.hub.clone();
            async move { hub.call_editor(EditorCall::HostsList).await }
        }));
        test_support::wait::within(
            "the call to reach the Editors",
            test_support::wait::LIVENESS,
            async {
                while !matches!(
                    offered_live.recv().await.unwrap().event,
                    Event::EditorCall { .. }
                ) {}
            },
        )
        .await;
    }

    let offered = pending_calls(&fixture.hub);
    assert_eq!(offered.len(), 16);
    let mut made = offered.clone();
    made.sort();
    assert_eq!(
        offered, made,
        "every reconnecting Editor must see the waiting calls in one order"
    );

    for id in &offered {
        assert!(
            fixture
                .hub
                .answer_editor_call(id, EditorReply::Ok(serde_json::json!([])))
        );
    }
    for caller in callers {
        caller.await.unwrap().unwrap();
    }
    fixture.hub.shutdown();
}

type ActingDuringTheRun = (String, Result<GatewayCaller, GatewayRefusal>);

#[tokio::test(flavor = "multi_thread")]
async fn a_threads_gateway_address_acts_only_while_one_of_its_runs_is_live() {
    let hub_slot: Arc<std::sync::OnceLock<Hub>> = Arc::new(std::sync::OnceLock::new());
    let during: Arc<Mutex<Vec<ActingDuringTheRun>>> = Arc::new(Mutex::new(Vec::new()));
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
                .gateway_caller(&token);
            during.lock().unwrap().push((token, acting));
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
    fixture.quiescent().await;
    let (token, first) = during.lock().unwrap()[0].clone();
    let first = first.expect("the address acts for its thread while the run is live");
    assert_eq!(first.thread, fixture.thread_id);
    assert!(
        first.ended.is_cancelled(),
        "calls admitted during the run stop acting once it ended"
    );
    assert!(
        matches!(
            fixture.hub.gateway_caller(&token),
            Err(GatewayRefusal::Idle { thread }) if thread == fixture.thread_id
        ),
        "the address refuses calls between runs"
    );

    fixture
        .hub
        .send_message(&fixture.thread_id, message("look again"))
        .await
        .unwrap();
    fixture.settled().await;
    let (again, second) = during.lock().unwrap()[1].clone();
    assert_eq!(
        again, token,
        "the next run keeps the address an engine may have cached"
    );
    let second = second.expect("the address acts for the next run");
    assert_eq!(second.thread, fixture.thread_id);
    assert!(second.ended.is_cancelled());
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_side_query_ending_during_a_run_does_not_end_the_runs_artisan_calls() {
    let hub_slot: Arc<std::sync::OnceLock<Hub>> = Arc::new(std::sync::OnceLock::new());
    let admitted: Arc<std::sync::OnceLock<GatewayCaller>> = Arc::new(std::sync::OnceLock::new());
    let run_started = Arc::new(Notify::new());
    let title_open = Arc::new(Notify::new());
    let artisan_called = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let fixture = fixture(Arc::new({
        let hub_slot = Arc::clone(&hub_slot);
        let admitted = Arc::clone(&admitted);
        let run_started = Arc::clone(&run_started);
        let title_open = Arc::clone(&title_open);
        let artisan_called = Arc::clone(&artisan_called);
        let release = Arc::clone(&release);
        move |turn, _io| {
            let url = turn
                .launch
                .gateway_mcp
                .first()
                .map(|(_, url)| url.clone())
                .expect("the run gets the gateway");
            let token = url.rsplit('/').next().expect("a token").to_owned();
            let hub = hub_slot
                .get()
                .expect("the hub is set before the run")
                .clone();
            let admitted = Arc::clone(&admitted);
            let run_started = Arc::clone(&run_started);
            let title_open = Arc::clone(&title_open);
            let artisan_called = Arc::clone(&artisan_called);
            let release = Arc::clone(&release);
            Box::pin(async move {
                run_started.notify_one();
                title_open.notified().await;
                let caller = hub
                    .gateway_caller(&token)
                    .expect("the address acts while the run is live");
                admitted.set(caller).expect("one run");
                artisan_called.notify_one();
                release.notified().await;
                Outcome::done()
            })
        }
    }))
    .await;
    fixture
        .hub
        .set_gateway("http://127.0.0.1:9/mcp/".to_owned(), false);
    hub_slot.set(fixture.hub.clone()).expect("set once");
    fixture
        .hub
        .send_message(&fixture.thread_id, message("wait for the Editor"))
        .await
        .unwrap();
    let liveness = test_support::wait::LIVENESS;
    test_support::wait::within("the run to start", liveness, run_started.notified()).await;
    let title = fixture
        .hub
        .gateway_pass(&fixture.thread_id)
        .await
        .expect("a side query of the same thread gets the gateway");
    title_open.notify_one();
    test_support::wait::within(
        "the run to call Artisan",
        liveness,
        artisan_called.notified(),
    )
    .await;
    let foreground = admitted.get().expect("the run called Artisan").clone();

    drop(title);

    assert!(
        !foreground.ended.is_cancelled(),
        "a title query finishing cancelled the running turn's Artisan call"
    );
    release.notify_one();
    fixture.settled().await;
    assert!(
        foreground.ended.is_cancelled(),
        "the run's Artisan calls stop once the thread's last activity ends"
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_runs_artisan_call_ends_although_a_title_query_keeps_the_thread_active() {
    let hub_slot: Arc<std::sync::OnceLock<Hub>> = Arc::new(std::sync::OnceLock::new());
    let admitted: Arc<std::sync::OnceLock<GatewayCaller>> = Arc::new(std::sync::OnceLock::new());
    let title_open = Arc::new(Notify::new());
    let run_started = Arc::new(Notify::new());
    let artisan_called = Arc::new(Notify::new());
    let fixture = fixture(Arc::new({
        let hub_slot = Arc::clone(&hub_slot);
        let admitted = Arc::clone(&admitted);
        let title_open = Arc::clone(&title_open);
        let run_started = Arc::clone(&run_started);
        let artisan_called = Arc::clone(&artisan_called);
        move |turn, io| {
            let url = turn
                .launch
                .gateway_mcp
                .first()
                .map(|(_, url)| url.clone())
                .expect("the run gets the gateway");
            let token = url.rsplit('/').next().expect("a token").to_owned();
            let hub = hub_slot
                .get()
                .expect("the hub is set before the run")
                .clone();
            let admitted = Arc::clone(&admitted);
            let title_open = Arc::clone(&title_open);
            let run_started = Arc::clone(&run_started);
            let artisan_called = Arc::clone(&artisan_called);
            Box::pin(async move {
                run_started.notify_one();
                title_open.notified().await;
                let caller = hub
                    .gateway_caller(&token)
                    .expect("the address acts while the run is live");
                admitted.set(caller).expect("one run");
                artisan_called.notify_one();
                io.cancel.cancelled().await;
                Outcome::cancelled()
            })
        }
    }))
    .await;
    fixture
        .hub
        .set_gateway("http://127.0.0.1:9/mcp/".to_owned(), false);
    hub_slot.set(fixture.hub.clone()).expect("set once");
    fixture
        .hub
        .send_message(&fixture.thread_id, message("wait for the agents"))
        .await
        .unwrap();
    let liveness = test_support::wait::LIVENESS;
    test_support::wait::within("the run to start", liveness, run_started.notified()).await;
    let title = fixture
        .hub
        .gateway_pass(&fixture.thread_id)
        .await
        .expect("a title query of the same thread gets the gateway");
    title_open.notify_one();
    test_support::wait::within(
        "the run to call Artisan",
        liveness,
        artisan_called.notified(),
    )
    .await;
    let call = admitted.get().expect("the run called Artisan").clone();

    fixture.hub.cancel(&fixture.thread_id).await.unwrap();

    test_support::wait::within(
        "the stopped run's Artisan call to end",
        liveness,
        call.finished(),
    )
    .await;
    assert!(
        !call.ended.is_cancelled(),
        "the title query still keeps the thread's gateway address active"
    );
    drop(title);
    fixture.settled().await;
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_computer_use_setting_that_is_neither_on_nor_off_fails_the_turn_and_says_why() {
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ran = Arc::clone(&started);
    let fixture = fixture(Arc::new(move |_, _| {
        ran.store(true, Ordering::SeqCst);
        Box::pin(async { Outcome::done() })
    }))
    .await;
    fixture
        .hub
        .set_gateway("http://127.0.0.1:9/mcp/".to_owned(), true);
    fixture
        .hub
        .store()
        .set_setting(COMPUTER_USE_KEY.to_owned(), serde_json::json!("yes"))
        .await
        .unwrap();

    fixture
        .hub
        .send_message(&fixture.thread_id, message("look around"))
        .await
        .unwrap();
    let runs = fixture.settled().await;

    assert_eq!(runs[0].status(), RunStatus::Failed, "{runs:#?}");
    assert!(
        runs[0]
            .error()
            .is_some_and(|error| error.contains("computer_use setting is neither true nor false")),
        "{runs:#?}"
    );
    assert!(
        !started.load(Ordering::SeqCst),
        "the engine is not launched without the tools the user turned on"
    );
    fixture.hub.shutdown();
}
