//! What Claude Code reports it can use, read from its `system/init` frame.
//!
//! The frame names every user-invocable skill (`plugin:skill` for plugin
//! skills), the installed plugins with their folders, and every MCP server
//! with its connection status. Descriptions are not in the frame: they come
//! from each skill's `SKILL.md`, looked up where Claude Code loads skills
//! from (the project's `.claude/skills`, the config dir's `skills`, and each
//! plugin's `skills`). A skill found in none of them shipped with the CLI.

use std::path::{Path, PathBuf};

use artisan_domain::{
    ConnectorStatus, INVENTORY_ITEMS_MAX, InventoryConnector, InventorySkill, InventorySkillSource,
    valid_inventory_name,
};
use serde_json::Value;

use crate::engine_inventory::{ConnectorReport, InventoryReport};

/// Largest `SKILL.md` read for its description.
const SKILL_FILE_MAX_BYTES: u64 = 256 * 1024;

/// Where Claude Code looks for skills in this run.
#[derive(Clone, Debug)]
pub(crate) struct ClaudeSkillRoots {
    /// `CLAUDE_CONFIG_DIR`, when the launch sets it.
    pub(crate) config_dir: Option<PathBuf>,
    /// The run's project root.
    pub(crate) project_root: PathBuf,
}

/// Reads the inventory from one `system/init` line, or `None` for any other
/// line. Touches the file system for descriptions, so call it off the async
/// executor.
pub(crate) fn init_inventory(line: &str, roots: &ClaudeSkillRoots) -> Option<InventoryReport> {
    let frame: Value = serde_json::from_str(line).ok()?;
    if frame.get("type")?.as_str()? != "system" || frame.get("subtype")?.as_str()? != "init" {
        return None;
    }
    let plugins = frame
        .get("plugins")
        .and_then(Value::as_array)
        .map(|plugins| {
            plugins
                .iter()
                .filter_map(|plugin| {
                    Some((
                        plugin.get("name")?.as_str()?.to_owned(),
                        PathBuf::from(plugin.get("path")?.as_str()?),
                    ))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let skills = frame.get("skills").and_then(Value::as_array).map(|skills| {
        skills
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| valid_inventory_name(name))
            .take(INVENTORY_ITEMS_MAX)
            .map(|name| skill(name, roots, &plugins))
            .collect::<Vec<_>>()
    });
    let connectors = frame.get("mcp_servers").and_then(Value::as_array).map_or(
        ConnectorReport::Unchanged,
        |servers| {
            ConnectorReport::Replace(
                servers
                    .iter()
                    .filter_map(|server| {
                        let name = server.get("name")?.as_str()?;
                        valid_inventory_name(name).then(|| InventoryConnector {
                            name: name.to_owned(),
                            status: connector_status(
                                server.get("status").and_then(Value::as_str).unwrap_or(""),
                            ),
                            plugin: None,
                        })
                    })
                    .take(INVENTORY_ITEMS_MAX)
                    .collect(),
            )
        },
    );
    Some(InventoryReport { skills, connectors })
}

fn skill(name: &str, roots: &ClaudeSkillRoots, plugins: &[(String, PathBuf)]) -> InventorySkill {
    if let Some((plugin, skill)) = name.split_once(':') {
        let description = plugins
            .iter()
            .find(|(known, _)| known == plugin)
            .and_then(|(_, root)| description_at(&root.join("skills").join(skill)))
            .unwrap_or_default();
        return InventorySkill {
            name: name.to_owned(),
            description,
            source: InventorySkillSource::Plugin,
            plugin: valid_inventory_name(plugin).then(|| plugin.to_owned()),
        };
    }
    let project = roots.project_root.join(".claude").join("skills").join(name);
    if let Some(description) = description_at(&project) {
        return InventorySkill {
            name: name.to_owned(),
            description,
            source: InventorySkillSource::Project,
            plugin: None,
        };
    }
    if let Some(description) = roots
        .config_dir
        .as_ref()
        .and_then(|config| description_at(&config.join("skills").join(name)))
    {
        return InventorySkill {
            name: name.to_owned(),
            description,
            source: InventorySkillSource::User,
            plugin: None,
        };
    }
    InventorySkill {
        name: name.to_owned(),
        description: String::new(),
        source: InventorySkillSource::System,
        plugin: None,
    }
}

/// The description in `folder/SKILL.md`, if the file is there and its
/// frontmatter reads.
fn description_at(folder: &Path) -> Option<String> {
    let file = folder.join("SKILL.md");
    let metadata = std::fs::metadata(&file).ok()?;
    if !metadata.is_file() || metadata.len() > SKILL_FILE_MAX_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(&file).ok()?;
    let (_, description) =
        artisan_native_engine::extensions::skill::parse_frontmatter(&text).ok()?;
    Some(bounded_description(&description))
}

fn bounded_description(description: &str) -> String {
    let single_line = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut end = single_line
        .len()
        .min(artisan_domain::EXTENSION_DESCRIPTION_MAX_BYTES);
    while !single_line.is_char_boundary(end) {
        end -= 1;
    }
    single_line[..end].to_owned()
}

fn connector_status(status: &str) -> ConnectorStatus {
    match status {
        "connected" => ConnectorStatus::Connected,
        "failed" => ConnectorStatus::Failed,
        "needs-auth" => ConnectorStatus::NeedsAuth,
        "disabled" => ConnectorStatus::Disabled,
        _ => ConnectorStatus::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(folder: &Path, description: &str) {
        std::fs::create_dir_all(folder).expect("skill folder");
        std::fs::write(
            folder.join("SKILL.md"),
            format!(
                "---\nname: {}\ndescription: {description}\n---\nBody\n",
                folder
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("x")
            ),
        )
        .expect("skill file");
    }

    #[test]
    fn init_names_skills_by_source_and_servers_by_status() {
        let root = tempfile::tempdir().expect("temp dir");
        let config = root.path().join("config");
        let project = root.path().join("project");
        let plugin = root.path().join("plugin");
        write_skill(
            &config.join("skills").join("apple-design"),
            "Apple's approach.",
        );
        write_skill(
            &project.join(".claude").join("skills").join("release"),
            "Ship a release.",
        );
        write_skill(&plugin.join("skills").join("wrangler"), "Run Wrangler.");
        let line = serde_json::json!({
            "type": "system",
            "subtype": "init",
            "session_id": "s",
            "skills": ["apple-design", "release", "cloudflare:wrangler", "simplify"],
            "plugins": [{"name": "cloudflare", "path": plugin.to_str().expect("utf-8")}],
            "mcp_servers": [
                {"name": "claude.ai Notion", "status": "needs-auth"},
                {"name": "sentry", "status": "connected"},
                {"name": "slow", "status": "pending"},
            ],
        })
        .to_string();
        let roots = ClaudeSkillRoots {
            config_dir: Some(config),
            project_root: project,
        };
        let report = init_inventory(&line, &roots).expect("init report");
        let skills = report.skills.expect("skills");
        let by_name = |name: &str| {
            skills
                .iter()
                .find(|skill| skill.name == name)
                .cloned()
                .expect("skill")
        };
        assert_eq!(by_name("apple-design").source, InventorySkillSource::User);
        assert_eq!(by_name("apple-design").description, "Apple's approach.");
        assert_eq!(by_name("release").source, InventorySkillSource::Project);
        let wrangler = by_name("cloudflare:wrangler");
        assert_eq!(wrangler.source, InventorySkillSource::Plugin);
        assert_eq!(wrangler.plugin.as_deref(), Some("cloudflare"));
        assert_eq!(wrangler.description, "Run Wrangler.");
        assert_eq!(by_name("simplify").source, InventorySkillSource::System);
        let ConnectorReport::Replace(connectors) = report.connectors else {
            panic!("connectors replace the record");
        };
        assert_eq!(
            connectors
                .iter()
                .map(|connector| (connector.name.as_str(), connector.status))
                .collect::<Vec<_>>(),
            [
                ("claude.ai Notion", ConnectorStatus::NeedsAuth),
                ("sentry", ConnectorStatus::Connected),
                ("slow", ConnectorStatus::Pending),
            ]
        );
    }

    #[test]
    fn other_frames_report_nothing() {
        let roots = ClaudeSkillRoots {
            config_dir: None,
            project_root: PathBuf::from("/"),
        };
        assert!(init_inventory(r#"{"type":"assistant"}"#, &roots).is_none());
        assert!(init_inventory("not json", &roots).is_none());
    }
}
