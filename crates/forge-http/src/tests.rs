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
    stop: impl Fn(&Envelope) -> bool,
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
    let failed = events.iter().find_map(|envelope| match &envelope.event {
        Event::Run { run } if run.status == RunStatus::Failed => run.error.clone(),
        _ => None,
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
        .send_message(thread.id.clone(), input, false)
        .await
        .unwrap();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let marker = Arc::new(());
    let owner = Arc::downgrade(&marker);
    let harness_for: forge_runs::HarnessFor = Arc::new(move |engine| {
        let _retained = &marker;
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
        } else { engine });
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("current"), "1.0.0").unwrap();
        binary
    };
    // These are this fixture's own CLI processes under its temporary engine
    // homes. Claude cannot finish its sign-in probe until the test releases it.
    let claude = install("claude", "#!/bin/sh\n: > \"$0.started\"\nwhile [ ! -e \"$0.release\" ]; do sleep 0.02; done\nprintf '{\"loggedIn\":false}'\n");
    let codex = install("codex", "#!/bin/sh\n: > \"$0.started\"\nprintf 'not logged in\\n'\n");
    let marker = |binary: &std::path::Path, suffix: &str| {
        let mut path = binary.as_os_str().to_os_string();
        path.push(suffix);
        std::path::PathBuf::from(path)
    };
    let server = start(ServeConfig {
        data_dir: data,
        listen: "127.0.0.1:0".parse().unwrap(),
    }).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if marker(&claude, ".started").exists() && marker(&codex, ".started").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await;
    // Release even on a failed assertion so teardown cannot leave this
    // fixture's blocked probe alive. No active Forge is involved.
    std::fs::write(marker(&claude, ".release"), "").unwrap();
    drop(server);
    result.expect("Codex startup refresh must begin before slow Claude finishes");
}
