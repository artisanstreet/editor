//! The connection against a real Forge: pairing, the first state, live
//! events, and resuming after Forge restarts without missing a change.

use crate::ForgeClient;
use crate::connection::{self, ConnectionStatus, Update};
use crate::hosts::{self, HostKind};
use crate::runtime::runtime;
use artisan_api::tls::Identity;
use artisan_api::{CreateProject, Event, ProjectId};
use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

/// A Forge on its own runtime, so stopping it really drops every
/// connection, like a restart.
fn start_forge(
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
    updates: &mut UnboundedReceiver<Update>,
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
    .expect("update arrives")
}

fn project_created(update: &Update, path: &str) -> bool {
    matches!(update, Update::Event(envelope) if matches!(&envelope.event, Event::Project { project } if project.path == path))
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

    let (sender, mut updates) = futures::channel::mpsc::unbounded();
    connection::follow(
        client.clone(),
        sender,
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    net.block_on(next_matching(&mut updates, |update| {
        matches!(update, Update::State(_))
    }));
    net.block_on(next_matching(&mut updates, |update| {
        matches!(update, Update::Status(ConnectionStatus::Live))
    }));

    let first = dir.path().join("first");
    std::fs::create_dir_all(&first).unwrap();
    let first = first.display().to_string();
    net.block_on(client.create_project(&CreateProject {
        id: ProjectId::new(),
        path: first.clone(),
        name: None,
    }))
    .unwrap();
    net.block_on(next_matching(&mut updates, |update| {
        project_created(update, &first)
    }));

    // Forge stops; a change lands while the Editor is disconnected.
    drop(store);
    forge.shutdown_timeout(Duration::from_secs(5));
    net.block_on(next_matching(&mut updates, |update| {
        matches!(update, Update::Status(ConnectionStatus::Offline { .. }))
    }));
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
    net.block_on(next_matching(&mut updates, |update| {
        matches!(update, Update::Status(ConnectionStatus::Live))
    }));
    let update = net.block_on(next_matching(
        &mut updates,
        |update| matches!(update, Update::Event(envelope) if envelope.seq.is_some()),
    ));
    assert!(
        project_created(&update, &second),
        "the first stored change after reconnecting is the missed one: {update:?}"
    );
}
