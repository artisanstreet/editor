use super::*;
use std::path::{Path, PathBuf};

fn fake_engine() -> PathBuf {
    let test = std::env::current_exe().expect("the test binary has a path");
    let profile = test
        .parent()
        .and_then(Path::parent)
        .expect("test binaries live in <target>/<profile>/deps");
    let fake = profile
        .join("examples")
        .join(format!("fake_engine{}", std::env::consts::EXE_SUFFIX));
    assert!(
        fake.is_file(),
        "the fake engine {} is missing: build it with `cargo build -p forge-engines --example fake_engine`",
        fake.display()
    );
    fake
}

struct FakeCodex;

impl Harness for FakeCodex {
    fn run(&self, mut turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        turn.binary = fake_engine();
        turn.launch.env.extend([
            ("ARTISAN_FAKE".to_owned(), "codex-app-server".to_owned()),
            (
                "ARTISAN_TEST_SPAWNS".to_owned(),
                turn.home.join("spawns").display().to_string(),
            ),
            (
                "ARTISAN_TEST_INPUT".to_owned(),
                turn.home.join("input").display().to_string(),
            ),
        ]);
        forge_engines::harness(EngineId::Codex).run(turn, io)
    }
}

async fn agent_settles(fixture: &Fixture, agent: &ItemId) -> AgentInfo {
    let status = || async {
        fixture
            .hub
            .agent_status(&fixture.thread_id, agent.as_str())
            .await
            .unwrap()
    };
    test_support::wait::eventually(
        "the agent settles",
        test_support::wait::LIVENESS,
        || async {
            let info = status().await;
            info.state.is_settled() && !fixture.hub.is_live(&info.thread)
        },
    )
    .await;
    status().await
}

#[tokio::test(flavor = "multi_thread")]
async fn messaging_a_finished_codex_agent_whose_session_is_held_elsewhere_fails_it_visibly() {
    let harness: Arc<dyn Harness> = Arc::new(FakeCodex);
    let fixture =
        fixture_with_engine_harness(EngineId::Codex, Arc::new(move |_| Arc::clone(&harness))).await;
    let spawns = fixture.hub.engines().home(EngineId::Codex).join("spawns");
    let spawned = fixture
        .hub
        .spawn_agent(
            &fixture.thread_id,
            SpawnAgent {
                task: "Summarise the notes".to_owned(),
                prompt: "summarise".to_owned(),
                harness: Some(EngineId::Codex),
                ..SpawnAgent::default()
            },
        )
        .await
        .unwrap();
    let first = agent_settles(&fixture, &spawned.agent).await;
    assert_eq!(first.state, AgentState::Done, "{first:?}");
    let mut finished = fixture
        .hub
        .store()
        .latest_run(&first.thread)
        .await
        .unwrap()
        .expect("the agent ran");
    finished.session_id = Some("thread-held-elsewhere".to_owned());
    fixture.hub.store().upsert_run(finished).await.unwrap();
    let mut lock = spawns.into_os_string();
    lock.push("-thread-held-elsewhere.lock");
    let writer = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(PathBuf::from(lock))
        .unwrap();
    writer.try_lock().unwrap();

    let messaged = fixture
        .hub
        .message_agent(
            &fixture.thread_id,
            spawned.agent.as_str(),
            "again".to_owned(),
        )
        .await
        .unwrap();
    assert_eq!(messaged.state, AgentState::Working);
    let failed = agent_settles(&fixture, &spawned.agent).await;

    assert_eq!(failed.state, AgentState::Failed, "{failed:?}");
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("open in another Codex process")),
        "{failed:?}"
    );
    let child = fixture.items_of(&first.thread).await;
    let again = child
        .iter()
        .find(|item| item.preview() == Some("again"))
        .expect("the message is in the agent's thread");
    assert_eq!(again.status, Status::Failed);
    assert!(child.iter().any(|item| matches!(
        &item.kind,
        ItemKind::Error { message, .. }
            if message.inline().is_some_and(|text| text.contains("open in another Codex process"))
    )));
    let row = fixture
        .hub
        .store()
        .item(&spawned.agent)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, Status::Failed);
    drop(writer);
    fixture.hub.shutdown();
}
