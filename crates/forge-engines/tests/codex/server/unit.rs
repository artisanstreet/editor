use crate::{EngineEvent, Harness, Input, RunIo, Turn, UserInput};
use artisan_api::{Answer, Decision, EngineConfig, EngineId, Status, ThreadId};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[path = "../routing.rs"]
mod activity_routing;
#[path = "../server.rs"]
mod actor;
#[path = "../cancellation.rs"]
mod cancellation;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    env: Vec<(String, String)>,
}

impl Fixture {
    fn new() -> Self {
        Self::with(&[])
    }

    fn with(env: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        let path = |name: &str| root.join(name).to_string_lossy().into_owned();
        let env = [
            ("ARTISAN_TEST_SPAWNS".to_owned(), path("spawns")),
            ("ARTISAN_TEST_INPUT".to_owned(), path("input")),
        ]
        .into_iter()
        .chain(
            env.iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        )
        .collect();
        Self {
            _dir: dir,
            root,
            env,
        }
    }

    fn turn(&self, text: &str, session: Option<&str>) -> Turn {
        let mut launch = crate::fake_engine::launch("codex-app-server", &[]);
        launch.env.extend(self.env.iter().cloned());
        Turn {
            thread_id: ThreadId::new(),
            engine: EngineId::Codex,
            binary: crate::fake_engine::binary(),
            home: self.root.join("home"),
            cwd: self.root.clone(),
            config: EngineConfig::new(EngineId::Codex),
            session_id: session.map(str::to_owned),
            input: UserInput {
                text: text.to_owned(),
                ..UserInput::default()
            },
            launch,
            trace_dir: None,
        }
    }

    fn spawns(&self) -> usize {
        read(&self.root.join("spawns")).lines().count()
    }

    fn input(&self) -> String {
        read(&self.root.join("input"))
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

async fn run(turn: Turn) -> (crate::Outcome, Vec<EngineEvent>) {
    run_approving(turn, None).await
}

async fn run_approving(turn: Turn, approve: Option<&str>) -> (crate::Outcome, Vec<EngineEvent>) {
    let (answers, inbox) = mpsc::channel(8);
    let (events, mut received) = mpsc::channel(1024);
    let io = RunIo {
        inbox,
        events,
        cancel: CancellationToken::new(),
    };
    let mut out = Vec::new();
    let driven = async {
        let harness = super::CodexHarness.run(turn, io);
        tokio::pin!(harness);
        loop {
            tokio::select! {
                outcome = &mut harness => break outcome,
                Some(event) = received.recv() => {
                    if let EngineEvent::Item { key, status: Status::AwaitingUser, .. } = &event
                        && Some(key.as_str()) == approve
                    {
                        let answer = Answer::Approval { decision: Decision::AllowOnce };
                        answers.send(Input::Answer { key: key.clone(), answer }).await.unwrap();
                    }
                    out.push(event);
                }
            }
        }
    };
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(20), driven)
        .await
        .expect("the run ends");
    while let Ok(event) = received.try_recv() {
        out.push(event);
    }
    (outcome, out)
}

fn session(events: &[EngineEvent]) -> String {
    events
        .iter()
        .find_map(|event| match event {
            EngineEvent::Session { id } => Some(id.clone()),
            EngineEvent::SessionGone
            | EngineEvent::Model { .. }
            | EngineEvent::Title { .. }
            | EngineEvent::Item { .. }
            | EngineEvent::Image { .. }
            | EngineEvent::Status { .. }
            | EngineEvent::Live { .. }
            | EngineEvent::Usage(_)
            | EngineEvent::Commands(_)
            | EngineEvent::InputAccepted { .. } => None,
        })
        .expect("the run reports its Codex thread")
}

#[tokio::test]
async fn threads_share_one_app_server_across_turns_without_crosstalk() {
    let fixture = Fixture::new();
    let ((spawned, a), (plain, b)) = tokio::join!(
        run(fixture.turn("spawn", None)),
        run(fixture.turn("hello", None)),
    );
    assert_eq!(
        (spawned.status(), plain.status()),
        (artisan_api::RunStatus::Done, artisan_api::RunStatus::Done)
    );
    let (a_thread, b_thread) = (session(&a), session(&b));
    assert_ne!(a_thread, b_thread);
    let (a_seen, b_seen) = (format!("{a:?}"), format!("{b:?}"));
    assert!(
        a_seen.contains(&format!("reply on {a_thread} to spawn")),
        "{a_seen}"
    );
    assert!(
        b_seen.contains(&format!("reply on {b_thread} to hello")),
        "{b_seen}"
    );
    assert!(!a_seen.contains("to hello") && !b_seen.contains("to spawn"));
    assert!(a_seen.contains(&format!("child of {a_thread}")), "{a_seen}");
    assert!(a_seen.contains("Agent {"), "{a_seen}");
    assert!(
        !b_seen.contains("child of") && !b_seen.contains("Agent {"),
        "{b_seen}"
    );

    let mut again = fixture.turn("again", Some(&a_thread));
    again.config.model = Some("second-model".into());
    let (outcome, events) = run(again).await;
    assert_eq!(outcome.status(), artisan_api::RunStatus::Done);
    let seen = format!("{events:?}");
    assert!(
        seen.contains(&format!("reply on {a_thread} to again")),
        "{seen}"
    );
    assert_eq!(fixture.spawns(), 1, "one app-server for every turn");
    let last_turn = fixture
        .input()
        .lines()
        .rfind(|line| line.contains("\"turn/start\""))
        .unwrap()
        .to_owned();
    assert!(
        last_turn.contains("\"model\":\"second-model\""),
        "{last_turn}"
    );
}

#[tokio::test]
async fn a_cached_app_server_that_exits_before_opening_the_session_hands_the_turn_to_a_fresh_one() {
    let fixture = Fixture::new();
    let (first, events) = run(fixture.turn("then exit on resume", None)).await;
    assert_eq!(first.status(), artisan_api::RunStatus::Done, "{first:?}");
    let thread = session(&events);
    let (second, events) = run(fixture.turn("again", Some(&thread))).await;
    assert_eq!(second.status(), artisan_api::RunStatus::Done, "{second:?}");
    let seen = format!("{events:?}");
    assert!(
        seen.contains(&format!("reply on {thread} to again")),
        "{seen}"
    );
    assert_eq!(fixture.spawns(), 2, "the turn moved to a fresh app-server");
}

#[tokio::test]
async fn a_changed_codex_config_starts_a_fresh_app_server() {
    let fixture = Fixture::new();
    let (first, events) = run(fixture.turn("hello", None)).await;
    assert_eq!(first.status(), artisan_api::RunStatus::Done);
    let thread = session(&events);
    let config = fixture.root.join("home/.codex/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "[mcp_servers.docs]\nurl = \"https://example.com/mcp\"\n",
    )
    .unwrap();
    let (second, events) = run(fixture.turn("again", Some(&thread))).await;
    assert_eq!(second.status(), artisan_api::RunStatus::Done);
    assert!(format!("{events:?}").contains(&format!("reply on {thread} to again")));
    assert_eq!(fixture.spawns(), 2);
}

#[tokio::test]
async fn an_approval_reaches_only_the_thread_that_asked() {
    let fixture = Fixture::new();
    let ((asked, a), (other, b)) = tokio::join!(
        run_approving(fixture.turn("ask", None), Some("cmd-1")),
        run(fixture.turn("slow", None)),
    );
    assert_eq!(
        (asked.status(), other.status()),
        (artisan_api::RunStatus::Done, artisan_api::RunStatus::Done)
    );
    let (a_thread, b_thread) = (session(&a), session(&b));
    let (a_seen, b_seen) = (format!("{a:?}"), format!("{b:?}"));
    assert!(
        a_seen.contains(&format!("ask on {a_thread} was approved")),
        "{a_seen}"
    );
    assert!(
        b_seen.contains(&format!("reply on {b_thread} to slow")),
        "{b_seen}"
    );
    assert!(!b_seen.contains("cmd-1"), "{b_seen}");
    let answers = fixture
        .input()
        .lines()
        .filter(|line| line.contains("\"id\":900") && !line.contains("\"method\""))
        .count();
    assert_eq!(answers, 1, "{}", fixture.input());
}

#[tokio::test]
async fn retired_server_releases_a_thread_while_another_run_continues() {
    let fixture = Fixture::new();
    let (_, events) = run(fixture.turn("hello", None)).await;
    let thread = session(&events);
    let other = tokio::spawn(run(fixture.turn("slow", None)));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !fixture
            .input()
            .lines()
            .any(|line| line.contains("\"text\":\"slow\""))
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the other turn starts before retiring its server");
    let config = fixture.root.join("home/.codex/config.toml");
    std::fs::write(&config, "# changed launch\n").unwrap();
    let (outcome, events) = run(fixture.turn("again", Some(&thread))).await;
    assert_eq!(
        outcome.status(),
        artisan_api::RunStatus::Done,
        "{outcome:?}"
    );
    assert!(format!("{events:?}").contains(&format!("reply on {thread} to again")));
    assert_eq!(fixture.spawns(), 2);
    assert!(
        !other.is_finished(),
        "the other turn is still running after handoff"
    );
    let (outcome, _) = other.await.unwrap();
    assert_eq!(
        outcome.status(),
        artisan_api::RunStatus::Done,
        "the other turn completes normally"
    );
}

#[tokio::test]
async fn changing_context_window_releases_the_previous_writer() {
    let fixture = Fixture::new();
    let (_, events) = run(fixture.turn("hello", None)).await;
    let thread = session(&events);
    let other = super::server::attach(&fixture.turn("other", None))
        .await
        .unwrap();
    let mut again = fixture.turn("again", Some(&thread));
    again
        .config
        .options
        .insert("context_window".into(), serde_json::json!(100_000));
    let (outcome, _) = run(again).await;
    assert_eq!(
        outcome.status(),
        artisan_api::RunStatus::Done,
        "{outcome:?}"
    );
    assert_eq!(fixture.spawns(), 2);
    drop(other);
}

#[tokio::test]
async fn quotes_backslashes_and_unicode_reach_codex_and_come_back_intact() {
    let fixture = Fixture::new();
    let said = r#"say "hi" \ to Bjørn ✓"#;
    let (outcome, events) = run(fixture.turn(said, None)).await;
    assert_eq!(
        outcome.status(),
        artisan_api::RunStatus::Done,
        "{:?}",
        outcome.error()
    );
    let thread = session(&events);
    let expected = format!("reply on {thread} to {said}");
    assert!(
        events.iter().any(|event| matches!(
            event,
            EngineEvent::Item { kind: artisan_api::ItemKind::AssistantMessage { text, .. }, .. }
                if text.inline() == Some(expected.as_str())
        )),
        "{events:?}"
    );
}
