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

mod prompt;
mod skills;

pub use prompt::{
    PromptApplied, PromptFile, apply_prompt, normalize_prompt, prompt_digest, read_prompt_file,
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

/// A stdio server (`command`, `args`, `env`) or, with `url`, an HTTP one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

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
            Entry::Ours(_) => remove_entry(&link)?,
            Entry::Missing => {}
        }
        place(&target, &link)?;
    }
    Ok(())
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
            fs::write(link.join(COPY_MARKER), target.to_string_lossy().as_bytes())
        }
        Err(error) => Err(error),
    }
}

/// Copies a folder; files come out writable even if the source was not.
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let destination = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &destination)?;
        } else if kind.is_file() {
            fs::write(&destination, fs::read(entry.path())?)?;
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
            for server in servers {
                launch.args.push("-c".to_owned());
                launch.args.push(format!(
                    "mcp_servers.{}={}",
                    codex_key(&server.name),
                    codex_server(server)
                ));
            }
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

/// Codex splits `-c` paths on dots, so a key can't be quoted.
fn codex_key(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A TOML inline table. JSON strings are valid TOML basic strings.
fn codex_server(server: &McpServer) -> String {
    let quote = |text: &str| Value::from(text).to_string();
    if let Some(url) = &server.url {
        return format!("{{url={}}}", quote(url));
    }
    let args: Vec<String> = server.args.iter().map(|arg| quote(arg)).collect();
    let env: Vec<String> = server
        .env
        .iter()
        .map(|(key, value)| format!("{}={}", quote(key), quote(value)))
        .collect();
    format!(
        "{{command={},args=[{}],env={{{}}}}}",
        quote(&server.command),
        args.join(","),
        env.join(",")
    )
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

/// One-time import of real skill folders already in engine homes into
/// `skills_dir` (copies; a name already there wins). Returns the names
/// imported.
///
/// # Errors
///
/// When a folder can't be copied into `skills_dir`.
pub fn import_skills(
    engine_homes: &[(EngineId, PathBuf)],
    skills_dir: &Path,
) -> Result<Vec<String>> {
    let mut imported = Vec::new();
    for (engine, home) in engine_homes {
        let Some(paths) = paths(*engine, home) else {
            continue;
        };
        let Ok(entries) = fs::read_dir(&paths.skills) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Only real folders: links are Forge's or point somewhere the user manages.
            let real_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if name.starts_with('.') || !real_dir || !entry.path().join("SKILL.md").is_file() {
                continue;
            }
            if import_folder(&entry.path(), skills_dir, &name)? {
                imported.push(name);
            }
        }
    }
    Ok(imported)
}

/// Best-effort import of skills from the old content-addressed store
/// (`<old data>/extensions/store/<digest>/`), named by their `SKILL.md`.
/// Skills that fail to copy are logged and skipped.
///
/// # Errors
///
/// Never at present; kept fallible for the caller's `?`.
pub fn import_legacy_extensions(old_data_dir: &Path, skills_dir: &Path) -> Result<Vec<String>> {
    let store = old_data_dir.join("extensions/store");
    let Ok(entries) = fs::read_dir(&store) else {
        return Ok(Vec::new());
    };
    let mut imported = Vec::new();
    for entry in entries.flatten() {
        let Ok(text) = fs::read_to_string(entry.path().join("SKILL.md")) else {
            continue;
        };
        let Some(name) = frontmatter_name(&text) else {
            tracing::info!(folder = %entry.path().display(), "old skill without a usable name; skipped");
            continue;
        };
        match import_folder(&entry.path(), skills_dir, &name) {
            Ok(true) => imported.push(name),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(skill = %name, error = %format!("{error:#}"), "couldn't import an old skill")
            }
        }
    }
    Ok(imported)
}

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

fn frontmatter_field(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let value = lines
        .take_while(|line| line.trim() != "---")
        .find_map(|line| line.strip_prefix(key))?;
    let value = value.trim().trim_matches(['"', '\'']);
    (!value.is_empty()).then(|| value.to_owned())
}

/// `name:` from YAML frontmatter, if it is a single safe path component.
fn frontmatter_name(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let name = lines
        .take_while(|line| line.trim() != "---")
        .find_map(|line| line.strip_prefix("name:"))?
        .trim()
        .trim_matches(['"', '\''])
        .to_owned();
    let safe = !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', ':']);
    safe.then_some(name)
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

        let homes = [(EngineId::Claude, claude), (EngineId::Codex, codex)];
        let first = import_skills(&homes, &skills_dir).unwrap();
        let again = import_skills(&homes, &skills_dir).unwrap();
        let legacy = import_legacy_extensions(&old, &skills_dir).unwrap();
        assert_eq!(first, ["design"]);
        assert!(again.is_empty());
        assert_eq!(legacy, ["legacy"]);
        assert_eq!(listing(&skills_dir, root), ["design/", "kept/", "legacy/"]);
        assert!(skills_dir.join("design/SKILL.md").is_file());
    }
}
