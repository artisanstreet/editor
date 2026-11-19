use super::*;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use base64::Engine as _;
use std::sync::{Arc, Mutex};

pub(super) const MACHINE: &str = "mch_11111111111111111111111111";
pub(super) const SECRET: [u8; 32] = [0x21_u8; 32];

pub(super) fn test_client() -> reqwest::Client {
    reqwest::Client::new()
}

fn machine_proof_ok(headers: &HeaderMap) {
    for name in [
        "x-ast-machine",
        "x-ast-timestamp",
        "x-ast-nonce",
        "authorization",
    ] {
        assert!(headers.contains_key(name), "missing proof header {name}");
    }
    assert!(
        headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("AST-Machine ")
    );
}

fn manifest_bytes() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "format": "ast-editor-config/1",
        "mcp_servers": [{"name": "s", "command": "srv"}],
        "skills": [],
        "global_prompt": "synced prompt",
    }))
    .unwrap()
}

fn bundle_hash() -> String {
    hex::encode(sha2::Sha256::digest(manifest_bytes()))
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: probe skill\n---\n\n# Probe\n")
}

fn skill_file(path: &str, contents: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "path": path,
        "content_base64": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(contents),
        "sha256": hex::encode(sha2::Sha256::digest(contents)),
    })
}

#[derive(Clone, Default)]
pub(super) struct Stub {
    reports: Arc<Mutex<Vec<serde_json::Value>>>,
    refused_reports: Arc<std::sync::atomic::AtomicUsize>,
    tamper_manifest: bool,
    manifest_override: Option<Vec<u8>>,
}

impl Stub {
    pub(super) fn report_statuses(&self) -> Vec<serde_json::Value> {
        self.reports
            .lock()
            .unwrap()
            .iter()
            .map(|report| report["status"].clone())
            .collect()
    }

    pub(super) fn refuse_reports(&self, count: usize) {
        self.refused_reports
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }
}

fn stub_manifest(stub: &Stub) -> Vec<u8> {
    stub.manifest_override
        .clone()
        .unwrap_or_else(manifest_bytes)
}

async fn latest(State(stub): State<Stub>, headers: HeaderMap) -> impl axum::response::IntoResponse {
    machine_proof_ok(&headers);
    let manifest = stub_manifest(&stub);
    let hash = hex::encode(sha2::Sha256::digest(&manifest));
    if headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|tag| tag.trim() == format!("\"{hash}\""))
    {
        return (StatusCode::NOT_MODIFIED, Vec::new());
    }
    let body = serde_json::to_vec(&serde_json::json!({
        "bundle": {
            "id": "bundle_000000000000000000000001",
            "version": 3,
            "content_hash": hash,
            "r2_key": "cfg/acct/bundle-3.json",
            "created_at": "2026-10-26T20:00:00.000Z",
        },
        "manifest": serde_json::from_slice::<serde_json::Value>(&manifest_bytes()).unwrap(),
    }))
    .unwrap();
    (StatusCode::OK, body)
}

async fn serve_manifest(State(stub): State<Stub>, headers: HeaderMap) -> Vec<u8> {
    machine_proof_ok(&headers);
    if stub.tamper_manifest {
        return b"{}".to_vec();
    }
    stub_manifest(&stub)
}

async fn report(
    State(stub): State<Stub>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl axum::response::IntoResponse {
    machine_proof_ok(&headers);
    if stub
        .refused_reports
        .fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |refused| refused.checked_sub(1),
        )
        .is_ok()
    {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(request.get("bundleId").is_some());
    stub.reports.lock().unwrap().push(request);
    Ok(axum::Json(serde_json::json!({
        "machine_id": MACHINE,
        "bundle_id": "bundle_000000000000000000000001",
        "status": "applied",
        "applied_at": "2026-10-26T20:01:00.000Z",
    })))
}

pub(super) fn test_hub(dir: &Path) -> forge_runs::Hub {
    let store = forge_store::Store::open(dir).unwrap();
    let engines = forge_engines::Engines::new(dir);
    forge_runs::Hub::new(store, engines, dir.to_owned())
}

pub(super) async fn stub_server(
    tamper: bool,
    manifest: Option<Vec<u8>>,
) -> (String, Stub, test_support::http::StubServer) {
    use axum::routing::{get, post};
    let stub = Stub {
        reports: Arc::new(Mutex::new(Vec::new())),
        refused_reports: Arc::default(),
        tamper_manifest: tamper,
        manifest_override: manifest,
    };
    let router = axum::Router::new()
        .route(CONFIG_BUNDLES_LATEST_PATH, get(latest))
        .route("/v1/config/bundles/3/manifest", get(serve_manifest))
        .route(CONFIG_STATE_PATH, post(report))
        .with_state(stub.clone());
    let server = test_support::http::StubServer::serve(router).await;
    (server.url().to_owned(), stub, server)
}

#[tokio::test]
async fn config_sync_applies_through_hub_then_revalidates() {
    let (base, stub, _server) = stub_server(false, None).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let outcome = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path())
        .await
        .unwrap();
    let ConfigOutcome::Updated { state, .. } = outcome else {
        panic!("first sync must apply");
    };
    assert_eq!(state.version, 3);
    assert_eq!(state.content_hash, bundle_hash());
    assert_eq!(stub.reports.lock().unwrap().len(), 1);
    let prompt: Option<serde_json::Value> = hub.store().setting("global_prompt").await.unwrap();
    assert_eq!(
        prompt
            .and_then(|v| v.as_str().map(str::to_owned))
            .as_deref(),
        Some("synced prompt")
    );
    let mcp: Vec<artisan_api::McpServer> = serde_json::from_value(
        hub.store()
            .setting("mcp_servers")
            .await
            .unwrap()
            .unwrap_or_default(),
    )
    .unwrap();
    assert!(
        mcp.iter()
            .any(|server| server.name == "s" && server.command == "srv")
    );

    let outcome = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path())
        .await
        .unwrap();
    assert!(
        matches!(outcome, ConfigOutcome::Current(_)),
        "ETag 304 must not reapply"
    );
    assert_eq!(
        stub.reports.lock().unwrap().len(),
        1,
        "no second report on 304"
    );
}

#[tokio::test]
async fn config_sync_rejects_tampered_manifest_bytes() {
    let (base, stub, _server) = stub_server(true, None).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let result = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path()).await;
    result.unwrap_err();
    assert!(!sync_state_path(dir.path()).exists());
    assert_eq!(stub.reports.lock().unwrap().len(), 1);
    assert_eq!(
        stub.reports.lock().unwrap()[0]["status"],
        serde_json::json!("failed")
    );
}

fn skill_manifest(skill: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "format": "ast-editor-config/1",
        "mcp_servers": [{"name": "s", "command": "srv"}],
        "skills": [skill],
        "global_prompt": "synced prompt",
    }))
    .unwrap()
}

fn bundled_skill(name: &str, extra: Vec<(&str, &[u8])>) -> serde_json::Value {
    let mut files = vec![skill_file("SKILL.md", skill_md(name).as_bytes())];
    for (path, contents) in extra {
        files.push(skill_file(path, contents));
    }
    serde_json::json!({
        "name": name,
        "source": "builtin",
        "bundle": {"files": files},
    })
}

#[tokio::test]
async fn bundled_skill_installs_and_reads_back_enabled() {
    let manifest = skill_manifest(&bundled_skill(
        "syncprobed",
        vec![("refs/notes.md", b"notes")],
    ));
    let (base, stub, _server) = stub_server(false, Some(manifest)).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let outcome = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path())
        .await
        .unwrap();
    assert!(matches!(outcome, ConfigOutcome::Updated { .. }));
    let catalog = hub.extensions().skill_catalog().await.unwrap();
    let installed = catalog
        .iter()
        .find(|entry| entry.id == "local.syncprobed")
        .expect("installed skill in catalog");
    assert!(installed.enabled, "installed skill is enabled");
    let prompt: Option<String> = hub
        .store()
        .setting("global_prompt")
        .await
        .unwrap()
        .and_then(|value| value.as_str().map(str::to_owned));
    assert_eq!(prompt.as_deref(), Some("synced prompt"));
    assert_eq!(stub.reports.lock().unwrap().len(), 1);
    assert_eq!(
        stub.reports.lock().unwrap()[0]["status"],
        serde_json::json!("applied")
    );
}

#[tokio::test]
async fn a_skill_that_cannot_install_changes_nothing_else() {
    let bad_skill = serde_json::json!({
        "name": "brokenskill",
        "source": "builtin",
        "bundle": {"files": [skill_file("refs/notes.md", b"notes")]},
    });
    let manifest = skill_manifest(&bad_skill);
    let (base, stub, _server) = stub_server(false, Some(manifest)).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let result = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path()).await;
    assert!(result.is_err(), "missing SKILL.md must fail the apply");
    let prompt: Option<serde_json::Value> = hub.store().setting("global_prompt").await.unwrap();
    assert_eq!(prompt, None);
    let catalog = hub.extensions().skill_catalog().await.unwrap();
    assert!(
        catalog.iter().all(|entry| entry.id != "local.brokenskill"),
        "no partial install may remain"
    );
    assert!(!sync_state_path(dir.path()).exists());
    let reports = stub.reports.lock().unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["status"], serde_json::json!("failed"));
    assert!(
        reports[0]["detail"].as_str().unwrap().contains("SKILL.md"),
        "detail names the cause: {}",
        reports[0]["detail"]
    );
    drop(reports);
}

#[tokio::test]
async fn unknown_skill_without_bundle_fails_named() {
    let manifest = skill_manifest(&serde_json::json!({
        "name": "not-installed-anywhere",
        "source": "builtin",
    }));
    let (base, stub, _server) = stub_server(false, Some(manifest)).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let result = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path()).await;
    result.unwrap_err();
    let reports = stub.reports.lock().unwrap();
    assert_eq!(reports[0]["status"], serde_json::json!("failed"));
    assert!(
        reports[0]["detail"]
            .as_str()
            .unwrap()
            .contains("not-installed-anywhere"),
        "detail names the skill: {}",
        reports[0]["detail"]
    );
    drop(reports);
}

pub(super) async fn mcp_servers(store: &forge_store::Store) -> Vec<artisan_api::McpServer> {
    store
        .setting("mcp_servers")
        .await
        .unwrap()
        .map(|servers| serde_json::from_value(servers).unwrap())
        .unwrap_or_default()
}

async fn add_local(store: &forge_store::Store, server: artisan_api::McpServer) {
    let previous = mcp_servers(store)
        .await
        .into_iter()
        .find(|existing| existing.name == server.name);
    store
        .mutate_mcp(artisan_api::McpMutation::Upsert { previous, server })
        .await
        .unwrap();
}

fn mcp_manifest(server: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "format": "ast-editor-config/1",
        "mcp_servers": [server],
    }))
    .unwrap()
}

async fn sync_with(manifest: Vec<u8>, hub: &forge_runs::Hub, dir: &Path) -> ConfigOutcome {
    let (base, _stub, _server) = stub_server(false, Some(manifest)).await;
    sync_config(&test_client(), &base, MACHINE, &SECRET, hub, dir)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_server_created_on_this_machine_survives_an_account_server_of_the_same_name() {
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let store = hub.store();
    let local = artisan_api::McpServer {
        name: "s".into(),
        command: "local-search".into(),
        headers: [("Authorization".to_owned(), "Bearer local".to_owned())].into(),
        ..Default::default()
    };
    add_local(store, local.clone()).await;
    add_local(
        store,
        artisan_api::McpServer {
            name: "mine".into(),
            command: "local".into(),
            ..Default::default()
        },
    )
    .await;
    let manifest = mcp_manifest(&serde_json::json!({"name": "s", "command": "srv"}));
    let ConfigOutcome::Updated { state, kept_local } =
        sync_with(manifest.clone(), &hub, dir.path()).await
    else {
        panic!("a new bundle applies");
    };
    assert_eq!(kept_local, [LocalMcpServerKept { name: "s".into() }]);
    assert!(state.mcp_servers.is_empty(), "{:?}", state.mcp_servers);
    let servers = mcp_servers(store).await;
    assert!(servers.contains(&local), "{servers:?}");
    assert!(servers.iter().any(|server| server.name == "mine"));

    let changed = mcp_manifest(&serde_json::json!({"name": "s", "command": "srv2"}));
    let ConfigOutcome::Updated { kept_local, .. } = sync_with(changed, &hub, dir.path()).await
    else {
        panic!("a changed bundle applies");
    };
    assert_eq!(kept_local, [LocalMcpServerKept { name: "s".into() }]);
    assert!(mcp_servers(store).await.contains(&local));
}

#[tokio::test]
async fn synced_servers_keep_credentials_only_while_their_destination_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let store = hub.store();
    let mcp_host = || {
        test_support::http::StubServer::serve(
            axum::Router::new().fallback(|| async { StatusCode::OK }),
        )
    };
    let (first_host, other_host) = (mcp_host().await, mcp_host().await);
    let first = |path: &str| format!("{}{path}", first_host.url());
    let other = format!("{}/v2", other_host.url());
    let bundle = |url: &str| mcp_manifest(&serde_json::json!({"name": "s", "url": url}));
    sync_with(bundle(&first("/v1")), &hub, dir.path()).await;
    let mut placed = mcp_servers(store).await.pop().unwrap();
    placed
        .headers
        .insert("Authorization".into(), "Bearer local".into());
    add_local(store, placed).await;

    sync_with(bundle(&first("/v2")), &hub, dir.path()).await;
    let same_origin = mcp_servers(store).await.pop().unwrap();
    assert_eq!(same_origin.url, Some(first("/v2")));
    assert_eq!(
        same_origin.headers.get("Authorization").map(String::as_str),
        Some("Bearer local")
    );

    sync_with(bundle(&other), &hub, dir.path()).await;
    let moved = mcp_servers(store).await.pop().unwrap();
    assert_eq!(moved.url, Some(other));
    assert!(
        moved.headers.is_empty(),
        "a header for the first host went to another provider: {:?}",
        moved.headers
    );

    let launched = |command: &str, args: &[&str], others: &[&str]| {
        let servers: Vec<_> =
            std::iter::once(serde_json::json!({"name": "s", "command": command, "args": args}))
                .chain(
                    others
                        .iter()
                        .map(|name| serde_json::json!({"name": name, "command": "other"})),
                )
                .collect();
        serde_json::to_vec(&serde_json::json!({
            "format": "ast-editor-config/1",
            "mcp_servers": servers,
        }))
        .unwrap()
    };
    let stdio = async || {
        mcp_servers(store)
            .await
            .into_iter()
            .find(|server| server.name == "s")
            .unwrap()
    };
    let with_key = async || {
        let mut placed = stdio().await;
        placed.env.insert("API_KEY".into(), "secret".into());
        add_local(store, placed).await;
    };
    sync_with(launched("npx", &["search-server"], &[]), &hub, dir.path()).await;
    with_key().await;
    sync_with(
        launched("npx", &["search-server"], &["t"]),
        &hub,
        dir.path(),
    )
    .await;
    assert_eq!(
        stdio().await.env.get("API_KEY").map(String::as_str),
        Some("secret"),
        "an unchanged server lost its secrets when another server changed"
    );

    sync_with(
        launched("npx", &["other-package"], &["t"]),
        &hub,
        dir.path(),
    )
    .await;
    let repackaged = stdio().await;
    assert_eq!(repackaged.args, ["other-package"]);
    assert!(
        repackaged.env.is_empty(),
        "a different package run by the same command received the old secrets: {:?}",
        repackaged.env
    );

    with_key().await;
    sync_with(
        launched("other-server", &["other-package"], &[]),
        &hub,
        dir.path(),
    )
    .await;
    let replaced = stdio().await;
    assert_eq!(replaced.command, "other-server");
    assert!(
        replaced.env.is_empty(),
        "a replacement executable received the old secrets: {:?}",
        replaced.env
    );
}

#[test]
fn a_logged_machine_identity_never_shows_its_key() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let key = [0x5a_u8; 32];
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
    std::fs::write(
        dir.path().join("ast_account.json"),
        serde_json::json!({ "secret_b64u": encoded, "machine_id": "machine-1" }).to_string(),
    )
    .unwrap();
    let identity = load_machine_identity(dir.path()).unwrap().unwrap();
    let logged = format!("{identity:?}");
    assert!(logged.contains("machine-1"), "{logged}");
    assert!(!logged.contains("90, 90"), "{logged}");
    assert!(!logged.contains(&encoded), "{logged}");
}
