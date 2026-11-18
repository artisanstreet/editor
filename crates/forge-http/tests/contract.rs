#![cfg(target_os = "linux")]

#[path = "support/editor.rs"]
#[expect(
    dead_code,
    reason = "the contract only reads and posts; drafts and raw streams are the simulation's"
)]
mod client;

use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, EngineStatus, InstallState, ItemKind,
    ItemsPage, ProjectId, RunStatus, SendMessage, SignInState, State, ThreadId,
};
use client::{Editor, Outcome};
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use test_support::transcript::{Step, Transcript};

const PROMPT: &str = "Reply with the single word pong and nothing else.";
const PROBED_WITHIN: Duration = Duration::from_secs(120);
const ANSWERED_WITHIN: Duration = Duration::from_secs(300);
const DISCRIMINATORS: [&str; 14] = [
    "/type",
    "/subtype",
    "/method",
    "/request/subtype",
    "/response/subtype",
    "/event/type",
    "/event/delta/type",
    "/event/content_block/type",
    "/params/item/type",
    "/properties/part/type",
    "/properties/info/role",
    "/message/role",
    "/data/item/type",
    "/data/part/type",
];

#[tokio::test(flavor = "multi_thread")]
async fn claude_code_speaks_the_protocol_forge_reads() {
    contract(EngineId::Claude, "ANTHROPIC_API_KEY").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_speaks_the_protocol_forge_reads() {
    contract(EngineId::Codex, "OPENAI_API_KEY").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opencode_speaks_the_protocol_forge_reads() {
    contract(EngineId::OpenCode, "OPENCODE_API_KEY").await;
}

async fn contract(engine: EngineId, credential: &str) {
    let dir = tempfile::tempdir().expect("create the Forge folder");
    let data = dir.path().join("forge");
    let version = forge_engines::Engines::new(&data)
        .install(engine)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "install the newest {engine} the way Forge does: {}",
                diagnostics::chain(&error)
            )
        });
    say(&format!("installed {engine} {version}"));
    let server = forge_http::start(forge_http::ServeConfig {
        data_dir: data.clone(),
        listen: SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
    })
    .await
    .unwrap_or_else(|error| panic!("Forge starts: {}", diagnostics::chain(&error)));
    let address = server.local_addr().expect("Forge listens");
    let store = server.store().clone();
    let invite = forge_http::invite(&store)
        .await
        .unwrap_or_else(|error| panic!("invite the Editor: {}", diagnostics::chain(&error)));
    tokio::spawn(server.run());
    let editor = Editor::new(&invite.forge_certificate);
    editor.pair(address, invite.code).await;

    let status = probed(&editor, address, engine).await;
    assert_eq!(
        status.install,
        InstallState::Installed {
            version: version.clone(),
            update: None
        },
        "Forge reads the {engine} it installed"
    );
    if let SignInState::Failed { message } = &status.sign_in {
        panic!("Forge cannot read whether {engine} {version} is signed in: {message}");
    }

    if std::env::var_os(credential).is_none_or(|value| value.is_empty()) {
        say(&format!(
            "::notice title={engine} contract::{credential} is not set, so {engine} {version} was only installed and probed; no prompt ran"
        ));
        return;
    }
    let thread = answered(&editor, address, engine, dir.path()).await;
    let live = live_transcript(&data, engine, &thread);
    let drift = unrecorded_shapes(engine, &live);
    if let Some(out) = std::env::var_os("CONTRACT_OUT").map(PathBuf::from) {
        std::fs::create_dir_all(&out).expect("create the contract output folder");
        std::fs::write(out.join(format!("{engine}-{version}.jsonl")), live.render())
            .expect("save the live transcript");
        std::fs::write(
            out.join(format!("{engine}-drift.txt")),
            drift
                .iter()
                .map(String::as_str)
                .chain([""])
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .expect("save the unrecorded shapes");
    }
    assert!(
        drift.is_empty(),
        "{engine} {version} sends shapes no recorded transcript has; record the live transcript as a fixture and check the decoder reads it:\n{}",
        drift.iter().cloned().collect::<Vec<_>>().join("\n")
    );
}

async fn probed(editor: &Editor, address: SocketAddr, engine: EngineId) -> EngineStatus {
    let status = || async {
        let engines: Vec<EngineStatus> = editor.get(address, "/engines").await;
        engines
            .into_iter()
            .find(|status| status.engine == engine)
            .expect("Forge lists every engine")
    };
    test_support::wait::eventually(
        "Forge to probe the engine's sign-in",
        PROBED_WITHIN,
        || async { status().await.sign_in != SignInState::Unknown },
    )
    .await;
    status().await
}

async fn answered(editor: &Editor, address: SocketAddr, engine: EngineId, root: &Path) -> ThreadId {
    let work = root.join("work");
    std::fs::create_dir_all(&work).expect("create the project folder");
    let project = ProjectId::new();
    accepted::<Value>(
        editor
            .post(
                address,
                "/projects",
                &CreateProject {
                    id: project.clone(),
                    path: work.display().to_string(),
                    name: None,
                },
                None,
            )
            .await,
    );
    let thread = ThreadId::new();
    accepted::<Value>(
        editor
            .post(
                address,
                "/threads",
                &CreateThread {
                    id: thread.clone(),
                    project_id: project,
                    config: EngineConfig::new(engine),
                    title: Some("Contract".to_owned()),
                    parent_thread_id: None,
                    spawned_by: None,
                },
                None,
            )
            .await,
    );
    accepted::<Value>(
        editor
            .post(
                address,
                &format!("/threads/{thread}/messages"),
                &SendMessage {
                    draft: None,
                    id: artisan_api::ItemId::new(),
                    text: PROMPT.into(),
                    images: Vec::new(),
                    config: None,
                },
                None,
            )
            .await,
    );
    test_support::wait::eventually("the engine to answer", ANSWERED_WITHIN, || async {
        let state: State = editor.get(address, "/state").await;
        let runs: Vec<_> = state
            .runs
            .iter()
            .filter(|run| run.thread_id == thread)
            .collect();
        !runs.is_empty() && runs.iter().all(|run| run.status() != RunStatus::Running)
    })
    .await;
    let page: ItemsPage = editor
        .get(address, &format!("/threads/{thread}/transcript?limit=100"))
        .await;
    let runs: Vec<_> = page
        .runs
        .iter()
        .map(|run| (run.status(), run.error()))
        .collect();
    assert!(
        runs.iter().all(|(status, _)| *status == RunStatus::Done),
        "the {engine} turn finishes: {runs:?}"
    );
    let replied = page.items.iter().any(|item| match &item.kind {
        ItemKind::AssistantMessage { text, .. } => text
            .inline()
            .is_some_and(|text| text.to_lowercase().contains("pong")),
        ItemKind::UserMessage { .. }
        | ItemKind::Tool { .. }
        | ItemKind::Agent { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => false,
    });
    assert!(
        replied,
        "the {engine} reply reaches the thread: {:#?}",
        page.items
    );
    thread
}

fn live_transcript(data: &Path, engine: EngineId, thread: &ThreadId) -> Transcript {
    let folder = forge_engines::trace::folder(data);
    let prefix = format!("{engine}-{thread}-");
    let mut traces: Vec<PathBuf> = std::fs::read_dir(&folder)
        .expect("list the engine traces")
        .map(|entry| entry.expect("read a trace entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect();
    traces.sort();
    let launches = traces
        .iter()
        .map(|path| {
            let trace = std::fs::read_to_string(path).expect("read the engine trace");
            test_support::transcript::record(&trace, &[])
                .unwrap_or_else(|error| panic!("record {}: {error}", path.display()))
        })
        .collect();
    Transcript::new(test_support::transcript::Source::Recorded, launches)
}

fn unrecorded_shapes(engine: EngineId, live: &Transcript) -> BTreeSet<String> {
    let recorded = recorded_shapes(engine);
    shapes(live)
        .into_iter()
        .filter(|shape| !recorded.contains(shape))
        .collect()
}

fn recorded_shapes(engine: EngineId) -> BTreeSet<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../forge-engines/transcripts");
    [
        root.join(engine.to_string()),
        root.join("replay").join(engine.to_string()),
    ]
    .iter()
    .flat_map(|folder| std::fs::read_dir(folder).expect("list the recorded transcripts"))
    .map(|entry| entry.expect("read a transcript entry").path())
    .filter(|path| {
        path.extension()
            .is_some_and(|extension| extension == "jsonl")
    })
    .flat_map(|path| {
        let text = std::fs::read_to_string(&path).expect("read a recorded transcript");
        Transcript::parse(&text).map_or_else(
            |_not_a_replay| frames_of_lines(&text),
            |transcript| shapes(&transcript),
        )
    })
    .collect()
}

fn frames_of_lines(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|frame| shape(&frame))
        .collect()
}

fn shapes(transcript: &Transcript) -> BTreeSet<String> {
    (0..transcript.launches())
        .filter_map(|launch| transcript.launch(launch))
        .flatten()
        .filter_map(|step| match step {
            Step::Out { text } => serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|frame| shape(&frame)),
            Step::Request { reply, .. } => shape(reply),
            Step::In { .. } | Step::Closed | Step::Exit { .. } => None,
        })
        .collect()
}

fn shape(frame: &Value) -> Option<String> {
    let parts: Vec<String> = DISCRIMINATORS
        .iter()
        .filter_map(|pointer| {
            frame
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(|value| format!("{pointer}={value}"))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn say(line: &str) {
    writeln!(std::io::stdout(), "{line}").expect("the contract writes to the job log");
}

fn accepted<T>(outcome: Outcome<T>) -> T {
    match outcome {
        Outcome::Accepted(value) => value,
        Outcome::Refused { status, error } => panic!("refused with {status}: {error:?}"),
        Outcome::Lost => unreachable!("invariant: contract requests are never cut"),
    }
}
