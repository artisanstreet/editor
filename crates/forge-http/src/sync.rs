//! Account config sync, Forge ← Core: fetch the latest bundle, check the
//! manifest bytes against `bundle.content_hash`, apply them through the Hub
//! and report the result to Core.

use anyhow::{Context, Result};
use sha2::Digest as _;
use std::path::{Component, Path, PathBuf};

use crate::core::now_sec;

pub const CONFIG_BUNDLES_LATEST_PATH: &str = "/v1/config/bundles/latest";
pub const CONFIG_STATE_PATH: &str = "/v1/config/state";
const MANIFEST_FORMAT: &str = "ast-editor-config/1";

fn config_manifest_path(version: u64) -> String {
    format!("/v1/config/bundles/{version}/manifest")
}

/// The last bundle this Forge applied.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConfigState {
    pub bundle_id: String,
    pub version: u64,
    pub content_hash: String,
    pub etag: String,
    /// The MCP servers that bundle placed. Sync replaces or removes only
    /// these; servers added on this machine are never touched.
    #[serde(default)]
    pub mcp_servers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigOutcome {
    Current(ConfigState),
    Updated(ConfigState),
}

pub fn sync_state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ast_sync.json")
}

fn read_state(data_dir: &Path) -> Result<Option<ConfigState>> {
    let path = sync_state_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).with_context(|| format!("read {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn write_state(data_dir: &Path, state: &ConfigState) -> Result<()> {
    let path = sync_state_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))
}

#[derive(serde::Deserialize)]
struct BundleInfo {
    id: String,
    version: u64,
    content_hash: String,
}

#[derive(serde::Deserialize)]
struct BundleLatest {
    #[serde(default)]
    bundle: Option<BundleInfo>,
}

/// Strict, so a Core shape change fails loudly instead of half-applying.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EditorManifest {
    format: String,
    #[serde(default)]
    mcp_servers: Vec<ManifestMcpServer>,
    #[serde(default)]
    skills: Vec<ManifestSkill>,
    #[serde(default)]
    global_prompt: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestMcpServer {
    name: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(serde::Deserialize)]
struct ManifestSkill {
    name: String,
    /// Installed on machines that lack the skill; absent means the skill
    /// must already be in the catalog.
    #[serde(default)]
    bundle: Option<SkillBundle>,
}

#[derive(serde::Deserialize)]
struct SkillBundle {
    files: Vec<SkillBundleFile>,
}

/// Per-file hashes are not checked: the whole manifest is already checked
/// against `bundle.content_hash`.
#[derive(serde::Deserialize)]
struct SkillBundleFile {
    path: String,
    content_base64: String,
}

/// Machine-proof requests to Core.
struct Core<'a> {
    http: &'a reqwest::Client,
    base: &'a str,
    machine_id: &'a str,
    secret: &'a [u8; 32],
}

impl Core<'_> {
    fn get(&self, path: &str) -> Result<reqwest::RequestBuilder> {
        let headers = crate::core::CoreClient::proof_headers(
            self.machine_id,
            self.secret,
            "GET",
            path,
            b"",
            now_sec(),
        )?;
        Ok(self
            .http
            .get(format!("{}{path}", self.base))
            .headers(headers))
    }

    async fn report(&self, bundle_id: &str, status: &str, detail: &str) -> Result<()> {
        let body = serde_json::to_vec(&serde_json::json!({
            "bundleId": bundle_id,
            "status": status,
            "detail": detail,
        }))?;
        let headers = crate::core::CoreClient::proof_headers(
            self.machine_id,
            self.secret,
            "POST",
            CONFIG_STATE_PATH,
            &body,
            now_sec(),
        )?;
        self.http
            .post(format!("{}{CONFIG_STATE_PATH}", self.base))
            .headers(headers)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .context("reach Core")?
            .error_for_status()
            .context("report the config bundle")?;
        Ok(())
    }
}

pub async fn sync_config(
    http: &reqwest::Client,
    core_base_url: &str,
    machine_id: &str,
    secret: &[u8; 32],
    hub: &forge_runs::Hub,
    data_dir: &Path,
) -> Result<ConfigOutcome> {
    let core = Core {
        http,
        base: core_base_url,
        machine_id,
        secret,
    };
    let previous = read_state(data_dir)?;
    let mut request = core.get(CONFIG_BUNDLES_LATEST_PATH)?;
    if let Some(etag) = previous
        .as_ref()
        .map(|state| state.etag.as_str())
        .filter(|etag| !etag.is_empty())
    {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request.send().await.context("reach Core")?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return previous
            .map(ConfigOutcome::Current)
            .context("Core answered 304 but this Forge has applied no bundle");
    }
    let response = response
        .error_for_status()
        .context("fetch the latest config bundle")?;
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let latest: BundleLatest = response
        .json()
        .await
        .context("read the latest config bundle")?;
    let Some(bundle) = latest.bundle else {
        return previous
            .map(ConfigOutcome::Current)
            .context("Core has no config bundle yet");
    };
    if let Some(current) = previous
        .as_ref()
        .filter(|state| state.bundle_id == bundle.id && state.content_hash == bundle.content_hash)
    {
        return Ok(ConfigOutcome::Current(current.clone()));
    }
    let bytes = core
        .get(&config_manifest_path(bundle.version))?
        .send()
        .await
        .context("reach Core")?
        .error_for_status()
        .context("fetch the manifest")?
        .bytes()
        .await
        .context("read the manifest")?;
    let synced_before = previous.map(|state| state.mcp_servers).unwrap_or_default();
    let applied = async {
        anyhow::ensure!(
            hex::encode(sha2::Sha256::digest(&bytes)) == bundle.content_hash,
            "the manifest does not match the bundle hash"
        );
        let manifest: EditorManifest =
            serde_json::from_slice(&bytes).context("read the manifest")?;
        anyhow::ensure!(
            manifest.format == MANIFEST_FORMAT,
            "unknown manifest format {}",
            manifest.format
        );
        let changed = apply(hub, &manifest, &synced_before).await?;
        let synced = manifest
            .mcp_servers
            .into_iter()
            .map(|server| server.name)
            .collect();
        anyhow::Ok((changed, synced))
    }
    .await;
    let (changed, mcp_servers) = match applied {
        Ok(applied) => applied,
        Err(error) => {
            if let Err(report) = core
                .report(&bundle.id, "failed", &format!("{error:#}"))
                .await
            {
                tracing::debug!("{report:#}");
            }
            return Err(error).context("apply the config bundle");
        }
    };
    let state = ConfigState {
        bundle_id: bundle.id,
        version: bundle.version,
        content_hash: bundle.content_hash,
        etag,
        mcp_servers,
    };
    write_state(data_dir, &state)?;
    if let Err(error) = core
        .report(&state.bundle_id, "applied", &changed.join(","))
        .await
    {
        tracing::warn!(
            "applied config bundle {} but could not tell Core: {error:#}",
            state.version
        );
    }
    Ok(ConfigOutcome::Updated(state))
}

/// Skills go first, so a skill that cannot install leaves the prompt and
/// the MCP servers as they were.
async fn apply(
    hub: &forge_runs::Hub,
    manifest: &EditorManifest,
    synced_before: &[String],
) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for skill in &manifest.skills {
        if let Some(bundle) = &skill.bundle {
            install_bundle(hub, &skill.name, bundle).await?;
            changed.push(format!("skill:{}", skill.name));
        }
    }
    if !manifest.skills.is_empty() {
        let catalog = hub.skill_catalog().await?;
        for skill in &manifest.skills {
            let suffix = format!(".{}", skill.name);
            let entry = catalog
                .iter()
                .find(|entry| {
                    entry.id == skill.name
                        || entry.name == skill.name
                        || entry.id.ends_with(&suffix)
                })
                .with_context(|| {
                    format!(
                        "skill {} is not installed and the bundle carries none",
                        skill.name
                    )
                })?;
            if !entry.enabled {
                hub.set_skill_enabled(&entry.id, true).await?;
                changed.push(format!("skill:{}", entry.id));
            }
        }
        hub.reconcile_skills().await.context("reconcile skills")?;
    }
    if let Some(text) = manifest.global_prompt.as_deref() {
        let store = hub.store();
        let current = store.setting("global_prompt").await?;
        if current.as_ref().and_then(serde_json::Value::as_str) != Some(text) {
            let base = store.prompt_version().await?;
            store
                .save_prompt_version(base, text.to_owned())
                .await?
                .map_err(|latest| {
                    anyhow::anyhow!(
                        "the global prompt changed while syncing (now version {latest})"
                    )
                })?;
            hub.reconcile_global_prompt()
                .await
                .context("reconcile the global prompt")?;
            changed.push("globalPrompt".to_owned());
        }
    }
    changed.extend(apply_mcp(hub.store(), &manifest.mcp_servers, synced_before).await?);
    hub.check_mcp_auth().await;
    Ok(changed)
}

async fn install_bundle(hub: &forge_runs::Hub, name: &str, bundle: &SkillBundle) -> Result<()> {
    use base64::Engine as _;
    let dir = tempfile::tempdir().context("stage a skill bundle")?;
    for file in &bundle.files {
        let relative = Path::new(&file.path);
        anyhow::ensure!(
            relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
            "skill {name} bundles an unsafe path {}",
            file.path
        );
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&file.content_base64)
            .with_context(|| format!("decode {} in skill {name}", file.path))?;
        let path = dir.path().join(relative);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, bytes).await?;
    }
    hub.install_skill(dir.path().to_path_buf(), true)
        .await
        .with_context(|| format!("install skill {name}"))?;
    Ok(())
}

/// Upserts the manifest's servers, keeping each one's local fields (env,
/// headers), and removes only servers an earlier bundle placed.
async fn apply_mcp(
    store: &forge_store::Store,
    servers: &[ManifestMcpServer],
    synced_before: &[String],
) -> Result<Vec<String>> {
    let current: Vec<artisan_api::McpServer> = store
        .setting("mcp_servers")
        .await?
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let mut changed = Vec::new();
    for entry in servers {
        let previous = current.iter().find(|server| server.name == entry.name);
        let server = artisan_api::McpServer {
            name: entry.name.clone(),
            command: entry.command.clone().unwrap_or_default(),
            args: entry.args.clone().unwrap_or_default(),
            url: entry.url.clone(),
            ..previous.cloned().unwrap_or_default()
        };
        if previous != Some(&server) {
            store
                .mutate_mcp(artisan_api::McpMutation::Upsert {
                    previous: previous.cloned(),
                    server,
                })
                .await?;
            changed.push(format!("mcp:{}", entry.name));
        }
    }
    let dropped = current.iter().filter(|server| {
        synced_before.contains(&server.name)
            && !servers.iter().any(|entry| entry.name == server.name)
    });
    for server in dropped {
        store
            .mutate_mcp(artisan_api::McpMutation::Remove {
                previous: server.clone(),
            })
            .await?;
        changed.push(format!("mcp-removed:{}", server.name));
    }
    Ok(changed)
}

/// The machine identity `ae account enroll/confirm` wrote to
/// `<data_dir>/ast_account.json`.
#[derive(Clone, Debug)]
pub struct MachineIdentity {
    pub machine_id: String,
    pub account_id: Option<String>,
    pub secret: [u8; 32],
}

/// `None` when this Forge is not enrolled.
pub fn load_machine_identity(data_dir: &Path) -> Result<Option<MachineIdentity>> {
    use base64::Engine as _;
    #[derive(serde::Deserialize)]
    struct AccountFile {
        #[serde(default)]
        secret_b64u: Option<String>,
        #[serde(default)]
        machine_id: Option<String>,
        #[serde(default)]
        account_id: Option<String>,
    }
    let path = data_dir.join("ast_account.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let file: AccountFile =
        serde_json::from_slice(&bytes).with_context(|| format!("read {}", path.display()))?;
    let (Some(secret_b64u), Some(machine_id)) = (file.secret_b64u, file.machine_id) else {
        return Ok(None);
    };
    let secret: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(secret_b64u.as_bytes())
        .context("read the machine secret")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("the machine secret is not 32 bytes"))?;
    Ok(Some(MachineIdentity {
        machine_id,
        account_id: file.account_id,
        secret,
    }))
}

/// Syncs at startup and then every `interval`. Core has no push channel for
/// config changes, so this is the one place sync polls.
pub(crate) async fn run_config_loop(
    data_dir: PathBuf,
    hub: forge_runs::Hub,
    identity: MachineIdentity,
    shutdown: tokio_util::sync::CancellationToken,
    interval: std::time::Duration,
) {
    let base_url = crate::core::core_base_url(None);
    let http = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(http) => http,
        Err(error) => {
            tracing::warn!("config sync off: {error:#}");
            return;
        }
    };
    let mut failing = false;
    loop {
        let synced = sync_config(
            &http,
            &base_url,
            &identity.machine_id,
            &identity.secret,
            &hub,
            &data_dir,
        )
        .await;
        match synced {
            Ok(outcome) => {
                failing = false;
                if let ConfigOutcome::Updated(state) = outcome {
                    tracing::info!(version = state.version, "applied config bundle");
                }
            }
            Err(error) if failing => tracing::debug!("config sync failed: {error:#}"),
            Err(error) => {
                failing = true;
                tracing::warn!("config sync failed: {error:#}");
            }
        }
        tokio::select! {
            () = shutdown.cancelled() => break,
            () = tokio::time::sleep(interval) => {},
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test fixtures; failures fail the test")]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use base64::Engine as _;
    use std::sync::{Arc, Mutex};

    const MACHINE: &str = "mch_11111111111111111111111111";
    const SECRET: [u8; 32] = [0x21_u8; 32];

    fn test_client() -> reqwest::Client {
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
    struct Stub {
        reports: Arc<Mutex<Vec<serde_json::Value>>>,
        tamper_manifest: bool,
        manifest_override: Option<Vec<u8>>,
    }

    fn stub_manifest(stub: &Stub) -> Vec<u8> {
        stub.manifest_override
            .clone()
            .unwrap_or_else(manifest_bytes)
    }

    async fn latest(
        State(stub): State<Stub>,
        headers: HeaderMap,
    ) -> impl axum::response::IntoResponse {
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
        let _ = stub;
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
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(request.get("bundleId").is_some());
        stub.reports.lock().unwrap().push(request);
        axum::Json(serde_json::json!({
            "machine_id": MACHINE,
            "bundle_id": "bundle_000000000000000000000001",
            "status": "applied",
            "applied_at": "2026-10-26T20:01:00.000Z",
        }))
    }

    fn test_hub(dir: &std::path::Path) -> forge_runs::Hub {
        let store = forge_store::Store::open(dir).unwrap();
        let engines = forge_engines::Engines::new(dir);
        forge_runs::Hub::new(store, engines, dir.to_owned())
    }

    async fn stub_server(
        tamper: bool,
        manifest: Option<Vec<u8>>,
    ) -> (String, Stub, tokio::task::JoinHandle<()>) {
        use axum::routing::{get, post};
        let stub = Stub {
            reports: Arc::new(Mutex::new(Vec::new())),
            tamper_manifest: tamper,
            manifest_override: manifest,
        };
        let router = axum::Router::new()
            .route(CONFIG_BUNDLES_LATEST_PATH, get(latest))
            .route("/v1/config/bundles/3/manifest", get(serve_manifest))
            .route(CONFIG_STATE_PATH, post(report))
            .with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{address}"), stub, task)
    }

    /// Full config journey against Core-shaped routes: 200 applies through
    /// the Hub (prompt text + MCP list observable in the store), reports
    /// `applied`, then ETag 304 means current with no reapply.
    #[tokio::test]
    async fn config_sync_applies_through_hub_then_revalidates() {
        let (base, stub, _server) = stub_server(false, None).await;
        let dir = tempfile::tempdir().unwrap();
        let hub = test_hub(dir.path());
        let outcome = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path())
            .await
            .unwrap();
        let ConfigOutcome::Updated(state) = outcome else {
            panic!("first sync must apply");
        };
        assert_eq!(state.version, 3);
        assert_eq!(state.content_hash, bundle_hash());
        assert_eq!(stub.reports.lock().unwrap().len(), 1);
        // Observable engine state, not a cached file.
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

    /// Manifest bytes that do not hash to `bundle.content_hash` are rejected
    /// before anything applies; last-good stays absent and `failed` reports.
    #[tokio::test]
    async fn config_sync_rejects_tampered_manifest_bytes() {
        let (base, stub, _server) = stub_server(true, None).await;
        let dir = tempfile::tempdir().unwrap();
        let hub = test_hub(dir.path());
        let result = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path()).await;
        assert!(result.is_err());
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

    /// A bundled skill installs through the real installer and reads back
    /// enabled in the catalog; prompt + MCP converge in the same apply.
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
        assert!(matches!(outcome, ConfigOutcome::Updated(_)));
        let catalog = hub.skill_catalog().await.unwrap();
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

    /// A bundle missing SKILL.md fails the whole apply: prompt and MCP are
    /// unchanged, nothing is installed, last-good stays absent, and the
    /// failure is reported with detail.
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
        // Nothing applied: prompt untouched, no skill installed, no state.
        let prompt: Option<serde_json::Value> = hub.store().setting("global_prompt").await.unwrap();
        assert_eq!(prompt, None);
        let catalog = hub.skill_catalog().await.unwrap();
        assert!(
            catalog.iter().all(|entry| entry.id != "local.brokenskill"),
            "no partial install may remain"
        );
        assert!(!super::sync_state_path(dir.path()).exists());
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

    /// An unknown skill with no bundle fails with a named error instead of
    /// a silent no-op.
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
        assert!(result.is_err());
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

    /// Sync owns only the MCP servers it placed: a server added on this
    /// machine survives, and a synced server keeps its local headers.
    #[tokio::test]
    async fn sync_keeps_local_mcp_servers_and_their_headers() {
        let (base, _stub, _server) = stub_server(false, None).await;
        let dir = tempfile::tempdir().unwrap();
        let hub = test_hub(dir.path());
        let store = hub.store();
        let local = [
            artisan_api::McpServer {
                name: "mine".into(),
                command: "local".into(),
                ..Default::default()
            },
            artisan_api::McpServer {
                name: "s".into(),
                command: "old".into(),
                headers: [("Authorization".to_owned(), "Bearer x".to_owned())].into(),
                ..Default::default()
            },
        ];
        for server in local {
            store
                .mutate_mcp(artisan_api::McpMutation::Upsert {
                    previous: None,
                    server,
                })
                .await
                .unwrap();
        }
        sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path())
            .await
            .unwrap();
        let mcp: Vec<artisan_api::McpServer> =
            serde_json::from_value(store.setting("mcp_servers").await.unwrap().unwrap()).unwrap();
        assert!(
            mcp.iter()
                .any(|server| server.name == "mine" && server.command == "local"),
            "{mcp:?}"
        );
        let synced = mcp.iter().find(|server| server.name == "s").unwrap();
        assert_eq!(synced.command, "srv");
        assert_eq!(
            synced.headers.get("Authorization").map(String::as_str),
            Some("Bearer x")
        );
    }
}
