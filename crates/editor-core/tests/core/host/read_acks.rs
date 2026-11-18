use artisan_api::{Event, ThreadId, tls::Identity};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use editor_core::runtime::runtime;
use editor_core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_support::http::StubServer;
use test_support::wait::wait_until;

#[derive(Clone, Default)]
struct Stub {
    acked: Arc<Mutex<Vec<String>>>,
    release: Arc<tokio::sync::Notify>,
}

fn thread(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "project_id": "project",
        "title": id,
        "config": {"engine": "claude"},
        "activity_seq": 5,
        "read_seq": 0,
        "seq": 3,
        "created_at": 0,
        "updated_at": 0,
        "archived": false,
    })
}

async fn acknowledge(State(stub): State<Stub>, Path(id): Path<String>) -> Response {
    stub.acked.lock().expect("acked list").push(id.clone());
    match id.as_str() {
        "gone" => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"code": "not_found", "message": "no such thread"})),
        )
            .into_response(),
        "held" => {
            stub.release.notified().await;
            let mut updated = thread(&id);
            updated["seq"] = serde_json::json!(4);
            updated["read_seq"] = serde_json::json!(5);
            axum::Json(updated).into_response()
        }
        _ => {
            let mut updated = thread(&id);
            updated["seq"] = serde_json::json!(4);
            updated["read_seq"] = serde_json::json!(5);
            axum::Json(updated).into_response()
        }
    }
}

struct Forge {
    _directory: tempfile::TempDir,
    _server: StubServer,
    stub: Stub,
    core: Core,
    host: HostId,
}

fn forge() -> Forge {
    let stub = Stub::default();
    let router = axum::Router::new()
        .route("/threads/{id}", axum::routing::patch(acknowledge))
        .with_state(stub.clone());
    let server = runtime().block_on(StubServer::serve(router));
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .expect("core opens");
    let identity = Identity::generate("read-acks-test").expect("identity");
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
        _directory: directory,
        _server: server,
        stub,
        core,
        host: config.id,
    }
}

impl Forge {
    fn read(&mut self, id: &str) {
        let thread_id = ThreadId(id.into());
        let host = self.core.host_mut(&self.host).expect("host");
        host.observe_thread(Some(thread_id.clone()));
        let sender = host.test_transition_sender();
        runtime().block_on(sender.event(Event::Thread {
            thread: serde_json::from_value(thread(id)).expect("thread"),
        }));
        let host = self.host.clone();
        wait_until(
            "the observed thread arrives",
            Duration::from_secs(10),
            || {
                self.core.drain_with(|_, _| {});
                self.core
                    .host(&host)
                    .expect("host")
                    .threads
                    .contains_key(&thread_id)
            },
        );
        self.core
            .host_mut(&host)
            .expect("host")
            .acknowledge_read(thread_id, 5);
    }

    fn wait_for_ack(&mut self, id: &str) {
        let acked = Arc::clone(&self.stub.acked);
        wait_until(
            &format!("Forge to receive the read acknowledgement of {id}"),
            Duration::from_secs(10),
            || {
                self.core.drain_with(|_, _| {});
                acked
                    .lock()
                    .expect("acked list")
                    .iter()
                    .any(|acked| acked == id)
            },
        );
    }
}

#[test]
fn a_thread_forge_no_longer_has_does_not_block_other_read_acknowledgements() {
    let mut forge = forge();
    forge.read("gone");
    forge.wait_for_ack("gone");
    forge.read("kept");
    forge.wait_for_ack("kept");
}

impl Forge {
    fn delete(&mut self, id: &str) {
        let sender = self
            .core
            .host(&self.host)
            .expect("host")
            .test_transition_sender();
        runtime().block_on(sender.event(Event::ThreadRemoved {
            thread_id: ThreadId(id.into()),
        }));
        let host = self.host.clone();
        wait_until("the deletion applies", Duration::from_secs(10), || {
            self.core.drain_with(|_, _| {});
            !self.has_thread(&host, id)
        });
    }

    fn has_thread(&self, host: &HostId, id: &str) -> bool {
        self.core
            .host(host)
            .expect("host")
            .threads
            .contains_key(&ThreadId(id.into()))
    }
}

#[test]
fn deleting_a_thread_cancels_its_waiting_read_acknowledgement() {
    let mut forge = forge();
    forge.read("held");
    forge.wait_for_ack("held");
    forge.delete("held");
    forge.read("kept");
    forge.wait_for_ack("kept");
}

#[test]
fn a_read_acknowledgement_answered_after_a_delete_keeps_the_thread_deleted() {
    let mut forge = forge();
    forge.read("held");
    forge.wait_for_ack("held");
    forge.delete("held");
    forge.stub.release.notify_one();
    let host = forge.host.clone();
    wait_until(
        "the acknowledgement settles",
        Duration::from_secs(10),
        || {
            forge.core.drain_with(|_, _| {});
            !forge
                .core
                .host(&host)
                .expect("host")
                .test_read_ack_in_flight()
        },
    );
    assert!(
        !forge.has_thread(&host, "held"),
        "a late acknowledgement does not bring the deleted thread back"
    );
}
