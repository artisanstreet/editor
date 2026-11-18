use sha2::Digest as _;
use std::path::{Component, Path, PathBuf};

use crate::core::now_sec;

const CONFIG_BUNDLES_LATEST_PATH: &str = "/v1/config/bundles/latest";
const CONFIG_STATE_PATH: &str = "/v1/config/state";
const MANIFEST_FORMAT: &str = "ast-editor-config/1";

fn config_manifest_path(version: u64) -> String {
    format!("/v1/config/bundles/{version}/manifest")
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConfigState {
    pub bundle_id: String,
    pub version: u64,
    pub content_hash: String,
    pub etag: String,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub report: AppliedReport,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AppliedReport {
    #[default]
    Delivered,
    Pending {
        changed: Vec<String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SyncFile {
    applied: Option<ConfigState>,
    claimed_mcp_servers: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredSync {
    Current(SyncFile),
    Legacy(ConfigState),
}

impl From<StoredSync> for SyncFile {
    fn from(stored: StoredSync) -> Self {
        match stored {
            StoredSync::Current(file) => file,
            StoredSync::Legacy(state) => {
                let claimed_mcp_servers = state.mcp_servers.clone();
                Self {
                    applied: Some(state),
                    claimed_mcp_servers,
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigOutcome {
    Current(ConfigState),
    Updated {
        state: ConfigState,
        kept_local: Vec<LocalMcpServerKept>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalMcpServerKept {
    pub name: String,
}

fn sync_state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ast_sync.json")
}

#[derive(Debug, thiserror::Error)]
pub enum SyncStateError {
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("encode {}", path.display())]
    Encode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("replace {}", path.display())]
    Replace {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the worker that reads and writes {} stopped", path.display())]
    Worker {
        path: PathBuf,
        #[source]
        source: tokio::task::JoinError,
    },
}

fn read_state(data_dir: &Path) -> Result<SyncFile, SyncStateError> {
    let path = sync_state_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<StoredSync>(&bytes)
            .map(SyncFile::from)
            .map_err(|source| SyncStateError::Parse { path, source }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SyncFile::default()),
        Err(source) => Err(SyncStateError::Read { path, source }),
    }
}

async fn load_state(data_dir: &Path) -> Result<SyncFile, SyncStateError> {
    let dir = data_dir.to_path_buf();
    tokio::task::spawn_blocking(move || read_state(&dir))
        .await
        .map_err(|source| SyncStateError::Worker {
            path: sync_state_path(data_dir),
            source,
        })?
}

async fn save_state(data_dir: &Path, file: SyncFile) -> Result<(), SyncStateError> {
    let dir = data_dir.to_path_buf();
    tokio::task::spawn_blocking(move || write_state(&dir, &file))
        .await
        .map_err(|source| SyncStateError::Worker {
            path: sync_state_path(data_dir),
            source,
        })?
}

fn write_state(data_dir: &Path, file: &SyncFile) -> Result<(), SyncStateError> {
    let path = sync_state_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(file).map_err(|source| SyncStateError::Encode {
        path: path.clone(),
        source,
    })?;
    std::fs::write(&tmp, bytes).map_err(|source| SyncStateError::Write {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, &path).map_err(|source| SyncStateError::Replace { path, source })
}

#[derive(Debug, thiserror::Error)]
pub enum SyncConfigError {
    #[error(transparent)]
    State(#[from] SyncStateError),
    #[error("sign the request for {step}")]
    Proof {
        step: &'static str,
        #[source]
        source: crate::core::ProofHeadersError,
    },
    #[error("reach Core to {step}")]
    Reach {
        step: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("Core refused to {step}")]
    Refused {
        step: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("read Core's answer to {step}")]
    Read {
        step: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("the bundle's manifest is larger than {limit} bytes")]
    ManifestTooLarge { limit: usize },
    #[error("Core answered 304 but this Forge has applied no bundle")]
    NotModifiedWithoutBundle,
    #[error("Core has no config bundle yet")]
    NoBundle,
    #[error("apply config bundle {bundle} version {version}")]
    Apply {
        bundle: String,
        version: u64,
        #[source]
        source: Box<ApplyManifestError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("encode the {status} report of bundle {bundle}")]
    Encode {
        bundle: String,
        status: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("sign the {status} report of bundle {bundle}")]
    Proof {
        bundle: String,
        status: &'static str,
        #[source]
        source: crate::core::ProofHeadersError,
    },
    #[error("reach Core with the {status} report of bundle {bundle}")]
    Reach {
        bundle: String,
        status: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("Core refused the {status} report of bundle {bundle}")]
    Refused {
        bundle: String,
        status: &'static str,
        #[source]
        source: reqwest::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyManifestError {
    #[error("the manifest does not match the bundle hash {expected}")]
    Hash { expected: String },
    #[error("read the manifest")]
    Parse(#[source] serde_json::Error),
    #[error("unknown manifest format {format}")]
    Format { format: String },
    #[error("the bundle carries {files} skill files; the limit is {limit}")]
    TooManyFiles { files: usize, limit: usize },
    #[error("the bundle's skill files decode to about {bytes} bytes; the limit is {limit}")]
    TooManyBytes { bytes: usize, limit: usize },
    #[error("install skill {skill} from the bundle")]
    InstallSkill {
        skill: String,
        #[source]
        source: Box<InstallBundleError>,
    },
    #[error("read the skill catalog")]
    Catalog(#[source] Box<forge_runs::SkillCatalogError>),
    #[error("skill {skill} is not installed and the bundle carries none")]
    SkillMissing { skill: String },
    #[error("enable skill {skill}")]
    EnableSkill {
        skill: String,
        #[source]
        source: Box<forge_runs::SetSkillEnabledError>,
    },
    #[error("reconcile skills")]
    ReconcileSkills(#[source] Box<forge_runs::ReconcileSkillsError>),
    #[error("read the global prompt")]
    ReadPrompt(#[source] forge_store::ReadSettingError),
    #[error("read the global prompt's version")]
    PromptVersion(#[source] forge_store::PromptVersionError),
    #[error("save the global prompt")]
    SavePrompt(#[source] forge_store::SavePromptError),
    #[error("the global prompt changed while syncing (now version {latest})")]
    PromptChanged { latest: u64 },
    #[error("reconcile the global prompt")]
    ReconcilePrompt(#[source] Box<forge_runs::GlobalPromptError>),
    #[error(transparent)]
    Mcp(#[from] ApplyMcpError),
}

#[derive(Debug, thiserror::Error)]
pub enum InstallBundleError {
    #[error("stage a skill bundle")]
    Stage(#[source] std::io::Error),
    #[error("the worker staging a skill bundle stopped")]
    Worker(#[source] tokio::task::JoinError),
    #[error("the bundle has an unsafe path {path}")]
    UnsafePath { path: String },
    #[error("decode {path}")]
    Decode {
        path: String,
        #[source]
        source: base64::DecodeError,
    },
    #[error("create the folder of {path}")]
    Folder {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("write {path}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Install(Box<forge_runs::InstallSkillError>),
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyMcpError {
    #[error("the bundle has an MCP server without a name")]
    Unnamed,
    #[error("MCP server {server} in the bundle has neither a command nor a URL")]
    TargetMissing { server: String },
    #[error("the bundle lists MCP server {server} more than once")]
    Duplicate { server: String },
    #[error("read the MCP servers")]
    Read(#[source] forge_store::ReadSettingError),
    #[error("decode the MCP servers")]
    Decode(#[source] serde_json::Error),
    #[error("record the MCP servers {servers:?} as synced before placing them")]
    Claim {
        servers: Vec<String>,
        #[source]
        source: SyncStateError,
    },
    #[error("place and remove the synced MCP servers")]
    Mutate(#[source] forge_store::MutateMcpError),
}

#[derive(Debug, thiserror::Error)]
pub enum MachineIdentityError {
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("read the machine secret in {}", path.display())]
    Secret {
        path: PathBuf,
        #[source]
        source: base64::DecodeError,
    },
    #[error("the machine secret in {} is {bytes} bytes, not 32", path.display())]
    SecretLength { path: PathBuf, bytes: usize },
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
    #[serde(default)]
    bundle: Option<SkillBundle>,
}

#[derive(serde::Deserialize)]
struct SkillBundle {
    files: Vec<SkillBundleFile>,
}

#[derive(serde::Deserialize)]
struct SkillBundleFile {
    path: String,
    content_base64: String,
}

struct Core<'a> {
    http: &'a reqwest::Client,
    base: &'a str,
    machine_id: &'a str,
    secret: &'a [u8; 32],
}

impl Core<'_> {
    fn get(
        &self,
        step: &'static str,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, SyncConfigError> {
        let headers = crate::core::CoreClient::proof_headers(
            self.machine_id,
            self.secret,
            "GET",
            path,
            b"",
            now_sec(),
        )
        .map_err(|source| SyncConfigError::Proof { step, source })?;
        Ok(self
            .http
            .get(format!("{}{path}", self.base))
            .headers(headers))
    }

    async fn report(
        &self,
        bundle_id: &str,
        status: &'static str,
        detail: &str,
    ) -> Result<(), ReportError> {
        let bundle = || bundle_id.to_owned();
        let body = serde_json::to_vec(&serde_json::json!({
            "bundleId": bundle_id,
            "status": status,
            "detail": detail,
        }))
        .map_err(|source| ReportError::Encode {
            bundle: bundle(),
            status,
            source,
        })?;
        let headers = crate::core::CoreClient::proof_headers(
            self.machine_id,
            self.secret,
            "POST",
            CONFIG_STATE_PATH,
            &body,
            now_sec(),
        )
        .map_err(|source| ReportError::Proof {
            bundle: bundle(),
            status,
            source,
        })?;
        self.http
            .post(format!("{}{CONFIG_STATE_PATH}", self.base))
            .headers(headers)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|source| ReportError::Reach {
                bundle: bundle(),
                status,
                source,
            })?
            .error_for_status()
            .map_err(|source| ReportError::Refused {
                bundle: bundle(),
                status,
                source,
            })?;
        Ok(())
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one sync turn from fetching the bundle to reporting it"
)]
pub async fn sync_config(
    http: &reqwest::Client,
    core_base_url: &str,
    machine_id: &str,
    secret: &[u8; 32],
    hub: &forge_runs::Hub,
    data_dir: &Path,
) -> Result<ConfigOutcome, SyncConfigError> {
    let core = Core {
        http,
        base: core_base_url,
        machine_id,
        secret,
    };
    let file = load_state(data_dir).await?;
    let latest_step = "fetch the latest config bundle";
    let mut request = core.get(latest_step, CONFIG_BUNDLES_LATEST_PATH)?;
    if let Some(etag) = file
        .applied
        .as_ref()
        .map(|state| state.etag.as_str())
        .filter(|etag| !etag.is_empty())
    {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request
        .send()
        .await
        .map_err(|source| SyncConfigError::Reach {
            step: latest_step,
            source,
        })?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return current(
            &core,
            data_dir,
            file,
            SyncConfigError::NotModifiedWithoutBundle,
        )
        .await;
    }
    let response = response
        .error_for_status()
        .map_err(|source| SyncConfigError::Refused {
            step: latest_step,
            source,
        })?;
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let latest: BundleLatest = response
        .json()
        .await
        .map_err(|source| SyncConfigError::Read {
            step: latest_step,
            source,
        })?;
    let Some(bundle) = latest.bundle else {
        return current(&core, data_dir, file, SyncConfigError::NoBundle).await;
    };
    if file.applied.as_ref().is_some_and(|state| {
        (&state.bundle_id, &state.content_hash) == (&bundle.id, &bundle.content_hash)
    }) {
        return current(&core, data_dir, file, SyncConfigError::NoBundle).await;
    }
    let manifest_step = "fetch the bundle's manifest";
    let mut response = core
        .get(manifest_step, &config_manifest_path(bundle.version))?
        .send()
        .await
        .map_err(|source| SyncConfigError::Reach {
            step: manifest_step,
            source,
        })?
        .error_for_status()
        .map_err(|source| SyncConfigError::Refused {
            step: manifest_step,
            source,
        })?;
    let too_large = SyncConfigError::ManifestTooLarge {
        limit: MAX_MANIFEST_BYTES,
    };
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
    {
        return Err(too_large);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| SyncConfigError::Read {
            step: manifest_step,
            source,
        })?
    {
        if bytes.len() + chunk.len() > MAX_MANIFEST_BYTES {
            return Err(too_large);
        }
        bytes.extend_from_slice(&chunk);
    }
    let applied = apply_manifest(hub, &bytes, &bundle.content_hash, data_dir, &file).await;
    let Applied {
        changed,
        placed: mcp_servers,
        kept_local,
    } = match applied {
        Ok(applied) => applied,
        Err(source) => {
            if let Err(report) = core
                .report(&bundle.id, "failed", &diagnostics::chain(&source))
                .await
            {
                tracing::debug!(
                    error = diagnostics::chain(&report),
                    "report a failed config bundle"
                );
            }
            return Err(SyncConfigError::Apply {
                bundle: bundle.id,
                version: bundle.version,
                source: Box::new(source),
            });
        }
    };
    let claimed_mcp_servers = mcp_servers.clone();
    let file = SyncFile {
        applied: Some(ConfigState {
            bundle_id: bundle.id,
            version: bundle.version,
            content_hash: bundle.content_hash,
            etag,
            mcp_servers,
            report: AppliedReport::Pending { changed },
        }),
        claimed_mcp_servers,
    };
    save_state(data_dir, file.clone()).await?;
    let state = deliver_report(&core, data_dir, file).await?;
    Ok(ConfigOutcome::Updated { state, kept_local })
}

async fn current(
    core: &Core<'_>,
    data_dir: &Path,
    file: SyncFile,
    unapplied: SyncConfigError,
) -> Result<ConfigOutcome, SyncConfigError> {
    if file.applied.is_none() {
        return Err(unapplied);
    }
    Ok(ConfigOutcome::Current(
        deliver_report(core, data_dir, file).await?,
    ))
}

async fn deliver_report(
    core: &Core<'_>,
    data_dir: &Path,
    mut file: SyncFile,
) -> Result<ConfigState, SyncStateError> {
    let state = file
        .applied
        .as_mut()
        .expect("invariant: only an applied bundle has a report to deliver");
    let AppliedReport::Pending { changed } = &state.report else {
        return Ok(state.clone());
    };
    match core
        .report(&state.bundle_id, "applied", &changed.join(","))
        .await
    {
        Ok(()) => {
            state.report = AppliedReport::Delivered;
            let delivered = state.clone();
            save_state(data_dir, file).await?;
            Ok(delivered)
        }
        Err(error) => {
            tracing::warn!(
                bundle = %state.bundle_id,
                version = state.version,
                error = diagnostics::chain(&error),
                "applied a config bundle but could not tell Core; telling it on the next sync"
            );
            Ok(state.clone())
        }
    }
}

struct Applied {
    changed: Vec<String>,
    placed: Vec<String>,
    kept_local: Vec<LocalMcpServerKept>,
}

const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;
const MAX_BUNDLE_FILES: usize = 4096;
const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;

fn admit_bundles(manifest: &EditorManifest) -> Result<(), ApplyManifestError> {
    let files: Vec<&SkillBundleFile> = manifest
        .skills
        .iter()
        .filter_map(|skill| skill.bundle.as_ref())
        .flat_map(|bundle| &bundle.files)
        .collect();
    if files.len() > MAX_BUNDLE_FILES {
        return Err(ApplyManifestError::TooManyFiles {
            files: files.len(),
            limit: MAX_BUNDLE_FILES,
        });
    }
    let bytes: usize = files
        .iter()
        .map(|file| file.content_base64.len() / 4 * 3 + 3)
        .sum();
    if bytes > MAX_BUNDLE_BYTES {
        return Err(ApplyManifestError::TooManyBytes {
            bytes,
            limit: MAX_BUNDLE_BYTES,
        });
    }
    Ok(())
}

async fn apply_manifest(
    hub: &forge_runs::Hub,
    bytes: &[u8],
    content_hash: &str,
    data_dir: &Path,
    file: &SyncFile,
) -> Result<Applied, ApplyManifestError> {
    if hex::encode(sha2::Sha256::digest(bytes)) != content_hash {
        return Err(ApplyManifestError::Hash {
            expected: content_hash.to_owned(),
        });
    }
    let manifest: EditorManifest =
        serde_json::from_slice(bytes).map_err(ApplyManifestError::Parse)?;
    if manifest.format != MANIFEST_FORMAT {
        return Err(ApplyManifestError::Format {
            format: manifest.format,
        });
    }
    apply(hub, &manifest, data_dir, file).await
}

async fn apply(
    hub: &forge_runs::Hub,
    manifest: &EditorManifest,
    data_dir: &Path,
    file: &SyncFile,
) -> Result<Applied, ApplyManifestError> {
    let destinations = mcp_destinations(&manifest.mcp_servers)?;
    admit_bundles(manifest)?;
    let mut staged = Vec::new();
    let applied = apply_staged(hub, manifest, destinations, data_dir, file, &mut staged).await;
    drop(tokio::task::spawn_blocking(move || drop(staged)));
    applied
}

async fn apply_staged(
    hub: &forge_runs::Hub,
    manifest: &EditorManifest,
    destinations: Vec<artisan_api::McpServer>,
    data_dir: &Path,
    file: &SyncFile,
    staged: &mut Vec<(String, tempfile::TempDir)>,
) -> Result<Applied, ApplyManifestError> {
    for skill in &manifest.skills {
        if let Some(bundle) = &skill.bundle {
            let dir =
                stage_bundle(bundle)
                    .await
                    .map_err(|source| ApplyManifestError::InstallSkill {
                        skill: skill.name.clone(),
                        source: Box::new(source),
                    })?;
            staged.push((skill.name.clone(), dir));
        }
    }
    let mut changed = apply_skills(hub, &manifest.skills, staged).await?;
    if let Some(text) = manifest.global_prompt.as_deref() {
        let store = hub.store();
        let current = store
            .setting("global_prompt")
            .await
            .map_err(ApplyManifestError::ReadPrompt)?;
        if current.as_ref().and_then(serde_json::Value::as_str) != Some(text) {
            let base = store
                .prompt_version()
                .await
                .map_err(ApplyManifestError::PromptVersion)?;
            store
                .save_prompt_version(base, text.to_owned())
                .await
                .map_err(ApplyManifestError::SavePrompt)?
                .map_err(|latest| ApplyManifestError::PromptChanged { latest })?;
            hub.extensions()
                .reconcile_global_prompt()
                .await
                .map_err(|error| ApplyManifestError::ReconcilePrompt(Box::new(error)))?;
            changed.push("globalPrompt".to_owned());
        }
    }
    let mcp = apply_mcp(hub.store(), destinations, data_dir, file).await?;
    changed.extend(mcp.changed);
    hub.mcp().check_auth().await;
    Ok(Applied {
        changed,
        placed: mcp.placed,
        kept_local: mcp.kept_local,
    })
}

async fn apply_skills(
    hub: &forge_runs::Hub,
    skills: &[ManifestSkill],
    staged: &[(String, tempfile::TempDir)],
) -> Result<Vec<String>, ApplyManifestError> {
    let mut changed = Vec::new();
    if skills.is_empty() {
        return Ok(changed);
    }
    let catalog = hub
        .extensions()
        .skill_catalog()
        .await
        .map_err(|error| ApplyManifestError::Catalog(Box::new(error)))?;
    if let Some(missing) = skills
        .iter()
        .filter(|skill| skill.bundle.is_none())
        .find(|skill| catalog_entry(&catalog, &skill.name).is_none())
    {
        return Err(ApplyManifestError::SkillMissing {
            skill: missing.name.clone(),
        });
    }
    for (skill, dir) in staged {
        hub.extensions()
            .install_skill(dir.path().to_path_buf(), true)
            .await
            .map_err(|error| ApplyManifestError::InstallSkill {
                skill: skill.clone(),
                source: Box::new(InstallBundleError::Install(Box::new(error))),
            })?;
        changed.push(format!("skill:{skill}"));
    }
    let catalog = hub
        .extensions()
        .skill_catalog()
        .await
        .map_err(|error| ApplyManifestError::Catalog(Box::new(error)))?;
    for skill in skills {
        let Some(entry) = catalog_entry(&catalog, &skill.name) else {
            return Err(ApplyManifestError::SkillMissing {
                skill: skill.name.clone(),
            });
        };
        if !entry.enabled {
            hub.extensions()
                .set_skill_enabled(&entry.id, true)
                .await
                .map_err(|source| ApplyManifestError::EnableSkill {
                    skill: entry.id.clone(),
                    source: Box::new(source),
                })?;
            changed.push(format!("skill:{}", entry.id));
        }
    }
    hub.extensions()
        .reconcile_skills()
        .await
        .map_err(|error| ApplyManifestError::ReconcileSkills(Box::new(error)))?;
    Ok(changed)
}

fn catalog_entry<'a>(
    catalog: &'a [artisan_api::CatalogSkill],
    name: &str,
) -> Option<&'a artisan_api::CatalogSkill> {
    let suffix = format!(".{name}");
    catalog
        .iter()
        .find(|entry| entry.id == name || entry.name == name || entry.id.ends_with(&suffix))
}

async fn stage_bundle(bundle: &SkillBundle) -> Result<tempfile::TempDir, InstallBundleError> {
    use base64::Engine as _;
    let mut files = Vec::with_capacity(bundle.files.len());
    for file in &bundle.files {
        let relative = Path::new(&file.path);
        if !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(InstallBundleError::UnsafePath {
                path: file.path.clone(),
            });
        }
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&file.content_base64)
            .map_err(|source| InstallBundleError::Decode {
                path: file.path.clone(),
                source,
            })?;
        files.push((file.path.clone(), bytes));
    }
    tokio::task::spawn_blocking(move || write_bundle(files))
        .await
        .map_err(InstallBundleError::Worker)?
}

fn write_bundle(files: Vec<(String, Vec<u8>)>) -> Result<tempfile::TempDir, InstallBundleError> {
    let dir = tempfile::tempdir().map_err(InstallBundleError::Stage)?;
    for (relative, bytes) in files {
        let path = dir.path().join(&relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| InstallBundleError::Folder {
                path: relative.clone(),
                source,
            })?;
        }
        std::fs::write(&path, bytes).map_err(|source| InstallBundleError::Write {
            path: relative,
            source,
        })?;
    }
    Ok(dir)
}

fn mcp_destinations(
    servers: &[ManifestMcpServer],
) -> Result<Vec<artisan_api::McpServer>, ApplyMcpError> {
    let mut destinations: Vec<artisan_api::McpServer> = Vec::with_capacity(servers.len());
    for entry in servers {
        if entry.name.trim().is_empty() {
            return Err(ApplyMcpError::Unnamed);
        }
        let destination = artisan_api::McpServer {
            name: entry.name.clone(),
            command: entry.command.clone().unwrap_or_default(),
            args: entry.args.clone().unwrap_or_default(),
            url: entry.url.clone(),
            ..artisan_api::McpServer::default()
        };
        if destination.command.trim().is_empty()
            && destination
                .url
                .as_ref()
                .is_none_or(|url| url.trim().is_empty())
        {
            return Err(ApplyMcpError::TargetMissing {
                server: entry.name.clone(),
            });
        }
        if destinations
            .iter()
            .any(|placed| placed.name == destination.name)
        {
            return Err(ApplyMcpError::Duplicate {
                server: entry.name.clone(),
            });
        }
        destinations.push(destination);
    }
    Ok(destinations)
}

async fn apply_mcp(
    store: &forge_store::Store,
    destinations: Vec<artisan_api::McpServer>,
    data_dir: &Path,
    file: &SyncFile,
) -> Result<Applied, ApplyMcpError> {
    let synced_before = &file.claimed_mcp_servers;
    let current: Vec<artisan_api::McpServer> = store
        .setting("mcp_servers")
        .await
        .map_err(ApplyMcpError::Read)?
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApplyMcpError::Decode)?
        .unwrap_or_default();
    let mut applied = Applied {
        changed: Vec::new(),
        placed: Vec::new(),
        kept_local: Vec::new(),
    };
    let mut mutations = Vec::new();
    for destination in &destinations {
        let previous = current
            .iter()
            .find(|server| server.name == destination.name);
        let server = match (previous, synced_before.contains(&destination.name)) {
            (Some(_), false) => {
                applied.kept_local.push(LocalMcpServerKept {
                    name: destination.name.clone(),
                });
                continue;
            }
            (Some(previous), true) => rebind(previous, destination.clone()),
            (None, _) => destination.clone(),
        };
        applied.placed.push(destination.name.clone());
        if previous != Some(&server) {
            applied.changed.push(format!("mcp:{}", destination.name));
            mutations.push(artisan_api::McpMutation::Upsert {
                previous: previous.cloned(),
                server,
            });
        }
    }
    for server in current.iter().filter(|server| {
        synced_before.contains(&server.name)
            && !destinations
                .iter()
                .any(|destination| destination.name == server.name)
    }) {
        applied.changed.push(format!("mcp-removed:{}", server.name));
        mutations.push(artisan_api::McpMutation::Remove {
            previous: server.clone(),
        });
    }
    if mutations.is_empty() {
        return Ok(applied);
    }
    let claimed: Vec<String> = synced_before
        .iter()
        .chain(&applied.placed)
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if &claimed != synced_before {
        save_state(
            data_dir,
            SyncFile {
                applied: file.applied.clone(),
                claimed_mcp_servers: claimed.clone(),
            },
        )
        .await
        .map_err(|source| ApplyMcpError::Claim {
            servers: claimed,
            source,
        })?;
    }
    store
        .mutate_mcp_together(mutations)
        .await
        .map_err(ApplyMcpError::Mutate)?;
    Ok(applied)
}

fn rebind(
    previous: &artisan_api::McpServer,
    destination: artisan_api::McpServer,
) -> artisan_api::McpServer {
    if same_destination(previous, &destination) {
        artisan_api::McpServer {
            env: previous.env.clone(),
            headers: previous.headers.clone(),
            sse: previous.sse,
            ..destination
        }
    } else {
        artisan_api::McpServer {
            sse: previous.sse,
            ..destination
        }
    }
}

fn same_destination(previous: &artisan_api::McpServer, next: &artisan_api::McpServer) -> bool {
    match (previous.url.as_deref(), next.url.as_deref()) {
        (Some(previous), Some(next)) => match (url::Url::parse(previous), url::Url::parse(next)) {
            (Ok(previous), Ok(next)) => previous.origin() == next.origin(),
            (Err(_), _) | (_, Err(_)) => false,
        },
        (None, None) => previous.command == next.command,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

#[derive(Clone, Debug)]
pub struct MachineIdentity {
    pub machine_id: String,
    pub account_id: Option<String>,
    pub secret: [u8; 32],
}

pub fn load_machine_identity(
    data_dir: &Path,
) -> Result<Option<MachineIdentity>, MachineIdentityError> {
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
        Err(source) => return Err(MachineIdentityError::Read { path, source }),
    };
    let file: AccountFile = match serde_json::from_slice(&bytes) {
        Ok(file) => file,
        Err(source) => return Err(MachineIdentityError::Parse { path, source }),
    };
    let (Some(secret_b64u), Some(machine_id)) = (file.secret_b64u, file.machine_id) else {
        return Ok(None);
    };
    let decoded =
        match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(secret_b64u.as_bytes()) {
            Ok(decoded) => decoded,
            Err(source) => return Err(MachineIdentityError::Secret { path, source }),
        };
    let Ok(secret) = <[u8; 32]>::try_from(decoded.as_slice()) else {
        return Err(MachineIdentityError::SecretLength {
            path,
            bytes: decoded.len(),
        });
    };
    Ok(Some(MachineIdentity {
        machine_id,
        account_id: file.account_id,
        secret,
    }))
}

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
            tracing::warn!(error = %error, "config sync off");
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
                if let ConfigOutcome::Updated { state, kept_local } = outcome {
                    tracing::info!(version = state.version, "applied config bundle");
                    for kept in kept_local {
                        tracing::warn!(
                            server = %kept.name,
                            "kept the MCP server created on this machine over the account's server of the same name"
                        );
                    }
                }
            }
            Err(error) if failing => {
                tracing::debug!(error = diagnostics::chain(&error), "config sync failed");
            }
            Err(error) => {
                failing = true;
                tracing::warn!(error = diagnostics::chain(&error), "config sync failed");
            }
        }
        tokio::select! {
            () = shutdown.cancelled() => break,
            () = tokio::time::sleep(interval) => {},
        }
    }
}

#[cfg(test)]
#[path = "../tests/sync.rs"]
mod sync_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test fixtures; failures fail the test")]
#[path = "../tests/sync/unit.rs"]
mod tests;
