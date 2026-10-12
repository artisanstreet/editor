//! Which skills go into the engines, and where each stands in each engine.
//!
//! Three sources (master's `extension_sync`): Artisan's own skills, embedded
//! in Forge from the repository's `skills/`; the user's own, one folder per
//! skill in `<data>/extensions/skills`; and skills an engine has in its own
//! skills folder, which Forge only reports until the user adopts them.

use super::{COPY_MARKER, Entry, frontmatter_field, frontmatter_name, inspect, paths, sync_skills};
use super::{Customisation, copy_dir, import_folder, write_if_changed};
use anyhow::{Context, Result, bail};
use artisan_api::{CatalogSkill, EngineId, SkillEngineState, SkillEngineStatus, SkillOrigin};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One first-party skill: its folder under the repository's `skills/` and
/// every file in it.
struct FirstParty {
    folder: &'static str,
    files: &'static [(&'static str, &'static [u8])],
}

const FIRST_PARTY: &[FirstParty] = &[FirstParty {
    folder: "artisan-editor",
    files: &[
        (
            "SKILL.md",
            include_bytes!("../../../../skills/artisan-editor/SKILL.md"),
        ),
        (
            "references/blocks.md",
            include_bytes!("../../../../skills/artisan-editor/references/blocks.md"),
        ),
    ],
}];

/// The id of a first-party skill.
fn first_party_id(folder: &str) -> String {
    format!("artisan.{folder}")
}

/// The id of a skill in the user's skills folder.
fn user_id(folder: &str) -> String {
    format!("local.{folder}")
}

/// `<data>/extensions/artisan`: where Forge writes its own skills out.
fn first_party_root(skills_dir: &Path) -> PathBuf {
    skills_dir.parent().map_or_else(
        || skills_dir.join(".artisan"),
        |extensions| extensions.join("artisan"),
    )
}

/// `<data>/extensions/backups`: engines' copies of adopted skills.
fn backups_root(skills_dir: &Path) -> PathBuf {
    skills_dir.parent().map_or_else(
        || skills_dir.join(".backups"),
        |extensions| extensions.join("backups"),
    )
}

/// The repository's `skills/` in a development build, linked directly so an
/// edit reaches the engines without a rebuild.
fn repository_root() -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../skills")
        .canonicalize()
        .ok()
        .filter(|root| root.is_dir())
}

/// Every folder Forge links engines' skills to: links into them are Forge's.
pub(super) fn owned_roots(skills_dir: &Path) -> Vec<PathBuf> {
    let mut roots = vec![skills_dir.to_path_buf(), first_party_root(skills_dir)];
    // Links into the old content-addressed store are Forge's too.
    if let Some(extensions) = skills_dir.parent() {
        roots.push(extensions.join("store"));
    }
    roots.extend(repository_root());
    roots
}

/// A skill Forge manages.
struct Managed {
    id: String,
    name: String,
    description: String,
    origin: SkillOrigin,
    folder: PathBuf,
    enabled: bool,
    /// A first-party skill already has this name; this one isn't placed.
    shadowed: bool,
}

/// Writes the embedded first-party skills out (or, in a development build,
/// points at the repository) and returns `(folder name, folder)`.
fn first_party(skills_dir: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let repository = repository_root();
    let root = first_party_root(skills_dir);
    let mut placed = Vec::new();
    for skill in FIRST_PARTY {
        if let Some(folder) = repository.as_ref().map(|root| root.join(skill.folder))
            && folder.join("SKILL.md").is_file()
        {
            placed.push((skill.folder.to_owned(), folder));
            continue;
        }
        let folder = root.join(skill.folder);
        for (path, bytes) in skill.files {
            write_if_changed(&folder.join(path), bytes)?;
        }
        placed.push((skill.folder.to_owned(), folder));
    }
    Ok(placed)
}

/// The skills Forge manages, first-party first, then the user's by folder.
fn managed(c: &Customisation) -> Result<Vec<Managed>> {
    let mut skills = Vec::new();
    if c.first_party {
        let placed = first_party(&c.skills_dir).with_context(|| {
            format!(
                "writing Artisan's skills into {}",
                first_party_root(&c.skills_dir).display()
            )
        })?;
        for (folder_name, folder) in placed {
            let text = fs::read_to_string(folder.join("SKILL.md")).unwrap_or_default();
            let id = first_party_id(&folder_name);
            skills.push(Managed {
                enabled: !c.disabled_skills.contains(&id),
                id,
                name: frontmatter_name(&text).unwrap_or_else(|| folder_name.clone()),
                description: frontmatter_field(&text, "description:").unwrap_or_default(),
                origin: SkillOrigin::FirstParty,
                folder,
                shadowed: false,
            });
        }
    }
    let mut users = Vec::new();
    if let Ok(entries) = fs::read_dir(&c.skills_dir) {
        for entry in entries.flatten() {
            let folder_name = entry.file_name().to_string_lossy().into_owned();
            if folder_name.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            let text = fs::read_to_string(entry.path().join("SKILL.md")).unwrap_or_default();
            let id = user_id(&folder_name);
            users.push(Managed {
                enabled: !c.disabled_skills.contains(&id),
                shadowed: skills
                    .iter()
                    .any(|skill: &Managed| skill.folder.file_name() == Some(folder_name.as_ref())),
                id,
                name: frontmatter_name(&text).unwrap_or_else(|| folder_name.clone()),
                description: frontmatter_field(&text, "description:").unwrap_or_default(),
                origin: SkillOrigin::User,
                folder: entry.path(),
            });
        }
    }
    users.sort_by(|a, b| a.name.cmp(&b.name));
    skills.extend(users);
    Ok(skills)
}

/// What every engine's skills folder should link: enabled skills by folder
/// name.
pub(super) fn wanted(c: &Customisation) -> Result<BTreeMap<String, PathBuf>> {
    Ok(managed(c)?
        .into_iter()
        .filter(|skill| skill.enabled && !skill.shadowed)
        .filter_map(|skill| {
            let name = skill.folder.file_name()?.to_string_lossy().into_owned();
            Some((name, skill.folder))
        })
        .collect())
}

/// The skills Forge puts into the engines, by name: Artisan's own and the
/// user's, less the ones turned off. A `$name` mention names one of these.
///
/// # Errors
///
/// When Artisan's own skills can't be written out.
pub fn enabled_skills(c: &Customisation) -> Result<Vec<artisan_api::Skill>> {
    let mut skills: Vec<artisan_api::Skill> = managed(c)?
        .into_iter()
        .filter(|skill| skill.enabled && !skill.shadowed)
        .map(|skill| artisan_api::Skill {
            name: skill.name,
            description: (!skill.description.is_empty()).then_some(skill.description),
            path: skill.folder.display().to_string(),
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

/// A skill folder an engine has on its own.
struct Found {
    engine: EngineId,
    folder: PathBuf,
    folder_name: String,
    digest: String,
}

/// Real skill folders in the engines' skills folders that aren't Forge's.
fn discovered(homes: &[(EngineId, Option<PathBuf>)], owned: &[PathBuf]) -> Vec<Found> {
    let mut found = Vec::new();
    for (engine, home) in homes {
        let Some(paths) = home.as_deref().and_then(|home| paths(*engine, home)) else {
            continue;
        };
        let Ok(entries) = fs::read_dir(&paths.skills) else {
            continue;
        };
        for entry in entries.flatten() {
            let folder_name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if folder_name.starts_with('.')
                || !matches!(inspect(&path, owned), Entry::Theirs)
                || !path.join("SKILL.md").is_file()
            {
                continue;
            }
            let Ok((digest, _, _)) = measure(&path) else {
                continue;
            };
            found.push(Found {
                engine: *engine,
                folder: path,
                folder_name,
                digest,
            });
        }
    }
    found
}

/// SHA-256 over every file (by relative path), the file count and bytes.
fn measure(folder: &Path) -> io::Result<(String, u32, u64)> {
    let mut files = Vec::new();
    walk(folder, Path::new(""), &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    for relative in &files {
        let content = fs::read(folder.join(relative))?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(
            u64::try_from(content.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        hasher.update(&content);
        bytes += u64::try_from(content.len()).unwrap_or(u64::MAX);
    }
    let digest = super::prompt::hex(&hasher.finalize());
    Ok((
        digest,
        u32::try_from(files.len()).unwrap_or(u32::MAX),
        bytes,
    ))
}

fn walk(root: &Path, relative: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let name = entry.file_name();
        if name == COPY_MARKER {
            continue;
        }
        let path = relative.join(&name);
        // Follows links, as engines do when they read a skill.
        let metadata = fs::metadata(entry.path())?;
        if metadata.is_dir() {
            walk(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn engine_label(engine: EngineId) -> &'static str {
    engine.display_name()
}

/// Links the enabled skills into every installed engine (`home` is `None`
/// for an engine that isn't installed), then reports every skill and where
/// it stands in each engine.
///
/// # Errors
///
/// When Artisan's own skills can't be written out.
pub fn skill_catalog(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
) -> Result<Vec<CatalogSkill>> {
    let managed = managed(c)?;
    let wanted = wanted(c)?;
    let owned = owned_roots(&c.skills_dir);
    let mut sync_errors: BTreeMap<EngineId, String> = BTreeMap::new();
    for (engine, home) in homes {
        let Some(paths) = home.as_deref().and_then(|home| paths(*engine, home)) else {
            continue;
        };
        if let Err(error) = sync_skills(&paths.skills, &wanted, &owned) {
            tracing::warn!(%engine, folder = %paths.skills.display(), "couldn't link the skills: {error}");
            sync_errors.insert(
                *engine,
                format!("Couldn't update {}: {error}.", paths.skills.display()),
            );
        }
    }
    let mut catalog: Vec<CatalogSkill> = managed
        .iter()
        .map(|skill| {
            let (_, file_count, byte_count) = measure(&skill.folder).unwrap_or_default();
            CatalogSkill {
                id: skill.id.clone(),
                name: skill.name.clone(),
                description: skill.description.clone(),
                origin: skill.origin,
                enabled: skill.enabled,
                file_count,
                byte_count,
                engines: homes
                    .iter()
                    .map(|(engine, home)| {
                        managed_state(skill, *engine, home.as_deref(), &sync_errors, &owned)
                    })
                    .collect(),
            }
        })
        .collect();
    catalog.extend(discovered_entries(homes, &owned));
    Ok(catalog)
}

fn unavailable_reason(engine: EngineId) -> String {
    format!(
        "{} has no skills folder Artisan can manage.",
        engine_label(engine)
    )
}

/// Where a managed skill stands in one engine.
fn managed_state(
    skill: &Managed,
    engine: EngineId,
    home: Option<&Path>,
    sync_errors: &BTreeMap<EngineId, String>,
    owned: &[PathBuf],
) -> SkillEngineState {
    let state = |status, reason: Option<String>| SkillEngineState {
        engine,
        status,
        reason,
    };
    let link_name = skill
        .folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(paths) = paths(engine, home.unwrap_or(Path::new(""))) else {
        return state(
            SkillEngineStatus::Unavailable,
            Some(unavailable_reason(engine)),
        );
    };
    if !skill.enabled {
        return state(SkillEngineStatus::Off, None);
    }
    if home.is_none() {
        return state(SkillEngineStatus::EngineAbsent, None);
    }
    if skill.shadowed {
        return state(
            SkillEngineStatus::Failed,
            Some(format!(
                "Artisan's own skill {link_name} has this folder name; rename yours."
            )),
        );
    }
    if let Some(error) = sync_errors.get(&engine) {
        return state(SkillEngineStatus::Failed, Some(error.clone()));
    }
    match inspect(&paths.skills.join(&link_name), owned) {
        Entry::Ours(Some(target)) if target == skill.folder => {
            state(SkillEngineStatus::Synced, None)
        }
        Entry::Ours(None) => state(SkillEngineStatus::Synced, None),
        Entry::Theirs => state(
            SkillEngineStatus::Conflict,
            Some(format!(
                "{} has its own skill named {link_name}. Remove it to let Artisan manage it.",
                engine_label(engine)
            )),
        ),
        Entry::Ours(Some(_)) | Entry::Missing => state(
            SkillEngineStatus::Failed,
            Some(format!(
                "Artisan could not link the skill in {}'s skills folder.",
                engine_label(engine)
            )),
        ),
    }
}

/// The skills engines have on their own; copies with the same name and
/// files are one skill found in several engines.
fn discovered_entries(
    homes: &[(EngineId, Option<PathBuf>)],
    owned: &[PathBuf],
) -> Vec<CatalogSkill> {
    let mut groups: BTreeMap<(String, String), Vec<Found>> = BTreeMap::new();
    for found in discovered(homes, owned) {
        groups
            .entry((found.folder_name.clone(), found.digest.clone()))
            .or_default()
            .push(found);
    }
    groups
        .into_iter()
        .map(|((folder_name, digest), copies)| {
            let first = &copies[0].folder;
            let text = fs::read_to_string(first.join("SKILL.md")).unwrap_or_default();
            let (_, file_count, byte_count) = measure(first).unwrap_or_default();
            let engines = homes
                .iter()
                .map(|(engine, home)| {
                    let (status, reason) = if copies.iter().any(|copy| copy.engine == *engine) {
                        (SkillEngineStatus::Native, None)
                    } else if paths(*engine, Path::new("")).is_none() {
                        (
                            SkillEngineStatus::Unavailable,
                            Some(unavailable_reason(*engine)),
                        )
                    } else if home.is_none() {
                        (SkillEngineStatus::EngineAbsent, None)
                    } else {
                        (SkillEngineStatus::NotPresent, None)
                    };
                    SkillEngineState {
                        engine: *engine,
                        status,
                        reason,
                    }
                })
                .collect();
            CatalogSkill {
                id: discovered_id(&folder_name, &digest),
                name: frontmatter_name(&text).unwrap_or_else(|| folder_name.clone()),
                description: frontmatter_field(&text, "description:").unwrap_or_default(),
                origin: SkillOrigin::Discovered,
                enabled: false,
                file_count,
                byte_count,
                engines,
            }
        })
        .collect()
}

fn discovered_id(folder_name: &str, digest: &str) -> String {
    format!("discovered.{folder_name}.{}", &digest[..12])
}

/// Adopts a skill found in the engines: copies it into the user's skills
/// folder and moves every engine's identical copy into
/// `<data>/extensions/backups`, so Forge's link takes its place. Returns the
/// folder name.
///
/// # Errors
///
/// When the skill is gone or changed, its name is taken, or copying fails.
pub fn adopt_skill(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
    id: &str,
) -> Result<String> {
    let owned = owned_roots(&c.skills_dir);
    let copies: Vec<Found> = discovered(homes, &owned)
        .into_iter()
        .filter(|found| discovered_id(&found.folder_name, &found.digest) == id)
        .collect();
    let Some(first) = copies.first() else {
        bail!("That skill is no longer in your engines' skills folders.");
    };
    let name = first.folder_name.clone();
    let taken = c.skills_dir.join(&name).exists()
        || (c.first_party && FIRST_PARTY.iter().any(|skill| skill.folder == name));
    if taken {
        bail!("Artisan already manages a skill named {name}.");
    }
    fs::create_dir_all(&c.skills_dir)
        .with_context(|| format!("creating {}", c.skills_dir.display()))?;
    import_folder(&first.folder, &c.skills_dir, &name)?;
    let backups = backups_root(&c.skills_dir);
    let stamp = artisan_api::now();
    for copy in &copies {
        let to = backups.join(format!("{}-{name}-{stamp}", copy.engine));
        move_folder(&copy.folder, &to).with_context(|| {
            format!(
                "moving {}'s copy of {name} to {}",
                copy.engine.display_name(),
                to.display()
            )
        })?;
    }
    Ok(name)
}

fn move_folder(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Across file systems.
    copy_dir(from, to)?;
    fs::remove_dir_all(from)
}
