//! What each engine reports it can use, per project.
//!
//! Engines know skills and MCP servers Artisan never placed: plugin skills,
//! project skills, and connectors configured in the engine or its vendor
//! account. Runs report them as they start (Claude's `system/init`, Codex's
//! `skills/list` and MCP startup notifications); the board keeps the last
//! report per engine and project, so the Editor's composer can offer them
//! before the next run. The board is a cache of engine reports, kept in a
//! file under the Forge's state root so a restart does not forget it; it is
//! never a source of truth for what Artisan installs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use artisan_domain::{
    ConnectorStatus, EngineInventory, INVENTORIES_MAX, INVENTORY_ITEMS_MAX, InventoryConnector,
    InventorySkill, InventorySkillSource,
};
use serde_json::{Value, json};

/// The cache file, under the Forge's state root.
const CACHE_FILE: &str = "extensions/engine-inventory.json";
/// Largest cache file read back.
const CACHE_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// What one run reported about its engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InventoryReport {
    /// Every skill the engine can load, when the report lists them.
    pub(crate) skills: Option<Vec<InventorySkill>>,
    /// The engine's MCP servers.
    pub(crate) connectors: ConnectorReport,
}

/// How a report changes the connectors on record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConnectorReport {
    /// The report says nothing about connectors.
    Unchanged,
    /// The complete list.
    Replace(Vec<InventoryConnector>),
    /// One server's new state.
    Update(InventoryConnector),
}

type Key = (String, Option<String>);

/// The Forge-wide record of engine reports.
#[derive(Default)]
pub(crate) struct EngineInventoryBoard {
    entries: Mutex<BTreeMap<Key, EngineInventory>>,
    /// Skill names Artisan placed or found in each engine's home, kept by
    /// the extension sync so a mention resolves before any run reported.
    placed_skills: Mutex<BTreeMap<String, Vec<String>>>,
    cache: OnceLock<PathBuf>,
    listener: OnceLock<Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for EngineInventoryBoard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EngineInventoryBoard")
            .field("cache", &self.cache.get())
            .finish_non_exhaustive()
    }
}

impl EngineInventoryBoard {
    /// Creates an empty board that keeps nothing on disk until
    /// [`Self::attach_cache`].
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Reads the cache under `state_root` and keeps later reports there.
    pub(crate) fn attach_cache(&self, state_root: &Path) {
        let path = state_root.join(CACHE_FILE);
        let restored = read_cache(&path);
        if !restored.is_empty() {
            let mut entries = self.entries();
            for inventory in restored {
                entries
                    .entry((inventory.engine_id.clone(), inventory.project_id.clone()))
                    .or_insert(inventory);
            }
        }
        let _ = self.cache.set(path);
    }

    /// Calls `listener` after every change the Editor should see.
    pub(crate) fn set_listener(&self, listener: Arc<dyn Fn() + Send + Sync>) {
        let _ = self.listener.set(listener);
    }

    /// Records one run's report. Notifies and rewrites the cache only when
    /// the record changed.
    pub(crate) fn record(
        &self,
        engine_id: &str,
        project_id: Option<&str>,
        report: InventoryReport,
    ) {
        let changed = {
            let mut entries = self.entries();
            let key = (engine_id.to_owned(), project_id.map(str::to_owned));
            if !entries.contains_key(&key) && entries.len() >= INVENTORIES_MAX {
                return;
            }
            let entry = entries.entry(key).or_insert_with(|| EngineInventory {
                engine_id: engine_id.to_owned(),
                project_id: project_id.map(str::to_owned),
                skills: Vec::new(),
                connectors: Vec::new(),
            });
            let before = entry.clone();
            apply(entry, report);
            let changed = *entry != before;
            if entry.validate().is_err() {
                *entry = before;
                false
            } else {
                changed
            }
        };
        if changed {
            self.persist();
            if let Some(listener) = self.listener.get() {
                listener();
            }
        }
    }

    /// Every engine's last report, by engine and project.
    pub(crate) fn snapshot(&self) -> Vec<EngineInventory> {
        self.entries().values().cloned().collect()
    }

    /// Replaces the skill names the extension sync placed or found in each
    /// engine's home.
    pub(crate) fn set_placed_skills(&self, placed: BTreeMap<String, Vec<String>>) {
        *self
            .placed_skills
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = placed;
    }

    /// Every skill name `engine_id` is known to load in `project_id`.
    pub(crate) fn known_skills(&self, engine_id: &str, project_id: Option<&str>) -> Vec<String> {
        let mut names = self
            .placed_skills
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(engine_id)
            .cloned()
            .unwrap_or_default();
        let entries = self.entries();
        for key in [
            (engine_id.to_owned(), project_id.map(str::to_owned)),
            (engine_id.to_owned(), None),
        ] {
            if let Some(inventory) = entries.get(&key) {
                names.extend(inventory.skills.iter().map(|skill| skill.name.clone()));
            }
        }
        names.sort();
        names.dedup();
        names
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, BTreeMap<Key, EngineInventory>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn persist(&self) {
        // Writes run on the blocking pool and may overlap: one at a time,
        // and a snapshot older than the last one written is dropped.
        static WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
        static TAKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let Some(path) = self.cache.get().cloned() else {
            return;
        };
        // Numbered under the entries lock, so a higher number is never an
        // older snapshot.
        let (taken, document) = {
            let entries = self.entries();
            (
                TAKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                Value::Array(entries.values().map(encode).collect()),
            )
        };
        let write = move || -> std::io::Result<()> {
            let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
            if *written > taken {
                return Ok(());
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let staged = path.with_extension("json.tmp");
            std::fs::write(&staged, serde_json::to_vec(&document)?)?;
            std::fs::rename(&staged, &path)?;
            *written = taken;
            Ok(())
        };
        let report = |error: std::io::Error| {
            eprintln!(
                "engine inventory cache write failed: {}",
                artisan_domain::error_chain::ErrorChain(&error)
            );
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(move || write().map_err(report));
            }
            Err(_) => {
                let _ = write().map_err(report);
            }
        }
    }
}

fn apply(entry: &mut EngineInventory, report: InventoryReport) {
    if let Some(mut skills) = report.skills {
        skills.sort_by(|left, right| left.name.cmp(&right.name));
        skills.dedup_by(|later, earlier| later.name == earlier.name);
        skills.truncate(INVENTORY_ITEMS_MAX);
        entry.skills = skills;
    }
    match report.connectors {
        ConnectorReport::Unchanged => {}
        ConnectorReport::Replace(mut connectors) => {
            connectors.sort_by(|left, right| left.name.cmp(&right.name));
            connectors.dedup_by(|later, earlier| later.name == earlier.name);
            connectors.truncate(INVENTORY_ITEMS_MAX);
            entry.connectors = connectors;
        }
        ConnectorReport::Update(connector) => {
            match entry
                .connectors
                .binary_search_by(|known| known.name.cmp(&connector.name))
            {
                Ok(index) => entry.connectors[index] = connector,
                Err(index) if entry.connectors.len() < INVENTORY_ITEMS_MAX => {
                    entry.connectors.insert(index, connector);
                }
                Err(_) => {}
            }
        }
    }
}

const fn source_name(source: InventorySkillSource) -> &'static str {
    match source {
        InventorySkillSource::User => "user",
        InventorySkillSource::Project => "project",
        InventorySkillSource::Plugin => "plugin",
        InventorySkillSource::System => "system",
    }
}

const fn status_name(status: ConnectorStatus) -> &'static str {
    match status {
        ConnectorStatus::Connected => "connected",
        ConnectorStatus::Pending => "pending",
        ConnectorStatus::NeedsAuth => "needs_auth",
        ConnectorStatus::Failed => "failed",
        ConnectorStatus::Disabled => "disabled",
    }
}

fn encode(inventory: &EngineInventory) -> Value {
    json!({
        "engine": inventory.engine_id,
        "project": inventory.project_id,
        "skills": inventory.skills.iter().map(|skill| json!({
            "name": skill.name,
            "description": skill.description,
            "source": source_name(skill.source),
            "plugin": skill.plugin,
        })).collect::<Vec<_>>(),
        "connectors": inventory.connectors.iter().map(|connector| json!({
            "name": connector.name,
            "status": status_name(connector.status),
            "plugin": connector.plugin,
        })).collect::<Vec<_>>(),
    })
}

fn read_cache(path: &Path) -> Vec<EngineInventory> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if metadata.len() > CACHE_MAX_BYTES {
        return Vec::new();
    }
    let Some(Value::Array(entries)) = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(decode)
        .filter(|inventory| inventory.validate().is_ok())
        .take(INVENTORIES_MAX)
        .collect()
}

fn decode(value: &Value) -> Option<EngineInventory> {
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(EngineInventory {
        engine_id: text(value, "engine")?,
        project_id: text(value, "project"),
        skills: value
            .get("skills")?
            .as_array()?
            .iter()
            .filter_map(|skill| {
                Some(InventorySkill {
                    name: text(skill, "name")?,
                    description: text(skill, "description").unwrap_or_default(),
                    source: match skill.get("source")?.as_str()? {
                        "user" => InventorySkillSource::User,
                        "project" => InventorySkillSource::Project,
                        "plugin" => InventorySkillSource::Plugin,
                        "system" => InventorySkillSource::System,
                        _ => return None,
                    },
                    plugin: text(skill, "plugin"),
                })
            })
            .collect(),
        connectors: value
            .get("connectors")?
            .as_array()?
            .iter()
            .filter_map(|connector| {
                Some(InventoryConnector {
                    name: text(connector, "name")?,
                    status: connector_status(connector.get("status")?.as_str()?)?,
                    plugin: text(connector, "plugin"),
                })
            })
            .collect(),
    })
}

fn connector_status(name: &str) -> Option<ConnectorStatus> {
    Some(match name {
        "connected" => ConnectorStatus::Connected,
        "pending" => ConnectorStatus::Pending,
        "needs_auth" => ConnectorStatus::NeedsAuth,
        "failed" => ConnectorStatus::Failed,
        "disabled" => ConnectorStatus::Disabled,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector(name: &str, status: ConnectorStatus) -> InventoryConnector {
        InventoryConnector {
            name: name.to_owned(),
            status,
            plugin: None,
        }
    }

    fn skill(name: &str) -> InventorySkill {
        InventorySkill {
            name: name.to_owned(),
            description: String::new(),
            source: InventorySkillSource::User,
            plugin: None,
        }
    }

    #[test]
    fn updates_merge_one_connector_and_replacements_take_the_list() {
        let board = EngineInventoryBoard::new();
        board.record(
            "codex",
            Some("p1"),
            InventoryReport {
                skills: None,
                connectors: ConnectorReport::Update(connector("sentry", ConnectorStatus::Pending)),
            },
        );
        board.record(
            "codex",
            Some("p1"),
            InventoryReport {
                skills: None,
                connectors: ConnectorReport::Update(connector(
                    "sentry",
                    ConnectorStatus::Connected,
                )),
            },
        );
        let snapshot = board.snapshot();
        assert_eq!(
            snapshot[0].connectors,
            vec![connector("sentry", ConnectorStatus::Connected)]
        );
        board.record(
            "codex",
            Some("p1"),
            InventoryReport {
                skills: Some(vec![skill("b"), skill("a"), skill("a")]),
                connectors: ConnectorReport::Replace(Vec::new()),
            },
        );
        let snapshot = board.snapshot();
        assert!(snapshot[0].connectors.is_empty());
        assert_eq!(
            snapshot[0]
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn known_skills_join_placed_and_reported_names() {
        let board = EngineInventoryBoard::new();
        board.set_placed_skills(BTreeMap::from([(
            "claude".to_owned(),
            vec!["artisan-editor".to_owned()],
        )]));
        board.record(
            "claude",
            Some("p1"),
            InventoryReport {
                skills: Some(vec![skill("cloudflare:wrangler")]),
                connectors: ConnectorReport::Unchanged,
            },
        );
        assert_eq!(
            board.known_skills("claude", Some("p1")),
            ["artisan-editor", "cloudflare:wrangler"]
        );
        assert_eq!(board.known_skills("claude", Some("p2")), ["artisan-editor"]);
    }

    #[test]
    fn the_cache_round_trips() {
        let root = tempfile::tempdir().expect("temp dir");
        let board = EngineInventoryBoard::new();
        board.attach_cache(root.path());
        board.record(
            "claude",
            None,
            InventoryReport {
                skills: Some(vec![skill("apple-design")]),
                connectors: ConnectorReport::Replace(vec![connector(
                    "claude.ai Notion",
                    ConnectorStatus::NeedsAuth,
                )]),
            },
        );
        let restored = read_cache(&root.path().join(CACHE_FILE));
        assert_eq!(restored, board.snapshot());
    }
}
