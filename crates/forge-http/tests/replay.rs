use crate::tests::{Device, paired};
use crate::{ServeConfig, start};
use artisan_api::{
    Answer, CreateProject, CreateThread, EngineConfig, EngineId, Envelope, Event, Item, ItemId,
    ItemKind, Permissions, ProjectId, Run, RunId, RunStatus, SendMessage, Seq, State, TextContent,
    ThreadId, ToolKind,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::io::Write as _;
use std::time::Duration;
use test_support::replay::ReplayingEngine;

#[path = "replay/claude.rs"]
mod claude;
#[path = "replay/codex.rs"]
mod codex;
#[path = "replay/opencode.rs"]
mod opencode;

const SIDE_QUERIES: [&str; 3] = [
    forge_runs::SUGGESTIONS_KEY,
    forge_runs::YOU_SHOULD_KNOW_KEY,
    forge_runs::SUMMARY_TITLES_KEY,
];

pub(crate) struct Replay {
    dir: tempfile::TempDir,
    engine_files: ReplayingEngine,
    engine: EngineId,
    editor: Device,
    project: ProjectId,
}

pub(crate) async fn replaying(engine: EngineId, fixture: &str) -> Replay {
    let dir = tempfile::tempdir().expect("create the Forge folder");
    let data = dir.path().join("forge");
    let engine_files = ReplayingEngine::install(&data, "replay_engine", engine, fixture);
    let server = start(ServeConfig {
        data_dir: data,
        listen: "127.0.0.1:0".parse().expect("a loopback address"),
    })
    .await
    .expect("Forge starts");
    let address = server.local_addr().expect("Forge listens");
    let store = server.store().clone();
    tokio::spawn(server.run());
    for key in SIDE_QUERIES {
        store
            .set_setting(key.to_owned(), serde_json::json!(false))
            .await
            .expect("turn off side queries, which would launch the engine again");
    }
    let editor = paired(&store, address, "replay").await;
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).expect("create the project folder");
    let project = ProjectId::new();
    let created = editor
        .http()
        .post(editor.url("/projects"))
        .json(&CreateProject {
            id: project.clone(),
            path: work.display().to_string(),
            name: None,
        })
        .send()
        .await
        .expect("create the project");
    assert!(created.status().is_success(), "the project is created");
    Replay {
        dir,
        engine_files,
        engine,
        editor,
        project,
    }
}

impl Replay {
    pub(crate) async fn head(&self) -> Seq {
        self.editor
            .http()
            .get(self.editor.url("/state"))
            .send()
            .await
            .expect("read the state")
            .json::<State>()
            .await
            .expect("decode the state")
            .head
    }

    pub(crate) async fn thread(&self, permissions: Permissions) -> ThreadId {
        let mut config = EngineConfig::new(self.engine);
        config.permissions = permissions;
        let thread = CreateThread {
            id: ThreadId::new(),
            project_id: self.project.clone(),
            config,
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        };
        self.post("/threads", &thread).await;
        thread.id
    }

    pub(crate) async fn send(&self, thread: &ThreadId, text: &str) {
        self.post(
            &format!("/threads/{thread}/messages"),
            &SendMessage {
                draft: None,
                id: ItemId::new(),
                text: text.into(),
                images: Vec::new(),
                config: None,
            },
        )
        .await;
    }

    pub(crate) async fn answer(&self, item: &ItemId, answer: &Answer) {
        self.post(&format!("/items/{item}/answer"), answer).await;
    }

    pub(crate) async fn stop(&self, thread: &ThreadId) {
        self.post(&format!("/threads/{thread}/cancel"), &()).await;
    }

    pub(crate) async fn carry_on(&self, thread: &ThreadId) {
        self.post(&format!("/threads/{thread}/continue"), &()).await;
    }

    async fn post(&self, path: &str, body: &(impl serde::Serialize + Sync)) {
        let response = self
            .editor
            .http()
            .post(self.editor.url(path))
            .json(body)
            .send()
            .await
            .unwrap_or_else(|error| panic!("POST {path}: {error}"));
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        assert!(status.is_success(), "POST {path} answered {status}: {text}");
    }

    pub(crate) async fn events(
        &self,
        since: Seq,
        what: &str,
        done: impl Fn(&Screen) -> bool + Send + Sync,
    ) -> Screen {
        let response = self
            .editor
            .http()
            .get(self.editor.url(&format!("/events?since={since}")))
            .send()
            .await
            .expect("open the event stream");
        let mut screen = Screen::default();
        test_support::sse::read_until(
            response.bytes_stream(),
            what,
            SETTLE,
            |envelope: &Envelope| {
                screen.apply(envelope);
                done(&screen)
            },
        )
        .await;
        screen
    }

    pub(crate) async fn settled(&self, since: Seq, thread: &ThreadId, runs: usize) -> Screen {
        self.events(since, "the thread's runs to settle", |screen| {
            screen.finished(thread) >= runs
        })
        .await
    }

    pub(crate) async fn trace(&self, thread: &ThreadId) -> Vec<serde_json::Value> {
        let folder = forge_engines::trace::folder(&self.dir.path().join("forge"));
        let prefix = format!("{}-{thread}-", self.engine);
        let records = || -> Vec<serde_json::Value> {
            let files: Vec<PathBuf> = std::fs::read_dir(&folder)
                .into_iter()
                .flatten()
                .map(|entry| entry.expect("list the engine traces").path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&prefix))
                })
                .collect();
            files
                .iter()
                .flat_map(|path| {
                    std::fs::read_to_string(path)
                        .expect("read the engine trace")
                        .lines()
                        .filter_map(|line| serde_json::from_str(line).ok())
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        test_support::wait::eventually("the engine trace to end", SETTLE, || async {
            records().iter().any(|record| record["kind"] == "end")
        })
        .await;
        records()
    }

    pub(crate) fn launches(&self) -> usize {
        self.engine_files.launches()
    }
}

impl Drop for Replay {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let log = self.engine_files.log();
            writeln!(std::io::stderr(), "what the replaying engine saw:\n{log}")
                .expect("the test writes to stderr");
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Screen {
    items: BTreeMap<ItemId, Item>,
    runs: BTreeMap<RunId, Run>,
    envelopes: Vec<Envelope>,
}

impl Screen {
    fn apply(&mut self, envelope: &Envelope) {
        self.envelopes.push(envelope.clone());
        if let Event::Item { item } = &envelope.event {
            self.items.insert(item.id.clone(), item.clone());
        } else if let Event::Run { run } = &envelope.event {
            self.runs.insert(run.id.clone(), run.clone());
        }
    }

    pub(crate) fn finished(&self, thread: &ThreadId) -> usize {
        self.runs
            .values()
            .filter(|run| &run.thread_id == thread && run.status() != RunStatus::Running)
            .count()
    }

    pub(crate) fn started(&self, thread: &ThreadId) -> bool {
        self.runs
            .values()
            .any(|run| &run.thread_id == thread && run.session_id.is_some())
    }

    pub(crate) fn runs(&self, thread: &ThreadId) -> Vec<(RunStatus, Option<String>)> {
        let mut runs: Vec<&Run> = self
            .runs
            .values()
            .filter(|run| &run.thread_id == thread)
            .collect();
        runs.sort_by_key(|run| run.started_at);
        runs.into_iter()
            .map(|run| (run.status(), run.error().map(str::to_owned)))
            .collect()
    }

    pub(crate) fn item(&self, thread: &ThreadId, matches: impl Fn(&Item) -> bool) -> Option<&Item> {
        self.items
            .values()
            .find(|item| &item.thread_id == thread && matches(item))
    }

    pub(crate) fn transcript(&self, thread: &ThreadId) -> Vec<String> {
        let mut items: Vec<&Item> = self
            .items
            .values()
            .filter(|item| &item.thread_id == thread)
            .collect();
        items.sort_by_key(|item| item.position);
        items.into_iter().map(line).collect()
    }

    pub(crate) fn envelopes(&self) -> &[Envelope] {
        &self.envelopes
    }
}

fn text(content: &TextContent) -> &str {
    content.inline().unwrap_or("[stored]")
}

fn line(item: &Item) -> String {
    let what = match &item.kind {
        ItemKind::UserMessage { text: body, .. } => format!("you: {}", text(body)),
        ItemKind::AssistantMessage { text: body, .. } => format!("assistant: {}", text(body)),
        ItemKind::Tool {
            tool: ToolKind::Shell { command, .. },
            ..
        } => format!("shell: {}", text(command)),
        ItemKind::Tool { tool, .. } => format!("tool: {tool:?}"),
        ItemKind::Agent { description, .. } => format!("agent: {}", text(description)),
        ItemKind::Question { .. } => "question".to_owned(),
        ItemKind::Plan { .. } => "plan".to_owned(),
        ItemKind::Image { .. } => "image".to_owned(),
        ItemKind::Compaction { .. } => "compaction".to_owned(),
        ItemKind::Notice { text: body, .. } => format!("notice: {}", text(body)),
        ItemKind::Error { message, .. } => format!("error: {}", text(message)),
    };
    format!("{what} ({:?})", item.status)
}

pub(crate) const SETTLE: Duration = Duration::from_secs(20);
