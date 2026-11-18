use crate::Launch;
use artisan_api::EngineId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

mod prompt;
mod skills;

pub use prompt::{
    ApplyPromptError, PromptApplied, PromptFile, apply_prompt, inspect_prompt, normalize_prompt,
    prompt_digest, read_prompt_file,
};
pub use skills::{
    AdoptSkillError, DiscardSkillError, FindSkillError, InstallSkillError, ReadSkillsError,
    RecoverSkillError, adopt_skill, discard_skill, enabled_skills, install_skill, skill_catalog,
};

const COPY_MARKER: &str = ".artisan-skill";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Customisation {
    pub prompt: Option<String>,
    pub mcp_servers: Vec<McpServer>,
    pub skills_dir: PathBuf,
    #[serde(default)]
    pub first_party: bool,
    #[serde(default)]
    pub disabled_skills: BTreeSet<String>,
}

pub use artisan_api::McpServer;

struct Paths {
    prompt: PathBuf,
    shadows: Vec<PathBuf>,
    skills: PathBuf,
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "None is an engine without a customisable home; every current engine has one"
)]
fn paths(engine: EngineId, home: &Path) -> Option<Paths> {
    let (config, prompt) = match engine {
        EngineId::Claude => (home.join(".claude"), "CLAUDE.md"),
        EngineId::Codex => (home.join(".codex"), "AGENTS.md"),
        EngineId::OpenCode => (home.join("config/opencode"), "AGENTS.md"),
    };
    let shadows = if engine == EngineId::Codex {
        vec![config.join("AGENTS.override.md")]
    } else {
        Vec::new()
    };
    Some(Paths {
        prompt: config.join(prompt),
        shadows,
        skills: config.join("skills"),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ReconcileSkillsError {
    #[error("prepare the skills")]
    Wanted(#[source] ReadSkillsError),
    #[error("linking {engine} skills into {}", path.display())]
    Link {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub fn reconcile_skills(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
) -> Result<(), ReconcileSkillsError> {
    let wanted = skills::wanted(c).map_err(ReconcileSkillsError::Wanted)?;
    let owned = skills::owned_roots(&c.skills_dir);
    for (engine, home) in homes {
        if let Some(paths) = home.as_deref().and_then(|home| paths(*engine, home)) {
            skills::quarantine_foreign(*engine, &paths.skills, &c.skills_dir, &owned);
            sync_skills(&paths.skills, &wanted, &owned).map_err(|source| {
                ReconcileSkillsError::Link {
                    engine: *engine,
                    path: paths.skills.clone(),
                    source,
                }
            })?;
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    #[error("bringing the global prompt in line for {engine}")]
    Prompt {
        engine: EngineId,
        #[source]
        source: ApplyPromptError,
    },
    #[error("preparing the skills for {engine}")]
    Wanted {
        engine: EngineId,
        #[source]
        source: ReadSkillsError,
    },
    #[error("linking {engine} skills into {}", path.display())]
    Link {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("passing MCP servers to {engine}")]
    Launch {
        engine: EngineId,
        #[source]
        source: LaunchError,
    },
}

pub fn materialize(
    engine: EngineId,
    home: &Path,
    c: &Customisation,
) -> Result<Launch, MaterializeError> {
    if let Some(paths) = paths(engine, home) {
        if let Some(prompt) = &c.prompt {
            match prompt::reconcile(engine, home, prompt, false) {
                Some(Ok((prompt::Reconciled::Drifted, _))) => {
                    tracing::info!(%engine, file = %paths.prompt.display(), "the global prompt was edited outside Artisan; leaving it until the user decides");
                }
                Some(Err(source)) => return Err(MaterializeError::Prompt { engine, source }),
                Some(Ok((prompt::Reconciled::Synced, _))) | None => {}
            }
        }
        let wanted =
            skills::wanted(c).map_err(|source| MaterializeError::Wanted { engine, source })?;
        let owned = skills::owned_roots(&c.skills_dir);
        skills::quarantine_foreign(engine, &paths.skills, &c.skills_dir, &owned);
        sync_skills(&paths.skills, &wanted, &owned).map_err(|source| MaterializeError::Link {
            engine,
            path: paths.skills.clone(),
            source,
        })?;
    }
    launch(engine, home, &c.mcp_servers)
        .map_err(|source| MaterializeError::Launch { engine, source })
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("artisan-tmp");
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}

fn write_private_if_changed(
    path: &Path,
    bytes: &[u8],
) -> Result<(), artisan_paths::private::PrivateFileError> {
    if fs::read(path).is_ok_and(|current| current == bytes) {
        return artisan_paths::private::restrict_file(path);
    }
    artisan_paths::private::write_file(path, bytes)
}

pub(crate) struct GatewayFile {
    path: PathBuf,
}

impl GatewayFile {
    pub(crate) fn write(
        home: &Path,
        servers: &[(String, String)],
    ) -> Result<Self, artisan_paths::private::PrivateFileError> {
        static LAUNCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let launch = LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = home
            .join(".artisan/gateway")
            .join(format!("{}-{launch}.json", std::process::id()));
        let servers: Map<String, Value> = servers
            .iter()
            .map(|(name, url)| (name.clone(), json!({ "type": "http", "url": url })))
            .collect();
        let config = json!({ "mcpServers": servers }).to_string();
        artisan_paths::private::write_file(&path, config.as_bytes())?;
        Ok(Self { path })
    }

    pub(crate) fn claude_args(&self, args: &[String]) -> Vec<String> {
        let path = self.path.to_string_lossy().into_owned();
        let mut args = args.to_vec();
        match args.iter().position(|arg| arg == "--mcp-config") {
            Some(flag) => args.insert((flag + 2).min(args.len()), path),
            None => args.extend(["--mcp-config".to_owned(), path]),
        }
        args
    }
}

impl Drop for GatewayFile {
    fn drop(&mut self) {
        if let Err(failure) = remove_file_if_present(&self.path) {
            let error: &dyn std::error::Error = &failure;
            tracing::warn!(path = %self.path.display(), error, "couldn't remove a finished launch's gateway file");
        }
    }
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn discard_leftover(path: &Path) {
    if let Err(error) = fs::remove_dir_all(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "couldn't remove a leftover skill folder");
    }
}

enum Entry {
    Missing,
    Ours(Option<PathBuf>),
    Theirs,
}

fn inspect(path: &Path, owned: &[PathBuf]) -> Entry {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Entry::Missing;
    };
    if metadata.file_type().is_symlink() {
        let Ok(target) = fs::read_link(path) else {
            return Entry::Theirs;
        };
        if owned.iter().any(|root| target.starts_with(root)) {
            return Entry::Ours(Some(target));
        }
        return Entry::Theirs;
    }
    if metadata.is_dir() && path.join(COPY_MARKER).is_file() {
        return Entry::Ours(None);
    }
    Entry::Theirs
}

fn sync_skills(
    engine_skills: &Path,
    wanted: &BTreeMap<String, PathBuf>,
    owned: &[PathBuf],
) -> io::Result<()> {
    if wanted.is_empty() && !engine_skills.exists() {
        return Ok(());
    }
    fs::create_dir_all(engine_skills)?;
    let staging = staging_root(engine_skills);
    discard_other_processes_leftovers(&staging);
    for entry in fs::read_dir(engine_skills)? {
        let path = entry?.path();
        let keep = path
            .file_name()
            .is_some_and(|name| wanted.contains_key(&*name.to_string_lossy()));
        if !keep && let Entry::Ours(_) = inspect(&path, owned) {
            remove_entry(&path)?;
            tracing::debug!(link = %path.display(), "removed a stale skill link");
        }
    }
    for (name, target) in wanted {
        let target = target.clone();
        let link = engine_skills.join(name);
        match inspect(&link, owned) {
            Entry::Theirs => {
                tracing::debug!(skill = %name, folder = %link.display(), "a skill folder of the user's has this name; leaving it");
                continue;
            }
            Entry::Ours(Some(current)) if current == target => continue,
            Entry::Ours(None) if copied_skill_matches(&link, &target) => continue,
            Entry::Ours(_) | Entry::Missing => {}
        }
        place(&target, &link, &staging)?;
    }
    Ok(())
}

fn copied_skill_matches(link: &Path, target: &Path) -> bool {
    use std::io::Read as _;
    let path = link.join(COPY_MARKER);
    let Ok(metadata) = fs::metadata(&path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return false;
    }
    let Ok(file) = fs::File::open(&path) else {
        return false;
    };
    let mut marker = String::new();
    if file
        .take(64 * 1024 + 1)
        .read_to_string(&mut marker)
        .is_err()
        || marker.len() > 64 * 1024
    {
        return false;
    }

    let Ok((digest, _, _)) = skills::measure_inner(target, true) else {
        return false;
    };
    marker == format!("{}\n{digest}", target.display())
}

fn remove_entry(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path).or_else(|_| fs::remove_dir(path))
    }
}

fn staging_root(engine_skills: &Path) -> PathBuf {
    engine_skills.with_file_name(".artisan-skills-staging")
}

fn staged_name(kind: &str, name: &std::ffi::OsStr) -> String {
    static STAGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        ".{kind}-{}-{}-{}",
        std::process::id(),
        STAGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        name.to_string_lossy()
    )
}

fn other_process_leftover<'name>(file_name: &'name str, kind: &str) -> Option<&'name str> {
    let rest = file_name
        .strip_prefix('.')?
        .strip_prefix(kind)?
        .strip_prefix('-')?;
    let (process, rest) = rest.split_once('-')?;
    let (_, name) = rest.split_once('-')?;
    let process: u32 = process.parse().ok()?;
    (process != std::process::id()).then_some(name)
}

fn discard_other_processes_leftovers(staging: &Path) {
    let entries = match fs::read_dir(staging) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(path = %staging.display(), %error, "couldn't look for leftover staged skills");
            return;
        }
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let leftover = ["prepared", "copy", "retired"]
            .iter()
            .any(|kind| other_process_leftover(&file_name, kind).is_some());
        if leftover && let Err(error) = remove_entry(&entry.path()) {
            tracing::warn!(path = %entry.path().display(), %error, "couldn't remove a leftover staged skill");
        }
    }
}

fn link_name(link: &Path) -> &std::ffi::OsStr {
    link.file_name()
        .expect("invariant: a skill entry is <skills folder>/<name>")
}

fn swap_in(prepared: &Path, destination: &Path, retired: &Path) -> io::Result<()> {
    match fs::rename(destination, retired) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return fs::rename(prepared, destination);
        }
        Err(error) => return Err(error),
    }
    if let Err(error) = fs::rename(prepared, destination) {
        return match fs::rename(retired, destination) {
            Ok(()) => Err(error),
            Err(restore) => Err(io::Error::new(
                error.kind(),
                format!(
                    "{error}; the previous copy stays at {}: {restore}",
                    retired.display()
                ),
            )),
        };
    }
    if let Err(error) = remove_entry(retired) {
        tracing::warn!(path = %retired.display(), %error, "couldn't remove a replaced skill copy");
    }
    Ok(())
}

fn publish(prepared: &Path, link: &Path, staging: &Path) -> io::Result<()> {
    let retired = staging.join(staged_name("retired", link_name(link)));
    swap_in(prepared, link, &retired).inspect_err(|_| {
        if let Err(error) = remove_entry(prepared) {
            tracing::warn!(path = %prepared.display(), %error, "couldn't remove a staged skill");
        }
    })
}

fn place(target: &Path, link: &Path, staging: &Path) -> io::Result<()> {
    fs::create_dir_all(staging)?;
    let prepared = staging.join(staged_name("prepared", link_name(link)));
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(target, &prepared);
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(target, &prepared);
    #[cfg(not(any(unix, windows)))]
    let linked: io::Result<()> = Err(io::ErrorKind::Unsupported.into());
    match linked {
        Ok(()) => publish(&prepared, link, staging),
        Err(error) if cfg!(windows) => {
            tracing::debug!(error = %error, skill = %target.display(), "symlink refused; copying the skill");
            publish_copy(target, link, staging)
        }
        Err(error) => Err(error),
    }
}

fn publish_copy(target: &Path, link: &Path, staging: &Path) -> io::Result<()> {
    fs::create_dir_all(staging)?;
    let prepared = staging.join(staged_name("copy", link_name(link)));
    let copied = copy_dir(target, &prepared).and_then(|()| {
        let (digest, _, _) = skills::measure(target)?;
        fs::write(
            prepared.join(COPY_MARKER),
            format!("{}\n{digest}", target.display()),
        )
    });
    if let Err(error) = copied {
        discard_leftover(&prepared);
        return Err(error);
    }
    publish(&prepared, link, staging)
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    let entries = skills::entries(from)?;
    fs::create_dir_all(to)?;
    for entry in entries {
        let relative = entry.path().strip_prefix(from).map_err(io::Error::other)?;
        let destination = to.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(destination)?;
        } else if entry.file_type().is_file() {
            fs::copy(entry.path(), &destination)?;
            let mut permissions = fs::metadata(&destination)?.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                permissions.set_mode(permissions.mode() | 0o200);
            }
            #[cfg(not(unix))]
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "Windows has no Unix mode bits; clearing readonly is the correct API there"
            )]
            permissions.set_readonly(false);
            fs::set_permissions(destination, permissions)?;
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("remove {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("encode {}", path.display())]
    EncodeJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("encode {}", path.display())]
    EncodeToml {
        path: PathBuf,
        #[source]
        source: toml::ser::Error,
    },
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: artisan_paths::private::PrivateFileError,
    },
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("reading {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("MCP server names must be nonempty and unique: {name:?}")]
    ServerName { name: String },
}

fn codex_server(server: &McpServer) -> toml::Table {
    let mut entry = toml::Table::new();
    if let Some(url) = &server.url {
        entry.insert("url".into(), url.clone().into());
        if !server.headers.is_empty() {
            entry.insert(
                "http_headers".into(),
                toml::Value::Table(
                    server
                        .headers
                        .iter()
                        .map(|(key, value)| (key.clone(), toml::Value::String(value.clone())))
                        .collect(),
                ),
            );
        }
    } else {
        entry.insert("command".into(), server.command.clone().into());
        entry.insert(
            "args".into(),
            toml::Value::Array(
                server
                    .args
                    .iter()
                    .cloned()
                    .map(toml::Value::String)
                    .collect(),
            ),
        );
        entry.insert(
            "env".into(),
            toml::Value::Table(
                server
                    .env
                    .iter()
                    .map(|(key, value)| (key.clone(), toml::Value::String(value.clone())))
                    .collect(),
            ),
        );
    }
    entry
}

fn launch(engine: EngineId, home: &Path, servers: &[McpServer]) -> Result<Launch, LaunchError> {
    let mut launch = Launch {
        mcp_servers: servers.to_vec(),
        ..Launch::default()
    };
    match engine {
        EngineId::Claude => {
            let path = home.join(".artisan/claude-mcp.json");
            if servers.is_empty() {
                remove_file_if_present(&path).map_err(|source| LaunchError::Remove {
                    path: path.clone(),
                    source,
                })?;
            } else {
                let config = json!({ "mcpServers": claude_servers(servers) });
                let text = serde_json::to_string_pretty(&config).map_err(|source| {
                    LaunchError::EncodeJson {
                        path: path.clone(),
                        source,
                    }
                })?;
                write_private_if_changed(&path, text.as_bytes()).map_err(|source| {
                    LaunchError::Write {
                        path: path.clone(),
                        source,
                    }
                })?;
                launch.args = vec![
                    "--mcp-config".to_owned(),
                    path.to_string_lossy().into_owned(),
                ];
            }
        }
        EngineId::Codex => {
            let path = home.join(".codex/config.toml");
            let mut config: toml::Table = match fs::read_to_string(&path) {
                Ok(text) => toml::from_str(&text).map_err(|source| LaunchError::Parse {
                    path: path.clone(),
                    source,
                })?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => toml::Table::new(),
                Err(source) => return Err(LaunchError::Read { path, source }),
            };
            let mut entries = toml::Table::new();
            for server in servers {
                if server.name.is_empty() || entries.contains_key(&server.name) {
                    return Err(LaunchError::ServerName {
                        name: server.name.clone(),
                    });
                }
                entries.insert(server.name.clone(), codex_server(server).into());
            }
            config.insert("mcp_servers".into(), entries.into());
            let text =
                toml::to_string_pretty(&config).map_err(|source| LaunchError::EncodeToml {
                    path: path.clone(),
                    source,
                })?;
            write_private_if_changed(&path, text.as_bytes())
                .map_err(|source| LaunchError::Write { path, source })?;
        }
        EngineId::OpenCode => {
            if !servers.is_empty() {
                let config = json!({ "mcp": opencode_servers(servers) });
                launch
                    .env
                    .push(("OPENCODE_CONFIG_CONTENT".to_owned(), config.to_string()));
            }
        }
    }
    Ok(launch)
}

pub const ARTISAN_MCP: &str = "artisan";

pub const COMPUTER_MCP: &str = "computer";

pub fn with_gateway_mcp(engine: EngineId, launch: &mut Launch, name: &str, url: &str) {
    launch.mcp_servers.push(McpServer {
        name: name.to_owned(),
        url: Some(url.to_owned()),
        ..McpServer::default()
    });
    match engine {
        EngineId::Claude | EngineId::Codex => {
            launch.gateway_mcp.push((name.to_owned(), url.to_owned()));
        }
        EngineId::OpenCode => merge_opencode_config(
            launch,
            &json!({ "mcp": { name: { "type": "remote", "url": url, "enabled": true } } }),
        ),
    }
}

fn claude_servers(servers: &[McpServer]) -> Map<String, Value> {
    servers
        .iter()
        .map(|server| {
            let value = server.url.as_ref().map_or_else(
                || json!({ "type": "stdio", "command": server.command, "args": server.args, "env": server.env }),
                |url| {
                    let mut value = json!({ "type": if server.sse { "sse" } else { "http" }, "url": url });
                    if !server.headers.is_empty() {
                        value["headers"] = json!(server.headers);
                    }
                    value
                },
            );
            (server.name.clone(), value)
        })
        .collect()
}

fn opencode_servers(servers: &[McpServer]) -> Map<String, Value> {
    servers
        .iter()
        .map(|server| {
            let value = server.url.as_ref().map_or_else(
                || {
                    let command: Vec<&str> = std::iter::once(server.command.as_str())
                        .chain(server.args.iter().map(String::as_str))
                        .collect();
                    json!({ "type": "local", "command": command, "environment": server.env, "enabled": true })
                },
                |url| {
                    let mut value = json!({ "type": "remote", "url": url, "enabled": true });
                    if !server.headers.is_empty() {
                        value["headers"] = json!(server.headers);
                    }
                    value
                },
            );
            (server.name.clone(), value)
        })
        .collect()
}

pub fn merge_opencode_config(launch: &mut Launch, extra: &Value) {
    let index = launch
        .env
        .iter()
        .position(|(key, _)| key == "OPENCODE_CONFIG_CONTENT");
    let mut config = index
        .and_then(|index| serde_json::from_str::<Value>(&launch.env[index].1).ok())
        .unwrap_or_else(|| json!({}));
    if let (Some(config), Some(extra)) = (config.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            match (config.get_mut(key), value) {
                (Some(Value::Object(existing)), Value::Object(more)) => {
                    existing.extend(more.clone());
                }
                _ => {
                    config.insert(key.clone(), value.clone());
                }
            }
        }
    }
    let entry = ("OPENCODE_CONFIG_CONTENT".to_owned(), config.to_string());
    match index {
        Some(index) => launch.env[index] = entry,
        None => launch.env.push(entry),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("copying {} into {}", from.display(), to.display())]
    Copy {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("placing {}", path.display())]
    Place {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn import_folder(from: &Path, skills_dir: &Path, name: &str) -> Result<bool, ImportError> {
    let to = skills_dir.join(name);
    if to.exists() {
        return Ok(false);
    }
    let staging = skills_dir.join(format!(".import-{name}"));
    discard_leftover(&staging);
    copy_dir(from, &staging).map_err(|source| ImportError::Copy {
        from: from.to_owned(),
        to: skills_dir.to_owned(),
        source,
    })?;
    fs::rename(&staging, &to).map_err(|source| ImportError::Place { path: to, source })?;
    Ok(true)
}

fn frontmatter(text: &str) -> Option<serde_yaml_ng::Value> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut yaml = String::new();
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        if yaml.len() + line.len() > 64 * 1024 {
            return None;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !closed {
        return None;
    }
    serde_yaml_ng::from_str(&yaml)
        .ok()
        .or_else(|| lenient_frontmatter(&yaml))
}

fn lenient_frontmatter(yaml: &str) -> Option<serde_yaml_ng::Value> {
    let mut map = serde_yaml_ng::Mapping::new();
    for line in yaml.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.is_empty() || key.starts_with(char::is_whitespace) {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        map.entry(key.into()).or_insert_with(|| value.into());
    }
    (!map.is_empty()).then_some(serde_yaml_ng::Value::Mapping(map))
}

fn read_skill_header(path: &Path) -> io::Result<String> {
    use std::io::{BufRead, Read};
    let file = fs::File::open(path)?;
    let mut reader = io::BufReader::new(file).take(64 * 1024);
    let mut header = String::new();
    if reader.read_line(&mut header)? == 0
        || header.strip_prefix('\u{feff}').unwrap_or(&header).trim() != "---"
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "skill lacks YAML frontmatter",
        ));
    }
    loop {
        let start = header.len();
        if reader.read_line(&mut header)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "skill metadata is unclosed or exceeds 64 KiB",
            ));
        }
        if header[start..].trim() == "---" {
            return Ok(header);
        }
    }
}

pub fn skill_name(text: &str) -> Option<String> {
    skill_metadata(text).map(|(name, _)| name)
}

fn skill_metadata(text: &str) -> Option<(String, String)> {
    skill_metadata_in(text, None)
}

fn skill_metadata_in(text: &str, folder: Option<&str>) -> Option<(String, String)> {
    let data = frontmatter(text)?;
    let field = |key: &str| {
        data.get(key)?
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let name = match data.get("name") {
        Some(_) => field("name")?,
        None => folder?.to_owned(),
    };
    if !artisan_mentions::valid_mention_name(&name) {
        return None;
    }
    Some((name, field("description").unwrap_or_default()))
}

#[cfg(test)]
fn frontmatter_name(text: &str) -> Option<String> {
    skill_metadata(text).map(|(name, _)| name)
}

#[cfg(test)]
fn frontmatter_field(text: &str, key: &str) -> Option<String> {
    frontmatter(text)?
        .get(key.trim_end_matches(':'))?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "../tests/customise/unit.rs"]
mod tests;

#[cfg(test)]
#[cfg(unix)]
#[path = "../tests/customise/launch.rs"]
mod launch_tests;

#[cfg(test)]
#[path = "../tests/customise/reform.rs"]
mod reform_tests;

#[cfg(test)]
#[path = "../tests/customise/catalog_read.rs"]
mod catalog_read_tests;

#[cfg(test)]
#[path = "../tests/customise/marker_budget.rs"]
mod marker_budget_tests;

#[cfg(test)]
#[path = "../tests/customise/copied_reconciliation.rs"]
mod copied_reconciliation_tests;
