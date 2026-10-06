//! Skill reconciliation across every managed engine home.
//!
//! Synchronous filesystem work, run off the async runtime: each enabled
//! managed skill is linked into every installed engine's skills folder,
//! disabled or removed ones lose Artisan's link, and the folders an engine
//! has on its own are read and reported as discovered skills, grouped by
//! content so identical copies in several engines are one entry.

use std::collections::BTreeMap;
use std::path::PathBuf;

use artisan_domain::{
    Extension, ExtensionEngineState, ExtensionEngineStatus, ExtensionKind, ExtensionOrigin,
};
use artisan_native_engine::ManagedEngine;
use artisan_native_engine::extensions::{
    EnginePresence, EngineSlot, LinkError, SkillPackage, native_skill_dirs, owned_links,
    place_link, read_skill_dir, remove_link,
};

/// One skill Artisan manages, with the folder engines link to.
#[derive(Clone, Debug)]
pub(crate) struct ManagedSkill {
    pub(crate) id: String,
    pub(crate) origin: ExtensionOrigin,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) digest: String,
    pub(crate) file_count: u32,
    pub(crate) byte_count: u64,
    pub(crate) enabled: bool,
    /// The folder engines link to, or why it is unavailable.
    pub(crate) target: Result<PathBuf, &'static str>,
}

impl ManagedSkill {
    /// Describes a managed skill from its package.
    pub(crate) fn from_package(
        id: String,
        origin: ExtensionOrigin,
        package: &SkillPackage,
        enabled: bool,
        target: Result<PathBuf, &'static str>,
    ) -> Self {
        Self {
            id,
            origin,
            name: package.name().to_owned(),
            description: package.description().to_owned(),
            digest: package.digest().to_owned(),
            file_count: u32::try_from(package.files().len()).unwrap_or(u32::MAX),
            byte_count: package.byte_count(),
            enabled,
            target,
        }
    }
}

/// A skill engines have on their own, and where each copy lives.
#[derive(Clone, Debug)]
pub(crate) struct DiscoveredSkill {
    pub(crate) extension: Extension,
    /// `(engine, folder)` of every copy with this content.
    pub(crate) copies: Vec<(ManagedEngine, PathBuf)>,
}

/// One engine-native skill's content and every folder holding it.
type FoundSkill = (SkillPackage, Vec<(ManagedEngine, PathBuf)>);

/// What one reconciliation found.
#[derive(Clone, Debug, Default)]
pub(crate) struct SkillSync {
    /// Managed skills, by name.
    pub(crate) managed: Vec<Extension>,
    /// Discovered skills, by name then identifier.
    pub(crate) discovered: Vec<DiscoveredSkill>,
}

/// Brings every installed engine home in line with `managed` and reports
/// each skill's state per engine.
pub(crate) fn sync_skills(
    slots: &[EngineSlot],
    managed: &[ManagedSkill],
    owned_roots: &[PathBuf],
    links_available: bool,
) -> SkillSync {
    let mut statuses: Vec<Vec<ExtensionEngineState>> = vec![Vec::new(); managed.len()];
    let mut found: BTreeMap<(String, String), FoundSkill> = BTreeMap::new();
    for slot in slots {
        let engine_id = slot.engine.id().to_owned();
        let EnginePresence::Installed = slot.presence else {
            let (status, reason) = absent_status(slot);
            for states in &mut statuses {
                states.push(state(&engine_id, status, reason.clone()));
            }
            continue;
        };
        let mut per_skill: Vec<(ExtensionEngineStatus, Option<String>)> =
            vec![(ExtensionEngineStatus::Synced, None); managed.len()];
        for home in &slot.homes {
            for (index, skill) in managed.iter().enumerate() {
                let outcome = place(slot.engine, home, skill, owned_roots, links_available);
                per_skill[index] = worse(per_skill[index].clone(), outcome);
            }
            for stale in owned_links(&home.skills_dir, owned_roots) {
                let wanted = managed
                    .iter()
                    .any(|skill| skill.enabled && skill.target.is_ok() && skill.name == stale);
                if !wanted {
                    let _ = remove_link(&home.skills_dir, &stale, owned_roots);
                }
            }
            for (_, folder) in native_skill_dirs(&home.skills_dir) {
                let Ok(package) = read_skill_dir(&folder) else {
                    continue;
                };
                let key = (package.name().to_owned(), package.digest().to_owned());
                found
                    .entry(key)
                    .or_insert_with(|| (package, Vec::new()))
                    .1
                    .push((slot.engine, folder));
            }
        }
        for (states, (status, reason)) in statuses.iter_mut().zip(per_skill) {
            states.push(state(&engine_id, status, reason));
        }
    }
    let managed_entries = managed
        .iter()
        .zip(statuses)
        .map(|(skill, engines)| Extension {
            id: skill.id.clone(),
            kind: ExtensionKind::Skill,
            origin: skill.origin,
            name: skill.name.clone(),
            description: skill.description.clone(),
            digest: skill.digest.clone(),
            file_count: skill.file_count,
            byte_count: skill.byte_count,
            enabled: skill.enabled,
            engines,
        })
        .collect();
    let discovered = found
        .into_values()
        .map(|(package, copies)| discovered_entry(slots, &package, copies))
        .collect();
    SkillSync {
        managed: managed_entries,
        discovered,
    }
}

/// Places or removes one managed skill in one engine home.
fn place(
    engine: ManagedEngine,
    home: &artisan_native_engine::extensions::EngineHomeLocations,
    skill: &ManagedSkill,
    owned_roots: &[PathBuf],
    links_available: bool,
) -> (ExtensionEngineStatus, Option<String>) {
    if !skill.enabled {
        return match remove_link(&home.skills_dir, &skill.name, owned_roots) {
            Ok(_) => (ExtensionEngineStatus::Off, None),
            Err(_) => failed(engine, "could not remove"),
        };
    }
    let target = match &skill.target {
        Ok(target) => target,
        Err(reason) => return (ExtensionEngineStatus::Failed, Some((*reason).to_owned())),
    };
    if !links_available {
        return (ExtensionEngineStatus::LinksUnavailable, None);
    }
    match place_link(&home.skills_dir, &skill.name, target, owned_roots) {
        Ok(()) => (ExtensionEngineStatus::Synced, None),
        Err(LinkError::Occupied) => (
            ExtensionEngineStatus::Conflict,
            Some(format!(
                "{} has its own skill named {}. Adopt or remove it to let Artisan manage it.",
                engine.display_name(),
                skill.name
            )),
        ),
        Err(LinkError::LinksUnavailable) => (ExtensionEngineStatus::LinksUnavailable, None),
        Err(LinkError::Io) => failed(engine, "could not link"),
    }
}

fn failed(engine: ManagedEngine, action: &str) -> (ExtensionEngineStatus, Option<String>) {
    (
        ExtensionEngineStatus::Failed,
        Some(format!(
            "Artisan {action} the skill in {}'s skills folder.",
            engine.display_name()
        )),
    )
}

fn discovered_entry(
    slots: &[EngineSlot],
    package: &SkillPackage,
    copies: Vec<(ManagedEngine, PathBuf)>,
) -> DiscoveredSkill {
    let engines = slots
        .iter()
        .map(|slot| {
            let engine_id = slot.engine.id();
            if copies.iter().any(|(engine, _)| *engine == slot.engine) {
                return state(engine_id, ExtensionEngineStatus::Native, None);
            }
            if slot.presence == EnginePresence::Installed {
                state(engine_id, ExtensionEngineStatus::NotPresent, None)
            } else {
                let (status, reason) = absent_status(slot);
                state(engine_id, status, reason)
            }
        })
        .collect();
    DiscoveredSkill {
        extension: Extension {
            id: discovered_id(package.name(), package.digest()),
            kind: ExtensionKind::Skill,
            origin: ExtensionOrigin::Discovered,
            name: package.name().to_owned(),
            description: package.description().to_owned(),
            digest: package.digest().to_owned(),
            file_count: u32::try_from(package.files().len()).unwrap_or(u32::MAX),
            byte_count: package.byte_count(),
            enabled: false,
            engines,
        },
        copies,
    }
}

/// The identifier of a discovered skill: its name and a digest prefix, so
/// differing copies under one name stay distinct.
pub(crate) fn discovered_id(name: &str, digest: &str) -> String {
    format!("discovered.{name}.{}", &digest[..12.min(digest.len())])
}

fn absent_status(slot: &EngineSlot) -> (ExtensionEngineStatus, Option<String>) {
    match slot.presence {
        EnginePresence::Unavailable(reason) => {
            (ExtensionEngineStatus::Unavailable, Some(reason.to_owned()))
        }
        EnginePresence::Absent | EnginePresence::Installed => {
            (ExtensionEngineStatus::EngineAbsent, None)
        }
    }
}

fn state(
    engine_id: &str,
    status: ExtensionEngineStatus,
    reason: Option<String>,
) -> ExtensionEngineState {
    ExtensionEngineState {
        engine_id: engine_id.to_owned(),
        status,
        reason,
    }
}

/// Keeps the more severe of two home outcomes for an engine with several
/// homes.
fn worse(
    left: (ExtensionEngineStatus, Option<String>),
    right: (ExtensionEngineStatus, Option<String>),
) -> (ExtensionEngineStatus, Option<String>) {
    let rank = |status: ExtensionEngineStatus| match status {
        ExtensionEngineStatus::Failed => 4,
        ExtensionEngineStatus::LinksUnavailable => 3,
        ExtensionEngineStatus::Conflict => 2,
        ExtensionEngineStatus::Off => 1,
        _ => 0,
    };
    if rank(right.0) > rank(left.0) {
        right
    } else {
        left
    }
}
