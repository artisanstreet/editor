//! Skills, MCP servers and the global prompt (§2.10a).
//!
//! Forge's data is the source of truth. [`materialize`] runs right before
//! every engine spawn: it writes the prompt file, links each skill folder
//! into the engine home, removes links Forge placed for skills that no
//! longer exist, and returns the launch arguments that carry MCP servers.
//! It never edits files engines rewrite themselves (`.claude.json`,
//! `config.toml`) and never touches a skill folder the user put there.

use crate::Launch;
use anyhow::{Context, Result};
use artisan_api::EngineId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(feature = "legacy-import")]
mod legacy;
#[cfg(feature = "legacy-import")]
pub use legacy::import_legacy_extensions;
mod prompt;
mod skills;

pub use prompt::{
    PromptApplied, PromptFile, apply_prompt, inspect_prompt, normalize_prompt, prompt_digest,
    read_prompt_file,
};
pub use skills::{adopt_skill, enabled_skills, skill_catalog};

/// Marks a skill folder Forge copied (where symlinks are unavailable).
const COPY_MARKER: &str = ".artisan-skill";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Customisation {
    /// The global prompt; empty removes the prompt file. `None` while the
    /// user hasn't chosen one: the engines' files are left as they are.
    pub prompt: Option<String>,
    pub mcp_servers: Vec<McpServer>,
    /// `<data>/extensions/skills`: one plain folder per skill.
    pub skills_dir: PathBuf,
    /// Whether Artisan's own skills go into the engines.
    #[serde(default)]
    pub first_party: bool,
    /// Skill ids (`artisan.<name>`, `local.<folder>`) the user turned off.
    #[serde(default)]
    pub disabled_skills: BTreeSet<String>,
}

pub use artisan_api::McpServer;

struct Paths {
    prompt: PathBuf,
    /// Prompt files that would shadow ours.
    shadows: Vec<PathBuf>,
    skills: PathBuf,
}

fn paths(engine: EngineId, home: &Path) -> Option<Paths> {
    let (config, prompt) = match engine {
        EngineId::Claude => (home.join(".claude"), "CLAUDE.md"),
        EngineId::Codex => (home.join(".codex"), "AGENTS.md"),
        EngineId::Grok => (home.join(".grok"), "AGENTS.md"),
        EngineId::OpenCode => (home.join("config/opencode"), "AGENTS.md"),
        EngineId::Cursor => return None,
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

/// Explicit skill mutation/install boundary; catalog reads never call this.
pub fn reconcile_skills(homes: &[(EngineId, Option<PathBuf>)], c: &Customisation) -> Result<()> {
    let wanted = skills::wanted(c)?;
    let owned = skills::owned_roots(&c.skills_dir);
    for (engine, home) in homes {
        if let Some(paths) = home.as_deref().and_then(|home| paths(*engine, home)) {
            sync_skills(&paths.skills, &wanted, &owned)
                .with_context(|| format!("linking {engine} skills"))?;
        }
    }
    Ok(())
}

/// Brings `home` in line with `c` and returns how to pass MCP servers.
///
/// # Errors
///
/// When the prompt, a skill link or the MCP config file can't be written.
pub fn materialize(engine: EngineId, home: &Path, c: &Customisation) -> Result<Launch> {
    if let Some(paths) = paths(engine, home) {
        if let Some(prompt) = &c.prompt {
            match apply_prompt(engine, home, prompt, false) {
                Some(PromptApplied::Drifted(_)) => {
                    tracing::info!(%engine, file = %paths.prompt.display(), "the global prompt was edited outside Artisan; leaving it until the user decides");
                }
                Some(PromptApplied::Failed(reason)) => {
                    tracing::warn!(%engine, file = %paths.prompt.display(), reason = %reason, "couldn't write the global prompt");
                }
                Some(PromptApplied::Synced) | None => {}
            }
        }
        let wanted =
            skills::wanted(c).with_context(|| format!("preparing the skills for {engine}"))?;
        sync_skills(&paths.skills, &wanted, &skills::owned_roots(&c.skills_dir))
            .with_context(|| format!("linking {engine} skills into {}", paths.skills.display()))?;
    }
    launch(engine, home, &c.mcp_servers).with_context(|| format!("passing MCP servers to {engine}"))
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Engines read these files while other runs start; never show a half write.
    let temp = path.with_extension("artisan-tmp");
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Whose an entry in an engine's skills folder is.
enum Entry {
    Missing,
    /// Forge's link or copy; the link target when it is a link.
    Ours(Option<PathBuf>),
    /// The user's or the engine's; never touched.
    Theirs,
}

/// `owned` holds the folders Forge links into (see [`skills::owned_roots`]).
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

/// Links every wanted skill (name → folder) into an engine's skills folder
/// and removes Forge's links to anything else, including skills turned off.
fn sync_skills(
    engine_skills: &Path,
    wanted: &BTreeMap<String, PathBuf>,
    owned: &[PathBuf],
) -> io::Result<()> {
    if wanted.is_empty() && !engine_skills.exists() {
        return Ok(());
    }
    fs::create_dir_all(engine_skills)?;
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
            Entry::Ours(_) => remove_entry(&link)?,
            Entry::Missing => {}
        }
        place(&target, &link)?;
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
        // A symlink (to a directory, on Windows too) or a file.
        fs::remove_file(path).or_else(|_| fs::remove_dir(path))
    }
}

fn place(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(target, link);
    #[cfg(not(any(unix, windows)))]
    let linked: io::Result<()> = Err(io::ErrorKind::Unsupported.into());
    match linked {
        Ok(()) => Ok(()),
        // Windows without Developer Mode can't create symlinks.
        Err(error) if cfg!(windows) => {
            tracing::debug!(error = %error, skill = %target.display(), "symlink refused; copying the skill");
            copy_dir(target, link)?;
            {
                let (digest, _, _) = skills::measure(target)?;
                fs::write(
                    link.join(COPY_MARKER),
                    format!("{}\n{digest}", target.display()),
                )
            }
        }
        Err(error) => Err(error),
    }
}

/// Copies a folder; files come out writable even if the source was not.
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    // Validate the whole supported shape before publishing or backing up anything.
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
            permissions.set_readonly(false);
            fs::set_permissions(destination, permissions)?;
        }
    }
    Ok(())
}

// --- launch channels -------------------------------------------------------

fn launch(engine: EngineId, home: &Path, servers: &[McpServer]) -> Result<Launch> {
    let mut launch = Launch::default();
    match engine {
        EngineId::Claude => {
            // A file, not an inline argument, so env values stay out of `ps`.
            let path = home.join(".artisan/claude-mcp.json");
            if servers.is_empty() {
                remove_file_if_present(&path)?;
            } else {
                let config = json!({ "mcpServers": claude_servers(servers) });
                write_if_changed(&path, serde_json::to_string_pretty(&config)?.as_bytes())?;
                launch.args = vec![
                    "--mcp-config".to_owned(),
                    path.to_string_lossy().into_owned(),
                ];
            }
        }
        EngineId::Codex => {
            let path = home.join(".codex/config.toml");
            let mut config: toml::Table = match fs::read_to_string(&path) {
                Ok(text) => {
                    toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => toml::Table::new(),
                Err(error) => return Err(error.into()),
            };
            let mut entries = toml::Table::new();
            for server in servers {
                anyhow::ensure!(
                    !server.name.is_empty() && !entries.contains_key(&server.name),
                    "MCP server names must be nonempty and unique"
                );
                let mut entry = toml::Table::new();
                if let Some(url) = &server.url {
                    entry.insert("url".into(), url.clone().into());
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
                                .map(|(key, value)| {
                                    (key.clone(), toml::Value::String(value.clone()))
                                })
                                .collect(),
                        ),
                    );
                }
                entries.insert(server.name.clone(), entry.into());
            }
            config.insert("mcp_servers".into(), entries.into());
            write_if_changed(&path, toml::to_string_pretty(&config)?.as_bytes())?;
        }
        EngineId::OpenCode => {
            if !servers.is_empty() {
                let config = json!({ "mcp": opencode_servers(servers) });
                launch
                    .env
                    .push(("OPENCODE_CONFIG_CONTENT".to_owned(), config.to_string()));
            }
        }
        EngineId::Grok | EngineId::Cursor => launch.mcp_servers = servers.to_vec(),
    }
    Ok(launch)
}

fn claude_servers(servers: &[McpServer]) -> Map<String, Value> {
    servers
        .iter()
        .map(|server| {
            let value = match &server.url {
                Some(url) => json!({ "type": "http", "url": url }),
                None => json!({ "type": "stdio", "command": server.command, "args": server.args, "env": server.env }),
            };
            (server.name.clone(), value)
        })
        .collect()
}

fn opencode_servers(servers: &[McpServer]) -> Map<String, Value> {
    servers
        .iter()
        .map(|server| {
            let value = if let Some(url) = &server.url {
                json!({ "type": "remote", "url": url, "enabled": true })
            } else {
                let command: Vec<&str> =
                    std::iter::once(server.command.as_str()).chain(server.args.iter().map(String::as_str)).collect();
                json!({ "type": "local", "command": command, "environment": server.env, "enabled": true })
            };
            (server.name.clone(), value)
        })
        .collect()
}

/// Merges another JSON object into an `OPENCODE_CONFIG_CONTENT` value in
/// `launch` (the `OpenCode` adapter adds its agents this way).
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
                    existing.extend(more.clone())
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

// --- imports ---------------------------------------------------------------

fn import_folder(from: &Path, skills_dir: &Path, name: &str) -> Result<bool> {
    let to = skills_dir.join(name);
    if to.exists() {
        return Ok(false);
    }
    // Copy beside the destination, then rename, so a half copy never looks like a skill.
    let staging = skills_dir.join(format!(".import-{name}"));
    let _ = fs::remove_dir_all(&staging);
    copy_dir(from, &staging)
        .with_context(|| format!("copying {} into {}", from.display(), skills_dir.display()))?;
    fs::rename(&staging, &to).with_context(|| format!("placing {}", to.display()))?;
    Ok(true)
}

fn frontmatter(text: &str) -> Option<serde_yaml_ng::Value> {
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
    serde_yaml_ng::from_str(&yaml).ok()
}

fn read_skill_header(path: &Path) -> io::Result<String> {
    use std::io::{BufRead, Read};
    let file = fs::File::open(path)?;
    let mut reader = io::BufReader::new(file).take(64 * 1024);
    let mut header = String::new();
    if reader.read_line(&mut header)? == 0 || header.trim() != "---" {
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

fn skill_metadata(text: &str) -> Option<(String, String)> {
    let data = frontmatter(text)?;
    let field = |key: &str| {
        data.get(key)?
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let name = field("name")?;
    if !artisan_mentions::valid_mention_name(&name) {
        return None;
    }
    Some((name, field("description").unwrap_or_default()))
}

#[cfg(any(test, feature = "legacy-import"))]
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
mod tests {
    use super::*;

    fn skill(dir: &Path, name: &str) {
        fs::create_dir_all(dir.join(name)).unwrap();
        fs::write(
            dir.join(name).join("SKILL.md"),
            format!("---\nname: {name}\n---\nbody"),
        )
        .unwrap();
    }

    fn listing(dir: &Path, root: &Path) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| {
                        let path = entry.path();
                        let name = entry.file_name().to_string_lossy().into_owned();
                        match fs::read_link(&path) {
                            Ok(target) => format!(
                                "{name} -> {}",
                                target.strip_prefix(root).unwrap_or(&target).display()
                            ),
                            Err(_) => format!("{name}/"),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    #[cfg(unix)]
    #[test]
    fn materialize_writes_prompt_links_skills_and_passes_mcp() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let skills_dir = root.join("data/extensions/skills");
        skill(&skills_dir, "review");
        skill(&skills_dir, "deploy");
        let c = Customisation {
            prompt: Some("Be brief.".into()),
            mcp_servers: vec![
                McpServer {
                    name: "files".into(),
                    command: "npx".into(),
                    args: vec!["-y".into(), "@mcp/files".into()],
                    env: BTreeMap::from([("ROOT".into(), "/srv \"x\"".into())]),
                    url: None,
                },
                McpServer {
                    name: "docs.web".into(),
                    url: Some("https://mcp.example/docs".into()),
                    ..McpServer::default()
                },
            ],
            skills_dir: skills_dir.clone(),
            ..Customisation::default()
        };

        let mut report = Vec::new();
        for engine in EngineId::ALL {
            let home = root.join(engine.as_str());
            let paths = paths(engine, &home);
            if let Some(paths) = &paths {
                // A user's own folder, a stale Forge link, a link from the old store,
                // and a Codex override that would shadow the prompt.
                skill(&paths.skills, "mine");
                skill(&paths.skills, "deploy");
                fs::create_dir_all(&paths.skills).unwrap();
                std::os::unix::fs::symlink(skills_dir.join("gone"), paths.skills.join("gone"))
                    .unwrap();
                std::os::unix::fs::symlink(
                    root.join("data/extensions/store/abc"),
                    paths.skills.join("old"),
                )
                .unwrap();
                for shadow in &paths.shadows {
                    fs::write(shadow, "override").unwrap();
                }
            }
            let launch = materialize(engine, &home, &c).unwrap();
            let files = paths.map(|paths| {
                (
                    paths
                        .prompt
                        .strip_prefix(&home)
                        .unwrap()
                        .display()
                        .to_string(),
                    fs::read_to_string(&paths.prompt).unwrap(),
                    paths.shadows.iter().all(|shadow| !shadow.exists()),
                    listing(&paths.skills, root),
                )
            });
            let mcp_file = fs::read_to_string(home.join(".artisan/claude-mcp.json")).ok();
            let args: Vec<String> = launch
                .args
                .iter()
                .map(|arg| arg.replace(&root.display().to_string(), "<root>"))
                .collect();
            report.push((
                engine.as_str(),
                files,
                args,
                launch.env,
                launch.mcp_servers.len(),
                mcp_file,
            ));
        }
        insta::assert_debug_snapshot!(report);

        // A skill deleted in Forge disappears from the engine on the next spawn;
        // a cleared prompt removes the file.
        fs::remove_dir_all(skills_dir.join("review")).unwrap();
        let cleared = Customisation {
            prompt: Some(String::new()),
            mcp_servers: Vec::new(),
            skills_dir,
            ..Customisation::default()
        };
        let home = root.join("claude");
        let launch = materialize(EngineId::Claude, &home, &cleared).unwrap();
        assert_eq!(
            listing(&home.join(".claude/skills"), root),
            ["deploy/", "mine/"]
        );
        assert!(!home.join(".claude/CLAUDE.md").exists());
        assert!(launch.args.is_empty() && !home.join(".artisan/claude-mcp.json").exists());
    }

    #[cfg(feature = "legacy-import")]
    #[test]
    fn imports_copy_real_skill_folders_once() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let skills_dir = root.join("skills");
        skill(&skills_dir, "kept");
        let claude = root.join("claude");
        let codex = root.join("codex");
        skill(&claude.join(".claude/skills"), "design");
        skill(&claude.join(".claude/skills"), "kept");
        skill(&codex.join(".codex/skills"), "design");
        skill(&codex.join(".codex/skills/.system"), "builtin");
        fs::create_dir_all(codex.join(".codex/skills/notes")).unwrap();
        let old = root.join("old");
        skill(&old.join("extensions/store"), "digest1");
        fs::write(
            old.join("extensions/store/digest1/SKILL.md"),
            "---\nname: \"legacy\"\n---\n",
        )
        .unwrap();
        skill(&old.join("extensions/store"), "digest2");
        fs::write(
            old.join("extensions/store/digest2/SKILL.md"),
            "---\nname: ../escape\n---\n",
        )
        .unwrap();

        assert!(
            import_folder(&claude.join(".claude/skills/design"), &skills_dir, "design").unwrap()
        );
        assert!(
            !import_folder(&codex.join(".codex/skills/design"), &skills_dir, "design").unwrap()
        );
        assert!(!import_folder(&claude.join(".claude/skills/kept"), &skills_dir, "kept").unwrap());
        let legacy = import_legacy_extensions(&old, &skills_dir);
        assert_eq!(legacy, ["legacy"]);
        assert_eq!(listing(&skills_dir, root), ["design/", "kept/", "legacy/"]);
        assert!(skills_dir.join("design/SKILL.md").is_file());
    }
}

#[cfg(test)]
mod reform_tests {
    use super::*;
    #[test]
    fn yaml_frontmatter_understands_multiline_and_quoted_values() {
        let text = "---\nname: 'review'\ndescription: >\n  First line\n  second line\n---\nbody";
        assert_eq!(frontmatter_name(text).as_deref(), Some("review"));
        assert_eq!(
            frontmatter_field(text, "description:").as_deref(),
            Some("First line second line")
        );
        assert_eq!(frontmatter_name("---\nname: '../outside'\n---"), None);
    }
    #[test]
    fn codex_mcp_preserves_distinct_names_and_keeps_secrets_out_of_args() {
        let directory = tempfile::tempdir().unwrap();
        let server = |name: &str| McpServer {
            name: name.into(),
            command: "tool".into(),
            args: vec![],
            env: std::collections::BTreeMap::from([("TOKEN".into(), "private".into())]),
            url: None,
        };
        let result = launch(
            EngineId::Codex,
            directory.path(),
            &[server("docs.web"), server("docs_web")],
        )
        .unwrap();
        assert!(result.args.is_empty());
        let config: toml::Table = toml::from_str(
            &fs::read_to_string(directory.path().join(".codex/config.toml")).unwrap(),
        )
        .unwrap();
        let servers = config["mcp_servers"].as_table().unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(
            servers["docs.web"]["env"]["TOKEN"].as_str(),
            Some("private")
        );
    }
    #[cfg(unix)]
    #[test]
    fn unsupported_skill_links_are_rejected_before_copying() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("SKILL.md"), "---\nname: linked\n---").unwrap();
        std::os::unix::fs::symlink(&source, source.join("cycle")).unwrap();
        let destination = directory.path().join("copy");
        assert!(copy_dir(&source, &destination).is_err());
        assert!(!destination.exists());
        fs::remove_file(source.join("cycle")).unwrap();
        let root_link = directory.path().join("root-link");
        std::os::unix::fs::symlink(&source, &root_link).unwrap();
        assert!(copy_dir(&root_link, &destination).is_err());
        assert!(!destination.exists());
    }
}

#[cfg(test)]
mod catalog_read_tests {
    use super::*;
    #[test]
    fn browsing_catalog_does_not_materialize_engine_homes() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("engine-home");
        let settings = Customisation {
            skills_dir: directory.path().join("skills"),
            first_party: true,
            ..Customisation::default()
        };
        let _ = skill_catalog(&[(EngineId::Claude, Some(home.clone()))], &settings).unwrap();
        assert!(!home.exists());
        assert!(!settings.skills_dir.exists());
    }
}

#[cfg(test)]
mod marker_budget_tests {
    use super::*;
    #[test]
    fn oversized_copy_marker_does_not_read_unbounded_data() {
        let folder = tempfile::tempdir().unwrap();
        let link = folder.path().join("copy");
        let target = folder.path().join("target");
        fs::create_dir_all(&link).unwrap();
        fs::create_dir_all(&target).unwrap();
        let file = fs::File::create(link.join(COPY_MARKER)).unwrap();
        file.set_len(64 * 1024 + 1).unwrap();
        assert!(!copied_skill_matches(&link, &target));
    }
}

#[cfg(test)]
mod copied_reconciliation_tests {
    use super::*;
    #[test]
    fn unchanged_real_copied_skill_keeps_files_and_timestamps_on_reconciliation() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let engine = dir.path().join("engine");
        let copy = engine.join("skill");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: skill\ndescription: fixture\n---\nUse this skill.\n",
        )
        .unwrap();
        copy_dir(&source, &copy).unwrap();
        let (digest, _, _) = skills::measure(&source).unwrap();
        fs::write(
            copy.join(COPY_MARKER),
            format!("{}\n{digest}", source.display()),
        )
        .unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(12345);
        for name in ["SKILL.md", COPY_MARKER] {
            fs::File::options()
                .write(true)
                .open(copy.join(name))
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let wanted = BTreeMap::from([("skill".to_owned(), source.clone())]);
        for _ in 0..3 {
            sync_skills(&engine, &wanted, std::slice::from_ref(&source)).unwrap();
        }
        assert!(fs::symlink_metadata(&copy).unwrap().is_dir());
        assert!(
            !fs::symlink_metadata(&copy)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        for name in ["SKILL.md", COPY_MARKER] {
            assert_eq!(
                fs::metadata(copy.join(name)).unwrap().modified().unwrap(),
                old,
                "unchanged copied {name} must not be rewritten"
            );
        }
        fs::write(
            source.join("SKILL.md"),
            "---\nname: skill\ndescription: changed\n---\nChanged instructions.\n",
        )
        .unwrap();
        assert!(!copied_skill_matches(&copy, &source));
        sync_skills(&engine, &wanted, std::slice::from_ref(&source)).unwrap();
        assert_eq!(
            fs::read(copy.join("SKILL.md")).unwrap(),
            fs::read(source.join("SKILL.md")).unwrap()
        );
    }
}
