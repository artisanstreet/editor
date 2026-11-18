use super::*;
use std::time::Duration;

async fn run_steering(turn: Turn, steers: &[&str]) -> (crate::Outcome, Vec<EngineEvent>) {
    let (inputs, inbox) = mpsc::channel(8);
    let (events, mut received) = mpsc::channel(1024);
    let io = RunIo {
        inbox,
        events,
        cancel: CancellationToken::new(),
    };
    let mut steers = steers.iter();
    let mut out = Vec::new();
    let driven = async {
        let harness = crate::codex::CodexHarness.run(turn, io);
        tokio::pin!(harness);
        loop {
            tokio::select! {
                outcome = &mut harness => break outcome,
                Some(event) = received.recv() => {
                    if matches!(event, EngineEvent::InputAccepted { .. }) {
                        for text in steers.by_ref() {
                            let steer = UserInput { text: (*text).to_owned(), ..UserInput::default() };
                            inputs.send(Input::Steer(steer)).await.unwrap();
                        }
                    }
                    out.push(event);
                }
            }
        }
    };
    let outcome = tokio::time::timeout(Duration::from_secs(20), driven)
        .await
        .expect("the run ends");
    (outcome, out)
}

async fn until_codex_read(fixture: &Fixture, needle: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.input().contains(needle) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_elapsed| panic!("Codex reads {needle}"));
}

#[tokio::test]
async fn a_codex_server_that_stops_reading_still_delivers_other_threads_replies() {
    let fixture = Fixture::new();
    let other = tokio::spawn(run(fixture.turn("slow", None)));
    until_codex_read(&fixture, "\"text\":\"slow\"").await;
    let flood = "x".repeat(4 * 1024 * 1024);
    let jammed = tokio::spawn({
        let turn = fixture.turn("jam", None);
        async move { run_steering(turn, &[flood.as_str()]).await }
    });
    let (outcome, events) = tokio::time::timeout(Duration::from_secs(10), other)
        .await
        .expect("a stalled write for one thread must not stop replies to another")
        .unwrap();
    assert_eq!(
        outcome.status(),
        artisan_api::RunStatus::Done,
        "{outcome:?}"
    );
    let thread = session(&events);
    assert!(format!("{events:?}").contains(&format!("reply on {thread} to slow")));
    jammed.abort();
}

#[tokio::test]
async fn a_stalled_handoff_does_not_hold_up_codex_threads_of_another_home() {
    let fixture = Fixture::new();
    let stalling = |text: &str, session: Option<&str>| {
        let mut turn = fixture.turn(text, session);
        turn.launch
            .env
            .push(("ARTISAN_TEST_STALL_RELEASE".into(), "1".into()));
        turn
    };
    let (_, events) = run(stalling("hello", None)).await;
    let thread = session(&events);
    let busy = crate::codex::server::attach(&stalling("busy", None))
        .await
        .unwrap();
    let config = fixture.root.join("home/.codex/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "# changed launch\n").unwrap();
    let resumed = stalling("again", Some(&thread));
    let handoff =
        tokio::spawn(async move { crate::codex::server::attach(&resumed).await.map(drop) });
    until_codex_read(&fixture, "thread/unsubscribe").await;
    let mut elsewhere = fixture.turn("hello", None);
    elsewhere.home = fixture.root.join("other-home");
    let attached = tokio::time::timeout(
        Duration::from_secs(10),
        crate::codex::server::attach(&elsewhere),
    )
    .await
    .expect("a thread in another home attaches while the handoff waits");
    assert!(attached.is_ok(), "{:?}", attached.err());
    assert!(
        !handoff.is_finished(),
        "the handoff still waits on its release"
    );
    handoff.abort();
    drop(busy);
}

#[tokio::test]
async fn rejected_steers_start_the_next_turn_in_the_order_they_were_sent() {
    let fixture = Fixture::new();
    let (outcome, _) =
        run_steering(fixture.turn("slow", None), &["first steer", "second steer"]).await;
    assert_eq!(
        outcome.status(),
        artisan_api::RunStatus::Done,
        "{outcome:?}"
    );
    let next_turn = fixture
        .input()
        .lines()
        .rfind(|line| line.contains("\"turn/start\""))
        .unwrap()
        .to_owned();
    let (first, second) = (
        next_turn
            .find("first steer")
            .expect("the first steer is in the next turn"),
        next_turn
            .find("second steer")
            .expect("the second steer is in the next turn"),
    );
    assert!(first < second, "{next_turn}");
}
