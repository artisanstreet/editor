#![cfg(unix)]

use artisan_api::{EditorCall, EditorReply, Event};
use axum::extract::{Path, State};
use editor_core::Clock;
use serde_json::json;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::{Arc, Mutex};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

type Replies = Arc<Mutex<Vec<(String, EditorReply)>>>;

async fn reply(
    State(replies): State<Replies>,
    Path(id): Path<String>,
    axum::Json(reply): axum::Json<EditorReply>,
) {
    replies.lock().expect("replies").push((id, reply));
}

struct Forge {
    stubbed: StubbedHost,
    replies: Replies,
}

fn forge() -> Forge {
    let replies = Replies::default();
    let router = axum::Router::new()
        .route("/editor/calls/{id}", axum::routing::post(reply))
        .with_state(Arc::clone(&replies));
    let directory = tempfile::tempdir().expect("temporary directory");
    std::fs::write(
        directory.path().join("settings.json"),
        b"{\"dark_mode\": false}",
    )
    .expect("seed settings.json");
    Forge {
        stubbed: StubbedHost::start_in(directory, empty_state(), router, Clock::system()),
        replies,
    }
}

impl Forge {
    fn call(&mut self, id: &str, call: EditorCall) -> EditorReply {
        self.stubbed.forge.publish(Event::EditorCall {
            id: id.to_owned(),
            call,
        });
        let replies = Arc::clone(&self.replies);
        let mut answered = None;
        self.stubbed
            .settle(&format!("the Editor to answer call {id}"), |_, _| {
                answered = replies
                    .lock()
                    .expect("replies")
                    .iter()
                    .find(|(answered, _)| answered == id)
                    .map(|(_, reply)| reply.clone());
                answered.is_some()
            });
        answered.expect("the call was answered")
    }

    fn stored(&self) -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(self.stubbed.directory.path().join("settings.json"))
                .expect("settings.json"),
        )
        .expect("settings.json is json")
    }
}

#[test]
fn an_agent_hears_that_a_setting_could_not_be_saved() {
    let mut forge = forge();
    let writable = std::fs::metadata(forge.stubbed.directory.path())
        .expect("folder metadata")
        .permissions();
    std::fs::set_permissions(
        forge.stubbed.directory.path(),
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
    std::fs::set_permissions(forge.stubbed.directory.path(), writable).expect("restore the folder");

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
