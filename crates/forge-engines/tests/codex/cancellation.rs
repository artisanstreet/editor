use super::*;

#[tokio::test]
async fn cancellation_during_turn_start_waits_for_its_id_and_interrupts_it() {
    let fixture = Fixture::new();
    let script = FAKE.replace(
        "    turn/start)",
        "    turn/interrupt)\n      printf '{\"id\":%s,\"result\":{}}\\n' \"$id\"\n      printf '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"%s\",\"turn\":{\"id\":\"%s\",\"items\":[],\"status\":\"interrupted\",\"error\":null}}}\\n' \"$thread\" \"$turn\" ;;\n    turn/start)\n      touch \"$ARTISAN_TEST_INPUT.starting\"\n      sleep 0.2",
    );
    std::fs::write(&fixture.binary, script).unwrap();
    let (_, inbox) = mpsc::channel(8);
    let (events, _received) = mpsc::channel(1024);
    let cancel = CancellationToken::new();
    let io = RunIo {
        inbox,
        events,
        cancel: cancel.clone(),
    };
    let harness = super::super::CodexHarness.run(fixture.turn("slow", None), io);
    let running = tokio::spawn(harness);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !fixture.root.join("input.starting").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, artisan_api::RunStatus::Cancelled);
    let input = read(&fixture.root.join("input"));
    let interrupt: serde_json::Value = input
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value["method"] == "turn/interrupt")
        .unwrap();
    assert_eq!(interrupt["params"]["turnId"], "turn-1");
    assert!(
        interrupt["params"]["threadId"]
            .as_str()
            .unwrap()
            .starts_with("thread-")
    );
}

#[tokio::test]
async fn an_unconfirmed_interrupt_fails_and_unsubscribes_instead_of_claiming_cancellation() {
    let fixture = Fixture::new();
    let script = FAKE.replace(
        "        if [ \"$text\" = slow ]; then sleep 1.5; fi",
        "        if [ \"$text\" = slow ]; then sleep 30; fi",
    );
    std::fs::write(&fixture.binary, script).unwrap();
    let (_peer_inputs, peer_inbox) = mpsc::channel(8);
    let (peer_events, _peer_received) = mpsc::channel(1024);
    let peer = tokio::spawn(super::super::CodexHarness.run(
        fixture.turn("ask", None),
        RunIo {
            inbox: peer_inbox,
            events: peer_events,
            cancel: CancellationToken::new(),
        },
    ));
    let (_inputs, inbox) = mpsc::channel(8);
    let (events, mut received) = mpsc::channel(1024);
    let cancel = CancellationToken::new();
    let running = tokio::spawn(super::super::CodexHarness.run(
        fixture.turn("slow", None),
        RunIo {
            inbox,
            events,
            cancel: cancel.clone(),
        },
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(event) = received.recv().await {
            if matches!(event, EngineEvent::InputAccepted { .. }) {
                break;
            }
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, artisan_api::RunStatus::Failed);
    assert!(outcome.error.unwrap().contains("did not confirm"));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !read(&fixture.root.join("input")).contains("thread/unsubscribe") {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!peer.is_finished());
    peer.abort();
    assert!(peer.await.is_err_and(|error| error.is_cancelled()));
}

#[tokio::test]
async fn a_deferred_interrupt_receives_its_full_confirmation_grace() {
    let fixture = Fixture::new();
    let script = FAKE.replace(
        "    turn/start)",
        "    turn/interrupt)\n      sleep 2\n      printf '{\"id\":%s,\"result\":{}}\\n' \"$id\"\n      printf '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"%s\",\"turn\":{\"id\":\"%s\",\"items\":[],\"status\":\"interrupted\",\"error\":null}}}\\n' \"$thread\" \"$turn\" ;;\n    turn/start)\n      touch \"$ARTISAN_TEST_INPUT.starting\"\n      sleep 4",
    );
    let script = script.replace(
        "        if [ \"$text\" = slow ]; then sleep 1.5; fi",
        "        if [ \"$text\" = slow ]; then sleep 30; fi",
    );
    std::fs::write(&fixture.binary, script).unwrap();
    let (_, inbox) = mpsc::channel(8);
    let (events, _received) = mpsc::channel(1024);
    let cancel = CancellationToken::new();
    let io = RunIo {
        inbox,
        events,
        cancel: cancel.clone(),
    };
    let harness = super::super::CodexHarness.run(fixture.turn("slow", None), io);
    let running = tokio::spawn(harness);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !fixture.root.join("input.starting").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, artisan_api::RunStatus::Cancelled);
    let input = read(&fixture.root.join("input"));
    let interrupt: serde_json::Value = input
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value["method"] == "turn/interrupt")
        .unwrap();
    assert_eq!(interrupt["params"]["turnId"], "turn-1");
    assert!(
        interrupt["params"]["threadId"]
            .as_str()
            .unwrap()
            .starts_with("thread-")
    );
}
