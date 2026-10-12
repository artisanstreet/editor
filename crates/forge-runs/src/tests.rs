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
        (self.0)(turn, io)
    }
}

struct Fixture {
    hub: Hub,
    thread_id: ThreadId,
    _dir: tempfile::TempDir,
}

async fn fixture(script: Script) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_path_buf();
    // A fake installed Claude so the run gets past "not installed".
    let version_dir = data.join("engines/claude/versions/1.0.0");
    std::fs::create_dir_all(&version_dir).unwrap();
    std::fs::write(version_dir.join("claude"), "").unwrap();
    std::fs::write(data.join("engines/claude/current"), "1.0.0").unwrap();
    let store = forge_store::Store::open(&data.join("forge")).unwrap();
    let engines = Engines::new(&data);
    let harness: Arc<dyn Harness> = Arc::new(Scripted(script));
    let hub = Hub::with_harness(
        store,
        engines,
        data.clone(),
        Arc::new(move |_| Arc::clone(&harness)),
    );
    let project = hub
        .store()
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: data.display().to_string(),
            name: None,
        })
        .await
        .unwrap();
    let config = EngineConfig::new(EngineId::Claude);
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
            text: text.to_owned(),
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
                if !self.hub.is_live(&self.thread_id) {
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
            let kind = ItemKind::Image {
                blob: artisan_api::BlobHash(String::new()),
                caption: Some(path),
            };
            io.emit(EngineEvent::Item {
                key: "ig1".into(),
                parent: None,
                status: Status::Done,
                kind,
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
            io.emit(EngineEvent::SteerAccepted { id: steer.id }).await;
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
        fixture.hub.live_snapshot().is_empty(),
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
                    text: String::new(),
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
            io.emit(EngineEvent::SteerAccepted { id: steer.id }).await;
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
    fixture.hub.cancel(&fixture.thread_id);
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
    assert_eq!(result.as_deref(), Some("Attachments now show on the gun."));
    assert_eq!(prompt.as_deref(), Some("Draw attachments on every gun."));
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
