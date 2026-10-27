//! Scenarios a client would notice: messages reach the engine, steers are
//! never lost, answers reach the right run, cancel settles everything.

use super::*;
use artisan_api::{
    ApprovalRequest, CreateProject, CreateThread, Decision, EngineConfig, ItemsQuery, Permissions,
    Phase, ProjectId, RunStatus, ToolKind,
};
use forge_engines::{EngineEvent, Outcome, RunIo, Turn};
use futures::future::BoxFuture;
use std::sync::Mutex;
use std::time::Duration;

type Script = Arc<dyn Fn(Turn, RunIo) -> BoxFuture<'static, Outcome> + Send + Sync>;

struct Scripted(Script);

impl Harness for Scripted {
    fn compact(
        &self,
        _turn: Turn,
        _cancel: CancellationToken,
    ) -> BoxFuture<'static, anyhow::Result<forge_engines::SideReply>> {
        Box::pin(async {
            Ok(forge_engines::SideReply {
                text: "## Objective\n- Continue the task.".to_owned(),
                ..Default::default()
            })
        })
    }

    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        let script = std::sync::Arc::clone(&self.0);
        Box::pin(async move {
            io.emit(EngineEvent::InputAccepted {
                id: turn.input.id.clone(),
            })
            .await;
            // Real engines name their session; the next run resumes it.
            io.emit(EngineEvent::Session {
                id: "session".to_owned(),
            })
            .await;
            script(turn, io).await
        })
    }
}

/// A scripted engine that reports only what its script says, delivery
/// included.
struct Unscripted(Script);

impl Harness for Unscripted {
    fn compact(
        &self,
        _turn: Turn,
        _cancel: CancellationToken,
    ) -> BoxFuture<'static, anyhow::Result<forge_engines::SideReply>> {
        Box::pin(async {
            Ok(forge_engines::SideReply {
                text: "## Objective\n- Continue the task.".to_owned(),
                ..Default::default()
            })
        })
    }

    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        (self.0)(turn, io)
    }
}

struct Fixture {
    hub: Hub,
    thread_id: ThreadId,
    dir: tempfile::TempDir,
}

async fn fixture(script: Script) -> Fixture {
    let harness: Arc<dyn Harness> = Arc::new(Scripted(script));
    fixture_with_harness(Arc::new(move |_| Arc::clone(&harness))).await
}

async fn fixture_with_harness(harness_for: HarnessFor) -> Fixture {
    fixture_with_engine_harness(EngineId::Claude, harness_for).await
}

async fn fixture_with_engine_harness(engine: EngineId, harness_for: HarnessFor) -> Fixture {
    fixture_with_parts(engine, harness_for, disk::Probe::system()).await
}

async fn fixture_with_parts(
    engine: EngineId,
    harness_for: HarnessFor,
    disk: disk::Probe,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_path_buf();
    install_fake(&data, engine);
    let store = forge_store::Store::open(&data.join("forge")).unwrap();
    let engines = Engines::new(&data);
    let hub = Hub::build(store, engines, data.clone(), harness_for, disk);
    let project = hub
        .store()
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: data.display().to_string(),
            name: None,
        })
        .await
        .unwrap();
    let config = EngineConfig::new(engine);
    let thread = hub
        .store()
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config,
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    Fixture {
        hub,
        thread_id: thread.id,
        dir,
    }
}

/// A fake installed engine so a run gets past "not installed".
fn install_fake(data: &std::path::Path, engine: EngineId) {
    let engine_dir = data.join("engines").join(engine.to_string());
    let version = if engine == EngineId::OpenCode {
        "2.1.0"
    } else {
        "1.0.0"
    };
    let version_dir = engine_dir.join("versions").join(version);
    let entry = if engine == EngineId::Codex {
        let architecture = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };
        let platform = if cfg!(target_os = "windows") {
            "pc-windows-msvc"
        } else if cfg!(target_os = "macos") {
            "apple-darwin"
        } else {
            "unknown-linux-musl"
        };
        let executable = if cfg!(target_os = "windows") {
            "codex.exe"
        } else {
            "codex"
        };
        version_dir.join(format!("vendor/{architecture}-{platform}/bin/{executable}"))
    } else if engine == EngineId::OpenCode {
        version_dir.join(if cfg!(windows) {
            "bin/opencode.exe"
        } else {
            "bin/opencode"
        })
    } else {
        version_dir.join(engine.to_string())
    };
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(entry, "").unwrap();
    std::fs::write(engine_dir.join("current"), version).unwrap();
}

fn message(text: &str) -> SendMessage {
    SendMessage {
        draft: None,
        id: ItemId::new(),
        text: text.to_owned(),
        images: Vec::new(),
        config: None,
    }
}

fn reply(key: &str, text: &str) -> EngineEvent {
    EngineEvent::Item {
        key: key.to_owned(),
        parent: None,
        status: Status::Done,
        kind: ItemKind::AssistantMessage {
            text: (text.to_owned()).into(),
            phase: Phase::Final,
        },
    }
}

impl Fixture {
    /// The item the engine knows as `key`.
    async fn by_native(&self, key: &str) -> Item {
        self.hub
            .store()
            .item_by_native(&self.thread_id, None, key)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("no item {key}"))
    }

    async fn items(&self) -> Vec<Item> {
        let query = ItemsQuery {
            before: None,
            after: None,
            limit: Some(500),
        };
        self.hub
            .store()
            .items_page(&self.thread_id, query)
            .await
            .unwrap()
            .items
    }

    async fn settled(&self) -> Vec<artisan_api::Run> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if !self.hub.is_live(&self.thread_id)
                    && !self
                        .hub
                        .store()
                        .pending_threads(8, Vec::new())
                        .await
                        .unwrap()
                        .contains(&self.thread_id)
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("run settles");
        let state = self.hub.store().state().await.unwrap();
        state
            .runs
            .into_iter()
            .filter(|run| run.thread_id == self.thread_id)
            .collect()
    }

    async fn wait_for(&self, check: impl Fn(&[Item]) -> bool + Send + Sync) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if check(&self.items().await) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("condition reached");
    }
}

/// User messages the thread shows as sent but not yet taken by an engine.
async fn undelivered(store: &forge_store::Store, thread: &ThreadId) -> Vec<Item> {
    let query = ItemsQuery {
        before: None,
        after: None,
        limit: Some(500),
    };
    let page = store.items_page(thread, query).await.unwrap();
    page.items
        .into_iter()
        .filter(|item| {
            item.status == Status::Running && matches!(item.kind, ItemKind::UserMessage { .. })
        })
        .collect()
}

fn texts(items: &[Item]) -> Vec<(String, Status)> {
    items
        .iter()
        .filter_map(|item| item.preview().map(|text| (text.to_owned(), item.status)))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_runs_and_the_reply_is_stored() {
    let fixture = fixture(Arc::new(|turn, io| {
        Box::pin(async move {
            io.emit(EngineEvent::Session {
                id: "session-1".into(),
            })
            .await;
            io.emit(reply("m1", &format!("you said {}", turn.input.text)))
                .await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("Fix the login bug\nplease"))
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Done);
    assert_eq!(runs[0].session_id.as_deref(), Some("session-1"));
    assert_eq!(
        texts(&fixture.items().await),
        vec![
            ("Fix the login bug\nplease".to_owned(), Status::Done),
            (
                "you said Fix the login bug\nplease".to_owned(),
                Status::Done
            )
        ]
    );
    let thread = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(thread.title.as_deref(), Some("Fix the login bug"));
}

/// Keeps a claude.ai session in the fixture's Claude home, as signing in
/// does.
fn sign_in_to_claude_ai(fixture: &Fixture) {
    let session = forge_engines::claudeai::Session::new(
        artisan_api::ClaudeAiSession {
            cookies: vec![("sessionKey".to_owned(), "key".to_owned())],
            user_agent: "agent".to_owned(),
        },
        "org".to_owned(),
        None,
    );
    session
        .save(&fixture.hub.engines().home(artisan_api::EngineId::Claude))
        .unwrap();
}

async fn turn_on_enhanced_claude(fixture: &Fixture) {
    fixture
        .hub
        .store()
        .set_setting(
            artisan_api::TRANSPORT_KEY.to_owned(),
            serde_json::json!(artisan_api::CLAUDE_AI),
        )
        .await
        .unwrap();
}

fn transport_of(config: &artisan_api::EngineConfig) -> Option<&str> {
    config
        .options
        .get("transport")
        .and_then(serde_json::Value::as_str)
}

/// The transport Claude's turns run on belongs to the account, not the
/// thread: a Forge setting names it, and every Claude thread follows it.
#[tokio::test(flavor = "multi_thread")]
async fn the_claude_transport_setting_reaches_every_claude_turn() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let asked = seen.clone();
    let fixture = fixture(Arc::new(move |turn, _io| {
        let asked = asked.clone();
        Box::pin(async move {
            asked.lock().unwrap().push(turn.config.clone());
            Outcome::done()
        })
    }))
    .await;
    turn_on_enhanced_claude(&fixture).await;
    sign_in_to_claude_ai(&fixture);
    fixture
        .hub
        .send_message(&fixture.thread_id, message("hello"))
        .await
        .unwrap();
    fixture.settled().await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        transport_of(&seen[0]),
        Some(forge_engines::CLAUDE_AI_TRANSPORT),
        "the setting names the transport for the turn"
    );
}

/// With the mode on and nothing signed in, the turn runs on Claude Code
/// and says why, rather than failing or switching silently.
#[tokio::test(flavor = "multi_thread")]
async fn enhanced_claude_without_a_session_runs_on_claude_code_and_says_so() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let asked = seen.clone();
    let fixture = fixture(Arc::new(move |turn, _io| {
        let asked = asked.clone();
        Box::pin(async move {
            asked.lock().unwrap().push(turn.config.clone());
            Outcome::done()
        })
    }))
    .await;
    turn_on_enhanced_claude(&fixture).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("hello"))
        .await
        .unwrap();
    fixture.settled().await;
    let seen = seen.lock().unwrap();
    assert_eq!(
        transport_of(&seen[0]),
        Some(forge_engines::CLAUDE_AI_UNAVAILABLE)
    );
}

/// Turning the mode on moves a Claude Code thread to claude.ai: its
/// session can't carry on there, so the next turn starts a fresh one from
/// the thread's history.
#[tokio::test(flavor = "multi_thread")]
async fn turning_enhanced_claude_on_moves_a_thread_to_claude_ai() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let asked = seen.clone();
    let fixture = fixture(Arc::new(move |turn, io| {
        let asked = asked.clone();
        Box::pin(async move {
            asked
                .lock()
                .unwrap()
                .push((turn.config.clone(), turn.session_id.clone()));
            io.emit(EngineEvent::InputAccepted {
                id: turn.input.id.clone(),
            })
            .await;
            io.emit(EngineEvent::Session {
                id: "b301e08e-3ad7-47b8".into(),
            })
            .await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("first"))
        .await
        .unwrap();
    fixture.settled().await;
    turn_on_enhanced_claude(&fixture).await;
    sign_in_to_claude_ai(&fixture);
    fixture
        .hub
        .send_message(&fixture.thread_id, message("second"))
        .await
        .unwrap();
    fixture.settled().await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(transport_of(&seen[0].0), None);
    assert_eq!(
        transport_of(&seen[1].0),
        Some(forge_engines::CLAUDE_AI_TRANSPORT)
    );
    assert_eq!(
        seen[1].1, None,
        "a Claude Code session does not carry on on claude.ai"
    );
}

/// A claude.ai conversation claude.ai deleted is not resumed: the engine
/// clears the session and the message runs again on a fresh one.
#[tokio::test(flavor = "multi_thread")]
async fn a_cleared_session_runs_the_message_again_on_a_fresh_one() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let asked = seen.clone();
    // This engine says itself what it delivered, as the real ones do.
    let harness: Arc<dyn Harness> = Arc::new(Unscripted(Arc::new(move |turn, io| {
        let asked = asked.clone();
        Box::pin(async move {
            let first = {
                let mut asked = asked.lock().unwrap();
                asked.push(turn.session_id.clone());
                asked.len() == 1
            };
            if first {
                io.emit(EngineEvent::InputAccepted {
                    id: turn.input.id.clone(),
                })
                .await;
                io.emit(EngineEvent::Session {
                    id: "claude.ai:conv-1".into(),
                })
                .await;
            } else if turn.session_id.is_some() {
                // The conversation is gone: the message is not delivered.
                io.emit(EngineEvent::Session { id: String::new() }).await;
            } else {
                io.emit(EngineEvent::InputAccepted {
                    id: turn.input.id.clone(),
                })
                .await;
                io.emit(EngineEvent::Session {
                    id: "claude.ai:conv-2".into(),
                })
                .await;
            }
            Outcome::done()
        })
    })));
    let fixture = fixture_with_harness(Arc::new(move |_| Arc::clone(&harness))).await;
    turn_on_enhanced_claude(&fixture).await;
    sign_in_to_claude_ai(&fixture);
    for text in ["first", "second"] {
        fixture
            .hub
            .send_message(&fixture.thread_id, message(text))
            .await
            .unwrap();
        fixture.settled().await;
    }
    let seen = seen.lock().unwrap();
    assert_eq!(
        *seen,
        vec![None, Some("claude.ai:conv-1".to_owned()), None],
        "the message ran again, on a fresh session"
    );
}

/// Without the setting, a Claude turn runs the official way.
#[tokio::test(flavor = "multi_thread")]
async fn a_claude_turn_without_the_transport_setting_runs_claude_code() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let asked = seen.clone();
    let fixture = fixture(Arc::new(move |turn, _io| {
        let asked = asked.clone();
        Box::pin(async move {
            asked.lock().unwrap().push(turn.config.clone());
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("hello"))
        .await
        .unwrap();
    fixture.settled().await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].options.get("transport").is_none(),
        "no setting, no transport: the official way"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_the_engine_saved_is_shown_from_a_blob() {
    let png = tempfile::NamedTempFile::with_suffix(".png").unwrap();
    image::RgbImage::from_pixel(4, 4, image::Rgb([200, 40, 40]))
        .save(png.path())
        .unwrap();
    let path = png.path().display().to_string();
    let fixture = fixture(Arc::new(move |_, io| {
        let path = path.clone();
        Box::pin(async move {
            io.emit(EngineEvent::Image {
                key: "ig1".into(),
                parent: None,
                path: path.into(),
                caption: None,
            })
            .await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("draw a red square"))
        .await
        .unwrap();
    fixture.settled().await;
    let items = fixture.items().await;
    let Some(ItemKind::Image { blob, caption }) = items
        .iter()
        .map(|item| &item.kind)
        .find(|kind| matches!(kind, ItemKind::Image { .. }))
    else {
        panic!("no image item in {items:?}")
    };
    assert_eq!(caption, &None);
    let stored = std::fs::read(
        fixture
            .hub
            .store()
            .blob_path(blob)
            .expect("the image is a stored blob"),
    )
    .unwrap();
    assert_eq!(&stored[8..12], b"WEBP");
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_is_told_which_skill_the_user_mentioned() {
    let fixture = fixture(Arc::new(|turn, io| {
        Box::pin(async move {
            io.emit(reply("m1", &turn.input.text)).await;
            Outcome::done()
        })
    }))
    .await;
    write_skill(&fixture.hub.skills_dir(), "review", "Reviews the diff");
    write_skill(&fixture.hub.skills_dir(), "release", "Ships it");
    fixture
        .hub
        .set_skill_enabled("local.release", false)
        .await
        .unwrap();
    let typed = "$review this with $artisan-editor, not $release or $5";
    fixture
        .hub
        .send_message(&fixture.thread_id, message(typed))
        .await
        .unwrap();
    fixture.settled().await;
    assert_eq!(
        texts(&fixture.items().await),
        vec![
            (typed.to_owned(), Status::Done),
            (
                format!(
                    "{typed}\n\n(The user asked for the skills `review`, `artisan-editor`: load and follow it for this request.)"
                ),
                Status::Done
            ),
        ],
        "the thread keeps what the user typed; Claude reads which skills to load: \
         Artisan's own counts, one the user turned off doesn't"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_steer_reaches_the_live_run() {
    let fixture = fixture(Arc::new(|_turn, mut io| {
        Box::pin(async move {
            io.emit(EngineEvent::Live {
                target: forge_engines::LiveKind::Reasoning,
                text: "thinking".into(),
                append: true,
            })
            .await;
            let Some(Input::Steer(steer)) = io.inbox.recv().await else {
                return Outcome::failed("no steer");
            };
            io.emit(EngineEvent::InputAccepted { id: steer.id }).await;
            io.emit(reply("m1", &format!("ok: {}", steer.text))).await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("start"))
        .await
        .unwrap();
    while !fixture
        .hub
        .live_snapshot()
        .1
        .iter()
        .any(|event| matches!(event, Event::Live { .. }))
    {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let steer = fixture
        .hub
        .send_message(&fixture.thread_id, message("also add tests"))
        .await
        .unwrap();
    assert!(matches!(
        steer.kind,
        ItemKind::UserMessage { steer: true, .. }
    ));
    let runs = fixture.settled().await;
    assert_eq!(runs.len(), 1, "the steer joined the live run");
    let items = texts(&fixture.items().await);
    assert!(items.contains(&("also add tests".to_owned(), Status::Done)));
    assert!(items.contains(&("ok: also add tests".to_owned(), Status::Done)));
    assert!(
        fixture.hub.live_snapshot().1.is_empty(),
        "reasoning is dropped when the run ends"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_thinking_line_ends_when_the_reply_starts() {
    let fixture = fixture(Arc::new(|_turn, mut io| {
        Box::pin(async move {
            io.emit(EngineEvent::Live {
                target: forge_engines::LiveKind::Reasoning,
                text: "Checking why the build has no ae binary".into(),
                append: true,
            })
            .await;
            io.emit(EngineEvent::Item {
                key: "m1".into(),
                parent: None,
                status: Status::Running,
                kind: ItemKind::AssistantMessage {
                    text: (String::new()).into(),
                    phase: Phase::Commentary,
                },
            })
            .await;
            io.emit(EngineEvent::Live {
                target: forge_engines::LiveKind::Text { key: "m1".into() },
                text: "mold and".into(),
                append: true,
            })
            .await;
            // Stays live, mid-reply, until the test has looked.
            let Some(Input::Steer(steer)) = io.inbox.recv().await else {
                return Outcome::failed("no steer");
            };
            io.emit(EngineEvent::InputAccepted { id: steer.id }).await;
            io.emit(reply("m1", "mold and lld")).await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("which linker?"))
        .await
        .unwrap();
    let live = |target: fn(&LiveTarget) -> bool| {
        fixture.hub.live_snapshot().1.into_iter().find_map(|event| {
            if let Event::Live {
                target: t, text, ..
            } = event
                && target(&t)
            {
                Some(text)
            } else {
                None
            }
        })
    };
    while live(|t| matches!(t, LiveTarget::Text { .. })).is_none() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        live(|t| matches!(t, LiveTarget::Reasoning { .. })).as_deref(),
        Some(""),
        "the reply streams, so the reasoning before it no longer shows"
    );
    fixture
        .hub
        .send_message(&fixture.thread_id, message("thanks"))
        .await
        .unwrap();
    fixture.settled().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_the_ending_run_never_took_starts_the_next_run() {
    let fixture = fixture(Arc::new(|turn, _io| {
        Box::pin(async move {
            if turn.input.text == "first" {
                // Busy until the second message is stored, never reads the inbox.
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("first"))
        .await
        .unwrap();
    // The first run has taken "first" and is busy.
    fixture
        .wait_for(|items| texts(items).contains(&("first".to_owned(), Status::Done)))
        .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("second"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| texts(items).contains(&("second".to_owned(), Status::Done)))
        .await;
    fixture.settled().await;
    let items = fixture.items().await;
    assert_ne!(
        items[0].run_id, items[1].run_id,
        "the second message got its own run"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_approval_answer_reaches_the_run() {
    let fixture = fixture(Arc::new(|_turn, mut io| {
        Box::pin(async move {
            let tool = |decision| ItemKind::Tool {
                tool: ToolKind::Shell {
                    command: "rm -rf build".into(),
                    cwd: None,
                    background: false,
                    watch: false,
                },
                title: None,
                output: None,
                exit_code: None,
                approval: Some(artisan_api::Approval {
                    request: ApprovalRequest::Command {
                        command: "rm -rf build".into(),
                        cwd: None,
                    },
                    reason: None,
                    options: vec![Decision::AllowOnce, Decision::DenyOnce],
                    decision,
                }),
            };
            io.emit(EngineEvent::Item {
                key: "t1".into(),
                parent: None,
                status: Status::AwaitingUser,
                kind: tool(None),
            })
            .await;
            let Some(Input::Answer { key, answer }) = io.inbox.recv().await else {
                return Outcome::failed("no answer");
            };
            assert_eq!(key, "t1");
            let artisan_api::Answer::Approval { decision } = answer else {
                return Outcome::failed("wrong answer");
            };
            io.emit(EngineEvent::Item {
                key,
                parent: None,
                status: Status::Declined,
                kind: tool(Some(decision)),
            })
            .await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("clean up"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| items.iter().any(|item| item.status == Status::AwaitingUser))
        .await;
    let waiting = fixture
        .items()
        .await
        .into_iter()
        .find(|item| item.status == Status::AwaitingUser)
        .unwrap();
    fixture
        .hub
        .answer(
            &waiting.id,
            artisan_api::Answer::Approval {
                decision: Decision::DenyOnce,
            },
        )
        .await
        .unwrap();
    fixture.settled().await;
    let tool = fixture
        .hub
        .store()
        .item(&waiting.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tool.status, Status::Declined);
}

/// Switching a thread to Full mid-run tells the engine, approves what
/// already waits, and approves what it asks afterwards: nothing waits on
/// the user once they chose Full.
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn switching_to_full_access_mid_run_approves_what_waits() {
    let fixture = fixture(Arc::new(|_turn, mut io| {
        Box::pin(async move {
            let tool = |decision| ItemKind::Tool {
                tool: ToolKind::Shell {
                    command: "ls /mnt/c".into(),
                    cwd: None,
                    background: false,
                    watch: false,
                },
                title: None,
                output: None,
                exit_code: None,
                approval: Some(artisan_api::Approval {
                    request: ApprovalRequest::Command {
                        command: "ls /mnt/c".into(),
                        cwd: None,
                    },
                    reason: None,
                    options: vec![Decision::AllowOnce, Decision::DenyOnce],
                    decision,
                }),
            };
            let mut heard = Vec::new();
            for key in ["t1", "t2"] {
                io.emit(EngineEvent::Item {
                    key: key.into(),
                    parent: None,
                    status: Status::AwaitingUser,
                    kind: tool(None),
                })
                .await;
                loop {
                    match io.inbox.recv().await {
                        Some(Input::Answer {
                            key: answered,
                            answer: artisan_api::Answer::Approval { decision },
                        }) if answered == key => {
                            heard.push(format!("{key} {decision:?}"));
                            io.emit(EngineEvent::Item {
                                key: key.into(),
                                parent: None,
                                status: Status::Done,
                                kind: tool(Some(decision)),
                            })
                            .await;
                            break;
                        }
                        Some(Input::SetPermissions(permissions)) => {
                            heard.push(format!("{permissions:?}"));
                        }
                        other => return Outcome::failed(format!("unexpected {other:?}")),
                    }
                }
            }
            if heard != ["Full", "t1 AllowOnce", "t2 AllowOnce"] {
                return Outcome::failed(format!("heard {heard:?}"));
            }
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("look around"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| items.iter().any(|item| item.status == Status::AwaitingUser))
        .await;
    let mut config = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap()
        .config;
    assert_eq!(config.permissions, artisan_api::Permissions::Auto);
    config.permissions = artisan_api::Permissions::Full;
    fixture
        .hub
        .update_thread(
            &fixture.thread_id,
            artisan_api::UpdateThread {
                config: Some(config),
                ..artisan_api::UpdateThread::default()
            },
        )
        .await
        .unwrap();
    fixture.settled().await;
    let run = fixture
        .hub
        .store()
        .latest_run(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, artisan_api::RunStatus::Done, "{:?}", run.error);
    assert_eq!(run.config.as_ref().unwrap().permissions, Permissions::Full);
    let statuses: Vec<_> = fixture
        .items()
        .await
        .into_iter()
        .filter(|item| matches!(item.kind, ItemKind::Tool { .. }))
        .map(|item| item.status)
        .collect();
    assert_eq!(statuses, [Status::Done, Status::Done]);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_ends_the_run_and_settles_open_items() {
    let fixture = fixture(Arc::new(|_turn, io| {
        Box::pin(async move {
            io.emit(EngineEvent::Item {
                key: "t1".into(),
                parent: None,
                status: Status::Running,
                kind: ItemKind::Tool {
                    tool: ToolKind::Shell {
                        command: "sleep 100".into(),
                        cwd: None,
                        background: false,
                        watch: false,
                    },
                    title: None,
                    output: None,
                    exit_code: None,
                    approval: None,
                },
            })
            .await;
            io.cancel.cancelled().await;
            Outcome::cancelled()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("wait"))
        .await
        .unwrap();
    fixture.wait_for(|items| items.len() == 2).await;
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status, RunStatus::Cancelled);
    let items = fixture.items().await;
    assert_eq!(items[1].status, Status::Cancelled);
}

/// A skill folder in `dir`.
fn write_skill(dir: &std::path::Path, name: &str, description: &str) {
    std::fs::create_dir_all(dir.join(name)).unwrap();
    std::fs::write(
        dir.join(name).join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nDo it well.\n"),
    )
    .unwrap();
}

/// What Claude's skills folder holds: `name -> target` for links.
fn claude_skills(hub: &Hub) -> Vec<String> {
    let dir = hub.engines().home(EngineId::Claude).join(".claude/skills");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            std::fs::read_link(entry.path()).map_or_else(
                |_| format!("{name}/"),
                |target| {
                    format!(
                        "{name} -> {}",
                        target.file_name().unwrap().to_string_lossy()
                    )
                },
            )
        })
        .collect();
    names.sort();
    names
}

fn claude_status(
    catalog: &[artisan_api::CatalogSkill],
    id: &str,
) -> artisan_api::SkillEngineStatus {
    let skill = catalog
        .iter()
        .find(|skill| skill.id == id)
        .unwrap_or_else(|| panic!("{id} in the catalog: {catalog:#?}"));
    skill
        .engines
        .iter()
        .find(|state| state.engine == EngineId::Claude)
        .unwrap()
        .status
}

#[tokio::test(flavor = "multi_thread")]
async fn a_skill_turned_off_never_reaches_the_engine() {
    let fixture = fixture(Arc::new(|_turn, io| {
        Box::pin(async move {
            io.emit(reply("m1", "done")).await;
            Outcome::done()
        })
    }))
    .await;
    let hub = &fixture.hub;
    write_skill(&hub.skills_dir(), "review", "Reviews the diff.");
    write_skill(&hub.skills_dir(), "deploy", "Ships it.");

    let catalog = hub.set_skill_enabled("local.review", false).await.unwrap();
    assert_eq!(
        hub.store().setting(DISABLED_SKILLS_KEY).await.unwrap(),
        Some(serde_json::json!(["local.review"])),
        "Forge remembers the skill is off"
    );
    assert_eq!(
        claude_status(&catalog, "local.review"),
        artisan_api::SkillEngineStatus::Off
    );
    assert_eq!(
        claude_status(&catalog, "local.deploy"),
        artisan_api::SkillEngineStatus::Synced
    );
    assert_eq!(
        claude_status(&catalog, "artisan.artisan-editor"),
        artisan_api::SkillEngineStatus::Synced,
        "Artisan's own skill is in every engine"
    );

    hub.send_message(&fixture.thread_id, message("go"))
        .await
        .unwrap();
    fixture.settled().await;
    assert_eq!(
        claude_skills(hub),
        ["artisan-editor -> artisan-editor", "deploy -> deploy"],
        "the run's engine has every skill but the one turned off"
    );

    hub.set_skill_enabled("local.review", true).await.unwrap();
    assert_eq!(
        claude_skills(hub),
        [
            "artisan-editor -> artisan-editor",
            "deploy -> deploy",
            "review -> review"
        ],
        "turning it back on puts it back"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clash_with_an_engines_own_skill_names_the_engine_as_the_ui_does() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let hub = &fixture.hub;
    write_skill(&hub.skills_dir(), "review", "Reviews the diff.");
    let claude_skills_dir = hub.engines().home(EngineId::Claude).join(".claude/skills");
    write_skill(&claude_skills_dir, "review", "Claude's own review.");

    let catalog = hub.skill_catalog().await.unwrap();
    let review = catalog
        .iter()
        .find(|skill| skill.id == "local.review")
        .expect("Forge's skill is in the catalog");
    let claude = review
        .engines
        .iter()
        .find(|state| state.engine == EngineId::Claude)
        .unwrap();
    assert_eq!(claude.status, artisan_api::SkillEngineStatus::Conflict);
    assert_eq!(
        claude.reason.as_deref(),
        Some("Claude Code has its own skill named review. Remove it to let Artisan manage it."),
        "the Marketplace shows the engine by the name it uses everywhere else"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn adopting_a_skill_found_in_an_engine_copies_it_into_forge() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let hub = &fixture.hub;
    let claude_skills_dir = hub.engines().home(EngineId::Claude).join(".claude/skills");
    write_skill(&claude_skills_dir, "pdf", "Fills PDF forms.");

    let catalog = hub.skill_catalog().await.unwrap();
    let found = catalog
        .iter()
        .find(|skill| skill.origin == artisan_api::SkillOrigin::Discovered)
        .expect("the engine's own skill is listed as found");
    assert_eq!(
        (
            found.name.as_str(),
            found.description.as_str(),
            found.enabled
        ),
        ("pdf", "Fills PDF forms.", false)
    );
    assert_eq!(
        claude_status(&catalog, &found.id),
        artisan_api::SkillEngineStatus::Native
    );

    let catalog = hub.adopt_skill(&found.id).await.unwrap();
    assert!(
        hub.skills_dir().join("pdf/SKILL.md").is_file(),
        "the skill is copied into Forge's skills folder"
    );
    assert_eq!(
        claude_status(&catalog, "local.pdf"),
        artisan_api::SkillEngineStatus::Synced,
        "Forge's copy now sits in the engine"
    );
    assert!(
        catalog
            .iter()
            .all(|skill| skill.origin != artisan_api::SkillOrigin::Discovered),
        "nothing is left unmanaged"
    );
    assert!(claude_skills(hub).contains(&"pdf -> pdf".to_owned()));
    let backups: Vec<_> = std::fs::read_dir(hub.data_dir().join("extensions/backups"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(backups.len(), 1, "the engine's copy is kept as a backup");

    let again = hub.adopt_skill(&found.id).await.unwrap_err();
    assert!(
        again.downcast_ref::<Refused>().is_some(),
        "adopting twice is refused: {again:#}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_edited_outside_artisan_is_reported_and_left_alone() {
    let fixture = fixture(Arc::new(|_turn, io| {
        Box::pin(async move {
            io.emit(reply("m1", "done")).await;
            Outcome::done()
        })
    }))
    .await;
    let hub = &fixture.hub;
    let file = hub
        .engines()
        .home(EngineId::Claude)
        .join(".claude/CLAUDE.md");

    let saved = hub
        .save_global_prompt(artisan_api::SavePrompt {
            base_version: 0,
            text: "Be brief.".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(saved.version, 1, "the first save is version 1");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Be brief.");
    let claude = |prompt: &artisan_api::GlobalPrompt| {
        prompt
            .engines
            .iter()
            .find(|state| state.engine == EngineId::Claude)
            .unwrap()
            .clone()
    };
    assert_eq!(
        claude(&saved).status,
        artisan_api::PromptEngineStatus::Synced
    );

    std::fs::write(&file, "My own rules.").unwrap();
    let prompt = hub.global_prompt().await.unwrap();
    let state = claude(&prompt);
    assert_eq!(state.status, artisan_api::PromptEngineStatus::Drifted);
    assert_eq!(state.external_text.as_deref(), Some("My own rules."));
    assert_eq!(state.file.as_deref(), Some(".claude/CLAUDE.md"));

    hub.send_message(&fixture.thread_id, message("go"))
        .await
        .unwrap();
    fixture.settled().await;
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "My own rules.",
        "a run doesn't overwrite the user's edit"
    );

    let refusal = hub
        .save_global_prompt(artisan_api::SavePrompt {
            base_version: 0,
            text: "Old base.".to_owned(),
        })
        .await
        .unwrap_err();
    assert!(
        refusal.downcast_ref::<Refused>().is_some(),
        "a save from an old version is refused: {refusal:#}"
    );

    let used = hub
        .resolve_global_prompt(artisan_api::ResolvePrompt::UseEngine {
            engine: EngineId::Claude,
        })
        .await
        .unwrap();
    assert_eq!(
        (used.version, used.text.as_str()),
        (2, "My own rules."),
        "the engine's edit becomes the next version"
    );
    assert_eq!(
        claude(&used).status,
        artisan_api::PromptEngineStatus::Synced
    );

    std::fs::write(&file, "Edited again.").unwrap();
    let restored = hub
        .resolve_global_prompt(artisan_api::ResolvePrompt::RestoreEngine {
            engine: EngineId::Claude,
        })
        .await
        .unwrap();
    assert_eq!(
        claude(&restored).status,
        artisan_api::PromptEngineStatus::Synced
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "My own rules.");
}

/// An agent cut off with its run and resumed in the next process reports
/// there: the report lands on the agent the user already sees, with the
/// task it was given, not on a second agent.
#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_agents_report_reaches_its_first_item() {
    let fixture = fixture(Arc::new(|turn, io| {
        Box::pin(async move {
            let first = turn.input.text == "start";
            io.emit(EngineEvent::Item {
                key: "toolu_agent".into(),
                parent: None,
                status: if first { Status::Running } else { Status::Done },
                kind: ItemKind::Agent {
                    engine: None,
                    thread: None,
                    name: None,
                    description: "Attachments drawn on guns".into(),
                    prompt: first.then(|| "Draw attachments on every gun.".into()),
                    agent_id: None,
                    context_used: None,
                    model: None,
                    background: true,
                    result: (!first).then(|| "Attachments now show on the gun.".into()),
                    label: None,
                },
            })
            .await;
            if first {
                Outcome::cancelled()
            } else {
                Outcome::done()
            }
        })
    }))
    .await;
    for text in ["start", "carry on"] {
        fixture
            .hub
            .send_message(&fixture.thread_id, message(text))
            .await
            .unwrap();
        fixture.settled().await;
    }
    let agents: Vec<Item> = fixture
        .items()
        .await
        .into_iter()
        .filter(|item| matches!(item.kind, ItemKind::Agent { .. }))
        .collect();
    assert_eq!(agents.len(), 1, "one agent, not a second one: {agents:?}");
    let ItemKind::Agent { prompt, result, .. } = &agents[0].kind else {
        unreachable!()
    };
    assert_eq!(agents[0].status, Status::Done);
    assert_eq!(
        result.as_ref().and_then(artisan_api::TextContent::inline),
        Some("Attachments now show on the gun.")
    );
    assert_eq!(
        prompt.as_ref().and_then(artisan_api::TextContent::inline),
        Some("Draw attachments on every gun.")
    );
}

/// A subagent stopped with its run and resumed in the next one (Claude's
/// `SendMessage`) names its first run's agent as the parent of its new
/// work: that work nests under the agent, which reads as working again
/// and settles with the run that resumed it.
#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_subagent_keeps_its_work_under_it() {
    let fixture = fixture(Arc::new(|turn, io| {
        Box::pin(async move {
            if turn.input.text == "start" {
                io.emit(EngineEvent::Item {
                    key: "toolu_agent".into(),
                    parent: None,
                    status: Status::Running,
                    kind: ItemKind::Agent {
                        engine: None,
                        thread: None,
                        name: Some("general-purpose".into()),
                        description: "Weapons in hands".into(),
                        prompt: None,
                        agent_id: None,
                        context_used: None,
                        model: None,
                        background: true,
                        result: None,
                        label: None,
                    },
                })
                .await;
                return Outcome::cancelled();
            }
            io.emit(EngineEvent::Item {
                key: "toolu_resumed_step".into(),
                parent: Some("toolu_agent".into()),
                status: Status::Done,
                kind: ItemKind::Tool {
                    tool: ToolKind::Shell {
                        command: "cargo test -p client".into(),
                        cwd: None,
                        background: false,
                        watch: false,
                    },
                    title: None,
                    output: None,
                    exit_code: Some(0),
                    approval: None,
                },
            })
            .await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("start"))
        .await
        .unwrap();
    fixture.settled().await;
    let agent = fixture
        .items()
        .await
        .into_iter()
        .find(|item| matches!(item.kind, ItemKind::Agent { .. }))
        .expect("the agent");
    assert_eq!(agent.status, Status::Cancelled, "stopped with its run");

    fixture
        .hub
        .send_message(&fixture.thread_id, message("resume it"))
        .await
        .unwrap();
    fixture.settled().await;
    let items = fixture.items().await;
    let step = fixture.by_native("toolu_resumed_step").await;
    assert_eq!(
        step.parent_id.as_ref(),
        Some(&agent.id),
        "nested under the agent"
    );
    let agent = items.iter().find(|item| item.id == agent.id).unwrap();
    assert_eq!(
        agent.status,
        Status::Done,
        "settled with the run that resumed it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_preparation_preserves_input_without_a_retry_loop() {
    let fixture = fixture(Arc::new(|_, _| {
        panic!("uninstalled engine must not launch")
    }))
    .await;
    std::fs::remove_file(fixture.dir.path().join("engines/claude/current")).unwrap();
    let input = fixture
        .hub
        .send_message(&fixture.thread_id, message("retry after installing"))
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Failed);
    let input = fixture.hub.store().item(&input.id).await.unwrap().unwrap();
    assert_eq!(input.status, Status::Running);
    assert!(input.run_id.is_none());
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!fixture.hub.is_live(&fixture.thread_id));
    assert_eq!(
        fixture
            .hub
            .store()
            .latest_run(&fixture.thread_id)
            .await
            .unwrap()
            .unwrap()
            .id,
        runs[0].id
    );
}

#[tokio::test]
async fn a_claimed_initial_input_is_not_delivered_again_as_a_steer() {
    let fixture = fixture(Arc::new(|turn, mut io| {
        Box::pin(async move {
            assert_eq!(turn.input.text, "one delivery");
            assert!(
                tokio::time::timeout(Duration::from_millis(20), io.inbox.recv())
                    .await
                    .is_err()
            );
            Outcome::done()
        })
    }))
    .await;
    let command = message("one delivery");
    let (item, _, _) = fixture
        .hub
        .store()
        .send_message(
            fixture.thread_id.clone(),
            command,
            false,
            forge_store::Acceptance::default(),
        )
        .await
        .unwrap();
    let (sender, receiver) = mpsc::channel(64);
    sender
        .send(Input::Steer(
            fixture
                .hub
                .user_input(&item, EngineId::Claude)
                .await
                .expect("fixture input is valid"),
        ))
        .await
        .unwrap();
    let status = super::run::drive(
        &fixture.hub,
        &fixture.thread_id,
        RunId::new(),
        receiver,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(status, RunStatus::Done);
    assert_eq!(
        fixture
            .hub
            .store()
            .item(&item.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        Status::Done
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nonblocking_question_can_be_answered_after_its_engine_exits() {
    let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&turns);
    let fixture = fixture(Arc::new(move |turn, io| {
        let first = observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        Box::pin(async move {
            if first {
                io.emit(EngineEvent::Item {
                    key: "async-question".into(),
                    parent: None,
                    status: Status::AwaitingUser,
                    kind: ItemKind::Question {
                        answer_text: None,
                        blocking: false,
                        questions: vec![artisan_api::Question {
                            id: "colour".into(),
                            header: None,
                            text: "Which colour?".into(),
                            multi_select: false,
                            options: vec![artisan_api::QuestionOption {
                                id: "option-0".into(),
                                label: "Blue".into(),
                                description: None,
                            }]
                            .into(),
                        }]
                        .into(),
                        answers: None,
                    },
                })
                .await;
            } else {
                assert!(turn.input.text.contains("Which colour?\nAnswer: Blue"));
                io.emit(reply("answered", "Using blue")).await;
            }
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("Ask me a question"))
        .await
        .unwrap();
    fixture.settled().await;
    let question = fixture
        .items()
        .await
        .into_iter()
        .find(|item| matches!(item.kind, ItemKind::Question { .. }))
        .unwrap();
    assert_eq!(question.status, Status::AwaitingUser);
    fixture
        .hub
        .answer(
            &question.id,
            Answer::Question {
                answers: vec![Some(artisan_api::QuestionAnswer::Choices {
                    ids: vec!["option-0".into()].into(),
                })],
            },
        )
        .await
        .unwrap();
    fixture.settled().await;
    assert_eq!(turns.load(std::sync::atomic::Ordering::SeqCst), 2);
    let question = fixture
        .hub
        .store()
        .item(&question.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(question.status, Status::Done);
    assert!(matches!(
        question.kind,
        ItemKind::Question {
            answers: Some(_),
            ..
        }
    ));
    assert!(
        fixture
            .items()
            .await
            .iter()
            .any(|item| item.preview() == Some("Using blue"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn controller_shutdown_preserves_undelivered_input() {
    let fixture = fixture(Arc::new(|_, io| {
        Box::pin(async move {
            io.cancel.cancelled().await;
            Outcome::cancelled()
        })
    }))
    .await;
    let first = fixture
        .hub
        .send_message(&fixture.thread_id, message("accepted"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| {
            items
                .iter()
                .any(|item| item.id == first.id && item.status == Status::Done)
        })
        .await;
    let pending = fixture
        .hub
        .send_message(&fixture.thread_id, message("next startup"))
        .await
        .unwrap();
    fixture.hub.shutdown();
    // The durable queue intentionally survives shutdown, so waiting for an
    // empty pending queue would contradict the behavior this test verifies.
    tokio::time::timeout(Duration::from_secs(10), fixture.hub.inner.tasks.wait())
        .await
        .expect("owned run exits on controller shutdown");
    assert!(!fixture.hub.is_live(&fixture.thread_id));
    let run = fixture
        .hub
        .store()
        .latest_run(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    // Unfinished, not cancelled by the user: it carries on after the restart.
    assert_eq!(run.status, RunStatus::Interrupted);
    assert_eq!(
        undelivered(fixture.hub.store(), &fixture.thread_id)
            .await
            .iter()
            .map(|item| &item.id)
            .collect::<Vec<_>>(),
        [&pending.id]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn canceling_a_queued_run_cancels_earlier_inputs_and_runs_later_inputs() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&seen);
    let fixture = fixture(Arc::new(move |turn, io| {
        let observed = Arc::clone(&observed);
        Box::pin(async move {
            observed.lock().unwrap().push(turn.input.text.clone());
            io.emit(reply("reply", "done")).await;
            Outcome::done()
        })
    }))
    .await;
    let slots = Arc::clone(&fixture.hub.inner.harness_slots)
        .acquire_many_owned(8)
        .await
        .unwrap();
    let earlier = fixture
        .hub
        .send_message(&fixture.thread_id, message("before cancel"))
        .await
        .unwrap();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    fixture
        .wait_for(|items| {
            items
                .iter()
                .any(|item| item.id == earlier.id && item.status == Status::Cancelled)
        })
        .await;
    let later = fixture
        .hub
        .send_message(&fixture.thread_id, message("after cancel"))
        .await
        .unwrap();
    drop(slots);
    fixture.hub.resume_pending();
    fixture.settled().await;
    assert_eq!(*seen.lock().unwrap(), ["after cancel"]);
    assert_eq!(
        fixture
            .hub
            .store()
            .item(&later.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        Status::Done
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn at_most_eight_harnesses_execute_at_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Semaphore::new(0));
    let observed = (
        std::sync::Arc::clone(&active),
        std::sync::Arc::clone(&maximum),
        std::sync::Arc::clone(&started),
        std::sync::Arc::clone(&release),
    );
    let fixture = fixture(Arc::new(move |_, _| {
        let (active, maximum, started, release) = observed.clone();
        Box::pin(async move {
            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(count, Ordering::SeqCst);
            started.fetch_add(1, Ordering::SeqCst);
            release.acquire().await.unwrap().forget();
            active.fetch_sub(1, Ordering::SeqCst);
            Outcome::done()
        })
    }))
    .await;
    let first = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    let mut threads = vec![fixture.thread_id.clone()];
    for _ in 1..10 {
        threads.push(
            fixture
                .hub
                .store()
                .create_thread(CreateThread {
                    id: ThreadId::new(),
                    project_id: first.project_id.clone(),
                    config: first.config.clone(),
                    title: None,
                    parent_thread_id: None,
                    spawned_by: None,
                })
                .await
                .unwrap()
                .id,
        );
    }
    for thread in &threads {
        fixture
            .hub
            .send_message(thread, message("work"))
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while started.load(Ordering::SeqCst) < 8 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 8);
    release.add_permits(10);
    tokio::time::timeout(Duration::from_secs(10), async {
        while started.load(Ordering::SeqCst) < 10
            || threads.iter().any(|thread| fixture.hub.is_live(thread))
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 10);
    assert!(maximum.load(Ordering::SeqCst) <= 8);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_coordinator_event_error_settles_the_run_and_open_outputs() {
    let fixture = fixture(Arc::new(|turn, io| {
        Box::pin(async move {
            io.emit(EngineEvent::Item {
                key: "open".into(),
                parent: None,
                status: Status::Running,
                kind: ItemKind::AssistantMessage {
                    text: (String::new()).into(),
                    phase: Phase::Commentary,
                },
            })
            .await;
            io.emit(EngineEvent::Image {
                key: "image".into(),
                parent: None,
                path: turn.home.join("does-not-exist.png"),
                caption: None,
            })
            .await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("generate"))
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Failed);
    assert!(runs[0].ended_at.is_some());
    assert!(
        fixture
            .items()
            .await
            .iter()
            .filter(|item| item.run_id.as_ref() == Some(&runs[0].id))
            .all(|item| item.status.is_terminal())
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn native_claude_frames_reach_store_once_and_answers_reach_the_provider() {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture = fixture_with_harness(Arc::new(forge_engines::harness)).await;
    let binary = fixture.hub.engines().binary(EngineId::Claude).unwrap();
    std::fs::write(&binary, r#"#!/bin/sh
read -r input
printf '%s\n' '{"type":"user","isReplay":true,"message":{"content":"go"}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"native-session","model":"native-model"}'
printf '%s\n' '{"type":"assistant","message":{"id":"question-message","content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Pick a color","options":[{"label":"Blue"}]}]}}]}}'
printf '%s\n' '{"type":"control_request","request_id":"question-request","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","tool_use_id":"q1","input":{"questions":[{"question":"Pick a color","options":[{"label":"Blue"}]}]}}}'
while read -r input; do
  case "$input" in
    *control_response*Blue*) break ;;
    *control_response*) exit 9 ;;
  esac
done
printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"q1","content":"Blue"}]}}'
printf '%s\n' '{"type":"assistant","message":{"id":"reply-message","stop_reason":"end_turn","content":[{"type":"text","text":"Native reply"}]}}'
printf '%s\n' '{"type":"result","subtype":"success","result":"Native reply","usage":{}}'
"#).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut events = fixture.hub.store().subscribe();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("go"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| {
            items.iter().any(|item| {
                matches!(item.kind, ItemKind::Question { .. })
                    && item.status == Status::AwaitingUser
            })
        })
        .await;
    let question = fixture
        .items()
        .await
        .into_iter()
        .find(|item| matches!(item.kind, ItemKind::Question { .. }))
        .unwrap();
    fixture
        .hub
        .answer(
            &question.id,
            Answer::Question {
                answers: vec![Some(artisan_api::QuestionAnswer::Choices {
                    ids: vec!["option-0".into()].into(),
                })],
            },
        )
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status, RunStatus::Done);
    let items = fixture.items().await;
    assert!(items.iter().any(|item| item.id == question.id && item.status == Status::Done && matches!(&item.kind, ItemKind::Question { answers: Some(answers), .. } if answers.inline() == Some(vec![Some(artisan_api::QuestionAnswer::Choices { ids: vec!["option-0".into()].into() })].as_slice()))));
    let mut full_prose_saves = 0;
    while let Ok(envelope) = events.try_recv() {
        if matches!(envelope.event, artisan_api::Event::Item { item } if matches!(&item.kind, ItemKind::AssistantMessage { text, .. } if text.inline() == Some("Native reply")))
        {
            full_prose_saves += 1;
        }
    }
    assert_eq!(
        full_prose_saves, 1,
        "native prose body is durably published exactly once"
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn durable_queued_threads_do_not_allocate_live_owners_until_a_slot_is_free() {
    let finish = Arc::new(Semaphore::new(0));
    let gate = Arc::clone(&finish);
    let fixture = fixture(Arc::new(move |_, io| {
        let gate = Arc::clone(&gate);
        Box::pin(async move {
            tokio::select! {
                () = io.cancel.cancelled() => Outcome::cancelled(),
                permit = gate.acquire_owned() => { permit.unwrap().forget(); Outcome::done() }
            }
        })
    }))
    .await;
    let first = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    let mut queued = Vec::new();
    for _ in 0..32 {
        let thread = fixture
            .hub
            .store()
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: first.project_id.clone(),
                config: first.config.clone(),
                title: None,
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .unwrap();
        fixture
            .hub
            .send_message(&thread.id, message("queued"))
            .await
            .unwrap();
        queued.push(thread.id);
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture.hub.lock().len() < 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.hub.lock().len(), 8);
    assert!(
        fixture.hub.inner.tasks.len() <= 10,
        "only dispatcher, status forwarder and eight run tasks"
    );
    let last = queued.last().unwrap();
    assert!(!fixture.hub.is_live(last));
    fixture.hub.cancel(last).await.unwrap();
    assert!(undelivered(fixture.hub.store(), last).await.is_empty());
    finish.add_permits(32);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if fixture.hub.lock().is_empty()
                && fixture
                    .hub
                    .store()
                    .pending_threads(8, Vec::new())
                    .await
                    .unwrap()
                    .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        fixture
            .hub
            .store()
            .latest_run(last)
            .await
            .unwrap()
            .is_none(),
        "queued cancel never creates a run"
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn new_input_during_failed_preparation_allows_one_new_attempt() {
    let fixture = fixture(Arc::new(|_, _| panic!("preparation must fail"))).await;
    std::fs::create_dir_all(fixture.dir.path().join("engines/claude")).unwrap();
    std::fs::write(
        fixture.dir.path().join("engines/claude/home"),
        "not a directory",
    )
    .unwrap();
    let owner = fixture.hub.clone();
    let (held, acquired) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let guard = tokio::task::spawn_blocking(move || {
        let _guard = owner.homes_lock();
        held.send(()).unwrap();
        released.recv().unwrap();
    });
    acquired.await.unwrap();
    let mut events = fixture.hub.store().subscribe();
    let earlier = fixture
        .hub
        .send_message(&fixture.thread_id, message("first"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if fixture
                .hub
                .store()
                .latest_run(&fixture.thread_id)
                .await
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let later = fixture
        .hub
        .send_message(&fixture.thread_id, message("second"))
        .await
        .unwrap();
    release.send(()).unwrap();
    guard.await.unwrap();
    fixture.settled().await;
    let latest = fixture
        .hub
        .store()
        .latest_run(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.status, RunStatus::Failed);
    assert_eq!(latest.input_cutoff, later.seq);
    assert!(latest.input_cutoff > earlier.seq);
    let mut attempts = std::collections::HashSet::new();
    while let Ok(event) = events.try_recv() {
        if let Event::Run { run } = event.event
            && run.status == RunStatus::Running
        {
            attempts.insert(run.id);
        }
    }
    assert_eq!(attempts.len(), 2);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        fixture
            .hub
            .store()
            .latest_run(&fixture.thread_id)
            .await
            .unwrap()
            .unwrap()
            .id,
        latest.id
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn one_thread_drains_multiple_bounded_initial_batches_in_order() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    let fixture = fixture(Arc::new(move |turn, _| {
        let record = Arc::clone(&record);
        Box::pin(async move {
            record.lock().unwrap().push(turn.input.text);
            Outcome::done()
        })
    }))
    .await;
    let slots = Arc::clone(&fixture.hub.inner.harness_slots)
        .acquire_many_owned(8)
        .await
        .unwrap();
    for n in 0..40 {
        fixture
            .hub
            .send_message(&fixture.thread_id, message(&format!("message-{n}")))
            .await
            .unwrap();
    }
    assert!(fixture.hub.lock().is_empty());
    drop(slots);
    fixture.hub.resume_pending();
    fixture.settled().await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].lines().filter(|line| !line.is_empty()).count(), 32);
    let delivered = seen
        .iter()
        .flat_map(|text| text.lines().filter(|line| !line.is_empty()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    drop(seen);
    assert_eq!(
        delivered,
        (0..40).map(|n| format!("message-{n}")).collect::<Vec<_>>()
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_bounded_batch_does_not_retry_its_already_observed_tail() {
    let fixture = fixture(Arc::new(|_, _| panic!("uninstalled engine"))).await;
    std::fs::remove_file(fixture.dir.path().join("engines/claude/current")).unwrap();
    let slots = Arc::clone(&fixture.hub.inner.harness_slots)
        .acquire_many_owned(8)
        .await
        .unwrap();
    let mut latest = 0;
    for n in 0..40 {
        latest = fixture
            .hub
            .send_message(&fixture.thread_id, message(&format!("message-{n}")))
            .await
            .unwrap()
            .seq;
    }
    drop(slots);
    fixture.hub.resume_pending();
    fixture.settled().await;
    let run = fixture
        .hub
        .store()
        .latest_run(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.input_cutoff, latest);
    assert_eq!(
        undelivered(fixture.hub.store(), &fixture.thread_id)
            .await
            .len(),
        40
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        fixture
            .hub
            .store()
            .latest_run(&fixture.thread_id)
            .await
            .unwrap()
            .unwrap()
            .id,
        run.id
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn shutdown_leaves_unclaimed_durable_threads_for_the_next_dispatcher() {
    let fixture = fixture(Arc::new(|_, io| {
        Box::pin(async move {
            io.cancel.cancelled().await;
            Outcome::cancelled()
        })
    }))
    .await;
    let first = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    let mut threads = Vec::new();
    for _ in 0..24 {
        let thread = fixture
            .hub
            .store()
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: first.project_id.clone(),
                config: first.config.clone(),
                title: None,
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .unwrap();
        fixture
            .hub
            .send_message(&thread.id, message("pending restart"))
            .await
            .unwrap();
        threads.push(thread.id);
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture.hub.lock().len() != 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.hub.shutdown();
    fixture.hub.inner.tasks.wait().await;
    assert!(fixture.hub.lock().is_empty());
    let observed = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&observed);
    let harness: Arc<dyn Harness> = Arc::new(Scripted(Arc::new(move |turn, _| {
        let record = Arc::clone(&record);
        Box::pin(async move {
            record
                .lock()
                .unwrap()
                .push((turn.thread_id, turn.input.text));
            Outcome::done()
        })
    })));
    let resumed = Hub::with_harness(
        fixture.hub.store().clone(),
        fixture.hub.engines().clone(),
        fixture.dir.path().to_path_buf(),
        Arc::new(move |_| Arc::clone(&harness)),
    );
    resumed.resume_pending();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if resumed.lock().is_empty()
                && resumed
                    .store()
                    .pending_threads(8, Vec::new())
                    .await
                    .unwrap()
                    .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let observed = observed.lock().unwrap();
    let delivered = observed
        .iter()
        .filter(|(_, text)| text == "pending restart")
        .count();
    let continued = observed
        .iter()
        .filter(|(_, text)| text.starts_with("Artisan restarted"))
        .count();
    assert_eq!(
        (delivered, continued),
        (16, 8),
        "eight acknowledged active inputs are not redelivered but carry on; sixteen queued inputs resume"
    );
    assert_eq!(
        observed
            .iter()
            .map(|(thread, _)| thread)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        24
    );
    assert!(observed.iter().all(|(thread, _)| threads.contains(thread)));
    drop(observed);
    resumed.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_historical_image_fails_a_created_run_without_dropping_or_retrying_input() {
    use image::ImageEncoder as _;
    let fixture = fixture(Arc::new(|_, _| {
        panic!("invalid image must fail before harness")
    }))
    .await;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&[1, 2, 3, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    let blob = fixture.hub.store().put_image(png).await.unwrap();
    let path = fixture.hub.store().blob_path(&blob.hash).unwrap();
    let slots = Arc::clone(&fixture.hub.inner.harness_slots)
        .acquire_many_owned(8)
        .await
        .unwrap();
    let mut message = message("historical image");
    message.images.push(blob.hash);
    let input = fixture
        .hub
        .send_message(&fixture.thread_id, message)
        .await
        .unwrap();
    std::fs::remove_file(path).unwrap();
    drop(slots);
    fixture.hub.resume_pending();
    fixture.settled().await;
    let run = fixture
        .hub
        .store()
        .latest_run(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        run.error
            .as_ref()
            .is_some_and(|error| error.contains("missing"))
    );
    assert_eq!(
        fixture
            .hub
            .store()
            .item(&input.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        Status::Running
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        fixture
            .hub
            .store()
            .latest_run(&fixture.thread_id)
            .await
            .unwrap()
            .unwrap()
            .id,
        run.id
    );
    fixture.hub.shutdown();
}

/// A message to an agent, or stopping it, names the agent's item, whatever
/// the engine calls the agent, so it shows in that agent's view; one to
/// another session names none.
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn a_message_to_an_agent_names_the_agents_item() {
    let fixture = fixture(Arc::new(|_turn, io| {
        Box::pin(async move {
            io.emit(EngineEvent::Item {
                key: "agent-call".into(),
                parent: None,
                status: Status::Running,
                kind: ItemKind::Agent {
                    engine: None,
                    thread: None,
                    name: None,
                    description: "Review".into(),
                    prompt: None,
                    agent_id: Some("a935b57d5f0b8fb33".into()),
                    context_used: None,
                    model: None,
                    background: true,
                    result: None,
                    label: None,
                },
            })
            .await;
            for (key, recipient) in [
                ("to-agent", "a935b57d5f0b8fb33"),
                ("to-session", "uds:/tmp/session"),
            ] {
                io.emit(EngineEvent::Item {
                    key: key.into(),
                    parent: None,
                    status: Status::Done,
                    kind: ItemKind::Tool {
                        tool: ToolKind::AgentMessage {
                            recipient: recipient.into(),
                            target: None,
                            summary: None,
                            text: "Check the parser.".into(),
                            direction: artisan_api::MessageDirection::Sent,
                        },
                        title: None,
                        output: None,
                        exit_code: None,
                        approval: None,
                    },
                })
                .await;
            }
            io.emit(EngineEvent::Item {
                key: "stop".into(),
                parent: None,
                status: Status::Done,
                kind: ItemKind::Tool {
                    tool: ToolKind::Agents {
                        action: artisan_api::AgentAction::Stop,
                        addresses: vec!["a935b57d5f0b8fb33".into(), "bnuubwmtm".into()],
                        targets: Vec::new(),
                    },
                    title: None,
                    output: None,
                    exit_code: None,
                    approval: None,
                },
            })
            .await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("delegate"))
        .await
        .unwrap();
    fixture.settled().await;
    let agent = fixture.by_native("agent-call").await;
    let target = |item: &Item| {
        let ItemKind::Tool {
            tool: ToolKind::AgentMessage { target, .. },
            ..
        } = &item.kind
        else {
            panic!("not a message: {item:?}");
        };
        target.clone()
    };
    let sent = fixture.by_native("to-agent").await;
    assert_eq!(target(&sent), Some(agent.id.clone()));
    assert_eq!(target(&fixture.by_native("to-session").await), None);
    // Stopping it names it too; the background shell beside it is no agent.
    let stop = fixture.by_native("stop").await;
    let ItemKind::Tool {
        tool: ToolKind::Agents { targets, .. },
        ..
    } = &stop.kind
    else {
        panic!("not an agents call: {:?}", stop.kind);
    };
    assert_eq!(targets, &vec![agent.id.clone()]);
    let view = fixture
        .hub
        .store()
        .transcript_page(
            &fixture.thread_id,
            Some(&agent.id),
            ItemsQuery {
                before: None,
                after: None,
                limit: Some(50),
            },
        )
        .await
        .unwrap();
    assert!(view.items.iter().any(|item| item.id == sent.id));
}

/// An approval rides on the item it gates: the answer reaches the engine
/// under that item's own id, and decisions the request didn't offer or
/// answers of the wrong kind never reach it.
#[tokio::test(flavor = "multi_thread")]
async fn an_approval_is_answered_on_the_item_it_gates() {
    let fixture = fixture(Arc::new(|_turn, mut io| {
        Box::pin(async move {
            let shell = |decision| ItemKind::Tool {
                tool: ToolKind::Shell {
                    command: "echo once".into(),
                    cwd: None,
                    background: false,
                    watch: false,
                },
                title: None,
                output: None,
                exit_code: None,
                approval: Some(artisan_api::Approval {
                    request: ApprovalRequest::Command {
                        command: "echo once".into(),
                        cwd: None,
                    },
                    reason: None,
                    options: vec![Decision::AllowOnce, Decision::DenyOnce],
                    decision,
                }),
            };
            io.emit(EngineEvent::Item {
                key: "execution".into(),
                parent: None,
                status: Status::AwaitingUser,
                kind: shell(None),
            })
            .await;
            let Some(Input::Answer {
                key,
                answer: artisan_api::Answer::Approval { decision },
            }) = io.inbox.recv().await
            else {
                return Outcome::failed("missing the approval answer");
            };
            if key != "execution" {
                return Outcome::failed(format!("answered {key}, not the gated item"));
            }
            io.emit(EngineEvent::Item {
                key,
                parent: None,
                status: Status::Running,
                kind: shell(Some(decision)),
            })
            .await;
            io.emit(EngineEvent::Status {
                key: "execution".into(),
                status: Status::Done,
            })
            .await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("execute once"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| items.iter().any(|item| item.status == Status::AwaitingUser))
        .await;
    let tool = fixture.by_native("execution").await;
    for refused in [
        artisan_api::Answer::Approval {
            decision: Decision::DenyAlways,
        },
        artisan_api::Answer::Approval {
            decision: Decision::Cancelled,
        },
        artisan_api::Answer::Question {
            answers: Vec::new(),
        },
    ] {
        assert!(fixture.hub.answer(&tool.id, refused).await.is_err());
    }
    fixture
        .hub
        .answer(
            &tool.id,
            artisan_api::Answer::Approval {
                decision: Decision::AllowOnce,
            },
        )
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert!(
        runs.iter()
            .all(|run| run.status == artisan_api::RunStatus::Done)
    );
    let tool = fixture.by_native("execution").await;
    assert_eq!(tool.status, Status::Done);
    assert!(matches!(
        &tool.kind,
        ItemKind::Tool { approval: Some(approval), .. } if approval.decision == Some(Decision::AllowOnce)
    ));
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn duplicate_label_offline_choices_resume_as_different_inputs_with_exact_ids() {
    let mut received = Vec::new();
    for selected in ["option-0", "option-1"] {
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = std::sync::Arc::clone(&turns);
        let resumed = Arc::new(std::sync::Mutex::new(String::new()));
        let captured = std::sync::Arc::clone(&resumed);
        let fixture = fixture(Arc::new(move |turn, io| {
            let first = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            let captured = std::sync::Arc::clone(&captured);
            Box::pin(async move {
                if first {
                    io.emit(EngineEvent::Item {
                        key: "async-duplicate".into(),
                        parent: None,
                        status: Status::AwaitingUser,
                        kind: ItemKind::Question {
                            answer_text: None,
                            blocking: false,
                            questions: vec![artisan_api::Question {
                                id: "selection".into(),
                                header: None,
                                text: "Which entry?".into(),
                                multi_select: false,
                                options: vec![
                                    artisan_api::QuestionOption {
                                        id: "option-0".into(),
                                        label: "Same".into(),
                                        description: None,
                                    },
                                    artisan_api::QuestionOption {
                                        id: "option-1".into(),
                                        label: "Same".into(),
                                        description: None,
                                    },
                                ]
                                .into(),
                            }]
                            .into(),
                            answers: None,
                        },
                    })
                    .await;
                } else {
                    *captured.lock().unwrap() = turn.input.text;
                    io.emit(reply("answer", "Selection received")).await;
                }
                Outcome::done()
            })
        }))
        .await;
        fixture
            .hub
            .send_message(&fixture.thread_id, message("Ask me"))
            .await
            .unwrap();
        fixture.settled().await;
        let question = fixture
            .items()
            .await
            .into_iter()
            .find(|item| matches!(item.kind, ItemKind::Question { .. }))
            .unwrap();
        assert_eq!(question.status, Status::AwaitingUser);
        fixture
            .hub
            .answer(
                &question.id,
                Answer::Question {
                    answers: vec![Some(artisan_api::QuestionAnswer::Choices {
                        ids: vec![selected.into()].into(),
                    })],
                },
            )
            .await
            .unwrap();
        fixture.settled().await;
        let saved = fixture
            .hub
            .store()
            .item(&question.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(saved.kind,ItemKind::Question {answers:Some(answers),..} if answers.inline()==Some(vec![Some(artisan_api::QuestionAnswer::Choices {ids:vec![selected.into()].into()})].as_slice()))
        );
        assert_eq!(turns.load(std::sync::atomic::Ordering::SeqCst), 2);
        let input = resumed.lock().unwrap().clone();
        assert!(
            fixture
                .items()
                .await
                .iter()
                .any(|item| matches!(&item.kind,ItemKind::UserMessage {text,..} if text.inline()==Some(input.as_str()))),
            "resumed native input must come from the durable accepted message"
        );
        received.push(input);
    }
    assert_ne!(received[0], received[1]);
    assert!(received[0].contains("Same (option 1 of 2)"));
    assert!(received[1].contains("Same (option 2 of 2)"));
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn normal_process_exit_cancels_unanswered_interactions_and_completes_running_output() {
    let fixture = fixture(Arc::new(|_turn, io| {
        Box::pin(async move {
            let approval = artisan_api::Approval {
                request: ApprovalRequest::Action {
                    description: "Continue?".into(),
                },
                reason: None,
                options: vec![Decision::AllowOnce],
                decision: None,
            };
            let kinds = [
                (
                    "blocking-question",
                    ItemKind::Question {
                        answer_text: None,
                        blocking: true,
                        questions: vec![artisan_api::Question {
                            id: "confirm".into(),
                            header: None,
                            text: "Continue?".into(),
                            multi_select: false,
                            options: vec![].into(),
                        }]
                        .into(),
                        answers: None,
                    },
                ),
                (
                    "tool-approval",
                    ItemKind::Tool {
                        tool: ToolKind::Shell {
                            command: "echo waiting".into(),
                            cwd: None,
                            background: false,
                            watch: false,
                        },
                        title: None,
                        output: None,
                        exit_code: None,
                        approval: Some(approval.clone()),
                    },
                ),
                (
                    "plan-approval",
                    ItemKind::Plan {
                        steps: vec![].into(),
                        proposal: Some("Plan".into()),
                        approval: Some(approval),
                    },
                ),
            ];
            for (key, kind) in kinds {
                io.emit(EngineEvent::Item {
                    key: key.into(),
                    parent: None,
                    status: Status::AwaitingUser,
                    kind,
                })
                .await;
            }
            io.emit(EngineEvent::Item {
                key: "ordinary-output".into(),
                parent: None,
                status: Status::Running,
                kind: ItemKind::AssistantMessage {
                    text: "Completed response".into(),
                    phase: Phase::Final,
                },
            })
            .await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("ask and finish"))
        .await
        .unwrap();
    fixture.settled().await;
    for key in ["blocking-question", "tool-approval", "plan-approval"] {
        let item = fixture.by_native(key).await;
        assert_eq!(
            item.status,
            Status::Cancelled,
            "unanswered {key} cannot become completed"
        );
        if key == "blocking-question" {
            assert!(
                fixture
                    .hub
                    .answer(
                        &item.id,
                        Answer::Question {
                            answers: vec![Some(artisan_api::QuestionAnswer::Text {
                                text: "yes".into()
                            })]
                        }
                    )
                    .await
                    .is_err()
            );
        }
    }
    let output = fixture.by_native("ordinary-output").await;
    assert_eq!(output.status, Status::Done);
    assert_eq!(
        fixture
            .hub
            .store()
            .latest_run(&fixture.thread_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        artisan_api::RunStatus::Done
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn native_approval_rpc_ids_restart_without_reusing_prior_run_items() {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture =
        fixture_with_engine_harness(EngineId::Codex, Arc::new(forge_engines::harness)).await;
    let binary = fixture.hub.engines().binary(EngineId::Codex).unwrap();
    std::fs::write(&binary,r"#!/usr/bin/env python3
import json, os, pathlib, sys
home=pathlib.Path(os.environ['CODEX_HOME']).parent
count=home/'native-run-count'
number=int(count.read_text())+1 if count.exists() else 1
count.write_text(str(number))
thread='native-session'
turn='native-turn-'+str(number)
item='native-tool-'+str(number)
def send(value):
    print(json.dumps(value),flush=True)
for line in sys.stdin:
    message=json.loads(line)
    method=message.get('method')
    if method=='initialize':
        send({'id':message['id'],'result':{}})
    elif method in ('thread/start','thread/resume'):
        send({'id':message['id'],'result':{'thread':{'id':'title-session' if message['params'].get('ephemeral') else thread}}})
    elif method=='thread/name/set':
        send({'id':message['id'],'result':{}})
    elif method=='turn/start':
        send({'id':message['id'],'result':{'turn':{'id':turn}}})
        if message['params']['threadId']=='title-session':
            send({'method':'item/completed','params':{'threadId':'title-session','turnId':turn,'item':{'id':'title','type':'agentMessage','text':json.dumps({'title':'Native approval requests'})}}})
            send({'method':'turn/completed','params':{'threadId':'title-session','turn':{'id':turn,'status':'completed'}}})
            continue
        send({'method':'item/started','params':{'threadId':thread,'turnId':turn,'item':{'id':item,'type':'commandExecution','command':'echo once','status':'inProgress'}}})
        send({'id':0,'method':'item/commandExecution/requestApproval','params':{'threadId':thread,'turnId':turn,'itemId':item,'command':'echo once'}})
    elif method is None and message.get('id')==0:
        decision=message['result']['decision']
        with (home/'native-replies').open('a') as file:
            file.write(json.dumps({'run':number,'id':0,'item':item,'decision':decision})+'\n')
        send({'method':'item/completed','params':{'threadId':thread,'turnId':turn,'item':{'id':item,'type':'commandExecution','command':'echo once','status':'completed' if decision=='accept' else 'declined','exitCode':0 if decision=='accept' else None}}})
        send({'method':'turn/completed','params':{'threadId':thread,'turn':{'id':turn,'status':'completed'}}})
        break
").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut identities = Vec::new();
    for (text, decision) in [
        ("first request", Decision::AllowOnce),
        ("second request", Decision::DenyOnce),
    ] {
        fixture
            .hub
            .send_message(&fixture.thread_id, message(text))
            .await
            .unwrap();
        fixture
            .wait_for(|items| items.iter().any(|item| item.status == Status::AwaitingUser))
            .await;
        let item = fixture
            .items()
            .await
            .into_iter()
            .find(|item| item.status == Status::AwaitingUser)
            .unwrap();
        assert!(
            identities.iter().all(|id| id != &item.id),
            "per-Run Work map must mint a new canonical request identity"
        );
        identities.push(item.id.clone());
        fixture
            .hub
            .answer(&item.id, Answer::Approval { decision })
            .await
            .unwrap();
        fixture.settled().await;
        assert_eq!(
            fixture
                .hub
                .store()
                .item(&item.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            if decision.allows() {
                Status::Done
            } else {
                Status::Declined
            }
        );
    }
    let approvals = fixture
        .items()
        .await
        .into_iter()
        .filter(|item| {
            matches!(
                item.kind,
                ItemKind::Tool {
                    approval: Some(_),
                    ..
                }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(approvals.len(), 2);
    assert_ne!(approvals[0].run_id, approvals[1].run_id);
    let replies = std::fs::read_to_string(
        fixture
            .hub
            .engines()
            .home(EngineId::Codex)
            .join("native-replies"),
    )
    .unwrap();
    let replies = replies
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["id"], 0);
    assert_eq!(replies[1]["id"], 0);
    assert_ne!(replies[0]["item"], replies[1]["item"]);
    assert_eq!(replies[0]["decision"], "accept");
    assert_eq!(replies[1]["decision"], "decline");
}

#[tokio::test(flavor = "multi_thread")]
async fn live_previews_report_missing_bytes_without_unbounded_sse_frames() {
    let fixture = fixture(Arc::new(|_turn, mut io| {
        Box::pin(async move {
            let _ = io.inbox.recv().await;
            Outcome::done()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("start"))
        .await
        .unwrap();
    let text = LiveTarget::Text {
        item_id: ItemId::new(),
    };
    let shell = LiveTarget::Shell {
        item_id: ItemId::new(),
    };
    let huge = "😀".repeat(3 * 1024 * 1024);
    fixture
        .hub
        .publish_live(&fixture.thread_id, text.clone(), huge.clone(), false);
    fixture
        .hub
        .publish_live(&fixture.thread_id, text.clone(), "tail".into(), true);
    fixture
        .hub
        .publish_live(&fixture.thread_id, shell.clone(), huge.clone(), true);
    fixture
        .hub
        .publish_live(&fixture.thread_id, shell.clone(), "tail".into(), true);
    let events = fixture.hub.live_snapshot().1;
    for target in [&text, &shell] {
        let event = events
            .iter()
            .find(|event| matches!(event, Event::Live { target: value, .. } if value == target))
            .unwrap();
        let Event::Live {
            text: preview,
            coverage,
            append,
            ..
        } = event
        else {
            unreachable!()
        };
        assert!(!append);
        assert!(preview.len() <= 256 * 1024);
        assert_eq!(coverage.total, huge.len() as u64 + 4);
        assert!(!coverage.is_complete(preview.len()));
        assert!(serde_json::to_vec(event).unwrap().len() < 2 * 1024 * 1024);
        if target == &text {
            assert_eq!(coverage.start, 0);
            assert!(!preview.ends_with("tail"));
        } else {
            assert_eq!(coverage.start, coverage.total - preview.len() as u64);
            assert!(preview.ends_with("tail"));
        }
    }
    fixture.hub.publish_live(
        &fixture.thread_id,
        text.clone(),
        "replacement".into(),
        false,
    );
    assert!(fixture.hub.live_snapshot().1.iter().any(|event| matches!(event, Event::Live { target, text: preview, coverage, .. } if target == &text && coverage.is_complete(preview.len()))));
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    fixture.settled().await;
}

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn large_saved_question_validates_selected_identity_without_expanding_options() {
    let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::new(std::sync::Mutex::new(String::new()));
    let fixture = fixture(Arc::new({
        let turns = std::sync::Arc::clone(&turns);
        let observed = std::sync::Arc::clone(&observed);
        move |turn, io| {
            let first = turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            let observed = std::sync::Arc::clone(&observed);
            Box::pin(async move {
                if first {
                    let options = (0..5000)
                        .map(|n| artisan_api::QuestionOption {
                            id: format!("choice-{n}"),
                            label: format!("Choice {n}"),
                            description: None,
                        })
                        .collect::<Vec<_>>();
                    io.emit(EngineEvent::Item {
                        key: "large-saved-question".into(),
                        parent: None,
                        status: Status::AwaitingUser,
                        kind: ItemKind::Question {
                            answer_text: None,
                            blocking: false,
                            questions: vec![artisan_api::Question {
                                id: "select".into(),
                                header: None,
                                text: "Choose one".into(),
                                multi_select: false,
                                options: options.into(),
                            }]
                            .into(),
                            answers: None,
                        },
                    })
                    .await;
                } else {
                    *observed.lock().unwrap() = turn.input.text;
                    io.emit(reply("resumed", "Selection received")).await;
                }
                Outcome::done()
            })
        }
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("Ask me"))
        .await
        .unwrap();
    fixture.settled().await;
    let question = fixture
        .items()
        .await
        .into_iter()
        .find(|item| matches!(item.kind, ItemKind::Question { .. }))
        .unwrap();
    let ItemKind::Question { questions, .. } = &question.kind else {
        panic!("question expected")
    };
    assert_eq!(questions.inline().unwrap()[0].options.len(), 5000);
    assert!(questions.inline().unwrap()[0].options.stored().is_some());
    assert!(serde_json::to_vec(&question).unwrap().len() < 4096);
    let head = fixture.hub.store().head();
    for ids in [
        vec!["missing".into()],
        vec!["choice-4999".into(), "choice-4999".into()],
    ] {
        assert!(
            fixture
                .hub
                .answer(
                    &question.id,
                    Answer::Question {
                        answers: vec![Some(artisan_api::QuestionAnswer::Choices {
                            ids: ids.into()
                        })],
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(fixture.hub.store().head(), head);
    }
    fixture
        .hub
        .answer(
            &question.id,
            Answer::Question {
                answers: vec![Some(artisan_api::QuestionAnswer::Choices {
                    ids: vec!["choice-4999".into()].into(),
                })],
            },
        )
        .await
        .unwrap();
    fixture.settled().await;
    assert_eq!(turns.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        observed
            .lock()
            .unwrap()
            .contains("Choose one\nAnswer: Choice 4999")
    );
    let saved = fixture
        .hub
        .store()
        .item(&question.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.status, Status::Done);
    assert!(
        matches!(saved.kind, ItemKind::Question {questions,answers:Some(answers),..}
        if questions.inline().unwrap()[0].options.stored().is_some() && answers.len()==1)
    );
    fixture.hub.shutdown();
    tokio::time::timeout(Duration::from_secs(3), fixture.hub.inner.tasks.wait())
        .await
        .unwrap();
}

/// `/ultracode` holds for the turn it starts and no longer: the message is
/// stored as typed, Claude reads the keyword in that prompt only, and the
/// next message runs without it.
#[tokio::test(flavor = "multi_thread")]
async fn ultracode_is_on_for_the_turn_it_starts_only() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&prompts);
    let fixture = fixture(Arc::new(move |turn, io| {
        seen.lock().unwrap().push(turn.input.text);
        Box::pin(async move {
            io.emit(reply("answer", "done")).await;
            Outcome::done()
        })
    }))
    .await;
    let typed = "/ultracode ship the parser";
    let user = fixture
        .hub
        .send_message(&fixture.thread_id, message(typed))
        .await
        .unwrap();
    assert!(
        matches!(user.kind, ItemKind::UserMessage { ref text, .. } if text.inline() == Some(typed))
    );
    fixture.settled().await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("now the lexer"))
        .await
        .unwrap();
    fixture.settled().await;
    assert_eq!(
        *prompts.lock().unwrap(),
        ["ultracode\nship the parser", "now the lexer"]
    );
}

/// A command with nothing to do fails the send: nothing is stored and no
/// engine starts.
#[tokio::test(flavor = "multi_thread")]
async fn invalid_slash_commands_never_persist_or_start_the_engine() {
    let fixture = fixture(Arc::new(|_, _| {
        Box::pin(async { panic!("invalid command started engine") })
    }))
    .await;
    for text in ["/ultrathink", "/ultracode", "/ultracode   "] {
        assert!(
            fixture
                .hub
                .send_message(&fixture.thread_id, message(text))
                .await
                .is_err()
        );
    }
    assert!(fixture.items().await.is_empty());
    assert!(fixture.settled().await.is_empty());
}

/// A harness whose runs report a session and whose side queries answer
/// from a script, recording each side question.
struct Asking {
    run: Script,
    side: Arc<dyn Fn(&Turn) -> String + Send + Sync>,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Harness for Asking {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        let script = std::sync::Arc::clone(&self.run);
        Box::pin(async move {
            io.emit(EngineEvent::InputAccepted {
                id: turn.input.id.clone(),
            })
            .await;
            io.emit(EngineEvent::Session {
                id: format!("session {}", turn.input.text),
            })
            .await;
            script(turn, io).await
        })
    }

    fn side_query(
        &self,
        turn: Turn,
        _cancel: CancellationToken,
    ) -> BoxFuture<'static, anyhow::Result<forge_engines::SideReply>> {
        self.asked.lock().unwrap().push(turn.input.text.clone());
        let text = (self.side)(&turn);
        Box::pin(async move {
            Ok(forge_engines::SideReply {
                text,
                usage: forge_engines::SideUsage::default(),
            })
        })
    }
}

async fn asking(
    run: Script,
    side: impl Fn(&Turn) -> String + Send + Sync + 'static,
) -> (Fixture, Arc<Mutex<Vec<String>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let harness: Arc<dyn Harness> = Arc::new(Asking {
        run,
        side: Arc::new(side),
        asked: Arc::clone(&asked),
    });
    let fixture = fixture_with_harness(Arc::new(move |_| Arc::clone(&harness))).await;
    (fixture, asked)
}

/// The guess at the next message is asked of the thread's own session
/// after the run ended, is replayed to a client that connects later, and is
/// withdrawn when the thread runs again.
#[tokio::test(flavor = "multi_thread")]
async fn a_suggested_next_message_lasts_until_the_thread_runs_again() {
    let (fixture, _) = asking(
        Arc::new(|_, _| Box::pin(async { Outcome::done() })),
        |turn| {
            let session = turn.session_id.clone().unwrap_or_default();
            format!("{{\"suggestion\": \"after {session}\"}}")
        },
    )
    .await;
    let mut events = fixture.hub.store().subscribe();
    let mut next_suggestion = async move || {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Event::Suggestion { thread_id, text } = events.recv().await.unwrap().event {
                    return (thread_id, text);
                }
            }
        })
        .await
        .expect("a suggestion event")
    };

    fixture
        .hub
        .send_message(&fixture.thread_id, message("one"))
        .await
        .unwrap();
    assert_eq!(
        next_suggestion().await,
        (
            fixture.thread_id.clone(),
            Some("after session one".to_owned())
        )
    );
    let (_, snapshot) = fixture.hub.live_snapshot();
    assert!(
        snapshot.contains(&Event::Suggestion {
            thread_id: fixture.thread_id.clone(),
            text: Some("after session one".to_owned()),
        }),
        "a client connecting now sees it: {snapshot:?}"
    );
    fixture.settled().await;

    fixture
        .hub
        .send_message(&fixture.thread_id, message("two"))
        .await
        .unwrap();
    assert_eq!(
        next_suggestion().await,
        (fixture.thread_id.clone(), None),
        "the next run withdraws it"
    );
    assert_eq!(
        next_suggestion().await,
        (
            fixture.thread_id.clone(),
            Some("after session two".to_owned())
        )
    );
}

fn tool(key: &str, tool: ToolKind) -> EngineEvent {
    EngineEvent::Item {
        key: key.to_owned(),
        parent: None,
        status: Status::Done,
        kind: ItemKind::Tool {
            tool,
            title: None,
            output: None,
            exit_code: Some(0),
            approval: None,
        },
    }
}

/// A turn that only looked around gets a suggestion and no note; a turn
/// that ran a migration and edited files is checked, and what the check
/// found shows in the thread.
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn a_turn_that_changed_things_gets_a_you_should_know_note() {
    let (fixture, asked) = asking(
        Arc::new(|turn, io| {
            Box::pin(async move {
                if turn.input.text == "change it" {
                    io.emit(tool(
                        "migrate",
                        ToolKind::Shell {
                            command: "sqlx migrate run".to_owned().into(),
                            cwd: None,
                            background: false,
                            watch: false,
                        },
                    ))
                    .await;
                    for (key, path) in [("e1", "db/schema.sql"), ("e2", "src/orders/store.rs")] {
                        io.emit(tool(
                            key,
                            ToolKind::Edit {
                                changes: vec![artisan_api::FileChange {
                                    path: path.to_owned(),
                                    op: artisan_api::FileOp::Modify,
                                    diff: None,
                                }]
                                .into(),
                            },
                        ))
                        .await;
                    }
                } else {
                    io.emit(tool(
                        "read",
                        ToolKind::Read {
                            path: "src/lib.rs".to_owned(),
                            range: None,
                        },
                    ))
                    .await;
                }
                io.emit(reply("answer", "Done.")).await;
                Outcome::done()
            })
        }),
        |turn| {
            if turn.input.text.contains("## Note") {
                r#"{"suggestion": "Ship it", "note": {"tag": "Heads up", "text": "Deleting a customer now deletes their orders."}}"#.to_owned()
            } else {
                r#"{"suggestion": "Change it"}"#.to_owned()
            }
        },
    )
    .await;
    let notes = |items: &[Item]| {
        items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    ItemKind::Notice {
                        kind: artisan_api::NoticeKind::YouShouldKnow { .. },
                        ..
                    }
                )
            })
            .count()
    };

    fixture
        .hub
        .send_message(&fixture.thread_id, message("look around"))
        .await
        .unwrap();
    fixture.settled().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while asked.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a side query after the first turn");
    assert!(
        !asked.lock().unwrap()[0].contains("## Note"),
        "looking around asks only for a suggestion"
    );
    assert_eq!(notes(&fixture.items().await), 0);

    fixture
        .hub
        .send_message(&fixture.thread_id, message("change it"))
        .await
        .unwrap();
    fixture.wait_for(|items| notes(items) == 1).await;
    let items = fixture.items().await;
    let note = items
        .iter()
        .find(|item| matches!(item.kind, ItemKind::Notice { .. }))
        .unwrap();
    assert_eq!(
        note.kind,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::YouShouldKnow { heads_up: true },
            text: "Deleting a customer now deletes their orders."
                .to_owned()
                .into(),
        }
    );
}

/// With titles, suggestions and notes off, nothing is asked on the side.
#[tokio::test(flavor = "multi_thread")]
async fn turned_off_side_queries_ask_nothing() {
    let (fixture, asked) = asking(
        Arc::new(|_, io| {
            Box::pin(async move {
                io.emit(reply("answer", "Done.")).await;
                Outcome::done()
            })
        }),
        |_| r#"{"suggestion": "Next"}"#.to_owned(),
    )
    .await;
    for key in [SUGGESTIONS_KEY, YOU_SHOULD_KNOW_KEY, SUMMARY_TITLES_KEY] {
        fixture
            .hub
            .store()
            .set_setting(key.to_owned(), serde_json::json!(false))
            .await
            .unwrap();
    }
    fixture
        .hub
        .send_message(&fixture.thread_id, message("one"))
        .await
        .unwrap();
    fixture.settled().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(asked.lock().unwrap().is_empty());
}

/// Switches the fixture's thread to `engine` with its next message.
fn message_on(engine: EngineId, text: &str) -> SendMessage {
    SendMessage {
        config: Some(EngineConfig::new(engine)),
        ..message(text)
    }
}

/// Runs that reply to each message once per entry of `replies`, recording
/// every prompt.
fn replying(replies: Vec<String>, prompts: Arc<Mutex<Vec<String>>>) -> Script {
    Arc::new(move |turn, io| {
        prompts.lock().unwrap().push(turn.input.text);
        let replies = replies.clone();
        Box::pin(async move {
            for (n, text) in replies.into_iter().enumerate() {
                io.emit(reply(&format!("m{n}"), &text)).await;
            }
            Outcome::done()
        })
    })
}

/// A thread moved to another engine continues the conversation: the new
/// session's first prompt carries the earlier messages, then the new one.
#[tokio::test(flavor = "multi_thread")]
async fn switching_engines_hands_the_conversation_to_the_new_session() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let (fixture, _) = asking(
        replying(vec!["the parser is done".to_owned()], Arc::clone(&prompts)),
        |_| "## Objective\n- ship the parser".to_owned(),
    )
    .await;
    install_fake(fixture.dir.path(), EngineId::Codex);
    fixture
        .hub
        .send_message(&fixture.thread_id, message("ship the parser"))
        .await
        .unwrap();
    fixture.settled().await;
    fixture
        .hub
        .send_message(
            &fixture.thread_id,
            message_on(EngineId::Codex, "now the lexer"),
        )
        .await
        .unwrap();
    fixture.settled().await;

    let prompts = prompts.lock().unwrap().clone();
    assert_eq!(prompts[0], "ship the parser");
    let handed = &prompts[1];
    assert!(
        handed.contains("<summary>\n## Objective\n- ship the parser"),
        "{handed}"
    );
    assert!(
        handed.contains("[User] (Done):\nship the parser\n"),
        "{handed}"
    );
    assert!(
        handed.contains("[Assistant] (Done):\nthe parser is done\n"),
        "{handed}"
    );
    assert!(
        handed.ends_with("</conversation-checkpoint>\n\nnow the lexer"),
        "{handed}"
    );
    // Even a short handoff is summarized; the divider still names the switch.
    let switches: Vec<_> = fixture
        .items()
        .await
        .into_iter()
        .filter_map(|item| {
            if let ItemKind::Compaction { switch, .. } = item.kind {
                Some((item.status, switch))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(switches.len(), 1);
    let (status, switch) = &switches[0];
    assert_eq!(*status, Status::Done);
    let switch = switch.as_ref().expect("the divider names the switch");
    assert_eq!(switch.from.engine, EngineId::Claude);
    assert_eq!(switch.to.engine, EngineId::Codex);
}

/// Older history than the new session's verbatim tail holds is summarized
/// by the session the thread leaves, shown as a compaction while it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_conversation_is_summarized_by_the_session_it_leaves() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let summarizer = Arc::new(Mutex::new(None));
    let asked_by = Arc::clone(&summarizer);
    let replies = (0..6)
        .map(|n| format!("{n}{}", "x".repeat(30_000)))
        .collect();
    let (fixture, _) = asking(replying(replies, Arc::clone(&prompts)), move |turn| {
        if turn.input.text.contains("## Objective") {
            *asked_by.lock().unwrap() = Some((turn.engine, turn.session_id.clone()));
            "## Objective\n- ship the parser".to_owned()
        } else {
            "{}".to_owned()
        }
    })
    .await;
    install_fake(fixture.dir.path(), EngineId::Codex);
    fixture
        .hub
        .send_message(&fixture.thread_id, message("ship the parser"))
        .await
        .unwrap();
    fixture.settled().await;
    fixture
        .hub
        .send_message(
            &fixture.thread_id,
            message_on(EngineId::Codex, "now the lexer"),
        )
        .await
        .unwrap();
    fixture.settled().await;

    assert_eq!(
        *summarizer.lock().unwrap(),
        Some((EngineId::Claude, Some("session ship the parser".to_owned())))
    );
    let handed = prompts.lock().unwrap()[1].clone();
    assert!(
        handed.contains("<summary>\n## Objective\n- ship the parser\n</summary>"),
        "{handed}"
    );
    // One oversized exchange stays in the summary, rather than an orphaned
    // tail of assistant messages with no user request.
    assert!(handed.contains("<recent-context>\n\n</recent-context>"));
    let compactions: Vec<_> = fixture
        .items()
        .await
        .into_iter()
        .filter(|item| matches!(item.kind, ItemKind::Compaction { .. }))
        .collect();
    assert_eq!(compactions.len(), 1);
    assert_eq!(compactions[0].status, Status::Done);
    let ItemKind::Compaction {
        summary: Some(summary),
        ..
    } = &compactions[0].kind
    else {
        panic!("checkpoint saved");
    };
    assert_eq!(
        fixture.hub.store().resolve_text(summary).await.unwrap(),
        "## Objective\n- ship the parser"
    );
}

/// A failed summary does not silently lose context or deliver the pending input.
#[tokio::test(flavor = "multi_thread")]
async fn a_switch_without_a_summary_fails_without_delivering_the_new_message() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let (fixture, asked) = asking(
        replying(vec!["parser done".into()], Arc::clone(&prompts)),
        |_| String::new(),
    )
    .await;
    install_fake(fixture.dir.path(), EngineId::Codex);
    fixture
        .hub
        .send_message(&fixture.thread_id, message("ship the parser"))
        .await
        .unwrap();
    fixture.settled().await;
    fixture
        .hub
        .send_message(
            &fixture.thread_id,
            message_on(EngineId::Codex, "now the lexer"),
        )
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(
        prompts.lock().unwrap().len(),
        1,
        "new session never started"
    );
    assert!(runs.iter().any(|run| {
        run.status == RunStatus::Failed
            && run
                .error
                .as_deref()
                .is_some_and(|error| error.contains("required template"))
    }));
    assert!(
        asked
            .lock()
            .unwrap()
            .iter()
            .any(|prompt| prompt.contains(forge_engines::COMPACTION_NUDGE)),
        "one format reminder"
    );
    assert!(fixture.items().await.iter().any(|item| matches!(
        item.kind,
        ItemKind::Compaction { summary: None, .. }
    ) && item.status != Status::Done));
}

/// A run that fails before its engine says anything shows why under the
/// message, instead of leaving it unanswered; an engine that reported its
/// own error isn't repeated.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_run_says_why_once() {
    let fixture = fixture(Arc::new(|turn, io| {
        Box::pin(async move {
            if turn.input.text == "own error" {
                io.emit(EngineEvent::Item {
                    key: "error".to_owned(),
                    parent: None,
                    status: Status::Done,
                    kind: ItemKind::Error {
                        kind: artisan_api::ErrorKind::Api,
                        message: "rate limited".into(),
                        retryable: true,
                    },
                })
                .await;
            }
            Outcome::failed("Codex thread/resume: no rollout found for thread id t")
        })
    }))
    .await;
    for text in ["silent", "own error"] {
        fixture
            .hub
            .send_message(&fixture.thread_id, message(text))
            .await
            .unwrap();
        fixture.settled().await;
    }
    let errors: Vec<_> = fixture
        .items()
        .await
        .into_iter()
        .filter_map(|item| {
            if let ItemKind::Error { message, .. } = item.kind {
                message.inline().map(str::to_owned)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        errors,
        [
            "Codex thread/resume: no rollout found for thread id t",
            "rate limited"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thread_without_a_project_works_in_its_own_folder() {
    let folders = Arc::new(Mutex::new(Vec::new()));
    let fixture = fixture(Arc::new({
        let folders = Arc::clone(&folders);
        move |turn, _| {
            folders.lock().unwrap().push((turn.thread_id, turn.cwd));
            Box::pin(async { Outcome::done() })
        }
    }))
    .await;
    let hub = &fixture.hub;
    hub.ensure_scratch_project().await.unwrap();
    let scratch = hub.store().project(&ProjectId::scratch()).await.unwrap();
    assert_eq!(
        scratch.map(|project| project.name),
        Some("No project".to_owned())
    );
    let mut threads = Vec::new();
    for _ in 0..2 {
        let thread = hub
            .store()
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: ProjectId::scratch(),
                config: EngineConfig::new(EngineId::Claude),
                title: None,
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .unwrap();
        threads.push(thread.id);
    }
    for text in ["Sketch a parser", "And test it"] {
        for thread in &threads {
            hub.send_message(thread, message(text)).await.unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                while hub.is_live(thread)
                    || hub
                        .store()
                        .pending_threads(8, Vec::new())
                        .await
                        .unwrap()
                        .contains(thread)
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("run settles");
        }
    }
    let folders = folders.lock().unwrap().clone();
    let folder_of = |thread: &ThreadId| {
        let mut seen: Vec<_> = folders
            .iter()
            .filter(|(id, _)| id == thread)
            .map(|(_, cwd)| cwd.clone())
            .collect();
        assert_eq!(seen.len(), 2, "two runs of {thread}");
        seen.dedup();
        assert_eq!(
            seen.len(),
            1,
            "every run of {thread} works in the same folder"
        );
        seen.remove(0)
    };
    let (first, second) = (folder_of(&threads[0]), folder_of(&threads[1]));
    assert_ne!(first, second);
    for folder in [&first, &second] {
        assert!(folder.is_dir());
        assert_eq!(
            folder.parent(),
            Some(hub.data_dir().join("scratch").as_path())
        );
        let name = folder.file_name().unwrap().to_string_lossy();
        assert!(name.contains("-sketch-a-parser-"), "{name}");
    }
}

/// What each run's engine was asked: the input text and the session it
/// resumed.
type Asked = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// The first run starts a background shell and waits to be stopped; every
/// later run answers at once.
fn stoppable(asked: &Asked) -> Script {
    let asked = Arc::clone(asked);
    Arc::new(move |turn, io| {
        let asked = Arc::clone(&asked);
        Box::pin(async move {
            let first = {
                let mut asked = asked.lock().unwrap();
                asked.push((turn.input.text.clone(), turn.session_id.clone()));
                asked.len() == 1
            };
            if first {
                io.emit(EngineEvent::Item {
                    key: "watch".into(),
                    parent: None,
                    status: Status::Running,
                    kind: ItemKind::Tool {
                        tool: ToolKind::Shell {
                            command: "cargo watch -x test".into(),
                            cwd: None,
                            background: true,
                            watch: false,
                        },
                        title: None,
                        output: None,
                        exit_code: None,
                        approval: None,
                    },
                })
                .await;
                io.cancel.cancelled().await;
                return Outcome::cancelled();
            }
            io.emit(reply("done", "picked it back up")).await;
            Outcome::done()
        })
    })
}

fn user_messages(items: &[Item]) -> usize {
    items
        .iter()
        .filter(|item| matches!(item.kind, ItemKind::UserMessage { .. }))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_turn_continues_without_a_message() {
    let asked = Asked::default();
    let fixture = fixture(stoppable(&asked)).await;
    // Nothing has stopped yet.
    assert!(fixture.hub.continue_turn(&fixture.thread_id).await.is_err());
    fixture
        .hub
        .send_message(&fixture.thread_id, message("refactor the parser"))
        .await
        .unwrap();
    fixture.wait_for(|items| items.len() == 2).await;
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    assert_eq!(fixture.settled().await[0].status, RunStatus::Cancelled);

    fixture.hub.continue_turn(&fixture.thread_id).await.unwrap();
    let continued = fixture.settled().await.remove(0);
    assert_eq!(
        continued.continuation,
        Some(artisan_api::Continuation::Requested)
    );
    assert_eq!(continued.status, RunStatus::Done);

    let asked = asked.lock().unwrap().clone();
    let (prompt, session) = &asked[1];
    assert_eq!(session.as_deref(), Some("session"));
    assert!(prompt.starts_with("Continue from where you left off."));
    // The shell the stop ended is named, so the agent starts it again.
    assert!(prompt.contains("background shell: cargo watch -x test"));

    // The user sees the turn go on, not a message they didn't send.
    let items = fixture.items().await;
    assert_eq!(user_messages(&items), 1);
    assert!(
        texts(&items)
            .iter()
            .any(|(text, _)| text == "picked it back up")
    );
    // Finished turns have nothing to continue.
    assert!(fixture.hub.continue_turn(&fixture.thread_id).await.is_err());
}

/// Stopped before the engine read the message, Continue sends the message,
/// not a bare "continue" the engine can't make sense of.
#[tokio::test(flavor = "multi_thread")]
async fn continuing_a_turn_stopped_before_its_message_sends_the_message() {
    // No `Scripted`: it acknowledges the input before the script runs.
    struct Unread(Script);
    impl Harness for Unread {
        fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
            (self.0)(turn, io)
        }
    }
    let asked = Asked::default();
    let script: Script = {
        let asked = Arc::clone(&asked);
        Arc::new(move |turn, io| {
            let asked = Arc::clone(&asked);
            Box::pin(async move {
                let first = {
                    let mut asked = asked.lock().unwrap();
                    asked.push((turn.input.text.clone(), turn.session_id.clone()));
                    asked.len() == 1
                };
                if first {
                    // Stopped before it read anything.
                    io.cancel.cancelled().await;
                    return Outcome::cancelled();
                }
                io.emit(EngineEvent::InputAccepted {
                    id: turn.input.id.clone(),
                })
                .await;
                io.emit(reply("done", "the wheel goes left")).await;
                Outcome::done()
            })
        })
    };
    let harness: Arc<dyn Harness> = Arc::new(Unread(script));
    let fixture = fixture_with_harness(Arc::new(move |_| Arc::clone(&harness))).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("where should the wheel go?"))
        .await
        .unwrap();
    while asked.lock().unwrap().is_empty() {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    assert_eq!(fixture.settled().await[0].status, RunStatus::Cancelled);
    let stopped = fixture.items().await;
    assert!(
        stopped
            .iter()
            .any(|item| matches!(item.kind, ItemKind::UserMessage { .. })
                && item.status == Status::Cancelled),
        "the stop caught the message before the engine read it"
    );

    fixture.hub.continue_turn(&fixture.thread_id).await.unwrap();
    let continued = fixture.settled().await.remove(0);
    assert_eq!(continued.status, RunStatus::Done);
    assert_eq!(continued.continuation, None);
    let asked = asked.lock().unwrap().clone();
    assert!(
        asked[1].0.contains("where should the wheel go?"),
        "{asked:?}"
    );
    let items = fixture.items().await;
    assert_eq!(user_messages(&items), 1);
    assert!(
        texts(&items)
            .iter()
            .any(|(text, _)| text == "the wheel goes left")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_running_when_forge_stopped_carries_on_after_the_restart() {
    let asked = Asked::default();
    let fixture = fixture(stoppable(&asked)).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("refactor the parser"))
        .await
        .unwrap();
    fixture.wait_for(|items| items.len() == 2).await;
    fixture.hub.stop(Duration::from_secs(10)).await;
    let Fixture {
        hub,
        thread_id,
        dir,
    } = fixture;
    drop(hub);
    let data = dir.path().to_path_buf();

    // The next Forge on the same data.
    let harness: Arc<dyn Harness> = Arc::new(Scripted(stoppable(&asked)));
    let store = forge_store::Store::open(&data.join("forge")).unwrap();
    let hub = Hub::with_harness(
        store,
        Engines::new(&data),
        data.clone(),
        Arc::new(move |_| Arc::clone(&harness)),
    );
    hub.resume_pending();
    let continued = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let latest = hub.store().latest_run(&thread_id).await.unwrap().unwrap();
            if latest.status == RunStatus::Done {
                return latest;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the interrupted turn carries on");
    assert_eq!(
        continued.continuation,
        Some(artisan_api::Continuation::Restart)
    );
    let asked = asked.lock().unwrap().clone();
    let (prompt, session) = &asked[1];
    assert_eq!(session.as_deref(), Some("session"));
    assert!(prompt.starts_with("Artisan restarted and interrupted this turn."));
    assert!(prompt.contains("background shell: cargo watch -x test"));
    hub.shutdown();
}

/// An Artisan agent of the fixture's thread whose own thread's last run a
/// restart interrupted, as a Forge starting up finds it.
async fn interrupted_agent(fixture: &Fixture) -> (ItemId, ThreadId) {
    let store = fixture.hub.store();
    let parent = store.thread(&fixture.thread_id).await.unwrap().unwrap();
    let agent = ItemId::new();
    store
        .upsert_item(forge_store::NewItem {
            id: agent.clone(),
            thread_id: fixture.thread_id.clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Running,
            kind: ItemKind::Agent {
                name: None,
                description: "Review the diff".into(),
                prompt: None,
                agent_id: None,
                context_used: None,
                model: None,
                background: true,
                result: None,
                label: None,
                engine: Some(EngineId::Claude),
                thread: None,
            },
        })
        .await
        .unwrap();
    let child = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: parent.project_id,
            config: parent.config,
            title: None,
            parent_thread_id: Some(fixture.thread_id.clone()),
            spawned_by: Some(agent.clone()),
        })
        .await
        .unwrap()
        .id;
    let mut item = store.item(&agent).await.unwrap().unwrap();
    if let ItemKind::Agent { thread, .. } = &mut item.kind {
        *thread = Some(child.clone());
    }
    store
        .upsert_item(forge_store::NewItem {
            id: item.id,
            thread_id: item.thread_id,
            run_id: None,
            parent_id: None,
            native_id: None,
            status: item.status,
            kind: item.kind,
        })
        .await
        .unwrap();
    store
        .upsert_run(artisan_api::Run {
            config: None,
            id: RunId::new(),
            thread_id: child.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: 0,
            status: RunStatus::Interrupted,
            usage: artisan_api::Usage::default(),
            session_id: Some("child-session".into()),
            error: None,
            seq: 0,
            started_at: artisan_api::now(),
            ended_at: Some(artisan_api::now()),
            continuation: None,
        })
        .await
        .unwrap();
    (agent, child)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_a_restart_interrupted_keeps_working_while_it_carries_on() {
    let fixture = fixture(Arc::new(|_, io| {
        Box::pin(async move {
            io.cancel.cancelled().await;
            Outcome::cancelled()
        })
    }))
    .await;
    let (agent, child) = interrupted_agent(&fixture).await;
    fixture.hub.store().request_continue(child).await.unwrap();
    fixture.hub.reconcile_agents().await;
    let item = fixture.hub.store().item(&agent).await.unwrap().unwrap();
    assert!(!item.status.is_terminal());
    assert_eq!(user_messages(&fixture.items().await), 0);
    fixture.hub.shutdown();
}

/// Cancellation reports, and results racing with Stop, must not become
/// fresh inputs that restart the parent the user just stopped.
#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_thread_with_background_agents_does_not_restart_it() {
    let fixture = fixture(Arc::new(|turn, io| {
        Box::pin(async move {
            if turn.input.text.contains("resume") {
                return Outcome::done();
            }
            io.cancel.cancelled().await;
            Outcome::cancelled()
        })
    }))
    .await;
    let (agent, child) = interrupted_agent(&fixture).await;
    for (thread, text) in [(&fixture.thread_id, "parent"), (&child, "child")] {
        fixture
            .hub
            .send_message(thread, message(text))
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut running = true;
            for thread in [&fixture.thread_id, &child] {
                running &= fixture
                    .hub
                    .store()
                    .latest_run(thread)
                    .await
                    .unwrap()
                    .is_some_and(|run| run.status == RunStatus::Running);
            }
            if running {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent and child start");
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture.hub.is_live(&fixture.thread_id) || fixture.hub.is_live(&child) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Stop settles both threads without restarting the parent");
    // Also exercise reconciliation after the live cancellation path.
    fixture.hub.reconcile_agents().await;
    let items = fixture.items().await;
    assert!(
        !items.iter().any(|item| matches!(
            &item.kind, ItemKind::UserMessage { from_agent: Some(from), .. } if from == &agent
        )),
        "an agent report restarted the stopped parent: {items:?}"
    );
    assert_eq!(
        fixture
            .hub
            .store()
            .latest_run(&fixture.thread_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        RunStatus::Cancelled
    );
    assert!(
        fixture
            .hub
            .store()
            .pending_inputs(&fixture.thread_id)
            .await
            .unwrap()
            .items
            .is_empty()
    );

    fixture
        .hub
        .send_message(&fixture.thread_id, message("resume"))
        .await
        .unwrap();
    fixture.settled().await;
    assert_eq!(
        fixture
            .hub
            .store()
            .latest_run(&fixture.thread_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        RunStatus::Done
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_successful_agent_result_arriving_after_stop_does_not_restart_its_parent() {
    let fixture = fixture(Arc::new(|_, io| {
        Box::pin(async move {
            io.cancel.cancelled().await;
            Outcome::cancelled()
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("parent"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| {
            items.iter().any(|item| {
                matches!(item.kind, ItemKind::UserMessage { .. }) && item.status == Status::Done
            })
        })
        .await;
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    fixture.settled().await;

    // The child's successful outcome was persisted, but its report has
    // not reached the parent yet when Stop finishes.
    let (_, child) = interrupted_agent(&fixture).await;
    let mut run = fixture
        .hub
        .store()
        .latest_run(&child)
        .await
        .unwrap()
        .unwrap();
    run.status = RunStatus::Done;
    fixture.hub.store().upsert_run(run).await.unwrap();
    fixture.hub.reconcile_agents().await;
    assert_eq!(user_messages(&fixture.items().await), 1);
    assert!(!fixture.hub.is_live(&fixture.thread_id));
    assert!(
        fixture
            .hub
            .store()
            .pending_inputs(&fixture.thread_id)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_a_restart_ended_reports_to_its_parent() {
    let fixture = fixture(Arc::new(|_, io| {
        Box::pin(async move {
            io.cancel.cancelled().await;
            Outcome::cancelled()
        })
    }))
    .await;
    let (agent, _child) = interrupted_agent(&fixture).await;
    fixture.hub.reconcile_agents().await;
    let item = fixture.hub.store().item(&agent).await.unwrap().unwrap();
    assert!(item.status.is_terminal());
    // The parent hears it failed instead of waiting on it forever.
    fixture
        .wait_for(|items| {
            items.iter().any(|item| {
                matches!(&item.kind, ItemKind::UserMessage { from_agent: Some(from), .. } if from == &agent)
            })
        })
        .await;
    // The report reads as the agent's, so the turn navigator leaves it out.
    let items = fixture.items().await;
    let report = items
        .iter()
        .find(|item| matches!(&item.kind, ItemKind::UserMessage { from_agent: Some(from), .. } if from == &agent))
        .unwrap();
    let outline = fixture
        .hub
        .store()
        .inspector(&fixture.thread_id)
        .await
        .unwrap()
        .outline;
    assert!(outline.iter().all(|entry| entry.id != report.id));
    fixture.hub.shutdown();
}

/// Another thread with `permissions`, in the fixture's project.
async fn second_thread(fixture: &Fixture, permissions: Permissions) -> ThreadId {
    let store = fixture.hub.store();
    let first = store.thread(&fixture.thread_id).await.unwrap().unwrap();
    let mut config = first.config.clone();
    config.permissions = permissions;
    store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: first.project_id,
            config,
            title: Some("Full access work".to_owned()),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap()
        .id
}

async fn items_of(hub: &Hub, thread: &ThreadId) -> Vec<Item> {
    let query = ItemsQuery {
        before: None,
        after: None,
        limit: Some(500),
    };
    hub.store().items_page(thread, query).await.unwrap().items
}

/// The approval card `thread` shows, once it does.
async fn approval_card(hub: &Hub, thread: &ThreadId) -> Item {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(card) = items_of(hub, thread)
                .await
                .into_iter()
                .find(|item| item.status == Status::AwaitingUser)
            {
                return card;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("an approval card")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_message_to_a_full_access_thread_waits_for_the_user_and_names_its_sender() {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let fixture = fixture(Arc::new({
        let heard = Arc::clone(&heard);
        move |turn, _io| {
            heard.lock().unwrap().push(turn.input.text);
            Box::pin(async { Outcome::done() })
        }
    }))
    .await;
    let caller = fixture.thread_id.clone();
    let target = second_thread(&fixture, Permissions::Full).await;
    let send = tokio::spawn({
        let hub = fixture.hub.clone();
        let (caller, target) = (caller.clone(), target.clone());
        async move {
            hub.send_as_agent(&caller, &target, "/compact and fix #1".into())
                .await
        }
    });
    let card = approval_card(&fixture.hub, &caller).await;
    assert!(
        items_of(&fixture.hub, &target).await.is_empty(),
        "nothing reaches the full-access thread before the user answers"
    );
    fixture
        .hub
        .answer(
            &card.id,
            artisan_api::Answer::Approval {
                decision: Decision::AllowOnce,
            },
        )
        .await
        .unwrap();
    let delivered = send.await.unwrap().unwrap();
    let ItemKind::UserMessage { sender, .. } = &delivered.kind else {
        panic!("not a message: {delivered:?}");
    };
    let sender = sender.as_ref().expect("the message names its sender");
    assert_eq!(sender.thread, caller);
    assert_eq!(sender.engine, EngineId::Claude);
    tokio::time::timeout(Duration::from_secs(10), async {
        while heard.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the target's engine runs");
    let heard = heard.lock().unwrap()[0].clone();
    assert!(
        heard.starts_with("[A Claude Code agent in the Artisan thread"),
        "the engine is told another agent wrote it: {heard}"
    );
    assert!(
        heard.ends_with("\n\n/compact and fix #1"),
        "a leading slash stays text: {heard}"
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_message_never_reaches_the_full_access_thread() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let caller = fixture.thread_id.clone();
    let target = second_thread(&fixture, Permissions::Full).await;
    let send = tokio::spawn({
        let hub = fixture.hub.clone();
        let (caller, target) = (caller.clone(), target.clone());
        async move {
            hub.send_as_agent(&caller, &target, "run rm -rf".into())
                .await
        }
    });
    let card = approval_card(&fixture.hub, &caller).await;
    fixture
        .hub
        .answer(
            &card.id,
            artisan_api::Answer::Approval {
                decision: Decision::DenyOnce,
            },
        )
        .await
        .unwrap();
    assert!(send.await.unwrap().is_err());
    assert!(items_of(&fixture.hub, &target).await.is_empty());
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_message_to_an_equal_thread_goes_without_asking() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let target = second_thread(&fixture, Permissions::Auto).await;
    let sent = fixture
        .hub
        .send_as_agent(&fixture.thread_id, &target, "hello".into())
        .await
        .unwrap();
    assert!(matches!(
        &sent.kind,
        ItemKind::UserMessage { sender: Some(sender), .. } if sender.thread == fixture.thread_id
    ));
    assert!(
        !items_of(&fixture.hub, &fixture.thread_id)
            .await
            .iter()
            .any(|item| item.status == Status::AwaitingUser)
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_access_thread_adds_an_mcp_server_without_a_card() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let thread = second_thread(&fixture, Permissions::Full).await;
    fixture
        .hub
        .ask_approval(
            &thread,
            "mcp_server_add",
            "Give every harness the MCP server x".into(),
        )
        .await
        .unwrap();
    assert!(
        items_of(&fixture.hub, &thread).await.is_empty(),
        "full access shows no approval card"
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_to_full_access_allows_the_card_that_waits() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let thread = fixture.thread_id.clone();
    let asking = tokio::spawn({
        let (hub, thread) = (fixture.hub.clone(), thread.clone());
        async move {
            hub.ask_approval(
                &thread,
                "mcp_server_add",
                "Give every harness the MCP server x".into(),
            )
            .await
        }
    });
    let card = approval_card(&fixture.hub, &thread).await;
    let mut config = fixture
        .hub
        .store()
        .thread(&thread)
        .await
        .unwrap()
        .unwrap()
        .config;
    config.permissions = Permissions::Full;
    fixture
        .hub
        .update_thread(
            &thread,
            artisan_api::UpdateThread {
                config: Some(config),
                ..artisan_api::UpdateThread::default()
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), asking)
        .await
        .expect("the card is answered")
        .unwrap()
        .unwrap();
    let card = items_of(&fixture.hub, &thread)
        .await
        .into_iter()
        .find(|item| item.id == card.id)
        .unwrap();
    assert_eq!(card.status, Status::Done);
    fixture.hub.shutdown();
}

/// A run whose disk has `free` bytes, checked every few milliseconds.
async fn fixture_on_disk(script: Script, free: Arc<AtomicU64>) -> Fixture {
    let harness: Arc<dyn Harness> = Arc::new(Scripted(script));
    let probe = disk::Probe {
        free_space: Arc::new(move |_| Ok(free.load(Ordering::Relaxed))),
        every: Duration::from_millis(10),
    };
    fixture_with_parts(
        EngineId::Claude,
        Arc::new(move |_| Arc::clone(&harness)),
        probe,
    )
    .await
}

const GIB: u64 = 1 << 30;

fn notices(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Notice { text, .. } = &item.kind {
                text.inline().map(str::to_owned)
            } else {
                None
            }
        })
        .collect()
}

fn errors(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Error { message, .. } = &item.kind {
                message.inline().map(str::to_owned)
            } else {
                None
            }
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_low_disk_tells_the_agent_once_to_free_space() {
    let free = Arc::new(AtomicU64::new(7 * GIB + GIB / 2));
    let fixture = fixture_on_disk(
        Arc::new(|_turn, mut io| {
            Box::pin(async move {
                let Some(Input::Steer(steer)) = io.inbox.recv().await else {
                    return Outcome::failed("no steer");
                };
                // Many more checks pass while the disk stays low.
                tokio::time::sleep(Duration::from_millis(200)).await;
                if io.inbox.try_recv().is_ok() {
                    return Outcome::failed("told twice");
                }
                io.emit(reply("m1", &steer.text)).await;
                Outcome::done()
            })
        }),
        free,
    )
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("build it"))
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status, RunStatus::Done, "{:?}", runs[0].error);
    let items = fixture.items().await;
    let told = texts(&items)
        .into_iter()
        .map(|(text, _)| text)
        .find(|text| text.starts_with("Disk low:"))
        .expect("the agent read the steer");
    assert!(told.contains("has 7.5 GiB free"), "{told}");
    assert!(told.contains("Never delete source, uncommitted or unmerged work"));
    let shown = notices(&items);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(
        shown[0].starts_with("Disk low: 7.5 GiB free on "),
        "{shown:?}"
    );
    assert!(
        !items.iter().any(|item| matches!(
            &item.kind,
            ItemKind::UserMessage { text, .. } if text.inline().is_some_and(|text| text.starts_with("Disk low"))
        )),
        "the steer is not shown as the user's message"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_disk_stops_the_run_and_says_why() {
    let free = Arc::new(AtomicU64::new(50 * GIB));
    let fixture = fixture_on_disk(
        Arc::new(|_turn, io| {
            Box::pin(async move {
                io.cancel.cancelled().await;
                Outcome::cancelled()
            })
        }),
        Arc::clone(&free),
    )
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("build it"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(fixture.hub.is_live(&fixture.thread_id));
    free.store(2 * GIB, Ordering::Relaxed);
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status, RunStatus::Failed);
    let shown = errors(&fixture.items().await);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(
        shown[0].starts_with("Stopped: only 2 GiB free on "),
        "{shown:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_sent_on_a_full_disk_can_clean_it_up() {
    let free = Arc::new(AtomicU64::new(2 * GIB));
    let disk = Arc::clone(&free);
    let fixture = fixture_on_disk(
        Arc::new(move |_turn, mut io| {
            let disk = Arc::clone(&disk);
            Box::pin(async move {
                // Told to free space, it does.
                let Some(Input::Steer(_)) = io.inbox.recv().await else {
                    return Outcome::failed("no steer");
                };
                disk.store(40 * GIB, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(100)).await;
                io.emit(reply("m1", "freed 38 GiB")).await;
                Outcome::done()
            })
        }),
        free,
    )
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("clear the build caches"))
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status, RunStatus::Done, "{:?}", runs[0].error);
    assert!(errors(&fixture.items().await).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_sent_on_a_full_disk_stops_if_it_keeps_filling_it() {
    let free = Arc::new(AtomicU64::new(2 * GIB));
    let fixture = fixture_on_disk(
        Arc::new(|_turn, io| {
            Box::pin(async move {
                io.cancel.cancelled().await;
                Outcome::cancelled()
            })
        }),
        Arc::clone(&free),
    )
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("build it"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(fixture.hub.is_live(&fixture.thread_id));
    free.store(GIB / 2, Ordering::Relaxed);
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status, RunStatus::Failed);
    let shown = errors(&fixture.items().await);
    assert!(
        shown[0].starts_with("Stopped: only 0.5 GiB free on "),
        "{shown:?}"
    );
}

/// Each run records its prompt and session; run `n` (from 0) ends as
/// `outcomes[n]`, and once they run out, replies and finishes.
fn failing_then_done(asked: &Asked, outcomes: Vec<Outcome>) -> Script {
    let asked = Arc::clone(asked);
    let outcomes = Arc::new(outcomes);
    Arc::new(move |turn, io| {
        let asked = Arc::clone(&asked);
        let outcomes = Arc::clone(&outcomes);
        Box::pin(async move {
            let run = {
                let mut asked = asked.lock().unwrap();
                asked.push((turn.input.text.clone(), turn.session_id.clone()));
                asked.len() - 1
            };
            if let Some(outcome) = outcomes.get(run) {
                return outcome.clone();
            }
            io.emit(reply("done", "the parser is refactored")).await;
            Outcome::done()
        })
    })
}

impl Fixture {
    /// The thread's latest run once it is `status`.
    async fn latest_run_once(&self, status: RunStatus) -> artisan_api::Run {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let latest = self.hub.store().latest_run(&self.thread_id).await.unwrap();
                if let Some(latest) = latest.filter(|run| run.status == status) {
                    return latest;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the thread's latest run reaches the status")
    }

    async fn thread(&self) -> artisan_api::Thread {
        self.hub
            .store()
            .thread(&self.thread_id)
            .await
            .unwrap()
            .unwrap()
    }

    /// Every run that left an item in the thread, oldest first.
    async fn runs(&self) -> Vec<artisan_api::Run> {
        let mut ids: Vec<RunId> = Vec::new();
        for item in self.items().await {
            if let Some(id) = item.run_id
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        let mut runs = Vec::new();
        for id in ids {
            runs.extend(self.hub.store().run(&id).await.unwrap());
        }
        runs.sort_by_key(|run| run.started_at);
        runs
    }
}

fn at_capacity() -> Outcome {
    Outcome::retryable(
        "Selected model is at capacity. Please try a different model.",
        forge_engines::Retry::Overloaded,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_failed_at_capacity_is_retried_until_it_goes_through() {
    let asked = Asked::default();
    let fixture = fixture(failing_then_done(
        &asked,
        vec![at_capacity(), at_capacity()],
    ))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("refactor the parser"))
        .await
        .unwrap();
    fixture.latest_run_once(RunStatus::Done).await;

    let continuations: Vec<_> = fixture
        .runs()
        .await
        .iter()
        .map(|run| (run.status, run.continuation))
        .collect();
    assert_eq!(
        continuations,
        [
            (RunStatus::Failed, None),
            (
                RunStatus::Failed,
                Some(artisan_api::Continuation::Overloaded)
            ),
            (RunStatus::Done, Some(artisan_api::Continuation::Overloaded)),
        ]
    );
    let asked = asked.lock().unwrap().clone();
    let (prompt, session) = &asked[2];
    assert_eq!(session.as_deref(), Some("session"));
    assert!(
        prompt.starts_with("The provider was overloaded"),
        "{prompt}"
    );
    assert_eq!(fixture.thread().await.parked_until, None);
    let items = fixture.items().await;
    assert_eq!(user_messages(&items), 1, "a retry sends no message");
    assert!(
        texts(&items)
            .iter()
            .any(|(text, _)| text == "the parser is refactored")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_the_usage_limit_stopped_waits_parked_until_the_limit_resets() {
    let asked = Asked::default();
    let resets_at = artisan_api::now() + 400;
    let limited = Outcome::retryable(
        "You've hit your limit",
        forge_engines::Retry::UsageLimit {
            resets_at: Some(resets_at),
        },
    );
    let fixture = fixture(failing_then_done(&asked, vec![limited])).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("refactor the parser"))
        .await
        .unwrap();
    fixture.latest_run_once(RunStatus::Failed).await;
    let parked = fixture.thread().await.parked_until.expect("parked");
    assert!(parked > resets_at, "it waits past the reset");

    let continued = fixture.latest_run_once(RunStatus::Done).await;
    assert!(continued.started_at >= parked);
    assert_eq!(
        continued.continuation,
        Some(artisan_api::Continuation::UsageLimit)
    );
    let asked = asked.lock().unwrap().clone();
    assert!(asked[1].0.starts_with("The usage limit stopped this turn"));
    assert_eq!(fixture.thread().await.parked_until, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_parked_thread_cancels_its_pickup() {
    let asked = Asked::default();
    let limited = Outcome::retryable(
        "You've hit your limit",
        forge_engines::Retry::UsageLimit {
            resets_at: Some(artisan_api::now() + 60_000),
        },
    );
    let fixture = fixture(failing_then_done(&asked, vec![limited])).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("refactor the parser"))
        .await
        .unwrap();
    fixture.latest_run_once(RunStatus::Failed).await;
    assert!(fixture.thread().await.parked_until.is_some());

    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    assert_eq!(fixture.thread().await.parked_until, None);
    assert!(
        !fixture
            .hub
            .store()
            .continue_pending(&fixture.thread_id)
            .await
            .unwrap()
    );
    assert_eq!(fixture.runs().await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_retrying_off_a_turn_failed_at_capacity_stays_failed() {
    let asked = Asked::default();
    let fixture = fixture(failing_then_done(&asked, vec![at_capacity()])).await;
    fixture
        .hub
        .store()
        .set_setting(
            forge_store::RETRY_WHEN_OVERLOADED_KEY.to_owned(),
            serde_json::Value::Bool(false),
        )
        .await
        .unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("refactor the parser"))
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Failed);
    assert_eq!(fixture.thread().await.parked_until, None);
    assert!(
        !fixture
            .hub
            .store()
            .continue_pending(&fixture.thread_id)
            .await
            .unwrap()
    );
}

/// The source engine/model determines the compactor even after a switch.
/// An obsolete saved curated-model preference is ignored.
#[tokio::test(flavor = "multi_thread")]
async fn compaction_always_inherits_the_source_model_and_settings() {
    for engine in [EngineId::Claude, EngineId::Codex, EngineId::OpenCode] {
        for legacy_preference in [false, true] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let capture = Arc::clone(&seen);
            let (fixture, _) = asking(
                Arc::new(|_, io| {
                    Box::pin(async move {
                        io.emit(reply("answer", "Finished.")).await;
                        Outcome::done()
                    })
                }),
                move |turn| {
                    if turn.input.text.contains("## Objective") {
                        assert_eq!(turn.config.permissions, Permissions::Full);
                        assert_eq!(turn.config.effort.as_deref(), Some("high"));

                        capture.lock().unwrap().push((
                            turn.engine,
                            turn.config.model.clone(),
                            turn.session_id.clone(),
                        ));
                        "## Objective\n- Continue".to_owned()
                    } else {
                        "{}".to_owned()
                    }
                },
            )
            .await;
            install_fake(fixture.dir.path(), engine);
            let destination = if engine == EngineId::Codex {
                EngineId::Claude
            } else {
                EngineId::Codex
            };
            install_fake(fixture.dir.path(), destination);
            // The initial empty thread has no history and needs no compaction.
            let mut config = EngineConfig::new(engine);
            config.model = Some("source-model".into());
            config.permissions = Permissions::Full;
            config.effort = Some("high".into());

            fixture
                .hub
                .send_message(
                    &fixture.thread_id,
                    SendMessage {
                        config: Some(config),
                        ..message("first task")
                    },
                )
                .await
                .unwrap();
            fixture.settled().await;
            fixture
                .hub
                .store()
                .set_setting(
                    "curated_compaction_models".to_owned(),
                    serde_json::json!(legacy_preference),
                )
                .await
                .unwrap();
            fixture
                .hub
                .send_message(&fixture.thread_id, message_on(destination, "continue task"))
                .await
                .unwrap();
            let runs = fixture.settled().await;
            assert!(
                runs.iter().all(|run| run.status == RunStatus::Done),
                "{runs:?}"
            );
            assert_eq!(
                *seen.lock().unwrap(),
                vec![(
                    engine,
                    Some("source-model".to_owned()),
                    Some("session first task".to_owned())
                )]
            );
            let thread = fixture
                .hub
                .store()
                .thread(&fixture.thread_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(thread.config.engine, destination);
            assert_eq!(thread.config.model, None);
        }
    }
}

/// A lost native session uses canonical tool evidence, resolved from blobs,
/// without mislabelling commentary or replaying the undelivered request.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_session_summarizes_tool_history_with_correct_roles() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let capture = Arc::clone(&prompts);
    let (fixture, _) = asking(
        Arc::new(move |turn, io| {
            capture.lock().unwrap().push(turn.input.text);
            Box::pin(async move {
                io.emit(EngineEvent::Session { id: String::new() }).await;
                io.emit(EngineEvent::Item {
                    key: "comment".into(),
                    parent: None,
                    status: Status::Done,
                    kind: ItemKind::AssistantMessage {
                        text: "Checking the parser".into(),
                        phase: Phase::Commentary,
                    },
                })
                .await;
                io.emit(EngineEvent::Item {
                    key: "test".into(),
                    parent: None,
                    status: Status::Done,
                    kind: ItemKind::Tool {
                        tool: ToolKind::Shell {
                            command: "cargo test parser".into(),
                            cwd: None,
                            background: false,
                            watch: false,
                        },
                        title: None,
                        output: Some(artisan_api::Output::from_text(&format!(
                            "test parser passed\n{}",
                            "🦀".repeat(20_000)
                        ))),
                        exit_code: Some(0),
                        approval: None,
                    },
                })
                .await;
                io.emit(reply("answer", "parser finished")).await;
                Outcome::done()
            })
        }),
        |turn| {
            if !turn.input.text.contains("## Objective") {
                return "{}".to_owned();
            }
            assert!(turn.session_id.is_none());
            assert!(turn.input.text.contains("[User] (Done):\nship the parser"));
            assert!(
                turn.input
                    .text
                    .contains("[Assistant] (Done):\nChecking the parser")
            );
            assert!(turn.input.text.contains("cargo test parser"));
            assert!(turn.input.text.contains("test parser passed"));
            assert!(turn.input.text.contains("[tool output truncated]"));
            assert!(!turn.input.text.contains("review now"));
            "## Objective\n- ship the parser\n## Work State\n### Completed\n- parser tests passed"
                .to_owned()
        },
    )
    .await;
    install_fake(fixture.dir.path(), EngineId::Codex);
    fixture
        .hub
        .send_message(&fixture.thread_id, message("ship the parser"))
        .await
        .unwrap();
    fixture.settled().await;
    fixture
        .hub
        .send_message(
            &fixture.thread_id,
            message_on(EngineId::Codex, "review now"),
        )
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert!(
        runs.iter().all(|run| run.status == RunStatus::Done),
        "{runs:?}"
    );
    assert_eq!(prompts.lock().unwrap().len(), 2);
    let handed = prompts.lock().unwrap()[1].clone();
    assert!(handed.contains("cargo test parser"));
    assert!(handed.ends_with("</conversation-checkpoint>\n\nreview now"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_during_handoff_waits_for_the_forks_cleanup() {
    struct BlockingSummary {
        entered: Arc<tokio::sync::Notify>,
        cleaned: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Harness for BlockingSummary {
        fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
            Box::pin(async move {
                io.emit(EngineEvent::InputAccepted { id: turn.input.id })
                    .await;
                io.emit(EngineEvent::Session {
                    id: "source".into(),
                })
                .await;
                io.emit(reply("answer", "done")).await;
                Outcome::done()
            })
        }
        fn compact(
            &self,
            _turn: Turn,
            cancel: CancellationToken,
        ) -> BoxFuture<'static, anyhow::Result<forge_engines::SideReply>> {
            let entered = Arc::clone(&self.entered);
            let cleaned = Arc::clone(&self.cleaned);
            Box::pin(async move {
                entered.notify_one();
                cancel.cancelled().await;
                tokio::time::sleep(Duration::from_millis(20)).await;
                cleaned.store(true, std::sync::atomic::Ordering::SeqCst);
                anyhow::bail!("cancelled after discarding the fork")
            })
        }
    }
    let entered = Arc::new(tokio::sync::Notify::new());
    let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let harness: Arc<dyn Harness> = Arc::new(BlockingSummary {
        entered: Arc::clone(&entered),
        cleaned: Arc::clone(&cleaned),
    });
    let fixture = fixture_with_harness(Arc::new(move |_| Arc::clone(&harness))).await;
    install_fake(fixture.dir.path(), EngineId::Codex);
    fixture
        .hub
        .send_message(&fixture.thread_id, message("start"))
        .await
        .unwrap();
    fixture.settled().await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message_on(EngineId::Codex, "continue"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    let runs = fixture.settled().await;
    assert!(cleaned.load(std::sync::atomic::Ordering::SeqCst));
    assert!(runs.iter().any(|run| run.status == RunStatus::Cancelled));
}

/// A long first turn and an independently delayed title response.
struct BackgroundTitle {
    session: String,
    release_title: Arc<Notify>,
    title_started: Arc<Notify>,
    named: Arc<Notify>,
    names: Arc<Mutex<Vec<(String, String)>>>,
}

impl Harness for BackgroundTitle {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        let session = self.session.clone();
        Box::pin(async move {
            io.emit(EngineEvent::InputAccepted { id: turn.input.id })
                .await;
            io.emit(EngineEvent::Session { id: session }).await;
            io.cancel.cancelled().await;
            Outcome::cancelled()
        })
    }

    fn side_query(
        &self,
        turn: Turn,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, anyhow::Result<forge_engines::SideReply>> {
        let release = Arc::clone(&self.release_title);
        let started = Arc::clone(&self.title_started);
        Box::pin(async move {
            assert!(
                turn.session_id.is_none(),
                "early titles must not fork or prompt the active session"
            );
            assert!(turn.input.text.contains("repair the sidebar"));
            started.notify_one();
            tokio::select! {
                () = release.notified() => Ok(forge_engines::SideReply {
                    text: r#"{"title":"Sidebar title generation"}"#.to_owned(),
                    ..Default::default()
                }),
                () = cancel.cancelled() => anyhow::bail!("title cancelled with working turn"),
            }
        })
    }

    fn set_title(&self, turn: Turn, title: String) -> BoxFuture<'static, anyhow::Result<()>> {
        self.names
            .lock()
            .unwrap()
            .push((turn.session_id.unwrap(), title));
        self.named.notify_one();
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn background_titles_replace_the_prompt_and_reach_the_adapter_before_the_turn_ends() {
    for engine in [EngineId::Codex, EngineId::Claude] {
        for stopped in [false, true] {
            let harness = Arc::new(BackgroundTitle {
                session: if engine == EngineId::Claude {
                    format!("{}conversation/leaf", artisan_api::CLAUDE_AI_SESSION_PREFIX)
                } else {
                    "codex-session".to_owned()
                },
                release_title: Arc::new(Notify::new()),
                title_started: Arc::new(Notify::new()),
                named: Arc::new(Notify::new()),
                names: Arc::new(Mutex::new(Vec::new())),
            });
            let adapter = Arc::clone(&harness) as Arc<dyn Harness>;
            let fixture =
                fixture_with_engine_harness(engine, Arc::new(move |_| Arc::clone(&adapter))).await;
            if engine == EngineId::Claude {
                sign_in_to_claude_ai(&fixture);
                turn_on_enhanced_claude(&fixture).await;
            }
            fixture
                .hub
                .send_message(&fixture.thread_id, message("repair the sidebar"))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), harness.title_started.notified())
                .await
                .unwrap();
            assert_eq!(
                fixture
                    .hub
                    .store()
                    .thread(&fixture.thread_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .title
                    .as_deref(),
                Some("repair the sidebar")
            );
            if stopped {
                fixture.hub.cancel(&fixture.thread_id).await.unwrap();
                fixture.settled().await;
                fixture
                    .hub
                    .send_message(&fixture.thread_id, message("keep working"))
                    .await
                    .unwrap();
                fixture.wait_for(|items| items.iter().any(|item| {
                    item.status == Status::Done && matches!(&item.kind,
                        ItemKind::UserMessage { text, .. } if text.inline().is_some_and(|text| text == "keep working"))
                })).await;
            }
            harness.release_title.notify_one();
            tokio::time::timeout(Duration::from_secs(5), harness.named.notified())
                .await
                .unwrap();
            assert_eq!(
                fixture
                    .hub
                    .store()
                    .thread(&fixture.thread_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .title
                    .as_deref(),
                Some("Sidebar title generation")
            );
            assert_eq!(
                *harness.names.lock().unwrap(),
                [(
                    harness.session.clone(),
                    "Sidebar title generation".to_owned()
                )]
            );
            assert!(
                fixture.hub.is_live(&fixture.thread_id),
                "title must be applied while work continues"
            );
            fixture.hub.cancel(&fixture.thread_id).await.unwrap();
            fixture.settled().await;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_late_background_title_preserves_a_manual_rename() {
    let harness = Arc::new(BackgroundTitle {
        session: "codex-session".to_owned(),
        release_title: Arc::new(Notify::new()),
        title_started: Arc::new(Notify::new()),
        named: Arc::new(Notify::new()),
        names: Arc::new(Mutex::new(Vec::new())),
    });
    let adapter = Arc::clone(&harness) as Arc<dyn Harness>;
    let fixture =
        fixture_with_engine_harness(EngineId::Codex, Arc::new(move |_| Arc::clone(&adapter))).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("repair the sidebar"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), harness.title_started.notified())
        .await
        .unwrap();
    // Bypass Hub's memory-only eligibility too: the durable comparison must
    // protect changes from another writer while the query is in flight.
    fixture
        .hub
        .store()
        .update_thread(
            &fixture.thread_id,
            artisan_api::UpdateThread {
                title: Some("My chosen name".to_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    harness.release_title.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let querying = fixture
                .hub
                .inner
                .title_queries
                .lock()
                .unwrap()
                .contains(&fixture.thread_id);
            if !querying {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        fixture
            .hub
            .store()
            .thread(&fixture.thread_id)
            .await
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("My chosen name")
    );
    assert!(
        harness.names.lock().unwrap().is_empty(),
        "a discarded title must not rename the native session"
    );
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    fixture.settled().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn native_titles_replace_the_prompt_while_every_adapter_is_working() {
    for engine in [EngineId::Codex, EngineId::Claude, EngineId::OpenCode] {
        let adapter: Arc<dyn Harness> = Arc::new(Scripted(Arc::new(|_, io| {
            Box::pin(async move {
                io.emit(EngineEvent::Title {
                    title: "Native background title".to_owned(),
                })
                .await;
                io.emit(reply("waiting", "Still working")).await;
                io.cancel.cancelled().await;
                Outcome::cancelled()
            })
        })));
        let fixture =
            fixture_with_engine_harness(engine, Arc::new(move |_| Arc::clone(&adapter))).await;
        fixture
            .hub
            .send_message(&fixture.thread_id, message("repair the sidebar"))
            .await
            .unwrap();
        fixture
            .wait_for(|items| {
                items
                    .iter()
                    .any(|item| matches!(&item.kind, ItemKind::AssistantMessage { .. }))
            })
            .await;
        assert_eq!(
            fixture
                .hub
                .store()
                .thread(&fixture.thread_id)
                .await
                .unwrap()
                .unwrap()
                .title
                .as_deref(),
            Some("Native background title")
        );
        assert!(fixture.hub.is_live(&fixture.thread_id));
        fixture.hub.cancel(&fixture.thread_id).await.unwrap();
        fixture.settled().await;
    }
}
