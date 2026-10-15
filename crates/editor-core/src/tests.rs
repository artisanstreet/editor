//! The connection against a real Forge: pairing, the first state, live
//! events, and resuming after Forge restarts without missing a change.

use crate::ForgeClient;
use crate::connection::{self, ConnectionStatus, Update};
use crate::hosts::{self, HostKind};
use crate::runtime::runtime;
use artisan_api::tls::Identity;
use artisan_api::{CreateProject, Event, ProjectId};
use futures::StreamExt as _;
use futures::channel::mpsc::Receiver;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

/// A Forge on its own runtime, so stopping it really drops every
/// connection, like a restart.
pub(crate) fn start_forge(
    data: &Path,
    listen: SocketAddr,
) -> (SocketAddr, forge_store::Store, tokio::runtime::Runtime) {
    let forge = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let server = forge
        .block_on(forge_http::start(forge_http::ServeConfig {
            data_dir: data.to_path_buf(),
            listen,
        }))
        .unwrap();
    let address = server.local_addr().unwrap();
    let store = server.store().clone();
    forge.spawn(async move {
        let _ = server.run().await;
    });
    (address, store, forge)
}

async fn next_matching(
    updates: &mut Receiver<Update>,
    stage: &str,
    check: impl Fn(&Update) -> bool,
) -> Update {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let update = updates.next().await.expect("connection keeps running");
            if check(&update) {
                return update;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {stage}"))
}

fn project_created(update: &Update, path: &str) -> bool {
    matches!(update, Update::Event(envelope, _) if matches!(&envelope.event, Event::Project { project } if project.path == path))
}

#[test]
fn follows_forge_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("forge");
    let (address, store, forge) = start_forge(&data, "127.0.0.1:0".parse().unwrap());
    let net = runtime();

    let identity = Identity::generate("artisan-editor").unwrap();
    let invite = forge.block_on(forge_http::invite(&store)).unwrap();
    let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate).unwrap();
    let host = net
        .block_on(hosts::pair(
            &address.to_string(),
            &invite.code,
            &fingerprint,
            &identity,
            "test",
            "Forge".into(),
            HostKind::Local,
        ))
        .unwrap();
    let client = ForgeClient::new(&host.address, &host.fingerprint, &identity).unwrap();

    let (sender, mut updates) = futures::channel::mpsc::channel(64);
    let (observed, observation) = tokio::sync::watch::channel(None);
    let _connection = connection::follow(
        client.clone(),
        sender,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        observation,
    );
    net.block_on(next_matching(
        &mut updates,
        "bootstrap snapshot",
        |update| matches!(update, Update::State(_)),
    ));
    net.block_on(next_matching(
        &mut updates,
        "initial event stream",
        |update| matches!(update, Update::Status(ConnectionStatus::Live)),
    ));

    // Navigation changes only the event filter. It must not announce a host
    // disconnection or bootstrap a replacement state before the new stream.
    for thread in [Some(artisan_api::ThreadId::new()), None] {
        observed.send(thread).unwrap();
        net.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    match updates.next().await.expect("connection remains owned") {
                        Update::Status(ConnectionStatus::Live) => break,
                        Update::Status(status) => {
                            panic!("navigation changed host status: {status:?}")
                        }
                        Update::State(_) => panic!("navigation reloaded host state"),
                        Update::Event(..) => {}
                    }
                }
            })
            .await
        })
        .unwrap();
    }

    let first = dir.path().join("first");
    std::fs::create_dir_all(&first).unwrap();
    let first = first.display().to_string();
    net.block_on(client.create_project(&CreateProject {
        id: ProjectId::new(),
        path: first.clone(),
        name: None,
    }))
    .unwrap();
    net.block_on(next_matching(
        &mut updates,
        "first project event",
        |update| project_created(update, &first),
    ));

    // Forge stops; a change lands while the Editor is disconnected.
    drop(store);
    forge.shutdown_timeout(Duration::from_secs(5));
    net.block_on(next_matching(
        &mut updates,
        "offline after restart",
        |update| matches!(update, Update::Status(ConnectionStatus::Offline { .. })),
    ));
    let (_, store, forge) = start_forge(&data, address);
    let second = dir.path().join("second");
    std::fs::create_dir_all(&second).unwrap();
    let second = second.display().to_string();
    forge
        .block_on(store.create_project(CreateProject {
            id: ProjectId::new(),
            path: second.clone(),
            name: None,
        }))
        .unwrap();

    // The Editor reconnects with its last seq and gets exactly the missed change.
    net.block_on(next_matching(
        &mut updates,
        "resumed event stream",
        |update| matches!(update, Update::Status(ConnectionStatus::Live)),
    ));
    let update = net.block_on(next_matching(
        &mut updates,
        "missed project replay",
        |update| matches!(update, Update::Event(envelope, _) if envelope.seq.is_some()),
    ));
    assert!(
        project_created(&update, &second),
        "the first stored change after reconnecting is the missed one: {update:?}"
    );
}

/// The renderer opens 512 logical paragraphs over paired HTTPS without fetching
/// the full 20 MiB source. Counters surround actual ForgeClient calls, not Store.
#[gpui::test]
fn sparse_stored_renderer_bootstrap_uses_three_paired_https_requests(
    cx: &mut gpui::TestAppContext,
) {
    // The fixture intentionally exercises the real HTTP and Markdown worker threads.
    cx.executor().allow_parking();
    use artisan_api::{
        CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, Status, TextContent,
        ThreadId,
    };
    use editor_ui::markdown_renderer::{
        MarkdownContentFetch, MarkdownPreparation, MarkdownRenderer, MarkdownSourceIndexFetch,
        StoredPreparation,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let dir = tempfile::tempdir().unwrap();
    let (address, store, forge) = start_forge(
        &dir.path().join("isolated-source-forge"),
        "127.0.0.1:0".parse().unwrap(),
    );
    let identity = Identity::generate("sparse-source-client").unwrap();
    let invite = forge.block_on(forge_http::invite(&store)).unwrap();
    let pin = artisan_api::tls::fingerprint_of(&invite.forge_certificate).unwrap();
    runtime()
        .block_on(hosts::pair(
            &address.to_string(),
            &invite.code,
            &pin,
            &identity,
            "source-test",
            "Forge".into(),
            HostKind::Local,
        ))
        .unwrap();
    let client = ForgeClient::new(&address.to_string(), &pin, &identity).unwrap();
    let project = forge
        .block_on(store.create_project(CreateProject {
            id: ProjectId::new(),
            path: dir.path().display().to_string(),
            name: None,
        }))
        .unwrap();
    let thread = forge
        .block_on(store.create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Codex),
            title: None,
            parent_thread_id: None,
        }))
        .unwrap();
    let text = format!(
        "[far]: https://example.com\n\n{}Latest &amp; [reference][far].\n",
        format!("{}\n\n", "β".repeat(20530)).repeat(511)
    );
    assert!(text.len() > 20 * 1024 * 1024);
    let saved = forge
        .block_on(store.upsert_item(forge_store::NewItem {
            id: ItemId::new(),
            thread_id: thread.id,
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::AssistantMessage {
                text: text.into(),
                phase: Phase::Final,
            },
        }))
        .unwrap();
    let ItemKind::AssistantMessage {
        text: TextContent::Stored(reference),
        ..
    } = saved.kind
    else {
        panic!("must externalize source");
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let bytes = Arc::new(AtomicUsize::new(0));
    let raw_client = client.clone();
    let raw_calls = calls.clone();
    let raw_bytes = bytes.clone();
    let raw: MarkdownContentFetch = Arc::new(move |reference, offset, limit| {
        let client = raw_client.clone();
        let calls = raw_calls.clone();
        let bytes = raw_bytes.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let range = client
                .content_range(&reference, offset, limit)
                .await
                .map_err(|error| error.to_string())?;
            bytes.fetch_add(range.text.len(), Ordering::SeqCst);
            Ok(range)
        })
    });
    let index_calls = calls.clone();
    let index: MarkdownSourceIndexFetch = Arc::new(move |reference, format, offset, limit| {
        let client = client.clone();
        let calls = index_calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            client
                .source_index_range(&reference, format, offset, limit)
                .await
                .map_err(|error| error.to_string())
        })
    });
    let renderer = MarkdownRenderer::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let source = loop {
        let state = cx.update(|cx| {
            renderer.prepare_stored_source(
                "remote-index".into(),
                reference.clone(),
                artisan_api::SourceIndexFormat::Markdown,
                raw.clone(),
                index.clone(),
                cx,
                |_| {},
            )
        });
        match state {
            StoredPreparation::Ready(source) => break source,
            StoredPreparation::Failed(error) => panic!("{error}"),
            StoredPreparation::Pending => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "HTTPS descriptor bootstrap timed out"
        );
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(source.block_count(), 512);
    assert_eq!(source.loaded_range(), 448..512);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        bytes.load(Ordering::SeqCst),
        0,
        "bootstrap must download no original source"
    );
    assert!(!renderer.is_stored_leaf_prepared(&"remote-leaf".into(), &source, 511));
    loop {
        let state = cx.update(|cx| {
            renderer.prepare_stored_leaf("remote-leaf".into(), &source, 511, cx, |_| {})
        });
        match state {
            MarkdownPreparation::Ready(leaf) => {
                assert_eq!(leaf.block_count(), 1);
                assert!(!leaf.is_preview());
                break;
            }
            MarkdownPreparation::Failed(error) => panic!("{error}"),
            MarkdownPreparation::Pending => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "HTTPS visible-leaf preparation timed out"
        );
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "one descriptor, one exact parser-record range, one original leaf range"
    );
    assert!(bytes.load(Ordering::SeqCst) < 65536);
    assert!(renderer.is_stored_leaf_prepared(&"remote-leaf".into(), &source, 511));
    // Preparation is intentionally separate from actual body paint/read evidence.
    cx.update(|cx| renderer.clear(cx));
    drop(store);
    forge.shutdown_timeout(Duration::from_secs(5));
}
