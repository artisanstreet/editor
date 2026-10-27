//! Account config + credential sync: Forge ← Core.
//!
//! Shapes mirror Core `src/contracts/v1.ts` + `src/http.ts` exactly; tests
//! stub those same routes (same paths, same auth, same envelope names).
//!
//! Config: `GET /v1/config/bundles/latest` (machine proof or session;
//! `ETag: "<contentHash>"`, `304` after auth on `If-None-Match`) answers
//! `{bundle: {id, version, contentHash, r2Key, createdAt} | null,
//! manifest?}`. The canonical manifest bytes come from
//! `GET /v1/config/bundles/:version/manifest` (same ETag scheme,
//! `X-Content-Sha256`, immutable). The Forge hashes THOSE bytes and
//! compares to `bundle.contentHash` — never a hash of the wrapper body
//! against a field inside it. Verified bundles apply through the Hub
//! (global prompt, MCP servers, skills) and report via
//! `POST /v1/config/state`; `304` or same version+hash never reapplies.
//!
//! Engine artifacts: assigned machines download via
//! `GET /v1/engine-artifacts/:id/download` (machine proof) and verify
//! every file's `sha256` + `0600`/`0400` mode before materializing.
//! Freshness: a diverged local auth with no newer assigned export is kept,
//! never overwritten. Assignment *discovery* has no Core machine endpoint
//! (see `docs/config-vault-sync-requirements.md`); sync consumes explicit
//! artifact ids until it exists.

use anyhow::{Context, Result};
use sha2::Digest as _;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Latest bundle + manifest (machine proof or session).
pub const CONFIG_BUNDLES_LATEST_PATH: &str = "/v1/config/bundles/latest";
/// Canonical manifest bytes for one bundle version (machine proof).
pub fn config_manifest_path(version: u64) -> String {
    format!("/v1/config/bundles/{version}/manifest")
}
/// Applied-revision report (machine proof).
pub const CONFIG_STATE_PATH: &str = "/v1/config/state";
/// Assigned-machine envelope download (machine proof).
pub fn artifact_download_path(artifact_id: &str) -> String {
    format!("/v1/engine-artifacts/{artifact_id}/download")
}

fn now_sec() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
    )
    .unwrap_or(0)
}

/// Last-good config state (Forge data dir).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConfigState {
    pub bundle_id: String,
    pub version: u64,
    pub content_hash: String,
    pub etag: String,
    pub applied_at_sec: i64,
}

/// What a config sync did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigOutcome {
    /// `304`, or same version+hash already applied: nothing reapplied.
    Current(ConfigState),
    /// A newer verified bundle applied through the Hub + reported.
    Updated(ConfigState),
}

/// Where sync state lives: `<data_dir>/ast_sync.json`.
pub fn sync_state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ast_sync.json")
}

/// Where applied manifests live: `<data_dir>/ast_config/`.
pub fn config_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("ast_config")
}

fn read_state(data_dir: &Path) -> Result<Option<ConfigState>> {
    let path = sync_state_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("read {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display()))?,
    }
}

fn write_state(data_dir: &Path, state: &ConfigState) -> Result<()> {
    use std::io::Write as _;
    let path = sync_state_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("write {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer_pretty(&mut file, state)?;
        file.flush()?;
    }
    std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

/// Core `BundleInfo` (forward-compatible read).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BundleInfo {
    pub id: String,
    pub version: u64,
    #[serde(rename = "contentHash")]
    pub content_hash: String,
    #[serde(rename = "r2Key")]
    pub r2_key: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

/// Core `BundleLatestResp` (forward-compatible read; the inline manifest
/// is advisory — canonical bytes come from the manifest route).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BundleLatest {
    #[serde(default)]
    pub bundle: Option<BundleInfo>,
}

/// Core `EditorManifest` (strict: unknown fields rejected on our read, so
/// a Core shape change fails loudly instead of half-applying).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditorManifest {
    pub format: String,
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: Vec<ManifestMcpServer>,
    #[serde(default)]
    pub skills: Vec<ManifestSkill>,
    #[serde(rename = "globalPrompt", default)]
    pub global_prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestMcpServer {
    pub name: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSkill {
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub version: Option<String>,
    /// Optional inline bundle (Core `SkillRef.bundle` shape): files staged
    /// to a tempdir, then installed through the real `hub.install_skill`
    /// path. Absent = the skill must already exist in the catalog
    /// (enabled, not installed, by sync).
    #[serde(default)]
    pub bundle: Option<SkillBundle>,
}

/// A bundled skill file (Core `SkillFile` shape, mirrored exactly):
/// relative path, base64url bytes, hex sha256 of the decoded bytes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillBundleFile {
    pub path: String,
    #[serde(rename = "contentBase64")]
    pub content_base64: String,
    pub sha256: String,
}

/// Embedded skill bundle.
///
/// An embedded skill bundle (Core `SkillRef.bundle` shape): 1–64 files,
/// total decoded ≤ 1 MiB, every file hash-verified, SKILL.md required at
/// the root. Installs verbatim under the skill root on machines that lack
/// the skill; machines that already have it keep using `source`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillBundle {
    pub files: Vec<SkillBundleFile>,
}

/// Per-skill bundle decoded-bytes ceiling (matches Core server-side).
pub const SKILL_BUNDLE_MAX_BYTES: usize = 1024 * 1024;
/// Manifest entry caps (match Core publish validation).
pub const MANIFEST_MAX_MCP_SERVERS: usize = 64;
pub const MANIFEST_MAX_SKILLS: usize = 256;

/// Fetch latest bundle.
///
/// Fetch the latest bundle conditionally; verify the canonical manifest
/// bytes; apply through the Hub; report. `304` (or same version+hash
/// already applied) means current — no reapply. On failure the last-good
/// state file is untouched and `failed` is reported with detail; settings
/// converge idempotently on the next good sync.
#[expect(clippy::too_many_lines, reason = "fetch/verify/apply/report in one sync turn")]
pub async fn sync_config(
    http: &reqwest::Client,
    core_base_url: &str,
    machine_id: &str,
    secret: &[u8; 32],
    hub: &forge_runs::Hub,
    data_dir: &Path,
) -> Result<ConfigOutcome> {
    let previous = read_state(data_dir)?;
    let mut request = http.get(format!("{core_base_url}{CONFIG_BUNDLES_LATEST_PATH}"));
    if let Some(previous) = previous.as_ref()
        && !previous.etag.is_empty()
    {
        request = request.header("If-None-Match", previous.etag.as_str());
    }
    let headers = crate::core::proof_headers_for(
        machine_id,
        secret,
        "GET",
        CONFIG_BUNDLES_LATEST_PATH,
        b"",
        now_sec(),
    )?;
    let response = request
        .headers(headers)
        .send()
        .await
        .context("reach Core latest bundle")?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        let previous = previous.context("304 with no last-good state")?;
        return Ok(ConfigOutcome::Current(previous));
    }
    if !response.status().is_success() {
        anyhow::bail!("Core latest bundle answered {}", response.status());
    }
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let latest: BundleLatest = response.json().await.context("read Core latest answer")?;
    let Some(bundle) = latest.bundle else {
        if let Some(previous) = previous {
            return Ok(ConfigOutcome::Current(previous));
        }
        anyhow::bail!("Core has no config bundle and Forge has no last-good state");
    };
    if let Some(previous) = previous.as_ref()
        && previous.bundle_id == bundle.id
        && previous.content_hash == bundle.content_hash
    {
        return Ok(ConfigOutcome::Current(previous.clone()));
    }
    // Canonical bytes from the manifest route; hash THEM.
    let manifest_path = config_manifest_path(bundle.version);
    let headers =
        crate::core::proof_headers_for(machine_id, secret, "GET", &manifest_path, b"", now_sec())?;
    let response = http
        .get(format!("{core_base_url}{manifest_path}"))
        .headers(headers)
        .send()
        .await
        .context("reach Core manifest bytes")?;
    if !response.status().is_success() {
        anyhow::bail!("Core manifest answered {}", response.status());
    }
    let bytes = response
        .bytes()
        .await
        .context("read Core manifest bytes")?
        .to_vec();
    let sha256 = hex::encode(sha2::Sha256::digest(&bytes));
    if sha256 != bundle.content_hash {
        report_state(http, core_base_url, machine_id, secret, &bundle.id, "failed", "manifest hash mismatch").await.ok();
        anyhow::bail!("manifest bytes do not match bundle.contentHash: not applying");
    }
    let manifest: EditorManifest =
        serde_json::from_slice(&bytes).context("read the canonical manifest")?;
    if manifest.format != "ast-editor-config/1" {
        report_state(http, core_base_url, machine_id, secret, &bundle.id, "failed", "unknown manifest format").await.ok();
        anyhow::bail!("unknown manifest format: not applying");
    }
    let detail = match apply_manifest(hub, &manifest).await {
        Ok(detail) => detail,
        Err(error) => {
            report_state(
                http,
                core_base_url,
                machine_id,
                secret,
                &bundle.id,
                "failed",
                &format!("{error:#}"),
            )
            .await
            .ok();
            return Err(error).context("apply the verified bundle");
        }
    };
    // Archive the verified canonical bytes atomically for audit.
    archive_manifest(data_dir, &bundle, &bytes)?;
    report_state(http, core_base_url, machine_id, secret, &bundle.id, "applied", &detail).await?;
    let state = ConfigState {
        bundle_id: bundle.id,
        version: bundle.version,
        content_hash: bundle.content_hash,
        etag,
        applied_at_sec: now_sec(),
    };
    write_state(data_dir, &state)?;
    Ok(ConfigOutcome::Updated(state))
}

fn archive_manifest(data_dir: &Path, bundle: &BundleInfo, bytes: &[u8]) -> Result<()> {
    let dir = config_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let staged = dir.join(format!(".bundle-{}.tmp", bundle.version));
    drop(std::fs::remove_dir_all(&staged));
    std::fs::create_dir_all(&staged)?;
    std::fs::write(staged.join("manifest.json"), bytes)?;
    let live = dir.join(format!("bundle-{}.json", bundle.version));
    if live.exists() {
        std::fs::remove_dir_all(&live).ok();
    }
    std::fs::rename(&staged, &live)
        .with_context(|| format!("publish {}", live.display()))?;
    std::fs::write(dir.join("current"), bundle.version.to_string())?;
    Ok(())
}

/// Apply a verified manifest through the Hub. Returns a human summary for
/// the state report. Order: validate everything, stage all skill bundles
/// to tempdirs, snapshot settings, then mutate. Any failure rolls settings
/// back and discards skills installed in this run, so `failed` never
/// leaves a partially changed config while last-good is claimed.
/// Observable effects only: prompt text, MCP list, and skill enablement
/// are read back after writing — nothing counts as applied until it reads
/// back correct.
async fn apply_manifest(
    hub: &forge_runs::Hub,
    manifest: &EditorManifest,
) -> Result<String> {
    validate_manifest(manifest)?;
    let staged = stage_skills(manifest)?;
    let store = hub.store();
    let snapshot = snapshot_settings(store).await?;
    let mut installed: Vec<String> = Vec::new();
    let result = apply_inner(hub, store, manifest, &staged, &mut installed).await;
    cleanup_staged(staged);
    match result {
        Ok(changed) => Ok(if changed.is_empty() {
            "already converged".into()
        } else {
            changed.join(",")
        }),
        Err(error) => {
            let mut detail = format!("{error:#}");
            if let Err(rollback) = restore_settings(store, &snapshot).await {
                detail = format!("{detail}; rollback also failed: {rollback:#}");
            }
            for id in &installed {
                if let Err(discard) = hub.discard_skill(id).await {
                    detail = format!("{detail}; could not remove skill {id}: {discard:#}");
                }
            }
            Err(anyhow::anyhow!("{detail}"))
        }
    }
}

/// Manifest shape checks before anything mutates: MCP entries, skill
/// bundle decoding, paths, and per-skill byte bounds.
fn validate_manifest(manifest: &EditorManifest) -> Result<()> {
    if manifest.format != "ast-editor-config/1" {
        anyhow::bail!("unknown manifest format");
    }
    if manifest.mcp_servers.len() > MANIFEST_MAX_MCP_SERVERS {
        anyhow::bail!("manifest has too many MCP servers");
    }
    if manifest.skills.len() > MANIFEST_MAX_SKILLS {
        anyhow::bail!("manifest has too many skills");
    }
    for entry in &manifest.mcp_servers {
        if entry.name.trim().is_empty() {
            anyhow::bail!("manifest names an empty MCP server");
        }
        if entry.command.as_deref().unwrap_or("").trim().is_empty()
            && entry.url.as_deref().unwrap_or("").trim().is_empty()
        {
            anyhow::bail!("manifest MCP server {} has no command or URL", entry.name);
        }
    }
    for skill in &manifest.skills {
        if skill.name.trim().is_empty() {
            anyhow::bail!("manifest names an empty skill");
        }
        if let Some(bundle) = skill.bundle.as_ref() {
            validate_skill_bundle(&skill.name, bundle)?;
        }
    }
    Ok(())
}

/// Mirror of Core's server-side bundle validation: strict paths (no
/// leading slash, no `..` segments, ≤ 256 chars), 1–64 files, SKILL.md at
/// the root, per-file sha256 over decoded bytes, total ≤ 1 MiB.
fn validate_skill_bundle(skill_name: &str, bundle: &SkillBundle) -> Result<()> {
    use base64::Engine as _;
    if bundle.files.is_empty() || bundle.files.len() > 64 {
        anyhow::bail!("skill {skill_name} bundle must hold 1-64 files");
    }
    let mut total = 0_usize;
    let mut has_skill_md = false;
    for file in &bundle.files {
        if file.path.is_empty()
            || file.path.len() > 256
            || file.path.starts_with('/')
            || file.path.split('/').any(|part| part == "..")
        {
            anyhow::bail!("skill {skill_name} bundles an unsafe path");
        }
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(file.content_base64.as_bytes())
            .with_context(|| {
                format!("skill {skill_name} has undecodable file {}", file.path)
            })?;
        total = total.saturating_add(raw.len());
        if total > SKILL_BUNDLE_MAX_BYTES {
            anyhow::bail!("skill {skill_name} bundle exceeds the size bound");
        }
        let sha256 = hex::encode(sha2::Sha256::digest(&raw));
        if sha256 != file.sha256 {
            anyhow::bail!("skill {skill_name} file {} hash mismatch", file.path);
        }
        if file.path == "SKILL.md" {
            has_skill_md = true;
        }
    }
    if !has_skill_md {
        anyhow::bail!("skill {skill_name} bundle must include SKILL.md");
    }
    Ok(())
}

/// A staged skill bundle: verified files on disk, ready to install.
struct StagedSkill {
    name: String,
    dir: tempfile::TempDir,
}

/// Stage every bundled skill to a tempdir. No settings change yet.
fn stage_skills(manifest: &EditorManifest) -> Result<Vec<StagedSkill>> {
    use base64::Engine as _;
    let mut staged = Vec::new();
    for skill in &manifest.skills {
        let Some(bundle) = skill.bundle.as_ref() else {
            continue;
        };
        let dir = tempfile::tempdir().context("stage a skill bundle")?;
        for file in &bundle.files {
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(file.content_base64.as_bytes())
                .with_context(|| format!("skill {} has undecodable file {}", skill.name, file.path))?;
            let path = dir.path().join(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, raw)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
        }
        staged.push(StagedSkill {
            name: skill.name.clone(),
            dir,
        });
    }
    Ok(staged)
}

fn cleanup_staged(staged: Vec<StagedSkill>) {
    for skill in staged {
        if let Err(error) = std::fs::remove_dir_all(skill.dir.path()) {
            tracing::warn!(skill = %skill.name, "could not remove staged skill: {error:#}");
        }
    }
}

/// Settings snapshot for rollback: prompt text + MCP + disabled skills.
struct SettingsSnapshot {
    prompt_text: Option<String>,
    mcp: Option<serde_json::Value>,
    disabled: Option<serde_json::Value>,
}

async fn snapshot_settings(store: &forge_store::Store) -> Result<SettingsSnapshot> {
    Ok(SettingsSnapshot {
        prompt_text: store
            .setting("global_prompt")
            .await?
            .and_then(|value| value.as_str().map(str::to_owned)),
        mcp: store.setting("mcp_servers").await?,
        disabled: store.setting("disabled_skills").await?,
    })
}

async fn restore_settings(
    store: &forge_store::Store,
    snapshot: &SettingsSnapshot,
) -> Result<()> {
    let current: Option<String> = store
        .setting("global_prompt")
        .await?
        .and_then(|value| value.as_str().map(str::to_owned));
    if current != snapshot.prompt_text
        && let Some(text) = snapshot.prompt_text.as_deref()
    {
        let base = store.prompt_version().await?;
        store
            .save_prompt_version(base, text.to_owned())
            .await?
            .map_err(|latest| {
                anyhow::anyhow!("prompt changed during rollback (now version {latest})")
            })?;
    }
    if let Some(mcp) = snapshot.mcp.clone() {
        store.set_setting("mcp_servers".into(), mcp).await?;
    }
    if let Some(disabled) = snapshot.disabled.clone() {
        store.set_setting("disabled_skills".into(), disabled).await?;
    }
    Ok(())
}

#[expect(clippy::too_many_lines, reason = "install/enable/prompt/MCP/verify in one apply")]
async fn apply_inner(
    hub: &forge_runs::Hub,
    store: &forge_store::Store,
    manifest: &EditorManifest,
    staged: &[StagedSkill],
    installed: &mut Vec<String>,
) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    // Skills first: install staged bundles through the real installer,
    // then enable everything the manifest names.
    for skill in staged {
        let installed_name = hub
            .install_skill(skill.dir.path().to_path_buf(), true)
            .await
            .with_context(|| format!("install skill {}", skill.name))?;
        installed.push(format!("local.{installed_name}"));
        changed.push(format!("skill:{}", skill.name));
    }
    if !manifest.skills.is_empty() {
        let catalog = hub.skill_catalog().await?;
        for skill in &manifest.skills {
            #[allow(
                clippy::suspicious_operation_groupings,
                reason = "ManifestSkill has only name; match id, name, or suffix"
            )]
            let hit = catalog.iter().find(|entry| {
                entry.id == skill.name || entry.name == skill.name || entry.id.ends_with(&format!(".{}", skill.name))
            });
            let Some(hit) = hit else {
                anyhow::bail!(
                    "manifest skill {} is not installed and carries no bundle",
                    skill.name
                );
            };
            if !hit.enabled {
                hub.set_skill_enabled(&hit.id, true).await?;
                changed.push(format!("skill:{}", hit.id));
            }
        }
        hub.reconcile_skills()
            .await
            .context("reconcile skills after sync")?;
    }
    // Global prompt: compare-and-swap on the stored version, one retry.
    if let Some(text) = manifest.global_prompt.as_deref() {
        let current: Option<String> = store
            .setting("global_prompt")
            .await?
            .and_then(|value| value.as_str().map(str::to_owned));
        if current.as_deref() != Some(text) {
            let mut base = store.prompt_version().await?;
            let mut saved = store.save_prompt_version(base, text.to_owned()).await?;
            if let Err(latest) = saved {
                base = latest;
                saved = store.save_prompt_version(base, text.to_owned()).await?;
            }
            saved.map_err(|latest| {
                anyhow::anyhow!("global prompt changed under us (now version {latest})")
            })?;
            changed.push("globalPrompt".to_owned());
        }
    }
    // MCP servers: converge by name. Server-managed secrets (`env`,
    // headers) stay local — Core never sends them.
    let current: Vec<artisan_api::McpServer> = store
        .setting("mcp_servers")
        .await?
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let mut current_by_name = HashMap::new();
    for server in &current {
        current_by_name.insert(server.name.clone(), server.clone());
    }
    let mut manifest_names = std::collections::HashSet::new();
    for entry in &manifest.mcp_servers {
        manifest_names.insert(entry.name.clone());
        let command = entry.command.clone().unwrap_or_default();
        let args = entry.args.clone().unwrap_or_default();
        let merged = artisan_api::McpServer {
            name: entry.name.clone(),
            command,
            args,
            env: current_by_name
                .get(&entry.name)
                .map_or_else(Default::default, |kept| kept.env.clone()),
            url: entry.url.clone(),
            ..artisan_api::McpServer::default()
        };
        let previous = current_by_name.get(&entry.name);
        if previous != Some(&merged) {
            store
                .mutate_mcp(artisan_api::McpMutation::Upsert {
                    previous: previous.cloned(),
                    server: merged,
                })
                .await?;
            changed.push(format!("mcp:{}", entry.name));
        }
    }
    for server in &current {
        if !manifest_names.contains(&server.name) {
            store
                .mutate_mcp(artisan_api::McpMutation::Remove {
                    previous: server.clone(),
                })
                .await?;
            changed.push(format!("mcp-removed:{}", server.name));
        }
    }
    hub.reconcile_global_prompt()
        .await
        .context("reconcile the prompt after sync")?;
    hub.check_mcp_auth().await;
    // Read back: nothing counts until it is observable.
    verify_applied(store, hub, manifest).await?;
    Ok(changed)
}

/// Read-back verification: prompt text, MCP list, skill enablement —
/// including skills installed from inline bundles this run.
async fn verify_applied(
    store: &forge_store::Store,
    hub: &forge_runs::Hub,
    manifest: &EditorManifest,
) -> Result<()> {
    if let Some(text) = manifest.global_prompt.as_deref() {
        let current: Option<String> = store
            .setting("global_prompt")
            .await?
            .and_then(|value| value.as_str().map(str::to_owned));
        anyhow::ensure!(
            current.as_deref() == Some(text),
            "global prompt did not read back"
        );
    }
    let current: Vec<artisan_api::McpServer> = store
        .setting("mcp_servers")
        .await?
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    for entry in &manifest.mcp_servers {
        let found = current.iter().find(|server| server.name == entry.name).with_context(|| {
            format!("MCP server {} missing after apply", entry.name)
        })?;
        anyhow::ensure!(
            found.command == entry.command.clone().unwrap_or_default()
                && found.args == entry.args.clone().unwrap_or_default()
                && found.url == entry.url,
            "MCP server {} did not read back",
            entry.name
        );
    }
    if !manifest.skills.is_empty() {
        let catalog = hub.skill_catalog().await?;
        for skill in &manifest.skills {
            #[allow(
                clippy::suspicious_operation_groupings,
                reason = "ManifestSkill has only name; match id, name, or local suffix"
            )]
            let enabled = catalog.iter().any(|entry| {
                (entry.id == skill.name
                    || entry.name == skill.name
                    || entry.id.ends_with(&format!("local.{}", skill.name)))
                    && entry.enabled
            });
            anyhow::ensure!(enabled, "skill {} is not enabled after apply", skill.name);
        }
    }
    Ok(())
}

/// `POST /v1/config/state` — report the applied (or failed) revision.
pub async fn report_state(
    http: &reqwest::Client,
    core_base_url: &str,
    machine_id: &str,
    secret: &[u8; 32],
    bundle_id: &str,
    status: &str,
    detail: &str,
) -> Result<()> {
    let request = serde_json::json!({
        "bundleId": bundle_id,
        "status": status,
        "detail": detail,
    });
    let body = serde_json::to_vec(&request)?;
    let headers =
        crate::core::proof_headers_for(machine_id, secret, "POST", CONFIG_STATE_PATH, &body, now_sec())?;
    let response = http
        .post(format!("{core_base_url}{CONFIG_STATE_PATH}"))
        .headers(headers)
        .json(&request)
        .send()
        .await
        .context("reach Core config state")?;
    if !response.status().is_success() {
        anyhow::bail!("Core config state answered {}", response.status());
    }
    Ok(())
}

/// The machine identity `ae account enroll/confirm` persisted
/// (`<data_dir>/ast_account.json`).
#[derive(Clone, Debug)]
pub struct MachineIdentity {
    pub machine_id: String,
    pub account_id: Option<String>,
    pub secret: [u8; 32],
}

/// Read the machine identity. `None` when the Forge is not enrolled
/// (accountless Forges stay untouched).
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
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display()))?,
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
        .map_err(|_secret| anyhow::anyhow!("the machine secret is not 32 bytes"))?;
    Ok(Some(MachineIdentity {
        machine_id,
        account_id: file.account_id,
        secret,
    }))
}

/// Daemon loop: sync on startup, then every interval. Accountless Forges
/// (no machine identity) are untouched.
pub async fn run_config_loop(
    data_dir: PathBuf,
    hub: forge_runs::Hub,
    shutdown: tokio_util::sync::CancellationToken,
    interval: std::time::Duration,
) {
    let Some(identity) = load_machine_identity(&data_dir)
        .map_err(|error| tracing::warn!("config sync disabled: {error:#}"))
        .ok()
        .flatten()
    else {
        return;
    };
    let machine_id = identity.machine_id;
    let secret = identity.secret;
    let base_url = crate::core::core_base_url(None);
    let http = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(http) => http,
        Err(error) => {
            tracing::warn!("config sync disabled: {error:#}");
            return;
        }
    };
    loop {
        match sync_config(&http, &base_url, &machine_id, &secret, &hub, &data_dir).await {
            Ok(ConfigOutcome::Updated(state)) => {
                tracing::info!(version = state.version, "applied config bundle");
            }
            Ok(ConfigOutcome::Current(_)) => {}
            Err(error) => tracing::warn!("config sync failed: {error:#}"),
        }
        tokio::select! {
            () = shutdown.cancelled() => break,
            () = tokio::time::sleep(interval) => {},
        }
    }
}

// ---------------------------------------------------------------------------
// Engine credential artifacts with freshness.
// ---------------------------------------------------------------------------

/// One file inside an `ast-engine-credential/1` envelope (Core shape).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactFile {
    pub path: String,
    pub mode: String,
    pub sha256: String,
    #[serde(rename = "contentBase64")]
    pub content_base64: String,
}

/// `ast-engine-credential/1` envelope (Core shape; inner file bytes opaque).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EngineArtifactEnvelope {
    pub format: String,
    pub engine: String,
    pub kind: String,
    #[serde(rename = "exportedAt")]
    pub exported_at: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<String>,
    pub files: Vec<ArtifactFile>,
}

/// `GET …/download` answer (Core shape).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactDownload {
    pub envelope: EngineArtifactEnvelope,
}

/// Last-applied record per artifact (Forge data dir sidecar).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VaultApplied {
    pub artifact_id: String,
    pub exported_at: String,
    pub local_files_hash: String,
    pub applied_at_sec: i64,
}

/// Local engine auth snapshot for freshness comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAuth {
    pub engine: String,
    /// Hex SHA-256 over the sorted credential file bytes ("" when absent).
    pub files_hash: String,
    /// Caller-provided probe outcome (`true` = engine reports signed in).
    pub probe_signed_in: bool,
}

/// Per-engine result of a vault sync.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineAuthReport {
    pub engine: String,
    pub status: AuthStatus,
    pub artifact_id: Option<String>,
}

/// Adapter + freshness outcome. `NeedsReauth` names engines whose probe
/// fails after sync — the operator must sign in again locally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthStatus {
    /// Local auth current; nothing changed.
    UpToDate,
    /// Newer assigned artifact applied and probe passes.
    Updated,
    /// Local auth is newer/diverged: kept local, NOT overwritten.
    KeptLocal,
    /// Probe fails: engine needs an interactive sign-in.
    NeedsReauth { reason: String },
}

/// Download an assigned envelope (machine proof) and verify every file's
/// `sha256` + mode. Expiry is enforced: a past `expiresAt` is
/// do-not-apply. Nothing is written here.
pub async fn download_artifact(
    http: &reqwest::Client,
    core_base_url: &str,
    machine_id: &str,
    secret: &[u8; 32],
    artifact_id: &str,
) -> Result<EngineArtifactEnvelope> {
    if artifact_id.contains('/') || artifact_id.contains("..") {
        anyhow::bail!("bad artifact id");
    }
    let path = artifact_download_path(artifact_id);
    let headers =
        crate::core::proof_headers_for(machine_id, secret, "GET", &path, b"", now_sec())?;
    let response = http
        .get(format!("{core_base_url}{path}"))
        .headers(headers)
        .send()
        .await
        .context("reach Core artifact download")?;
    if !response.status().is_success() {
        anyhow::bail!("Core artifact download answered {}", response.status());
    }
    let download: ArtifactDownload = response.json().await.context("read artifact")?;
    verify_envelope(&download.envelope)?;
    Ok(download.envelope)
}

/// Verify envelope shape + every file hash/mode. Expiry is do-not-apply.
pub fn verify_envelope(envelope: &EngineArtifactEnvelope) -> Result<()> {
    use base64::Engine as _;
    if envelope.format != "ast-engine-credential/1" {
        anyhow::bail!("unknown artifact format");
    }
    if !["claude", "codex", "opencode"].contains(&envelope.engine.as_str()) {
        anyhow::bail!("unknown artifact engine");
    }
    if !["oauth", "api_key"].contains(&envelope.kind.as_str()) {
        anyhow::bail!("unknown artifact kind");
    }
    if envelope.files.is_empty() {
        anyhow::bail!("artifact has no files");
    }
    for file in &envelope.files {
        if file.path.contains("..") || file.path.starts_with('/') || file.path.is_empty() {
            anyhow::bail!("artifact names an unsafe path");
        }
        if file.mode != "0600" && file.mode != "0400" {
            anyhow::bail!("artifact names an unsafe mode");
        }
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(file.content_base64.as_bytes())
            .context("decode an artifact file")?;
        let sha256 = hex::encode(sha2::Sha256::digest(&raw));
        if sha256 != file.sha256 {
            anyhow::bail!("artifact file hash mismatch: not applying");
        }
    }
    Ok(())
}

/// Apply a verified envelope.
///
/// Apply a verified envelope with freshness: never blindly overwrite newer
/// local engine auth. Returns the per-engine outcome; the caller probes the
/// engine afterwards and maps a failed probe to `NeedsReauth`.
pub fn apply_envelope(
    engine_home: &Path,
    envelope: &EngineArtifactEnvelope,
    artifact_id: &str,
    last_applied: Option<&VaultApplied>,
    local: &LocalAuth,
) -> Result<(AuthStatus, Option<VaultApplied>)> {
    // Freshness gate: a diverged local auth with no newer assigned export
    // is kept, never overwritten.
    if let Some(last) = last_applied
        && last.artifact_id == artifact_id
        && local.files_hash != last.local_files_hash
    {
        return Ok((AuthStatus::KeptLocal, None));
    }
    if let Some(last) = last_applied
        && last.artifact_id == artifact_id
        && local.files_hash == last.local_files_hash
    {
        return Ok((
            if local.probe_signed_in {
                AuthStatus::UpToDate
            } else {
                AuthStatus::NeedsReauth {
                    reason: "engine reports signed out".into(),
                }
            },
            None,
        ));
    }
    let mut files = std::collections::BTreeMap::new();
    for file in &envelope.files {
        files.insert(file.path.clone(), file.content_base64.clone());
    }
    let backup = backup_local(engine_home, files.keys())?;
    if let Err(error) = materialize(engine_home, &files) {
        restore_local(engine_home, &backup)?;
        return Ok((
            AuthStatus::NeedsReauth {
                reason: format!("materialize failed: {error:#}"),
            },
            None,
        ));
    }
    let applied = VaultApplied {
        artifact_id: artifact_id.to_owned(),
        exported_at: envelope.exported_at.clone(),
        local_files_hash: hash_names(engine_home, files.keys())?,
        applied_at_sec: now_sec(),
    };
    std::fs::remove_dir_all(&backup).ok();
    Ok((AuthStatus::Updated, Some(applied)))
}

fn hash_names<'a>(
    engine_home: &Path,
    names: impl IntoIterator<Item = &'a String>,
) -> Result<String> {
    use sha2::Digest as _;
    let mut names: Vec<&String> = names.into_iter().collect();
    names.sort();
    let mut digest = sha2::Sha256::new();
    for name in &names {
        if name.contains("..") || name.starts_with('/') {
            anyhow::bail!("artifact names an unsafe path");
        }
        let bytes = std::fs::read(engine_home.join(name)).unwrap_or_default();
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(&bytes);
    }
    Ok(hex::encode(digest.finalize()))
}

/// Hash the credential files currently on disk (same order as apply).
pub fn hash_local(engine_home: &Path, relpaths: &[String]) -> Result<String> {
    hash_names(engine_home, relpaths.iter())
}

/// Copy the current files aside so a failed apply can be rolled back.
fn backup_local<'a>(
    engine_home: &Path,
    names: impl IntoIterator<Item = &'a String>,
) -> Result<PathBuf> {
    let backup = engine_home.join(format!(".vault-backup-{}", now_sec()));
    for name in names {
        let source = engine_home.join(name);
        if source.is_file() {
            let target = backup.join(name);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source, &target)?;
        }
    }
    Ok(backup)
}

fn materialize(
    engine_home: &Path,
    files: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    use base64::Engine as _;
    for (name, contents) in files {
        if name.contains("..") || name.starts_with('/') {
            anyhow::bail!("artifact names an unsafe path");
        }
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(contents.as_bytes())
            .context("decode an artifact file")?;
        let path = engine_home.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, raw)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

/// Roll back to the pre-apply snapshot after a failed materialize.
fn restore_local(engine_home: &Path, backup: &Path) -> Result<()> {
    let mut stack = vec![backup.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path.strip_prefix(backup).context("backup escaped")?;
                if relative.components().any(|part| part.as_os_str() == "..") {
                    anyhow::bail!("backup names an unsafe path");
                }
                let target = engine_home.join(relative);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&path, &target)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test fixtures; failures fail the test"
)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use base64::Engine as _;
    use sha2::Digest as _;
    use std::sync::{Arc, Mutex};

    const MACHINE: &str = "mch_11111111111111111111111111";
    const SECRET: [u8; 32] = [0x21_u8; 32];

    fn test_client() -> reqwest::Client {
        reqwest::Client::new()
    }

    fn machine_proof_ok(headers: &HeaderMap) {
        for name in ["x-ast-machine", "x-ast-timestamp", "x-ast-nonce", "authorization"] {
            assert!(headers.contains_key(name), "missing proof header {name}");
        }
        assert!(headers["authorization"].to_str().unwrap().starts_with("AST-Machine "));
    }

    fn manifest_bytes() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "format": "ast-editor-config/1",
            "mcpServers": [{"name": "s", "command": "srv"}],
            "skills": [],
            "globalPrompt": "synced prompt",
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
            "contentBase64": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(contents),
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
        stub.manifest_override.clone().unwrap_or_else(manifest_bytes)
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
                "contentHash": hash,
                "r2Key": "cfg/acct/bundle-3.json",
                "createdAt": "2026-10-26T20:00:00.000Z",
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
            "machineId": MACHINE,
            "bundleId": "bundle_000000000000000000000001",
            "status": "applied",
            "appliedAt": "2026-10-26T20:01:00.000Z",
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
        let prompt: Option<serde_json::Value> =
            hub.store().setting("global_prompt").await.unwrap();
        assert_eq!(prompt.and_then(|v| v.as_str().map(str::to_owned)).as_deref(), Some("synced prompt"));
        let mcp: Vec<artisan_api::McpServer> = serde_json::from_value(
            hub.store().setting("mcp_servers").await.unwrap().unwrap_or_default(),
        )
        .unwrap();
        assert!(mcp.iter().any(|server| server.name == "s" && server.command == "srv"));

        let outcome = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path())
            .await
            .unwrap();
        assert!(
            matches!(outcome, ConfigOutcome::Current(_)),
            "ETag 304 must not reapply"
        );
        assert_eq!(stub.reports.lock().unwrap().len(), 1, "no second report on 304");
    }

    /// Manifest bytes that do not hash to `bundle.contentHash` are rejected
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

    fn envelope(file: &str, contents: &[u8]) -> EngineArtifactEnvelope {
        EngineArtifactEnvelope {
            format: "ast-engine-credential/1".into(),
            engine: "claude".into(),
            kind: "oauth".into(),
            exported_at: "2026-10-26T20:00:00.000Z".into(),
            expires_at: None,
            files: vec![ArtifactFile {
                path: file.into(),
                mode: "0600".into(),
                sha256: hex::encode(sha2::Sha256::digest(contents)),
                content_base64: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(contents),
            }],
        }
    }

    /// Freshness: a diverged local auth with no newer assigned export is
    /// kept, never overwritten.
    #[test]
    fn vault_keeps_newer_local_auth() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("creds"), b"local-newer").unwrap();
        let local = LocalAuth {
            engine: "claude".into(),
            files_hash: hash_local(home.path(), &["creds".to_owned()]).unwrap(),
            probe_signed_in: true,
        };
        let last = VaultApplied {
            artifact_id: "art_1".into(),
            exported_at: "2026-10-26T20:00:00.000Z".into(),
            local_files_hash: "older-hash".into(),
            applied_at_sec: 1,
        };
        let (status, applied) = apply_envelope(
            home.path(),
            &envelope("creds", b"assigned-same-export"),
            "art_1",
            Some(&last),
            &local,
        )
        .unwrap();
        assert_eq!(status, AuthStatus::KeptLocal);
        assert_eq!(applied, None);
        assert_eq!(
            std::fs::read(home.path().join("creds")).unwrap(),
            b"local-newer"
        );
    }

    /// Newer assigned exports apply (0600), verify, and roll back on
    /// failure; the adapter report names engines needing reauth.
    #[test]
    fn vault_applies_newer_revisions_and_reports_reauth() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("creds"), b"v1").unwrap();
        let local = LocalAuth {
            engine: "claude".into(),
            files_hash: hash_local(home.path(), &["creds".to_owned()]).unwrap(),
            probe_signed_in: true,
        };
        let (status, applied) = apply_envelope(
            home.path(),
            &envelope("creds", b"v2-assigned"),
            "art_2",
            None,
            &local,
        )
        .unwrap();
        assert_eq!(status, AuthStatus::Updated);
        let applied = applied.unwrap();
        assert_eq!(applied.artifact_id, "art_2");
        assert_eq!(std::fs::read(home.path().join("creds")).unwrap(), b"v2-assigned");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(home.path().join("creds")).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let mut bad = envelope("creds", b"v3");
        bad.files[0].sha256 = "00".into();
        let _local = LocalAuth {
            engine: "claude".into(),
            files_hash: hash_local(home.path(), &["creds".to_owned()]).unwrap(),
            probe_signed_in: false,
        };
        assert!(verify_envelope(&bad).is_err());
        let reports = [
            EngineAuthReport {
                engine: "claude".into(),
                status: AuthStatus::Updated,
                artifact_id: Some("art_2".into()),
            },
            EngineAuthReport {
                engine: "codex".into(),
                status: AuthStatus::NeedsReauth {
                    reason: "engine reports signed out".into(),
                },
                artifact_id: None,
            },
        ];
        assert!(
            reports.iter().any(|report| matches!(
                report.status,
                AuthStatus::NeedsReauth { .. }
            )),
            "codex needs an interactive sign-in"
        );
    }

fn skill_manifest(skill: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "format": "ast-editor-config/1",
        "mcpServers": [{"name": "s", "command": "srv"}],
        "skills": [skill],
        "globalPrompt": "synced prompt",
    }))
    .unwrap()
}

fn bundled_skill(name: &str, extra: Vec<(&str, &[u8])>) -> serde_json::Value {
    let mut files = vec![skill_file(
        "SKILL.md",
        skill_md(name).as_bytes(),
    )];
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
    let manifest = skill_manifest(&bundled_skill("syncprobed", vec![("refs/notes.md", b"notes")]));
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
async fn invalid_bundle_rolls_back_without_partial_apply() {
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

/// A per-file hash mismatch is caught before anything mutates.
#[tokio::test]
async fn tampered_bundle_file_hash_rejects_before_mutation() {
    let mut files = vec![skill_file("SKILL.md", skill_md("tampered").as_bytes())];
    files[0]["sha256"] = serde_json::json!("00");
    let manifest = skill_manifest(&serde_json::json!({
        "name": "tampered",
        "source": "builtin",
        "bundle": {"files": files},
    }));
    let (base, stub, _server) = stub_server(false, Some(manifest)).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    assert!(sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path()).await.is_err());
    assert!(!super::sync_state_path(dir.path()).exists());
    assert_eq!(stub.reports.lock().unwrap()[0]["status"], serde_json::json!("failed"));
}

}
