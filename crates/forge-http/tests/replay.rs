use crate::tests::{Device, paired};
use crate::{ServeConfig, start};
use artisan_api::{
    Answer, CreateProject, CreateThread, EngineConfig, EngineId, Envelope, Event, Item, ItemId,
    ItemKind, Permissions, ProjectId, Run, RunId, RunStatus, SendMessage, Seq, State, TextContent,
    ThreadId, ToolKind,
};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    _dir: tempfile::TempDir,
    folder: PathBuf,
    engine: EngineId,
    hub: forge_runs::Hub,
    editor: Device,
    project: ProjectId,
}

fn replayer() -> PathBuf {
    let test = std::env::current_exe().expect("the test binary has a path");
    let profile = test
        .parent()
        .and_then(Path::parent)
        .expect("test binaries live in <target>/<profile>/deps");
    let replayer = profile
        .join("examples")
        .join(format!("replay_engine{}", std::env::consts::EXE_SUFFIX));
    assert!(
        replayer.is_file(),
        "the replaying engine {} is missing: build it with `cargo build -p forge-http --example replay_engine` or run `cargo test -p forge-http` without --lib",
        replayer.display()
    );
    replayer
}

const fn version(engine: EngineId) -> &'static str {
    match engine {
        EngineId::OpenCode => "2.1.0",
        EngineId::Claude | EngineId::Codex => "1.0.0",
    }
}

fn install(data: &Path, engine: EngineId, fixture: &str) -> PathBuf {
    let engine_dir = data.join("engines").join(engine.to_string());
    let version = version(engine);
    let entry = engine_dir.join("versions").join(version).join(
        forge_engines::host_entry(engine)
            .expect("the replayed engine has a build on this platform"),
    );
    let folder = entry.parent().expect("the engine entry sits in a folder");
    std::fs::create_dir_all(folder).expect("create the replayed engine's folder");
    std::fs::copy(replayer(), &entry).expect("install the replaying engine");
    let transcript = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../forge-engines/transcripts/replay")
        .join(engine.to_string())
        .join(format!("{fixture}.jsonl"));
    std::fs::copy(&transcript, folder.join("transcript.jsonl"))
        .unwrap_or_else(|error| panic!("copy the transcript {}: {error}", transcript.display()));
    std::fs::write(engine_dir.join("current"), version).expect("select the replayed version");
    folder.to_path_buf()
}

pub(crate) async fn replaying(engine: EngineId, fixture: &str) -> Replay {
    let dir = tempfile::tempdir().expect("create the Forge folder");
    let data = dir.path().join("forge");
    let folder = install(&data, engine, fixture);
    let server = start(ServeConfig {
        data_dir: data,
        listen: "127.0.0.1:0".parse().expect("a loopback address"),
    })
    .await
    .expect("Forge starts");
    let address = server.local_addr().expect("Forge listens");
    let store = server.store().clone();
    let hub = server.hub.clone();
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
        _dir: dir,
        folder,
        engine,
        hub,
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

    pub(crate) async fn wound_down(&self, thread: &ThreadId) {
        test_support::wait::eventually("the stopped turn to wind down", SETTLE, || async {
            !self.hub.is_live(thread)
        })
        .await;
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

    pub(crate) fn launches(&self) -> usize {
        std::fs::read_to_string(self.folder.join("launches")).map_or(0, |count| {
            count
                .trim()
                .parse()
                .expect("the launch counter holds a number")
        })
    }
}

impl Drop for Replay {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let log = std::fs::read_to_string(self.folder.join("replay.log"))
                .unwrap_or_else(|error| format!("no replay log: {error}"));
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
