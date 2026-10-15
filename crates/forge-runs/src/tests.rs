//! Scenarios a client would notice: messages reach the engine, steers are
//! never lost, answers reach the right run, cancel settles everything.

use super::*;
use artisan_api::{
    ApprovalRequest, CreateProject, CreateThread, Decision, EngineConfig, ItemsQuery, Phase,
    ProjectId, RunStatus, ToolKind,
};
use forge_engines::{EngineEvent, Outcome, RunIo, Turn};
use futures::future::BoxFuture;
use std::time::Duration;

type Script = Arc<dyn Fn(Turn, RunIo) -> BoxFuture<'static, Outcome> + Send + Sync>;

struct Scripted(Script);

impl Harness for Scripted {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        let script = self.0.clone();
        Box::pin(async move {
            io.emit(EngineEvent::InputAccepted {
                id: turn.input.id.clone(),
            })
            .await;
            script(turn, io).await
        })
    }
}

struct Fixture {
    hub: Hub,
    thread_id: ThreadId,
    _dir: tempfile::TempDir,
}

async fn fixture(script: Script) -> Fixture {
    let harness: Arc<dyn Harness> = Arc::new(Scripted(script));
    fixture_with_harness(Arc::new(move |_| Arc::clone(&harness))).await
}

async fn fixture_with_harness(harness_for: HarnessFor) -> Fixture {
    fixture_with_engine_harness(EngineId::Claude, harness_for).await
}

async fn fixture_with_engine_harness(engine: EngineId, harness_for: HarnessFor) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_path_buf();
    // A fake installed engine so the run gets past "not installed".
    let engine_dir = data.join("engines").join(engine.to_string());
    let version_dir = engine_dir.join("versions/1.0.0");
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
    } else {
        version_dir.join(engine.to_string())
    };
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(entry, "").unwrap();
    std::fs::write(engine_dir.join("current"), "1.0.0").unwrap();
    let store = forge_store::Store::open(&data.join("forge")).unwrap();
    let engines = Engines::new(&data);
    let hub = Hub::with_harness(store, engines, data.clone(), harness_for);
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
        })
        .await
        .unwrap();
    Fixture {
        hub,
        thread_id: thread.id,
        _dir: dir,
    }
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

    async fn wait_for(&self, check: impl Fn(&[Item]) -> bool) {
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
        fixture
            .hub
            .live_snapshot()
            .1
            .into_iter()
            .find_map(|event| match event {
                Event::Live {
                    target: t, text, ..
                } if target(&t) => Some(text),
                _ => None,
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
            match std::fs::read_link(entry.path()) {
                Ok(target) => format!(
                    "{name} -> {}",
                    target.file_name().unwrap().to_string_lossy()
                ),
                Err(_) => format!("{name}/"),
            }
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
    let step = items
        .iter()
        .find(|item| item.native_id.as_deref() == Some("toolu_resumed_step"))
        .expect("the resumed work");
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
    std::fs::remove_file(fixture._dir.path().join("engines/claude/current")).unwrap();
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
    let observed = turns.clone();
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
    assert_eq!(run.status, RunStatus::Cancelled);
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
        active.clone(),
        maximum.clone(),
        started.clone(),
        release.clone(),
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
    std::fs::create_dir_all(fixture._dir.path().join("engines/claude")).unwrap();
    std::fs::write(
        fixture._dir.path().join("engines/claude/home"),
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
    assert_eq!(
        delivered,
        (0..40).map(|n| format!("message-{n}")).collect::<Vec<_>>()
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_bounded_batch_does_not_retry_its_already_observed_tail() {
    let fixture = fixture(Arc::new(|_, _| panic!("uninstalled engine"))).await;
    std::fs::remove_file(fixture._dir.path().join("engines/claude/current")).unwrap();
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
            record.lock().unwrap().push(turn.thread_id);
            Outcome::done()
        })
    })));
    let resumed = Hub::with_harness(
        fixture.hub.store().clone(),
        fixture.hub.engines().clone(),
        fixture._dir.path().to_path_buf(),
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
    assert_eq!(
        observed.len(),
        16,
        "eight acknowledged active inputs are not redelivered; sixteen queued inputs resume"
    );
    assert_eq!(
        observed
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        16
    );
    assert!(observed.iter().all(|thread| threads.contains(thread)));
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

#[tokio::test(flavor = "multi_thread")]
async fn separate_approval_callbacks_reserve_one_execution_identity() {
    let fixture = fixture(Arc::new(|_turn, mut io| {
        Box::pin(async move {
            let approval = |decision| artisan_api::Approval {
                request: ApprovalRequest::Command {
                    command: "echo once".into(),
                    cwd: None,
                },
                reason: None,
                options: vec![Decision::AllowOnce, Decision::DenyOnce],
                decision,
            };
            for key in ["callback-a", "callback-b"] {
                io.emit(EngineEvent::Approval {
                    key: key.into(),
                    parent: None,
                    target: Some("execution".into()),
                    status: Status::AwaitingUser,
                    approval: approval(None),
                })
                .await;
            }
            io.emit(EngineEvent::Item {
                key: "execution".into(),
                parent: None,
                status: Status::Running,
                kind: ItemKind::Tool {
                    tool: ToolKind::Shell {
                        command: "echo once".into(),
                        cwd: None,
                        background: false,
                    },
                    title: None,
                    output: None,
                    exit_code: None,
                    approval: None,
                },
            })
            .await;
            for _ in 0..2 {
                let Some(Input::Answer {
                    key,
                    answer: artisan_api::Answer::Approval { decision },
                }) = io.inbox.recv().await
                else {
                    return Outcome::failed("missing callback answer");
                };
                io.emit(EngineEvent::Approval {
                    key,
                    parent: None,
                    target: Some("execution".into()),
                    status: Status::Done,
                    approval: approval(Some(decision)),
                })
                .await;
            }
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
        .wait_for(|items| {
            items
                .iter()
                .filter(|item| matches!(item.kind, ItemKind::Approval { .. }))
                .count()
                == 2
                && items
                    .iter()
                    .any(|item| matches!(item.kind, ItemKind::Tool { .. }))
        })
        .await;
    let items = fixture.items().await;
    let tools: Vec<_> = items
        .iter()
        .filter(|item| matches!(item.kind, ItemKind::Tool { .. }))
        .collect();
    assert_eq!(tools.len(), 1);
    // Mismatched kinds and decisions never reach the native callback inbox.
    assert!(
        fixture
            .hub
            .answer(
                &tools[0].id,
                artisan_api::Answer::Approval {
                    decision: Decision::AllowOnce
                }
            )
            .await
            .is_err()
    );
    for item in items
        .iter()
        .filter(|item| matches!(item.kind, ItemKind::Approval { .. }))
    {
        assert!(
            matches!(&item.kind,ItemKind::Approval {target:Some(target),..} if target==&tools[0].id)
        );
        assert!(
            fixture
                .hub
                .answer(
                    &item.id,
                    artisan_api::Answer::Approval {
                        decision: Decision::DenyAlways
                    }
                )
                .await
                .is_err()
        );
        assert!(
            fixture
                .hub
                .answer(
                    &item.id,
                    artisan_api::Answer::Approval {
                        decision: Decision::Cancelled
                    }
                )
                .await
                .is_err()
        );
        assert!(
            fixture
                .hub
                .answer(
                    &item.id,
                    artisan_api::Answer::Question {
                        answers: Vec::new()
                    }
                )
                .await
                .is_err()
        );
        fixture
            .hub
            .answer(
                &item.id,
                artisan_api::Answer::Approval {
                    decision: Decision::AllowOnce,
                },
            )
            .await
            .unwrap();
    }
    fixture.settled().await;
    let items = fixture.items().await;
    assert_eq!(
        items
            .iter()
            .filter(|item| matches!(item.kind, ItemKind::Tool { .. }))
            .count(),
        1
    );
    assert!(
        items
            .iter()
            .filter(|item| matches!(item.kind, ItemKind::Approval { .. }))
            .all(|item| item.status == Status::Done)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_label_offline_choices_resume_as_different_inputs_with_exact_ids() {
    let mut received = Vec::new();
    for selected in ["option-0", "option-1"] {
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = turns.clone();
        let resumed = Arc::new(std::sync::Mutex::new(String::new()));
        let captured = resumed.clone();
        let fixture = fixture(Arc::new(move |turn, io| {
            let first = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            let captured = captured.clone();
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
                    "approval",
                    ItemKind::Approval {
                        target: None,
                        approval: approval.clone(),
                    },
                ),
                (
                    "tool-approval",
                    ItemKind::Tool {
                        tool: ToolKind::Shell {
                            command: "echo waiting".into(),
                            cwd: None,
                            background: false,
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
    let items = fixture.items().await;
    for key in [
        "blocking-question",
        "approval",
        "tool-approval",
        "plan-approval",
    ] {
        let item = items
            .iter()
            .find(|item| item.native_id.as_deref() == Some(key))
            .unwrap();
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
    let output = items
        .iter()
        .find(|item| item.native_id.as_deref() == Some("ordinary-output"))
        .unwrap();
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
async fn native_approval_rpc_ids_restart_without_reusing_prior_run_items() {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture =
        fixture_with_engine_harness(EngineId::Codex, Arc::new(forge_engines::harness)).await;
    let binary = fixture.hub.engines().binary(EngineId::Codex).unwrap();
    std::fs::write(&binary,r#"#!/usr/bin/env python3
import json, os, pathlib, sys
home=pathlib.Path(os.environ['HOME'])
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
        send({'id':message['id'],'result':{'thread':{'id':thread}}})
    elif method=='turn/start':
        send({'id':message['id'],'result':{'turn':{'id':turn}}})
        send({'method':'item/started','params':{'threadId':thread,'turnId':turn,'item':{'id':item,'type':'commandExecution','command':'echo once','status':'inProgress'}}})
        send({'id':0,'method':'item/commandExecution/requestApproval','params':{'threadId':thread,'turnId':turn,'itemId':item,'command':'echo once'}})
    elif method is None and message.get('id')==0:
        decision=message['result']['decision']
        with (home/'native-replies').open('a') as file:
            file.write(json.dumps({'run':number,'id':0,'item':item,'decision':decision})+'\n')
        send({'method':'item/completed','params':{'threadId':thread,'turnId':turn,'item':{'id':item,'type':'commandExecution','command':'echo once','status':'completed' if decision=='accept' else 'declined','exitCode':0 if decision=='accept' else None}}})
        send({'method':'turn/completed','params':{'threadId':thread,'turn':{'id':turn,'status':'completed'}}})
        break
"#).unwrap();
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
            .wait_for(|items| {
                items.iter().any(|item| {
                    item.native_id.as_deref() == Some("approval-request:0")
                        && item.status == Status::AwaitingUser
                })
            })
            .await;
        let item = fixture
            .items()
            .await
            .into_iter()
            .find(|item| {
                item.native_id.as_deref() == Some("approval-request:0")
                    && item.status == Status::AwaitingUser
            })
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
        .filter(|item| matches!(item.kind, ItemKind::Approval { .. }))
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
async fn large_saved_question_validates_selected_identity_without_expanding_options() {
    let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::new(std::sync::Mutex::new(String::new()));
    let fixture = fixture(Arc::new({
        let turns = turns.clone();
        let observed = observed.clone();
        move |turn, io| {
            let first = turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            let observed = observed.clone();
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

#[tokio::test(flavor = "multi_thread")]
async fn slash_settings_are_idempotent_and_the_next_prompt_keeps_thread_config() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let fixture = fixture(Arc::new(move |turn, io| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            assert_eq!(turn.config.effort.as_deref(), Some("high"));
            assert_eq!(
                turn.config.session_options.get("ultracode"),
                Some(&serde_json::json!(true))
            );
            assert_eq!(turn.input.text, "ultrathink\ninspect carefully");
            io.emit(reply("answer", "done")).await;
            Outcome::done()
        })
    }))
    .await;
    let mut command = message("/ultracode");
    let scope = format!("thread:{}", fixture.thread_id);
    let draft = fixture
        .hub
        .store()
        .set_draft(
            scope.clone(),
            artisan_api::WriteDraft {
                expected_revision: 0,
                body: serde_json::json!({"text":"/ultracode"}),
            },
        )
        .await
        .unwrap();
    command.draft = Some(artisan_api::DraftRevision {
        scope: scope.clone(),
        revision: draft.current.revision,
    });
    let mut config = EngineConfig::new(EngineId::Claude);
    config.effort = Some("high".to_owned());
    command.config = Some(config);
    let first = fixture
        .hub
        .send_message(&fixture.thread_id, command.clone())
        .await
        .unwrap();
    assert!(matches!(first.kind, ItemKind::Notice { .. }));
    let cleared = fixture.hub.store().draft(&scope).await.unwrap();
    assert!(cleared.body.is_null());
    let newer = fixture
        .hub
        .store()
        .set_draft(
            scope.clone(),
            artisan_api::WriteDraft {
                expected_revision: cleared.revision,
                body: serde_json::json!({"text":"newer draft"}),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .hub
            .send_message(&fixture.thread_id, command.clone())
            .await
            .unwrap(),
        first
    );
    assert_eq!(
        fixture.hub.store().draft(&scope).await.unwrap(),
        newer.current
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(fixture.settled().await.is_empty());
    assert_eq!(fixture.items().await.len(), 1);
    command.text = "/ultracode off".to_owned();
    assert!(
        fixture
            .hub
            .send_message(&fixture.thread_id, command)
            .await
            .is_err()
    );
    let typed = "/ultrathink inspect carefully";
    let user = fixture
        .hub
        .send_message(&fixture.thread_id, message(typed))
        .await
        .unwrap();
    assert!(
        matches!(user.kind, ItemKind::UserMessage { ref text, .. } if text.inline() == Some(typed))
    );
    fixture.settled().await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let thread = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        thread.config.session_options.get("ultracode"),
        Some(&serde_json::json!(true))
    );
    // Even an explicit preference edit made from a thread snapshot cannot
    // copy session state into the defaults for another thread.
    let mut edited = thread.config.clone();
    edited.effort = Some("max".to_owned());
    fixture
        .hub
        .store()
        .set_setting(
            "default_engine_config".to_owned(),
            serde_json::to_value(edited).unwrap(),
        )
        .await
        .unwrap();
    let preferences: EngineConfig = serde_json::from_value(
        fixture
            .hub
            .store()
            .setting("default_engine_config")
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert!(preferences.session_options.is_empty());
    let another = fixture
        .hub
        .store()
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: thread.project_id,
            title: None,
            config: preferences,
            parent_thread_id: None,
        })
        .await
        .unwrap();
    assert!(another.config.session_options.is_empty());
    let mut replay = message("/ultracode");
    replay.id = first.id;
    assert!(fixture.hub.send_message(&another.id, replay).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_slash_commands_never_persist_or_start_the_engine() {
    let fixture = fixture(Arc::new(|_, _| {
        Box::pin(async { panic!("invalid command started engine") })
    }))
    .await;
    for text in ["/ultrathink", "/ultracode maybe", "/ultracode on extra"] {
        assert!(
            fixture
                .hub
                .send_message(&fixture.thread_id, message(text))
                .await
                .is_err()
        );
    }
    let mut attached = message("/ultracode on");
    attached
        .images
        .push(artisan_api::BlobHash("unused".to_owned()));
    assert!(
        fixture
            .hub
            .send_message(&fixture.thread_id, attached)
            .await
            .is_err()
    );
    assert!(fixture.items().await.is_empty());
    assert!(fixture.settled().await.is_empty());
}
