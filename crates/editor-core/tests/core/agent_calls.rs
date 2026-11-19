#![cfg(unix)]

use artisan_api::{EditorCall, EditorReply, Event, tls::Identity};
use axum::extract::{Path, State};
use editor_core::runtime::runtime;
use editor_core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use serde_json::json;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_support::http::StubServer;

type Replies = Arc<Mutex<Vec<(String, EditorReply)>>>;

async fn reply(
    State(replies): State<Replies>,
    Path(id): Path<String>,
    axum::Json(reply): axum::Json<EditorReply>,
) {
    replies.lock().expect("replies").push((id, reply));
}

struct Forge {
    directory: tempfile::TempDir,
    _server: StubServer,
    replies: Replies,
    core: Core,
    host: HostId,
}

fn forge() -> Forge {
    let replies = Replies::default();
    let router = axum::Router::new()
        .route("/editor/calls/{id}", axum::routing::post(reply))
        .with_state(Arc::clone(&replies));
    let server = runtime().block_on(StubServer::serve(router));
    let directory = tempfile::tempdir().expect("temporary directory");
    std::fs::write(
        directory.path().join("settings.json"),
        b"{\"dark_mode\": false}",
    )
    .expect("seed settings.json");
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .expect("core opens");
    let identity = Identity::generate("agent-calls-test").expect("identity");
    let config = HostConfig {
        id: HostId("stub-forge".into()),
        name: "Stub".into(),
        address: server.url().trim_start_matches("http://").to_owned(),
        fingerprint: artisan_api::tls::fingerprint(
            identity.certificate_der().expect("certificate").as_ref(),
        ),
        kind: HostKind::Remote,
    };
    core.add_test_host(&config, ForgeClient::plain_for_tests(server.url()));
    core.host_mut(&config.id)
        .expect("host")
        .test_stop_connection();
    Forge {
        directory,
        _server: server,
        replies,
        core,
        host: config.id,
    }
}

impl Forge {
    fn call(&mut self, id: &str, call: EditorCall) -> EditorReply {
        let sender = self
            .core
            .host(&self.host)
            .expect("host")
            .test_transition_sender();
        runtime().block_on(sender.event(Event::EditorCall {
            id: id.to_owned(),
            call,
        }));
        let replies = Arc::clone(&self.replies);
        let mut answered = None;
        test_support::wait::wait_until(
            &format!("the Editor to answer call {id}"),
            Duration::from_secs(10),
            || {
                self.core.drain();
                answered = replies
                    .lock()
                    .expect("replies")
                    .iter()
                    .find(|(answered, _)| answered == id)
                    .map(|(_, reply)| reply.clone());
                answered.is_some()
            },
        );
        answered.expect("the call was answered")
    }

    fn stored(&self) -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(self.directory.path().join("settings.json")).expect("settings.json"),
        )
        .expect("settings.json is json")
    }
}

#[test]
fn an_agent_hears_that_a_setting_could_not_be_saved() {
    let mut forge = forge();
    let writable = std::fs::metadata(forge.directory.path())
        .expect("folder metadata")
        .permissions();
    std::fs::set_permissions(
        forge.directory.path(),
        std::fs::Permissions::from_mode(0o555),
    )
    .expect("make the settings folder read-only");

    let reply = forge.call(
        "set-dark-mode",
        EditorCall::SettingSet {
            key: "dark_mode".into(),
            value: json!(true),
        },
    );
    std::fs::set_permissions(forge.directory.path(), writable).expect("restore the folder");

    assert!(
        matches!(&reply, EditorReply::Error(error) if error.starts_with("save dark_mode")),
        "the agent is told the save failed, not that it succeeded: {reply:?}"
    );
    assert_eq!(forge.stored(), json!({"dark_mode": false}));
}

#[test]
fn an_agent_hears_success_only_once_the_setting_is_on_disk() {
    let mut forge = forge();

    let reply = forge.call(
        "set-dark-mode",
        EditorCall::SettingSet {
            key: "dark_mode".into(),
            value: json!(true),
        },
    );

    assert_eq!(
        reply,
        EditorReply::Ok(json!({"key": "dark_mode", "value": true}))
    );
    assert_eq!(forge.stored(), json!({"dark_mode": true}));
}
