use super::*;

const CHAT: &str = "claude.ai:conversation/leaf";

type Launched = Arc<Mutex<Vec<(Option<String>, Option<String>)>>>;

fn launched(turn: &Turn) -> (Option<String>, Option<String>) {
    (
        turn.session_id.clone(),
        transport_of(&turn.config).map(str::to_owned),
    )
}

struct Titling {
    title_started: Arc<Notify>,
    release_title: Arc<Notify>,
    named: Arc<Notify>,
    titled: Launched,
}

impl Harness for Titling {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(async move {
            io.emit(EngineEvent::InputAccepted { id: turn.input.id })
                .await;
            io.emit(EngineEvent::Session {
                id: CHAT.to_owned(),
            })
            .await;
            io.cancel.cancelled().await;
            Outcome::cancelled()
        })
    }

    fn side_query(
        &self,
        turn: Turn,
        _cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<forge_engines::SideReply, forge_engines::SideQueryError>> {
        let started = Arc::clone(&self.title_started);
        let release = Arc::clone(&self.release_title);
        drop(turn);
        Box::pin(async move {
            started.notify_one();
            release.notified().await;
            Ok(forge_engines::SideReply {
                text: r#"{"title":"Sidebar title generation"}"#.to_owned(),
                ..Default::default()
            })
        })
    }

    fn set_title(
        &self,
        turn: Turn,
        _title: String,
    ) -> BoxFuture<'static, Result<(), forge_engines::SetTitleError>> {
        self.titled.lock().unwrap().push(launched(&turn));
        self.named.notify_one();
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_title_reaches_the_claude_ai_chat_it_names_after_switching_to_claude_code() {
    let harness = Arc::new(Titling {
        title_started: Arc::new(Notify::new()),
        release_title: Arc::new(Notify::new()),
        named: Arc::new(Notify::new()),
        titled: Arc::default(),
    });
    let adapter: Arc<dyn Harness> = Arc::<Titling>::clone(&harness);
    let fixture = fixture_with_harness(Arc::new(move |_| Arc::clone(&adapter))).await;
    sign_in_to_claude_ai(&fixture);
    turn_on_enhanced_claude(&fixture).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("repair the sidebar"))
        .await
        .unwrap();
    fixture.notified(&harness.title_started).await;

    fixture
        .hub
        .store()
        .set_setting(
            artisan_api::TRANSPORT_KEY.to_owned(),
            serde_json::json!(artisan_api::CLAUDE_CODE),
        )
        .await
        .unwrap();
    harness.release_title.notify_one();
    fixture.notified(&harness.named).await;

    assert_eq!(
        *harness.titled.lock().unwrap(),
        [(
            Some(CHAT.to_owned()),
            Some(forge_engines::CLAUDE_AI_TRANSPORT.to_owned())
        )],
        "the claude.ai chat is renamed on claude.ai, never handed to Claude Code"
    );
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    fixture.settled().await;
}

struct Compacting {
    compacted: Launched,
}

impl Harness for Compacting {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(async move {
            io.emit(EngineEvent::InputAccepted { id: turn.input.id })
                .await;
            if turn.session_id.is_none() {
                io.emit(EngineEvent::Session {
                    id: CHAT.to_owned(),
                })
                .await;
            }
            Outcome::done()
        })
    }

    fn compact(
        &self,
        turn: Turn,
        _cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<forge_engines::SideReply, forge_engines::SideQueryError>> {
        self.compacted.lock().unwrap().push(launched(&turn));
        Box::pin(async {
            Ok(forge_engines::SideReply {
                text: "## Objective\n- Continue the task.".to_owned(),
                ..Default::default()
            })
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_of_claude_ai_summarizes_the_chat_without_resuming_it() {
    let compacted = Launched::default();
    let harness: Arc<dyn Harness> = Arc::new(Compacting {
        compacted: Arc::clone(&compacted),
    });
    let fixture = fixture_with_harness(Arc::new(move |_| Arc::clone(&harness))).await;
    sign_in_to_claude_ai(&fixture);
    turn_on_enhanced_claude(&fixture).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("first"))
        .await
        .unwrap();
    fixture.settled().await;

    std::fs::remove_file(forge_engines::claudeai::Session::path(
        &fixture.hub.engines().home(EngineId::Claude),
    ))
    .unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("second"))
        .await
        .unwrap();
    fixture.settled().await;

    assert_eq!(
        *compacted.lock().unwrap(),
        [(None, Some(forge_engines::CLAUDE_AI_UNAVAILABLE.to_owned()))],
        "the summary of a chat Forge can no longer reach is written from the transcript"
    );
}
