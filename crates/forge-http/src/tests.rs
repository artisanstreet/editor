//! A client's view over real mutual TLS: pairing, auth, sending, and the
//! event stream resuming after a seq.

use super::*;
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, Envelope, Event, ItemId, PairRequest,
    ProjectId, RunStatus, SendMessage, State, ThreadId,
};
use std::time::Duration;

struct Device {
    http: reqwest::Client,
    base: String,
}

fn device(forge_certificate: &str, address: SocketAddr) -> Device {
    let identity = Identity::generate("artisan-editor").unwrap();
    let config =
        tls::client_config(&identity, &tls::fingerprint_of(forge_certificate).unwrap()).unwrap();
    let http = reqwest::Client::builder()
        .use_preconfigured_tls(config)
        .build()
        .unwrap();
    Device {
        http,
        base: format!("https://localhost:{}", address.port()),
    }
}

impl Device {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

async fn forge() -> (tempfile::TempDir, SocketAddr, Store) {
    let dir = tempfile::tempdir().unwrap();
    let server = start(ServeConfig {
        data_dir: dir.path().join("forge"),
        listen: "127.0.0.1:0".parse().unwrap(),
    })
    .await
    .unwrap();
    let address = server.local_addr().unwrap();
    let store = server.store().clone();
    tokio::spawn(server.run());
    (dir, address, store)
}

/// Reads SSE envelopes until `stop` says so.
async fn read_events(
    response: reqwest::Response,
    stop: impl Fn(&Envelope) -> bool + Send + Sync,
) -> Vec<Envelope> {
    let mut response = response;
    let mut buffer = String::new();
    let mut envelopes = Vec::new();
    let read = async {
        while let Some(chunk) = response.chunk().await.unwrap() {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(end) = buffer.find("\n\n") {
                let block: String = buffer.drain(..end + 2).collect();
                let data: String = block
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim)
                    .collect();
                if data.is_empty() {
                    continue;
                }
                let envelope: Envelope = serde_json::from_str(&data).unwrap();
                let done = stop(&envelope);
                envelopes.push(envelope);
                if done {
                    return;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .expect("events arrive");
    envelopes
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn a_paired_device_sends_a_message_and_follows_the_run() {
    let (_dir, address, store) = forge().await;
    let invite = invite(&store).await.unwrap();
    let editor = device(&invite.forge_certificate, address);

    // Not paired yet.
    let refused = editor.http.get(editor.url("/state")).send().await.unwrap();
    assert_eq!(refused.status(), 401);

    let pair = PairRequest {
        code: invite.code.to_lowercase(),
        device_name: "laptop".into(),
    };
    let paired = editor
        .http
        .post(editor.url("/pair"))
        .json(&pair)
        .send()
        .await
        .unwrap();
    assert_eq!(paired.status(), 200);
    let again = editor
        .http
        .post(editor.url("/pair"))
        .json(&pair)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 401, "a code works once");

    let body = editor
        .http
        .get(editor.url("/state"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let forge_key = crate::identity(&store).await.unwrap().key;
    assert!(
        !body.contains(&forge_key),
        "Forge's private key never reaches a client"
    );
    let state: State = serde_json::from_str(&body).unwrap();
    let since = state.head;

    let missing_project = CreateThread {
        id: ThreadId::new(),
        project_id: ProjectId::new(),
        config: EngineConfig::new(EngineId::Claude),
        title: None,
        parent_thread_id: None,
        spawned_by: None,
    };
    let missing = editor
        .http
        .post(editor.url("/threads"))
        .json(&missing_project)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert_eq!(
        missing.json::<artisan_api::ApiError>().await.unwrap().code,
        artisan_api::ErrorCode::NotFound,
    );
    assert_eq!(store.head(), since, "canonical rejection performs no write");

    let refused_skill = editor
        .http
        .post(editor.url("/skills/no-such-artisan-skill/disable"))
        .send()
        .await
        .unwrap();
    assert_eq!(refused_skill.status(), 409);
    assert_eq!(
        refused_skill
            .json::<artisan_api::ApiError>()
            .await
            .unwrap()
            .code,
        artisan_api::ErrorCode::Conflict,
    );
    assert_eq!(store.head(), since, "extension refusal performs no write");

    let project = CreateProject {
        id: ProjectId::new(),
        path: std::env::temp_dir().display().to_string(),
        name: None,
    };
    assert!(
        editor
            .http
            .post(editor.url("/projects"))
            .json(&project)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let thread = CreateThread {
        id: ThreadId::new(),
        project_id: project.id.clone(),
        config: EngineConfig::new(EngineId::Claude),
        title: None,
        parent_thread_id: None,
        spawned_by: None,
    };
    assert!(
        editor
            .http
            .post(editor.url("/threads"))
            .json(&thread)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let message = SendMessage {
        draft: None,
        id: ItemId::new(),
        text: "hello".into(),
        images: Vec::new(),
        config: None,
    };
    let sent = editor
        .http
        .post(editor.url(&format!("/threads/{}/messages", thread.id)))
        .json(&message)
        .send()
        .await
        .unwrap();
    assert!(sent.status().is_success());

    // Claude Code isn't installed in this Forge, so the run fails — and says why.
    let stream = editor
        .http
        .get(editor.url(&format!("/events?since={since}")))
        .send()
        .await
        .unwrap();
    let events = read_events(
        stream,
        |envelope| matches!(&envelope.event, Event::Run { run } if run.status == RunStatus::Failed),
    )
    .await;
    assert!(matches!(events[0].event, Event::Hello { .. }));
    let failed = events.iter().find_map(|envelope| {
        if let Event::Run { run } = &envelope.event
            && run.status == RunStatus::Failed
        {
            run.error.clone()
        } else {
            None
        }
    });
    assert_eq!(failed.as_deref(), Some("Claude Code is not installed"));
    let seqs: Vec<_> = events.iter().filter_map(|envelope| envelope.seq).collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "stored changes arrive in seq order"
    );
    assert!(seqs.first().is_some_and(|first| *first > since));

    // Resuming after the last seq replays nothing old.
    let last = *seqs.last().unwrap();
    let stream = editor
        .http
        .get(editor.url(&format!("/events?since={last}")))
        .send()
        .await
        .unwrap();
    let events = read_events(stream, |envelope| {
        matches!(envelope.event, Event::Engine { .. })
    })
    .await;
    assert!(
        events
            .iter()
            .all(|envelope| envelope.seq.is_none_or(|seq| seq > last)),
        "resume cannot replay any old change"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn rich_resources_use_the_paired_forge_project_and_reject_non_regular_files() {
    let (dir, address, store) = forge().await;
    let invite = invite(&store).await.unwrap();
    let editor = device(&invite.forge_certificate, address);
    assert!(
        editor
            .http
            .post(editor.url("/pair"))
            .json(&PairRequest {
                code: invite.code,
                device_name: "resource fixture".into(),
            })
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let project_dir = dir.path().join("owning-forge-project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let filename = project_dir.join("reference with spaces.txt");
    std::fs::write(&filename, b"owned by this Forge\n").unwrap();
    let project = CreateProject {
        id: ProjectId::new(),
        path: project_dir.display().to_string(),
        name: None,
    };
    assert!(
        editor
            .http
            .post(editor.url("/projects"))
            .json(&project)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let thread = CreateThread {
        id: ThreadId::new(),
        project_id: project.id,
        config: EngineConfig::new(EngineId::Claude),
        title: None,
        parent_thread_id: None,
        spawned_by: None,
    };
    assert!(
        editor
            .http
            .post(editor.url("/threads"))
            .json(&thread)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let endpoint = editor.url(&format!("/threads/{}/resource", thread.id));
    for path in [
        "reference with spaces.txt".to_owned(),
        url::Url::from_file_path(&filename).unwrap().to_string(),
    ] {
        let response = editor
            .http
            .get(&endpoint)
            .query(&[("path", path)])
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(
            &response.bytes().await.unwrap()[..],
            b"owned by this Forge\n"
        );
    }
    let directory = editor
        .http
        .get(&endpoint)
        .query(&[("path", ".")])
        .send()
        .await
        .unwrap();
    assert_eq!(directory.status(), 400);
    let oversized = project_dir.join("oversized.bin");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(33 * 1024 * 1024)
        .unwrap();
    assert_eq!(
        editor
            .http
            .get(&endpoint)
            .query(&[("path", "oversized.bin")])
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        editor
            .http
            .get(&endpoint)
            .query(&[("path", "missing.bin")])
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    #[cfg(unix)]
    {
        let fifo = project_dir.join("waiting-pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            editor
                .http
                .get(&endpoint)
                .query(&[("path", "waiting-pipe")])
                .send(),
        )
        .await
        .expect("FIFO must be rejected before waiting for a writer")
        .unwrap();
        assert_eq!(response.status(), 400);
    }
}

/// What an Editor shows for a refused request comes from these messages
/// and codes: a missing thing is 404 and named once, a stale answer is a
/// conflict, and settings with their own route cannot be written raw.
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn refusals_name_what_is_missing_or_stale_and_only_switches_save_raw() {
    let (_dir, address, store) = forge().await;
    let invite = invite(&store).await.unwrap();
    let editor = device(&invite.forge_certificate, address);
    let pair = PairRequest {
        code: invite.code,
        device_name: "refusals".into(),
    };
    let paired = editor.http.post(editor.url("/pair")).json(&pair);
    assert!(paired.send().await.unwrap().status().is_success());

    let missing = editor
        .http
        .get(editor.url("/threads/no-such-thread"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    let error: artisan_api::ApiError = missing.json().await.unwrap();
    assert_eq!(error.message, "thread no-such-thread not found");

    let answer = artisan_api::Answer::Approval {
        decision: artisan_api::Decision::AllowOnce,
    };
    let unknown = editor
        .http
        .post(editor.url("/items/no-such-item/answer"))
        .json(&answer)
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);

    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/refusals".into(),
            name: None,
        })
        .await
        .unwrap();
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            title: None,
            config: EngineConfig::new(EngineId::Claude),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    let said = store
        .upsert_item(forge_store::NewItem {
            id: ItemId::new(),
            thread_id: thread.id,
            run_id: None,
            parent_id: None,
            native_id: None,
            status: artisan_api::Status::Done,
            kind: artisan_api::ItemKind::AssistantMessage {
                text: "done".into(),
                phase: artisan_api::Phase::Final,
            },
        })
        .await
        .unwrap();
    let stale = editor
        .http
        .post(editor.url(&format!("/items/{}/answer", said.id)))
        .json(&answer)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 409);

    let raw = editor
        .http
        .put(editor.url("/settings/global_prompt"))
        .json(&serde_json::json!("unversioned"))
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status(), 400);
    assert_ne!(
        store.setting("global_prompt").await.unwrap(),
        Some(serde_json::json!("unversioned"))
    );

    let off = editor
        .http
        .put(editor.url("/settings/you_should_know"))
        .json(&serde_json::json!(false))
        .send()
        .await
        .unwrap();
    assert_eq!(off.status(), 204);
    assert_eq!(
        store.setting("you_should_know").await.unwrap(),
        Some(serde_json::json!(false))
    );
    let garbled = editor
        .http
        .put(editor.url("/settings/suggestions"))
        .json(&serde_json::json!("off"))
        .send()
        .await
        .unwrap();
    assert_eq!(garbled.status(), 400);

    // The Behaviour card's switches and Claude's transport save too.
    for key in [
        "summary_titles",
        "continue_after_restart",
        "retry_when_overloaded",
        "continue_after_usage_limit",
    ] {
        let off = editor
            .http
            .put(editor.url(&format!("/settings/{key}")))
            .json(&serde_json::json!(false))
            .send()
            .await
            .unwrap();
        assert_eq!(off.status(), 204, "{key}");
        assert_eq!(
            store.setting(key).await.unwrap(),
            Some(serde_json::json!(false))
        );
    }
    let claude_ai = editor
        .http
        .put(editor.url("/settings/claude_transport"))
        .json(&serde_json::json!("claude_ai"))
        .send()
        .await
        .unwrap();
    assert_eq!(claude_ai.status(), 204);
    assert_eq!(
        store.setting("claude_transport").await.unwrap(),
        Some(serde_json::json!("claude_ai"))
    );
    let unknown = editor
        .http
        .put(editor.url("/settings/claude_transport"))
        .json(&serde_json::json!("carrier pigeon"))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_editors_get_item_pages_zstd_compressed_but_events_stay_plain() {
    let (_dir, address, store) = forge().await;
    let invite = invite(&store).await.unwrap();
    let identity = Identity::generate("artisan-editor").unwrap();
    let fingerprint = tls::fingerprint_of(&invite.forge_certificate).unwrap();
    let client = |zstd: bool| {
        reqwest::Client::builder()
            .use_preconfigured_tls(tls::client_config(&identity, &fingerprint).unwrap())
            .zstd(zstd)
            .build()
            .unwrap()
    };
    // The Editor's client on a remote host, and one that shows the wire.
    let (editor, wire) = (client(true), client(false));
    let url = |path: &str| format!("https://localhost:{}{path}", address.port());
    let pair = PairRequest {
        code: invite.code,
        device_name: "compression".into(),
    };
    let paired = editor.post(url("/pair")).json(&pair).send().await.unwrap();
    assert!(paired.status().is_success());
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/compression".into(),
            name: None,
        })
        .await
        .unwrap();
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            title: None,
            config: EngineConfig::new(EngineId::Claude),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    for reply in 0..40 {
        store
            .upsert_item(forge_store::NewItem {
                id: ItemId::new(),
                thread_id: thread.id.clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: artisan_api::Status::Done,
                kind: artisan_api::ItemKind::AssistantMessage {
                    text: format!("Reply {reply}: the build passed and the tests are green.")
                        .into(),
                    phase: artisan_api::Phase::Final,
                },
            })
            .await
            .unwrap();
    }
    let items = url(&format!("/threads/{}/items?limit=40", thread.id));

    let compressed = wire.get(&items).header("accept-encoding", "zstd");
    let compressed = compressed.send().await.unwrap();
    assert_eq!(compressed.headers()["content-encoding"], "zstd");
    let compressed = compressed.bytes().await.unwrap().len();
    let plain = wire.get(&items).send().await.unwrap();
    assert!(plain.headers().get("content-encoding").is_none());
    let plain = plain.bytes().await.unwrap();
    assert!(
        compressed * 3 < plain.len(),
        "{compressed} compressed bytes for {} plain",
        plain.len()
    );

    let page: artisan_api::ItemsPage = editor
        .get(&items)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let expected: artisan_api::ItemsPage = serde_json::from_slice(&plain).unwrap();
    assert_eq!(page.items.len(), 40);
    assert_eq!(page.items, expected.items);

    // The event stream flushes every event, so it is never compressed.
    let events = wire
        .get(url("/events?since=0"))
        .header("accept-encoding", "zstd")
        .send()
        .await
        .unwrap();
    assert!(events.headers().get("content-encoding").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_late_bind_leaves_pending_inputs_and_releases_startup_owners() {
    use std::sync::Arc;
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("forge");
    let store = Store::open(&data).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: directory.path().display().to_string(),
            name: None,
        })
        .await
        .unwrap();
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    let input = SendMessage {
        id: ItemId::new(),
        text: "must stay pending".into(),
        images: Vec::new(),
        config: None,
        draft: None,
    };
    let input_id = input.id.clone();
    store
        .send_message(
            thread.id.clone(),
            input,
            false,
            forge_store::Acceptance::default(),
        )
        .await
        .unwrap();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let marker = Arc::new(());
    let owner = Arc::downgrade(&marker);
    let harness_for: forge_runs::HarnessFor = Arc::new(move |engine| {
        // Owns the marker, so dropping the harness releases it.
        let _ = &marker;
        forge_engines::harness(engine)
    });
    let result = start_with_harness(
        ServeConfig {
            data_dir: data.clone(),
            listen: occupied.local_addr().unwrap(),
        },
        harness_for,
    )
    .await;
    assert!(
        result.is_err(),
        "the occupied listener rejects this controller"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while owner.upgrade().is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "failed startup retained Hub dispatcher/status owners"
        );
        tokio::task::yield_now().await;
    }
    assert!(
        store.state().await.unwrap().runs.is_empty(),
        "a failed bind must not claim pending inputs"
    );
    assert_eq!(
        store.item(&input_id).await.unwrap().unwrap().status,
        artisan_api::Status::Running
    );
    let _released =
        lock_data_dir(&data).expect("failed startup releases only its temporary instance lock");
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_refresh_starts_codex_while_claude_probe_is_still_waiting() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("forge");
    let install = |engine: &str, script: &str| {
        let root = data.join("engines").join(engine);
        let version = root.join("versions/1.0.0");
        std::fs::create_dir_all(&version).unwrap();
        let binary = version.join(if engine == "codex" {
            "vendor/x86_64-unknown-linux-musl/bin/codex"
        } else {
            engine
        });
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("current"), "1.0.0").unwrap();
        binary
    };
    // These are this fixture's own CLI processes under its temporary engine
    // homes. Claude cannot finish its sign-in probe until the test releases it.
    let claude = install(
        "claude",
        "#!/bin/sh\n: > \"$0.started\"\nwhile [ ! -e \"$0.release\" ]; do sleep 0.02; done\nprintf '{\"loggedIn\":false}'\n",
    );
    let codex = install(
        "codex",
        "#!/bin/sh\n: > \"$0.started\"\nprintf 'not logged in\\n'\n",
    );
    let marker = |binary: &std::path::Path, suffix: &str| {
        let mut path = binary.as_os_str().to_os_string();
        path.push(suffix);
        std::path::PathBuf::from(path)
    };
    let server = start(ServeConfig {
        data_dir: data,
        listen: "127.0.0.1:0".parse().unwrap(),
    })
    .await
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if marker(&claude, ".started").exists() && marker(&codex, ".started").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    // Release even on a failed assertion so teardown cannot leave this
    // fixture's blocked probe alive. No active Forge is involved.
    std::fs::write(marker(&claude, ".release"), "").unwrap();
    drop(server);
    result.expect("Codex startup refresh must begin before slow Claude finishes");
}

/// An engine as the gateway test scripts it. Codex agents answer with what
/// they were asked; a Claude thread asked to "recall" uses the Artisan MCP it
/// was launched with, and answers with what the tools said.
struct McpScripted;

/// Posts one JSON-RPC request to the Artisan MCP and returns its result.
async fn mcp(
    http: &reqwest::Client,
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let reply: serde_json::Value = http
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    reply["result"].clone()
}

/// The ids a tool's markdown names in code spans.
fn ids(text: &str) -> Vec<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| span.len() == 36 && span.matches('-').count() == 4)
        .map(str::to_owned)
        .collect()
}

/// The text a tool call answered.
async fn tool(
    http: &reqwest::Client,
    url: &str,
    name: &str,
    arguments: serde_json::Value,
) -> String {
    let result = mcp(
        http,
        url,
        "tools/call",
        serde_json::json!({ "name": name, "arguments": arguments }),
    )
    .await;
    assert_eq!(result["isError"], false, "{name} failed: {result}");
    result["content"][0]["text"].as_str().unwrap().to_owned()
}

impl forge_engines::Harness for McpScripted {
    #[expect(
        clippy::too_many_lines,
        reason = "a scripted engine: one branch per test prompt"
    )]
    fn run(
        &self,
        turn: forge_engines::Turn,
        io: forge_engines::RunIo,
    ) -> futures::future::BoxFuture<'static, forge_engines::Outcome> {
        Box::pin(async move {
            use forge_engines::EngineEvent;
            io.emit(EngineEvent::InputAccepted {
                id: turn.input.id.clone(),
            })
            .await;
            let answer = if turn.engine == EngineId::Codex {
                format!("child heard: {}", turn.input.text)
            } else if turn.input.text == "recall" {
                // Claude takes the server as inline `--mcp-config` JSON.
                let url = turn
                    .launch
                    .args
                    .iter()
                    .filter_map(|arg| serde_json::from_str::<serde_json::Value>(arg).ok())
                    .find_map(|config| {
                        config["mcpServers"]["artisan"]["url"]
                            .as_str()
                            .map(str::to_owned)
                    })
                    .expect("a Claude thread is launched with the Artisan MCP");
                let http = reqwest::Client::new();
                let hello = mcp(
                    &http,
                    &url,
                    "initialize",
                    serde_json::json!({ "protocolVersion": "2025-06-18" }),
                )
                .await;
                assert_eq!(hello["serverInfo"]["name"], "artisan");
                // The engine reports its own call, as Claude does; recall
                // must not find it.
                io.emit(EngineEvent::Item {
                    key: "search".to_owned(),
                    parent: None,
                    status: artisan_api::Status::Done,
                    kind: artisan_api::ItemKind::Tool {
                        tool: artisan_api::ToolKind::Mcp {
                            server: "artisan".into(),
                            tool: "threads_search".into(),
                            input: Some(r#"{"query":"zebra-42"}"#.into()),
                        },
                        title: None,
                        output: None,
                        exit_code: None,
                        approval: None,
                    },
                })
                .await;
                let found = tool(
                    &http,
                    &url,
                    "threads_search",
                    serde_json::json!({ "query": "zebra-42" }),
                )
                .await;
                let spawned = tool(
                    &http,
                    &url,
                    "agent_spawn",
                    serde_json::json!({ "task": "echo", "prompt": "say hi", "harness": "codex" }),
                )
                .await;
                let waited = tool(
                    &http,
                    &url,
                    "agent_wait",
                    serde_json::json!({ "agents": ids(&spawned), "timeout_seconds": 20 }),
                )
                .await;
                format!("FOUND {found}\nWAITED {waited}")
            } else if turn.input.text == "background" {
                let url = turn
                    .launch
                    .args
                    .iter()
                    .filter_map(|arg| serde_json::from_str::<serde_json::Value>(arg).ok())
                    .find_map(|config| {
                        config["mcpServers"]["artisan"]["url"]
                            .as_str()
                            .map(str::to_owned)
                    })
                    .unwrap();
                tool(
                    &reqwest::Client::new(),
                    &url,
                    "agent_spawn",
                    serde_json::json!({ "task": "echo later", "prompt": "later", "harness": "codex", "background": true }),
                )
                .await
            } else {
                format!("noted: {}", turn.input.text)
            };
            io.emit(EngineEvent::Item {
                key: "reply".to_owned(),
                parent: None,
                status: artisan_api::Status::Done,
                kind: artisan_api::ItemKind::AssistantMessage {
                    text: answer.into(),
                    phase: artisan_api::Phase::Final,
                },
            })
            .await;
            forge_engines::Outcome::done()
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn a_thread_recalls_another_thread_and_runs_a_codex_agent_over_the_gateway() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("forge");
    for engine in [EngineId::Claude, EngineId::Codex] {
        let root = data.join("engines").join(engine.to_string());
        let binary = root
            .join("versions/1.0.0")
            .join(if engine == EngineId::Codex {
                "vendor/x86_64-unknown-linux-musl/bin/codex"
            } else {
                "claude"
            });
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, "").unwrap();
        std::fs::write(root.join("current"), "1.0.0").unwrap();
    }
    let harness: Arc<dyn forge_engines::Harness> = Arc::new(McpScripted);
    let server = start_with_harness(
        ServeConfig {
            data_dir: data.clone(),
            listen: "127.0.0.1:0".parse().unwrap(),
        },
        Arc::new(move |_| Arc::clone(&harness)),
    )
    .await
    .unwrap();
    let hub = server.hub.clone();
    let store = server.store().clone();
    tokio::spawn(server.run());

    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: data.display().to_string(),
            name: None,
        })
        .await
        .unwrap();
    let new_thread = || CreateThread {
        id: ThreadId::new(),
        project_id: project.id.clone(),
        config: EngineConfig::new(EngineId::Claude),
        title: None,
        parent_thread_id: None,
        spawned_by: None,
    };
    let send = |text: &str| SendMessage {
        draft: None,
        id: ItemId::new(),
        text: text.to_owned(),
        images: Vec::new(),
        config: None,
    };
    let replied = |thread: ThreadId| {
        let store = store.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    if let Some(reply) = store.last_reply(&thread).await.unwrap() {
                        return reply;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the thread replies")
        }
    };

    let earlier = store.create_thread(new_thread()).await.unwrap();
    hub.send_message(&earlier.id, send("The deploy key lives at secret/zebra-42"))
        .await
        .unwrap();
    replied(earlier.id.clone()).await;

    let asking = store.create_thread(new_thread()).await.unwrap();
    hub.send_message(&asking.id, send("recall")).await.unwrap();
    let reply = replied(asking.id.clone()).await;

    let (found, waited) = reply.split_once("\nWAITED ").unwrap();
    assert!(
        found.contains("zebra-42"),
        "search shows the match: {found}"
    );
    let hits = ids(found);
    assert!(!hits.is_empty(), "each hit names its item: {found}");
    for hit in hits {
        assert_eq!(
            store
                .item(&ItemId::from(hit))
                .await
                .unwrap()
                .unwrap()
                .thread_id,
            earlier.id,
            "search finds the earlier thread, not its own call: {found}"
        );
    }
    assert!(
        waited.contains("child heard: say hi"),
        "the Codex agent's reply comes back: {waited}"
    );

    // The parent keeps an Agent item naming the harness and the agent's own
    // thread, which the Agents panel shows while it works.
    let items = store
        .transcript_page(
            &asking.id,
            None,
            artisan_api::ItemsQuery {
                before: None,
                after: None,
                limit: Some(50),
            },
        )
        .await
        .unwrap()
        .items;
    let (engine, child) = items
        .iter()
        .find_map(|item| {
            if let artisan_api::ItemKind::Agent { engine, thread, .. } = &item.kind {
                Some((*engine, thread.clone()))
            } else {
                None
            }
        })
        .expect("the parent shows the agent");
    assert_eq!(engine, Some(EngineId::Codex), "with its harness");
    let child = child.expect("and opens its own thread");
    assert_eq!(
        store.thread(&child).await.unwrap().unwrap().config.engine,
        EngineId::Codex
    );
    assert_eq!(
        store.last_reply(&child).await.unwrap().as_deref(),
        Some("child heard: say hi"),
        "the agent starts from its prompt alone"
    );

    // A background agent's result wakes the parent as the agent's report,
    // which the Editor shows as the agent's, not as the user's message.
    let background = store.create_thread(new_thread()).await.unwrap();
    hub.send_message(&background.id, send("background"))
        .await
        .unwrap();
    let report = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let items = store
                .transcript_page(
                    &background.id,
                    None,
                    artisan_api::ItemsQuery {
                        before: None,
                        after: None,
                        limit: Some(50),
                    },
                )
                .await
                .unwrap()
                .items;
            if let Some(report) = items.into_iter().find_map(|item| {
                if let artisan_api::ItemKind::UserMessage {
                    text,
                    from_agent: Some(agent),
                    ..
                } = item.kind
                {
                    Some((text, agent))
                } else {
                    None
                }
            }) {
                return report;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the agent's result arrives");
    assert!(
        report.0.inline().unwrap().contains("child heard: later"),
        "{:?}",
        report.0
    );
    assert!(matches!(
        store.item(&report.1).await.unwrap().unwrap().kind,
        artisan_api::ItemKind::Agent {
            engine: Some(EngineId::Codex),
            ..
        }
    ));
}
