//! The shared app-server, driven through the harness against a fake
//! `codex app-server` that serves any number of threads.

use crate::{EngineEvent, Harness, Input, Launch, RunIo, Turn, UserInput};
use artisan_api::{Answer, Decision, EngineConfig, EngineId, Status, ThreadId};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Answers each turn with "reply on <thread> to <text>" after a short delay,
/// so turns of different threads overlap ("slow" waits longer). A turn whose
/// text is "spawn" also starts a child thread that says "child of <thread>".
/// One whose text is "ask" asks to run a command and ends saying whether the
/// first answer it got approved it.
const FAKE: &str = r#"#!/usr/bin/env bash
echo $$ >> "$ARTISAN_TEST_SPAWNS"
threads=0
turns=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$ARTISAN_TEST_INPUT"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  thread=$(printf '%s' "$line" | sed -n 's/.*"threadId":"\([^"]*\)".*/\1/p')
  if [ -z "$method" ] && [ "$id" = 900 ] && [ -z "$answered" ]; then
    answered=1
    case "$line" in *'"accept"'*) word=approved ;; *) word=refused ;; esac
    printf '{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg-ask","text":"ask on %s was %s","phase":"final_answer"},"threadId":"%s","turnId":"%s"}}\n' "$ask_thread" "$word" "$ask_thread" "$ask_turn"
    printf '{"method":"turn/completed","params":{"threadId":"%s","turn":{"id":"%s","items":[],"status":"completed","error":null}}}\n' "$ask_thread" "$ask_turn"
  fi
  case "$method" in
    initialize) printf '{"id":%s,"result":{"userAgent":"fake"}}\n' "$id" ;;
    thread/start)
      threads=$((threads + 1))
      printf '{"id":%s,"result":{"thread":{"id":"thread-%s"},"model":"fake-model"}}\n' "$id" "$threads" ;;
    thread/resume) printf '{"id":%s,"result":{"thread":{"id":"%s"},"model":"fake-model"}}\n' "$id" "$thread" ;;
    turn/start)
      turns=$((turns + 1))
      turn="turn-$turns"
      text=$(printf '%s' "$line" | sed -n 's/.*"text":"\([^"]*\)".*/\1/p')
      printf '{"id":%s,"result":{"turn":{"id":"%s"}}}\n' "$id" "$turn"
      if [ "$text" = ask ]; then
        ask_thread=$thread
        ask_turn=$turn
        (
          sleep 0.5
          printf '{"id":900,"method":"item/commandExecution/requestApproval","params":{"threadId":"%s","turnId":"%s","itemId":"cmd-1","command":"ls"}}\n' "$thread" "$turn"
        ) &
        continue
      fi
      (
        sleep 0.3
        if [ "$text" = slow ]; then sleep 1.5; fi
        if [ "$text" = spawn ]; then
          printf '{"method":"thread/started","params":{"thread":{"id":"child-%s","parentThreadId":"%s","agentNickname":"Helper"}}}\n' "$thread" "$thread"
          printf '{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"child-msg-%s","text":"child of %s","phase":"final_answer"},"threadId":"child-%s","turnId":"child-turn"}}\n' "$thread" "$thread" "$thread"
        fi
        printf '{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg-%s","text":"reply on %s to %s","phase":"final_answer"},"threadId":"%s","turnId":"%s"}}\n' "$turn" "$thread" "$text" "$thread" "$turn"
        printf '{"method":"turn/completed","params":{"threadId":"%s","turn":{"id":"%s","items":[],"status":"completed","error":null}}}\n' "$thread" "$turn"
      ) &
      ;;
  esac
done
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    binary: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        let binary = root.join("codex");
        std::fs::write(&binary, FAKE).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            _dir: dir,
            root,
            binary,
        }
    }

    fn turn(&self, text: &str, session: Option<&str>) -> Turn {
        let path = |name: &str| self.root.join(name).to_string_lossy().into_owned();
        Turn {
            trace_dir: None,
            thread_id: ThreadId::new(),
            engine: EngineId::Codex,
            binary: self.binary.clone(),
            home: self.root.join("home"),
            cwd: self.root.clone(),
            config: EngineConfig::new(EngineId::Codex),
            session_id: session.map(str::to_owned),
            input: UserInput {
                text: text.to_owned(),
                ..UserInput::default()
            },
            launch: Launch {
                env: vec![
                    ("ARTISAN_TEST_SPAWNS".into(), path("spawns")),
                    ("ARTISAN_TEST_INPUT".into(), path("input")),
                ],
                ..Launch::default()
            },
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

/// One run to the end: how it ended and everything it reported.
async fn run(turn: Turn) -> (crate::Outcome, Vec<EngineEvent>) {
    run_approving(turn, None).await
}

/// [`run`], allowing the approval on `approve` once it is asked.
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
            EngineEvent::Model { .. }
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

/// Every Codex thread runs on one app-server that outlives its turns, so
/// what Codex remembers about a thread (like falling back from WebSockets to
/// HTTPS) carries to its next turn. Threads sharing the process only see
/// their own replies and agents, and a later turn's settings reach Codex
/// even though the thread is still loaded with the old ones.
#[tokio::test]
async fn threads_share_one_app_server_across_turns_without_crosstalk() {
    let fixture = Fixture::new();
    let ((spawned, a), (plain, b)) = tokio::join!(
        run(fixture.turn("spawn", None)),
        run(fixture.turn("hello", None)),
    );
    assert_eq!(
        (spawned.status, plain.status),
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
    // The child thread's agent and its words land only in the run that spawned it.
    assert!(a_seen.contains(&format!("child of {a_thread}")), "{a_seen}");
    assert!(a_seen.contains("Agent {"), "{a_seen}");
    assert!(
        !b_seen.contains("child of") && !b_seen.contains("Agent {"),
        "{b_seen}"
    );

    let mut again = fixture.turn("again", Some(&a_thread));
    again.config.model = Some("second-model".into());
    let (outcome, events) = run(again).await;
    assert_eq!(outcome.status, artisan_api::RunStatus::Done);
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

/// Codex reads its MCP servers when it starts, so a changed `config.toml`
/// starts a fresh app-server for the next run.
#[tokio::test]
async fn a_changed_codex_config_starts_a_fresh_app_server() {
    let fixture = Fixture::new();
    let (first, events) = run(fixture.turn("hello", None)).await;
    assert_eq!(first.status, artisan_api::RunStatus::Done);
    let thread = session(&events);
    let config = fixture.root.join("home/.codex/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "[mcp_servers.docs]\nurl = \"https://example.com/mcp\"\n",
    )
    .unwrap();
    let (second, events) = run(fixture.turn("again", Some(&thread))).await;
    assert_eq!(second.status, artisan_api::RunStatus::Done);
    assert!(format!("{events:?}").contains(&format!("reply on {thread} to again")));
    assert_eq!(fixture.spawns(), 2);
}

/// Codex asks the thread that wants to run a command; with two threads on
/// one app-server, the question reaches only that thread's run, and the
/// user's answer is the one Codex gets.
#[tokio::test]
async fn an_approval_reaches_only_the_thread_that_asked() {
    let fixture = Fixture::new();
    let ((asked, a), (other, b)) = tokio::join!(
        run_approving(fixture.turn("ask", None), Some("cmd-1")),
        run(fixture.turn("slow", None)),
    );
    assert_eq!(
        (asked.status, other.status),
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
