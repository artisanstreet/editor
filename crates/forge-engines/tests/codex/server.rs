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
    test_support::wait::eventually(
        &format!("Codex to read {needle}"),
        test_support::wait::LIVENESS,
        || async { fixture.input().contains(needle) },
    )
    .await;
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
    until_codex_read(&fixture, "\"text\":\"jam\"").await;
    fixture.release_slow();
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
    let ((outcome, _), ()) = tokio::join!(
        run_steering(fixture.turn("slow", None), &["first steer", "second steer"]),
        fixture.release_slow_once_read("second steer"),
    );
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

#[tokio::test]
async fn a_turn_whose_reader_falls_far_behind_codex_still_gets_its_whole_reply() {
    let fixture = Fixture::new();
    let (logs, _capturing) = test_support::logs::capture();
    let (_inputs, inbox) = mpsc::channel(8);
    let (events, mut received) = mpsc::channel(1);
    let harness = tokio::spawn(crate::codex::CodexHarness.run(
        fixture.turn("flood", None),
        RunIo {
            inbox,
            events,
            cancel: CancellationToken::new(),
        },
    ));
    let mut streamed = loop {
        let event = received.recv().await.expect("the turn streams its reply");
        if let EngineEvent::Live { text, .. } = &event {
            break text.len();
        }
    };
    let taken_before_the_reader_blocked = 3;
    let unread_limit = 32;
    let in_flight = 2;
    let most_written = taken_before_the_reader_blocked + unread_limit + in_flight;
    test_support::wait::eventually(
        "Forge to stop reading Codex while the run reads nothing",
        test_support::wait::LIVENESS,
        || std::future::ready(logs.text().contains("Forge stopped reading Codex")),
    )
    .await;
    test_support::wait::eventually(
        "Codex to write what Forge read before it stopped",
        test_support::wait::LIVENESS,
        || {
            std::future::ready(
                fixture.flooded_mib() >= taken_before_the_reader_blocked + unread_limit,
            )
        },
    )
    .await;
    let written = fixture.flooded_mib();
    assert!(
        written <= most_written,
        "Codex wrote {written} MiB while the run read none, so Forge buffered it all"
    );
    assert!(
        !logs.text().contains("Forge reads Codex again"),
        "Forge read Codex again although the run had read nothing more"
    );

    let mib = 1024 * 1024;
    while let Some(event) = received.recv().await {
        if let EngineEvent::Live { text, .. } = &event {
            streamed += text.len();
        }
        let written = fixture.flooded_mib();
        let read = streamed / mib;
        assert!(
            written <= most_written + read,
            "Codex wrote {written} MiB while the run had read {read} MiB, so Forge buffered past its limit"
        );
    }
    let outcome = harness.await.unwrap();
    assert_eq!(
        outcome.status(),
        artisan_api::RunStatus::Done,
        "{outcome:?}"
    );
    assert_eq!(streamed, 48 * mib, "every delta reaches the run");
}

#[tokio::test]
async fn a_refused_turn_tells_the_user_why_but_keeps_codexs_words_out_of_the_log() {
    let fixture = Fixture::new();
    let (logs, _capturing) = test_support::logs::capture();

    let (outcome, _) = run_steering(fixture.turn("refuse with a secret", None), &[]).await;

    let failure = outcome.failure().map(|failure| failure.message.clone());
    assert!(
        failure
            .as_deref()
            .is_some_and(|message| message.contains("REFLECTED-SENTINEL")),
        "the user isn't told why Codex refused the turn: {failure:?}"
    );
    let logged = logs.text();
    assert!(
        logged.contains("Codex run failed") && !logged.contains("REFLECTED-SENTINEL"),
        "{logged}"
    );
}
