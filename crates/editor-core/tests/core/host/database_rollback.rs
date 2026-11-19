use artisan_api::{DatabaseRollback, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use editor_core::runtime::runtime;
use editor_core::{Core, HostConfig, HostId, HostKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_support::editor::forge::TlsServer;
use test_support::wait::wait_until;

const SET_ASIDE: &str = "/data/forge.sqlite3.rolled-back-from-32-0";
const SETTLED_WITHIN: Duration = Duration::from_secs(10);

fn state() -> State {
    State {
        head: 0,
        forge_version: "test".into(),
        protocol: artisan_api::PROTOCOL_VERSION,
        projects: Vec::new(),
        threads: Vec::new(),
        threads_next: None,
        runs: Vec::new(),
        settings: serde_json::Map::new(),
        engines: Vec::new(),
        waiting: Vec::new(),
        account: None,
        database_rollback: Some(DatabaseRollback {
            found: 32,
            restored: 31,
            set_aside: SET_ASIDE.into(),
            at: 0,
        }),
    }
}

struct Forge {
    _directory: tempfile::TempDir,
    _server: TlsServer,
    dismissals: Arc<AtomicUsize>,
    core: Core,
    host: HostId,
}

fn forge(dismissal: StatusCode) -> Forge {
    let dismissals = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&dismissals);
    let router = axum::Router::new()
        .route("/state", get(|| async { axum::Json(state()) }))
        .route(
            "/database-rollback",
            delete(move || {
                let counted = Arc::clone(&counted);
                async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    let failed: Response = (
                        dismissal,
                        axum::Json(
                            serde_json::json!({"code": "internal", "message": "disk failed"}),
                        ),
                    )
                        .into_response();
                    if dismissal.is_success() {
                        StatusCode::NO_CONTENT.into_response()
                    } else {
                        failed
                    }
                }
            }),
        );
    let server = runtime().block_on(TlsServer::serve(router));
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = test_support::editor::core::test_core(directory.path());
    let config = HostConfig {
        id: HostId("stub-forge".into()),
        name: "Stub".into(),
        address: server.address().to_owned(),
        fingerprint: server.fingerprint().to_owned(),
        kind: HostKind::Remote,
    };
    test_support::editor::core::connect(&mut core, &config);
    Forge {
        _directory: directory,
        _server: server,
        dismissals,
        core,
        host: config.id,
    }
}

impl Forge {
    fn notice(&mut self) -> Option<String> {
        self.core.drain_with(|_, _| {});
        self.core
            .host(&self.host)
            .expect("host")
            .rollback_notice()
            .message()
    }

    fn shown(&mut self) -> String {
        let mut shown = None;
        wait_until("the rollback notice shows", SETTLED_WITHIN, || {
            shown = self.notice();
            shown.is_some()
        });
        shown.expect("the notice showed")
    }

    fn dismiss(&mut self) {
        self.core
            .host_mut(&self.host)
            .expect("host")
            .dismiss_rollback_notice();
    }
}

#[test]
fn the_sidebar_says_forge_went_back_and_where_the_newer_data_is_until_dismissed() {
    let mut forge = forge(StatusCode::NO_CONTENT);
    let shown = forge.shown();
    assert!(shown.contains(SET_ASIDE), "{shown}");
    assert!(shown.contains("schema 31 instead of 32"), "{shown}");

    forge.dismiss();
    assert_eq!(
        forge.notice(),
        None,
        "the notice hides as soon as it is dismissed"
    );
    let dismissals = Arc::clone(&forge.dismissals);
    wait_until("Forge hears the dismissal", SETTLED_WITHIN, || {
        dismissals.load(Ordering::SeqCst) == 1
    });
    assert_eq!(forge.notice(), None);
}

#[test]
fn a_dismissal_forge_refuses_brings_the_notice_back_with_the_reason() {
    let mut forge = forge(StatusCode::INTERNAL_SERVER_ERROR);
    forge.shown();

    forge.dismiss();
    let mut returned = None;
    wait_until("the notice returns", SETTLED_WITHIN, || {
        returned = forge.notice();
        returned.is_some()
    });
    let returned = returned.expect("the notice returned");
    assert!(returned.contains(SET_ASIDE), "{returned}");
    assert!(returned.contains("Couldn't dismiss this"), "{returned}");
}
