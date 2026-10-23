//! Which skills go into the engines, and where each stands in each engine.
//!
//! Three sources (master's `extension_sync`): Artisan's own skills, embedded
//! in Forge from the repository's `skills/`; the user's own, one folder per
//! skill in `<data>/extensions/skills`; and skills an engine has in its own
//! skills folder, which Forge only reports until the user adopts them.

use super::{COPY_MARKER, Entry, inspect, paths, skill_metadata, skill_metadata_in};
use super::{Customisation, copy_dir, import_folder, read_skill_header, write_if_changed};
use anyhow::{Context, Result, bail};
use artisan_api::{CatalogSkill, EngineId, SkillEngineState, SkillEngineStatus, SkillOrigin};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MAX_CATALOG_RECORDS: usize = 4096;
const MAX_CATALOG_METADATA_BYTES: usize = 4 * 1024 * 1024;
#[derive(Default)]
struct CatalogBudget {
    records: usize,
    bytes: usize,
}
impl CatalogBudget {
    fn admit(&mut self, bytes: usize) -> Result<()> {
        self.records += 1;
        self.bytes = self.bytes.saturating_add(bytes);
        if self.records > MAX_CATALOG_RECORDS || self.bytes > MAX_CATALOG_METADATA_BYTES {
            bail!(
                "skill catalog requires paging: records={}/{} metadata_bytes={}/{}",
                self.records,
                MAX_CATALOG_RECORDS,
                self.bytes,
                MAX_CATALOG_METADATA_BYTES
            );
        }
        Ok(())
    }
}
fn catalog_metadata(skill: &CatalogSkill) -> usize {
    skill.id.len()
        + skill.name.len()
        + skill.description.len()
        + skill
            .engines
            .iter()
            .map(|state| state.reason.as_ref().map_or(0, String::len))
            .sum::<usize>()
}

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

/// `<data>/extensions/quarantine`: skills found in an engine's skills folder
/// that Artisan didn't install, one folder per engine laid out like that
/// engine's skills folder.
fn quarantine_root(skills_dir: &Path) -> PathBuf {
    skills_dir.parent().map_or_else(
        || skills_dir.join(".quarantine"),
        |extensions| extensions.join("quarantine"),
    )
}

/// How long a quarantined skill waits for Install or Discard.
const QUARANTINE_DAYS: u64 = 30;

/// When `name` entered the quarantine: a hidden file beside it, so the
/// skill's own files stay as they were.
fn quarantined_at(engine_quarantine: &Path, name: &str) -> PathBuf {
    engine_quarantine.join(format!(".{name}.at"))
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Moves everything in `engine_skills` that Forge didn't place into the
/// engine's quarantine, so an engine only ever reads what Artisan
/// installed; a newer copy replaces an older one of the same name. Then
/// drops quarantined skills older than [`QUARANTINE_DAYS`]. Hidden entries
/// (Codex's own `.system`) stay. Never fails the launch: what can't be moved
/// is logged and left.
pub(super) fn quarantine_foreign(
    engine: EngineId,
    engine_skills: &Path,
    skills_dir: &Path,
    owned: &[PathBuf],
) {
    let root = quarantine_root(skills_dir).join(engine.as_str());
    if let Ok(entries) = fs::read_dir(engine_skills) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if name.starts_with('.') || !matches!(inspect(&path, owned), Entry::Theirs) {
                continue;
            }
            let to = root.join(&name);
            let moved = fs::create_dir_all(&root)
                .and_then(|()| match fs::symlink_metadata(&to) {
                    Ok(_) => super::remove_entry(&to),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                })
                .and_then(|()| move_folder(&path, &to))
                .and_then(|()| fs::write(quarantined_at(&root, &name), unix_seconds().to_string()));
            match moved {
                Ok(()) => tracing::info!(
                    %engine,
                    skill = %name,
                    to = %to.display(),
                    "quarantined a skill Artisan didn't install"
                ),
                Err(error) => tracing::warn!(
                    %engine,
                    skill = %path.display(),
                    %error,
                    "couldn't quarantine a skill Artisan didn't install"
                ),
            }
        }
    }
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    let cutoff = unix_seconds().saturating_sub(QUARANTINE_DAYS * 24 * 60 * 60);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let stamp = quarantined_at(&root, &name);
        let at = fs::read_to_string(&stamp)
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok());
        if at.is_some_and(|at| at < cutoff) {
            let removed = super::remove_entry(&entry.path()).and_then(|()| fs::remove_file(&stamp));
            if let Err(error) = removed {
                tracing::warn!(%engine, skill = %name, %error, "couldn't drop an expired quarantined skill");
            }
        }
    }
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
fn first_party(skills_dir: &Path, materialize: bool) -> io::Result<Vec<(String, PathBuf)>> {
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
        if materialize {
            for (path, bytes) in skill.files {
                write_if_changed(&folder.join(path), bytes)?;
            }
        }
        placed.push((skill.folder.to_owned(), folder));
    }
    Ok(placed)
}

/// The skills Forge manages, first-party first, then the user's by folder.
fn managed(c: &Customisation) -> Result<Vec<Managed>> {
    let mut skills = Vec::new();
    let mut budget = CatalogBudget::default();
    if c.first_party {
        let placed = first_party(&c.skills_dir, false).with_context(|| {
            format!(
                "writing Artisan's skills into {}",
                first_party_root(&c.skills_dir).display()
            )
        })?;
        for (folder_name, folder) in placed {
            let text = match read_skill_header(&folder.join("SKILL.md")) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => FIRST_PARTY
                    .iter()
                    .find(|skill| skill.folder == folder_name)
                    .and_then(|skill| skill.files.iter().find(|(path, _)| *path == "SKILL.md"))
                    .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
                    .context("first-party skill has no embedded metadata")?,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("read first-party skill {folder_name}"));
                }
            };
            let (name, description) = skill_metadata(&text)
                .with_context(|| format!("invalid first-party skill metadata: {folder_name}"))?;
            budget.admit(name.len() + description.len() + folder.as_os_str().len())?;
            let id = first_party_id(&folder_name);
            skills.push(Managed {
                enabled: !c.disabled_skills.contains(&id),
                id,
                name,
                description,
                origin: SkillOrigin::FirstParty,
                folder,
                shadowed: false,
            });
        }
    }
    let mut users = Vec::new();
    let entries = match fs::read_dir(&c.skills_dir) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read managed skills {}", c.skills_dir.display()));
        }
    };
    if let Some(entries) = entries {
        for entry in entries {
            let entry = entry
                .with_context(|| format!("read managed skill entry {}", c.skills_dir.display()))?;
            let folder_name = entry.file_name().to_string_lossy().into_owned();
            if folder_name.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            let Ok(text) = read_skill_header(&entry.path().join("SKILL.md")) else {
                continue;
            };
            let Some((name, description)) = skill_metadata_in(&text, Some(&folder_name)) else {
                continue;
            };
            budget.admit(name.len() + description.len() + entry.path().as_os_str().len())?;
            let id = user_id(&folder_name);
            users.push(Managed {
                enabled: !c.disabled_skills.contains(&id),
                shadowed: skills
                    .iter()
                    .any(|skill: &Managed| skill.folder.file_name() == Some(folder_name.as_ref())),
                id,
                name,
                description,
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
    if c.first_party {
        first_party(&c.skills_dir, true)?;
    }
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
    name: String,
    description: String,
    digest: String,
    file_count: u32,
    byte_count: u64,
}

/// Real skill folders in the engines' skills folders that aren't Forge's.
fn discovered(
    homes: &[(EngineId, Option<PathBuf>)],
    quarantine: Option<&Path>,
    owned: &[PathBuf],
    cached: bool,
    wanted_folder: Option<&str>,
) -> Result<Vec<Found>> {
    let mut found = Vec::new();
    let mut budget = CatalogBudget::default();
    // Between passes a foreign skill can still sit in the engine's folder;
    // after one it is in the quarantine. Both are listed for Install.
    let folders = homes.iter().flat_map(|(engine, home)| {
        let native = home
            .as_deref()
            .and_then(|home| paths(*engine, home))
            .map(|paths| paths.skills);
        let held = quarantine.map(|root| root.join(engine.as_str()));
        native
            .into_iter()
            .chain(held)
            .map(|folder| (*engine, folder))
    });
    for (engine, skills) in folders {
        let engine = &engine;
        let entries = match fs::read_dir(&skills) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read {engine} native skills {}", skills.display()));
            }
        };
        for entry in entries {
            let entry = entry.with_context(|| {
                format!("read {engine} native skill entry {}", skills.display())
            })?;
            let folder_name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if wanted_folder.is_some_and(|wanted| wanted != folder_name) {
                continue;
            }
            if folder_name.starts_with('.')
                || !matches!(inspect(&path, owned), Entry::Theirs)
                || !path.join("SKILL.md").is_file()
            {
                continue;
            }
            let Ok(header) = read_skill_header(&path.join("SKILL.md")) else {
                continue;
            };
            let Some((name, description)) = skill_metadata_in(&header, Some(&folder_name)) else {
                tracing::warn!(folder = %path.display(), "invalid native skill metadata");
                continue;
            };
            budget.admit(
                name.len() + description.len() + folder_name.len() + path.as_os_str().len() + 64,
            )?;
            let Ok((digest, file_count, byte_count)) = measure_inner(&path, cached) else {
                continue;
            };
            found.push(Found {
                engine: *engine,
                folder: path,
                folder_name,
                name,
                description,
                digest,
                file_count,
                byte_count,
            });
        }
    }
    Ok(found)
}

/// Bounded, link-free skill content. The engine may read links; adoption
/// explicitly rejects them so the managed copy cannot silently omit assets.
pub(super) fn entries(folder: &Path) -> io::Result<Vec<walkdir::DirEntry>> {
    if fs::symlink_metadata(folder)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "linked skill roots are unsupported",
        ));
    }
    let mut entries = Vec::new();
    let mut bytes = 0u64;
    for entry in walkdir::WalkDir::new(folder)
        .min_depth(1)
        .max_depth(65)
        .follow_links(false)
        .follow_root_links(false)
        .max_open(8)
    {
        let entry = entry.map_err(io::Error::other)?;
        if entry.file_name() == COPY_MARKER {
            continue;
        }
        if entry.depth() > 64 || entry.file_type().is_symlink() || entries.len() >= 10000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "skill contains links or exceeds depth/file bounds",
            ));
        }
        bytes = bytes.saturating_add(entry.metadata().map_err(io::Error::other)?.len());
        if bytes > 128 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "skill exceeds 128 MiB",
            ));
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn counts(folder: &Path) -> io::Result<(u32, u64)> {
    let entries = entries(folder)?;
    let mut count = 0;
    let mut bytes = 0;
    for entry in entries
        .into_iter()
        .filter(|entry| entry.file_type().is_file())
    {
        count += 1;
        bytes += entry.metadata().map_err(io::Error::other)?.len();
    }
    Ok((count, bytes))
}

pub(super) fn measure(folder: &Path) -> io::Result<(String, u32, u64)> {
    measure_inner(folder, false)
}

#[derive(Clone)]
struct CachedMeasure {
    generation: Vec<(PathBuf, u64, std::time::SystemTime)>,
    value: (String, u32, u64),
}

pub(super) fn measure_inner(folder: &Path, cached: bool) -> io::Result<(String, u32, u64)> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<BTreeMap<PathBuf, CachedMeasure>>> =
        std::sync::OnceLock::new();
    use std::io::Read;
    let cache = CACHE.get_or_init(Default::default);

    let mut files = entries(folder)?
        .into_iter()
        .filter(|entry| entry.file_type().is_file())
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.path().cmp(right.path()));
    let generation = if cached {
        files
            .iter()
            .map(|entry| {
                let metadata = entry.metadata().map_err(io::Error::other)?;
                Ok((
                    entry.path().to_owned(),
                    metadata.len(),
                    metadata.modified()?,
                ))
            })
            .collect::<io::Result<Vec<_>>>()
            .ok()
            .filter(|generation| {
                generation
                    .iter()
                    .map(|(path, _, _)| path.as_os_str().len() + 48)
                    .sum::<usize>()
                    <= 64 * 1024
            })
    } else {
        None
    };
    if let Some(generation) = &generation {
        let cached = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = cached
            .get(folder)
            .filter(|entry| &entry.generation == generation)
        {
            return Ok(entry.value.clone());
        }
    }
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 8192];
    for entry in &files {
        let relative = entry
            .path()
            .strip_prefix(folder)
            .map_err(io::Error::other)?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(
            entry
                .metadata()
                .map_err(io::Error::other)?
                .len()
                .to_le_bytes(),
        );
        let mut file = fs::File::open(entry.path())?;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            bytes += read as u64;
            if bytes > 128 * 1024 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "skill grew beyond 128 MiB",
                ));
            }
            hasher.update(&buffer[..read]);
        }
    }
    let count = u32::try_from(files.len()).unwrap_or(u32::MAX);
    let value = (hex::encode(hasher.finalize()), count, bytes);
    if let Some(generation) = generation {
        let mut cached = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cached.len() >= 128 && !cached.contains_key(folder) {
            cached.clear();
        }
        cached.insert(
            folder.to_owned(),
            CachedMeasure {
                generation,
                value: value.clone(),
            },
        );
    }
    Ok(value)
}

/// Read-only catalog of managed and discovered skills and their actual engine state.
/// Mutation/installation calls reconcile_skills explicitly.
///
/// # Errors
///
/// When Artisan's own skills can't be written out.
pub fn skill_catalog(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
) -> Result<Vec<CatalogSkill>> {
    let managed = managed(c)?;
    let owned = owned_roots(&c.skills_dir);
    let mut catalog: Vec<CatalogSkill> = managed
        .iter()
        .map(|skill| -> Result<CatalogSkill> {
            let (file_count, byte_count) = if skill.origin == SkillOrigin::FirstParty
                && fs::symlink_metadata(&skill.folder)
                    .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
            {
                let source = FIRST_PARTY
                    .iter()
                    .find(|source| {
                        skill
                            .folder
                            .file_name()
                            .is_some_and(|name| name == source.folder)
                    })
                    .context("missing embedded first-party skill")?;
                (
                    u32::try_from(source.files.len()).unwrap_or(u32::MAX),
                    source
                        .files
                        .iter()
                        .map(|(_, bytes)| bytes.len() as u64)
                        .sum(),
                )
            } else {
                counts(&skill.folder)
                    .with_context(|| format!("inspect managed skill {}", skill.folder.display()))?
            };
            Ok(CatalogSkill {
                id: skill.id.clone(),
                name: skill.name.clone(),
                description: skill.description.clone(),
                origin: skill.origin,
                enabled: skill.enabled,
                file_count,
                byte_count,
                engines: homes
                    .iter()
                    .map(|(engine, home)| managed_state(skill, *engine, home.as_deref(), &owned))
                    .collect(),
            })
        })
        .collect::<Result<_>>()?;
    let mut budget = CatalogBudget::default();
    for skill in &catalog {
        budget.admit(catalog_metadata(skill))?;
    }
    for skill in discovered_entries(homes, &quarantine_root(&c.skills_dir), &owned)? {
        budget.admit(catalog_metadata(&skill))?;
        catalog.push(skill);
    }
    Ok(catalog)
}

fn unavailable_reason(engine: EngineId) -> String {
    format!(
        "{} has no skills folder Artisan can manage.",
        engine.display_name()
    )
}

/// Where a managed skill stands in one engine.
fn managed_state(
    skill: &Managed,
    engine: EngineId,
    home: Option<&Path>,
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
    let Some(paths) = paths(engine, home.unwrap_or_else(|| Path::new(""))) else {
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
    match inspect(&paths.skills.join(&link_name), owned) {
        Entry::Ours(Some(target)) if target == skill.folder => {
            state(SkillEngineStatus::Synced, None)
        }
        Entry::Ours(None) => state(SkillEngineStatus::Synced, None),
        Entry::Theirs => state(
            SkillEngineStatus::Conflict,
            Some(format!(
                "{} has its own skill named {link_name}. Remove it to let Artisan manage it.",
                engine.display_name()
            )),
        ),
        Entry::Ours(Some(_)) | Entry::Missing => state(
            SkillEngineStatus::Failed,
            Some(format!(
                "Artisan could not link the skill in {}'s skills folder.",
                engine.display_name()
            )),
        ),
    }
}

/// The skills engines have on their own; copies with the same name and
/// files are one skill found in several engines.
fn discovered_entries(
    homes: &[(EngineId, Option<PathBuf>)],
    quarantine: &Path,
    owned: &[PathBuf],
) -> Result<Vec<CatalogSkill>> {
    let mut groups: BTreeMap<(String, String), Vec<Found>> = BTreeMap::new();
    for found in discovered(homes, Some(quarantine), owned, true, None)? {
        groups
            .entry((found.folder_name.clone(), found.digest.clone()))
            .or_default()
            .push(found);
    }
    Ok(groups
        .into_iter()
        .map(|((folder_name, digest), copies)| {
            let name = copies[0].name.clone();
            let description = copies[0].description.clone();
            let (file_count, byte_count) = (copies[0].file_count, copies[0].byte_count);
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
                name,
                description,
                origin: SkillOrigin::Discovered,
                enabled: false,
                file_count,
                byte_count,
                engines,
            }
        })
        .collect())
}

fn discovered_id(folder_name: &str, digest: &str) -> String {
    format!("discovered.{folder_name}.{}", &digest[..12])
}

/// Every copy of the skill `id` names, quarantined or still in an engine.
fn found_copies(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
    id: &str,
) -> Result<Vec<Found>> {
    let (wanted_folder, _digest) = id
        .strip_prefix("discovered.")
        .and_then(|id| id.rsplit_once('.'))
        .filter(|(folder, digest)| {
            !folder.is_empty()
                && digest.len() == 12
                && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .context("invalid discovered skill identity")?;
    let quarantine = quarantine_root(&c.skills_dir);
    let owned = owned_roots(&c.skills_dir);
    Ok(
        discovered(homes, Some(&quarantine), &owned, false, Some(wanted_folder))?
            .into_iter()
            .filter(|found| discovered_id(&found.folder_name, &found.digest) == id)
            .collect(),
    )
}

/// Deletes every copy of a quarantined skill. Returns its name.
///
/// # Errors
///
/// When the skill is gone or a copy can't be deleted.
pub fn discard_skill(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
    id: &str,
) -> Result<String> {
    let copies = found_copies(homes, c, id)?;
    let Some(first) = copies.first() else {
        bail!("That skill is no longer in quarantine.");
    };
    let name = first.name.clone();
    for copy in &copies {
        super::remove_entry(&copy.folder)
            .with_context(|| format!("deleting {}", copy.folder.display()))?;
        if let Some(parent) = copy.folder.parent() {
            // The quarantine marker may never have been written.
            fs::remove_file(quarantined_at(parent, &copy.folder_name)).ok();
        }
    }
    Ok(name)
}

/// Installs a quarantined skill, or one still in an engine's folder.
///
/// Copies it into the user's skills folder and moves every identical copy
/// into `<data>/extensions/backups`, so Forge's link takes its place.
/// Returns the folder name.
///
/// # Errors
///
/// When the skill is gone or changed, its name is taken, or copying fails.
pub fn adopt_skill(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
    id: &str,
) -> Result<String> {
    let copies = found_copies(homes, c, id)?;
    let Some(first) = copies.first() else {
        bail!("That skill is no longer in quarantine.");
    };
    let name = first.folder_name.clone();
    let taken = c.skills_dir.join(&name).exists()
        || (c.first_party && FIRST_PARTY.iter().any(|skill| skill.folder == name));
    if taken {
        bail!("Artisan already manages a skill named {name}.");
    }
    fs::create_dir_all(&c.skills_dir)
        .with_context(|| format!("creating {}", c.skills_dir.display()))?;
    let staging = c
        .skills_dir
        .join(format!(".adopt-{name}-{}", artisan_api::now()));
    fs::create_dir(&staging).context("creating skill adoption staging")?;
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let result = (|| -> Result<()> {
        import_folder(&first.folder, &staging, &name)?;
        let backups = backups_root(&c.skills_dir);
        let stamp = artisan_api::now();
        for copy in &copies {
            let to = backups.join(format!("{}-{name}-{stamp}", copy.engine));
            move_folder(&copy.folder, &to).with_context(|| {
                format!("backing up {}'s skill {name}", copy.engine.display_name())
            })?;
            moved.push((copy.folder.clone(), to));
        }
        fs::rename(staging.join(&name), c.skills_dir.join(&name))
            .context("publishing adopted skill")?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for (original, backup) in moved.into_iter().rev() {
            if let Err(restore) = move_folder(&backup, &original) {
                rollback_errors.push(format!(
                    "{} remains backed up at {}: {restore}",
                    original.display(),
                    backup.display()
                ));
            }
        }
        fs::remove_dir_all(&staging).ok();
        if rollback_errors.is_empty() {
            return Err(error);
        }
        return Err(error.context(rollback_errors.join("; ")));
    }
    fs::remove_dir_all(staging).ok();
    for copy in &copies {
        if let Some(parent) = copy.folder.parent() {
            // The quarantine marker may never have been written.
            fs::remove_file(quarantined_at(parent, &copy.folder_name)).ok();
        }
    }
    Ok(name)
}

/// Copies a skill folder into Forge's skills folder, so Forge keeps it in
/// every engine, and returns its name. `replace` swaps out the user's skill
/// of the same name, which goes to the backups first.
///
/// # Errors
///
/// Phrased for the agent or user: no `SKILL.md`, a name Artisan already
/// uses, a folder inside an engine's skills folder, or a failed copy.
pub fn install_skill(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
    from: &Path,
    replace: bool,
) -> Result<String> {
    let from = from
        .canonicalize()
        .with_context(|| format!("{} isn't a folder Forge can read", from.display()))?;
    let text = read_skill_header(&from.join("SKILL.md")).with_context(|| {
        format!(
            "{} needs a SKILL.md that starts with YAML front matter",
            from.display()
        )
    })?;
    let (name, _) = skill_metadata(&text)
        .context("SKILL.md's front matter needs a `name` without spaces or slashes")?;
    if c.first_party && FIRST_PARTY.iter().any(|skill| skill.folder == name) {
        bail!("{name} is one of Artisan's own skills; pick another name.");
    }
    let inside = |root: &Path| root.canonicalize().is_ok_and(|root| from.starts_with(root));
    if inside(&c.skills_dir) {
        bail!("{name} is already installed.");
    }
    let engine_skills = homes
        .iter()
        .filter_map(|(engine, home)| paths(*engine, home.as_deref()?))
        .map(|paths| paths.skills);
    for skills in engine_skills {
        if inside(&skills) {
            bail!(
                "{} is inside an engine's skills folder, where Artisan removes what it didn't put. Write the skill in another folder and install that.",
                from.display()
            );
        }
    }
    let to = c.skills_dir.join(&name);
    if to.exists() && !replace {
        bail!("Artisan already has a skill named {name}; pass replace to update it.");
    }
    fs::create_dir_all(&c.skills_dir)
        .with_context(|| format!("creating {}", c.skills_dir.display()))?;
    let stamp = artisan_api::now();
    let staging = c.skills_dir.join(format!(".install-{name}-{stamp}"));
    let backup = backups_root(&c.skills_dir).join(format!("{name}-{stamp}"));
    let result = (|| -> Result<()> {
        copy_dir(&from, &staging).with_context(|| format!("copying {}", from.display()))?;
        let replaced = to.exists();
        if replaced {
            move_folder(&to, &backup).with_context(|| format!("backing up the old {name}"))?;
        }
        if let Err(error) = fs::rename(&staging, &to) {
            if replaced && let Err(restore) = move_folder(&backup, &to) {
                tracing::warn!(skill = %name, backup = %backup.display(), %restore, "couldn't restore the old skill; it stays in the backups");
            }
            return Err(error).context("placing the skill");
        }
        Ok(())
    })();
    if result.is_err() {
        fs::remove_dir_all(&staging).ok();
    }
    result.map(|()| name)
}

fn move_folder(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            if let Err(error) = copy_dir(from, to) {
                fs::remove_dir_all(to).ok();
                return Err(error);
            }
            if let Err(error) = fs::remove_dir_all(from) {
                // Restore any entries already removed; keep the complete backup
                // if the source itself cannot be repaired.
                if let Err(restore) = copy_dir(to, from) {
                    return Err(io::Error::new(
                        restore.kind(),
                        format!(
                            "{error}; restore failed: {restore}; complete backup retained at {}",
                            to.display()
                        ),
                    ));
                }
                fs::remove_dir_all(to).ok();
                return Err(io::Error::new(
                    error.kind(),
                    format!("{error}; original source restored"),
                ));
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod measurement_tests {
    use super::*;
    #[test]
    fn invalid_native_metadata_is_never_adoptable() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let native = home.join(".claude/skills/broken");
        std::fs::create_dir_all(&native).unwrap();
        std::fs::write(
            native.join("SKILL.md"),
            "---\nname: [not a scalar]\ndescription: bad\n---\n",
        )
        .unwrap();
        let customisation = Customisation {
            skills_dir: directory.path().join("managed"),
            ..Default::default()
        };
        let homes = [(EngineId::Claude, Some(home))];
        assert!(skill_catalog(&homes, &customisation).unwrap().is_empty());
        let digest = measure(&native).unwrap().0;
        assert!(adopt_skill(&homes, &customisation, &discovered_id("broken", &digest)).is_err());
        assert!(!customisation.skills_dir.exists());
        assert!(native.is_dir());
    }

    #[test]
    fn catalog_generation_reuses_content_but_adoption_always_revalidates_it() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("resource.txt");
        fs::write(&file, "abc").unwrap();
        let modified = fs::metadata(&file).unwrap().modified().unwrap();
        let first = measure_inner(folder.path(), true).unwrap();
        assert_eq!(measure_inner(folder.path(), true).unwrap(), first);
        fs::write(&file, "xyz").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        // An external tool may preserve timestamps; adoption must not trust cache.
        assert_ne!(measure(folder.path()).unwrap().0, first.0);
        fs::write(&file, "a longer resource").unwrap();
        assert_ne!(measure_inner(folder.path(), true).unwrap(), first);
    }
}

#[cfg(test)]
#[cfg(unix)]
mod managed_count_tests {
    use super::*;
    #[test]
    fn unsupported_managed_tree_does_not_publish_fabricated_zero_counts() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("skills");
        let skill = root.join("demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: Demonstration\n---\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(".", skill.join("cycle")).unwrap();
        let settings = Customisation {
            skills_dir: root,
            ..Customisation::default()
        };
        let error = skill_catalog(&[], &settings).unwrap_err();
        assert!(format!("{error:#}").contains("links"));
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    fn settings(root: &Path) -> Customisation {
        Customisation {
            skills_dir: root.to_owned(),
            ..Customisation::default()
        }
    }
    #[test]
    fn empty_native_directories_do_not_consume_metadata_admission() {
        let folder = tempfile::tempdir().unwrap();
        let home = folder.path().join("home");
        let native = home.join(".claude/skills");
        for i in 0..=MAX_CATALOG_RECORDS {
            fs::create_dir_all(native.join(format!("empty-{i}"))).unwrap();
        }
        assert!(
            skill_catalog(
                &[(EngineId::Claude, Some(home))],
                &settings(&folder.path().join("managed"))
            )
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn cumulative_native_metadata_is_an_explicit_error_not_a_truncated_catalog() {
        let folder = tempfile::tempdir().unwrap();
        let home = folder.path().join("home");
        let native = home.join(".claude/skills");
        for i in 0..100 {
            let skill = native.join(format!("skill-{i}"));
            fs::create_dir_all(&skill).unwrap();
            fs::write(
                skill.join("SKILL.md"),
                format!(
                    "---\nname: skill-{i}\ndescription: {}\n---\n",
                    "x".repeat(48 * 1024)
                ),
            )
            .unwrap();
        }
        let error = skill_catalog(
            &[(EngineId::Claude, Some(home))],
            &settings(&folder.path().join("managed")),
        )
        .unwrap_err();
        assert!(error.to_string().contains("metadata_bytes="));
        assert!(native.join("skill-99/SKILL.md").exists());
    }
    #[test]
    fn selecting_native_folder_avoids_hashing_unrelated_catalogs() {
        let folder = tempfile::tempdir().unwrap();
        let home = folder.path().join("home");
        let native = home.join(".claude/skills");
        for name in ["selected", "unrelated"] {
            let skill = native.join(name);
            fs::create_dir_all(&skill).unwrap();
            fs::write(
                skill.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Native\n---\n"),
            )
            .unwrap();
        }
        fs::write(
            native.join("unrelated/content.bin"),
            b"unselected native content",
        )
        .unwrap();
        let homes = [(EngineId::Claude, Some(home))];
        assert_eq!(discovered(&homes, None, &[], false, None).unwrap().len(), 2);
        let selected = discovered(&homes, None, &[], false, Some("selected")).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].folder_name, "selected");
    }
}

#[cfg(test)]
mod directory_failure_tests {
    use super::*;
    #[test]
    fn inaccessible_skill_roots_cannot_look_like_empty_desired_state() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("managed");
        fs::write(&root, b"not a directory").unwrap();
        let settings = Customisation {
            skills_dir: root,
            ..Customisation::default()
        };
        assert!(skill_catalog(&[], &settings).is_err());
        assert!(
            wanted(&settings).is_err(),
            "reconciliation must not delete links after a failed source read"
        );
        let home = folder.path().join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude/skills"), b"not a directory").unwrap();
        let settings = Customisation {
            skills_dir: folder.path().join("missing"),
            ..Customisation::default()
        };
        assert!(skill_catalog(&[(EngineId::Claude, Some(home))], &settings).is_err());
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod adoption_failure_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    const SKILL: &str =
        "---\nname: recoverable\ndescription: adoption fixture\n---\nKeep every instruction.\n";

    fn permission_failures_are_enforced() -> bool {
        let status = fs::read_to_string("/proc/self/status").unwrap();
        if status.lines().any(|line| line.starts_with("Uid:\t0\t")) {
            eprintln!("permission failure fixture skipped for root privileges");
            return false;
        }
        true
    }

    #[test]
    fn a_later_backup_failure_restores_every_original_and_allows_normal_retry() {
        if !permission_failures_are_enforced() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("claude");
        let codex = dir.path().join("codex");
        let claude_copy = claude.join(".claude/skills/recoverable");
        let codex_copy = codex.join(".codex/skills/recoverable");
        for copy in [&claude_copy, &codex_copy] {
            fs::create_dir_all(copy).unwrap();
            fs::write(copy.join("SKILL.md"), SKILL).unwrap();
        }
        let config = Customisation {
            skills_dir: dir.path().join("extensions/skills"),
            ..Default::default()
        };
        let homes = [
            (EngineId::Claude, Some(claude)),
            (EngineId::Codex, Some(codex)),
        ];
        let digest = measure(&claude_copy).unwrap().0;
        let id = discovered_id("recoverable", &digest);
        let parent = codex_copy.parent().unwrap();
        let permissions = fs::metadata(parent).unwrap().permissions();
        fs::set_permissions(parent, fs::Permissions::from_mode(0o555)).unwrap();
        let result = adopt_skill(&homes, &config, &id);
        fs::set_permissions(parent, permissions).unwrap();
        assert!(
            result.is_err(),
            "the second backup must fail after the first moved"
        );
        assert!(!config.skills_dir.join("recoverable").exists());
        for copy in [&claude_copy, &codex_copy] {
            assert_eq!(fs::read_to_string(copy.join("SKILL.md")).unwrap(), SKILL);
        }
        assert_eq!(
            fs::read_dir(backups_root(&config.skills_dir))
                .unwrap()
                .count(),
            0,
            "successful rollback leaves no stranded backup"
        );
        assert_eq!(adopt_skill(&homes, &config, &id).unwrap(), "recoverable");
        assert_eq!(
            fs::read_to_string(config.skills_dir.join("recoverable/SKILL.md")).unwrap(),
            SKILL
        );
        assert!(!claude_copy.exists() && !codex_copy.exists());
        assert_eq!(
            fs::read_dir(backups_root(&config.skills_dir))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn cross_device_partial_delete_failure_restores_content_before_retry() {
        if !permission_failures_are_enforced() {
            return;
        }
        let source_root = tempfile::tempdir().unwrap();
        let backup_root = tempfile::tempdir_in("/dev/shm").unwrap();
        let source = source_root.path().join("recoverable");
        let backup = backup_root.path().join("backup");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("SKILL.md"), SKILL).unwrap();
        let permissions = fs::metadata(source_root.path()).unwrap().permissions();
        fs::set_permissions(source_root.path(), fs::Permissions::from_mode(0o555)).unwrap();
        let result = move_folder(&source, &backup);
        fs::set_permissions(source_root.path(), permissions).unwrap();
        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("original source restored"),
            "must exercise cross-device copy followed by failed deletion and successful restoration: {error}"
        );
        assert_eq!(fs::read_to_string(source.join("SKILL.md")).unwrap(), SKILL);
        assert!(!backup.exists());
        move_folder(&source, &backup).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(backup.join("SKILL.md")).unwrap(), SKILL);
    }
}

#[cfg(test)]
mod install_tests {
    use super::*;

    fn skill(folder: &Path, description: &str) {
        fs::create_dir_all(folder).unwrap();
        fs::write(
            folder.join("SKILL.md"),
            format!("---\nname: release-notes\ndescription: {description}\n---\n"),
        )
        .unwrap();
    }

    #[test]
    fn an_installed_skill_reaches_every_engine_and_replace_keeps_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let homes = [
            (EngineId::Claude, Some(dir.path().join("claude"))),
            (EngineId::Codex, Some(dir.path().join("codex"))),
        ];
        let config = Customisation {
            skills_dir: dir.path().join("extensions/skills"),
            ..Default::default()
        };
        let draft = dir.path().join("project/release-notes");
        skill(&draft, "First");

        assert_eq!(
            install_skill(&homes, &config, &draft, false).unwrap(),
            "release-notes"
        );
        super::super::reconcile_skills(&homes, &config).unwrap();
        for linked in [
            dir.path()
                .join("claude/.claude/skills/release-notes/SKILL.md"),
            dir.path()
                .join("codex/.codex/skills/release-notes/SKILL.md"),
        ] {
            assert!(fs::read_to_string(linked).unwrap().contains("First"));
        }

        skill(&draft, "Second");
        let error = install_skill(&homes, &config, &draft, false).unwrap_err();
        assert!(format!("{error:#}").contains("pass replace"));
        install_skill(&homes, &config, &draft, true).unwrap();
        let installed = config.skills_dir.join("release-notes/SKILL.md");
        assert!(fs::read_to_string(installed).unwrap().contains("Second"));
        assert_eq!(
            fs::read_dir(backups_root(&config.skills_dir))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn a_skill_written_into_an_engine_folder_is_refused_with_the_way_out() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("claude");
        let config = Customisation {
            skills_dir: dir.path().join("extensions/skills"),
            ..Default::default()
        };
        let native = home.join(".claude/skills/release-notes");
        skill(&native, "Written in place");

        let error =
            install_skill(&[(EngineId::Claude, Some(home))], &config, &native, false).unwrap_err();
        assert!(format!("{error:#}").contains("Write the skill in another folder"));
        assert!(!config.skills_dir.join("release-notes").exists());
    }
}

#[cfg(test)]
mod quarantine_tests {
    use super::*;

    fn skill(folder: &Path, name: &str) {
        fs::create_dir_all(folder).unwrap();
        fs::write(
            folder.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} things\n---\n"),
        )
        .unwrap();
    }

    #[test]
    fn a_skill_artisan_didnt_install_is_quarantined_then_installed_or_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("claude");
        let codex = dir.path().join("codex");
        let homes = [
            (EngineId::Claude, Some(claude.clone())),
            (EngineId::Codex, Some(codex.clone())),
        ];
        let config = Customisation {
            skills_dir: dir.path().join("extensions/skills"),
            ..Default::default()
        };
        skill(&config.skills_dir.join("pdf"), "pdf");
        let stray = claude.join(".claude/skills/animation");
        skill(&stray, "animation");
        skill(&codex.join(".codex/skills/animation"), "animation");
        skill(&claude.join(".claude/skills/scratch"), "scratch");
        let system = codex.join(".codex/skills/.system");
        skill(&system, "system");

        super::super::reconcile_skills(&homes, &config).unwrap();

        // The engines keep only what Artisan installed, and Codex's own.
        assert!(!stray.exists());
        assert!(claude.join(".claude/skills/pdf/SKILL.md").exists());
        assert!(system.exists());
        let quarantine = quarantine_root(&config.skills_dir);
        assert!(quarantine.join("claude/animation/SKILL.md").exists());
        assert!(quarantine.join("codex/animation/SKILL.md").exists());

        // One entry per skill, from both engines, ready to install.
        let catalog = skill_catalog(&homes, &config).unwrap();
        let found: Vec<&CatalogSkill> = catalog
            .iter()
            .filter(|skill| skill.origin == SkillOrigin::Discovered)
            .collect();
        assert_eq!(found.len(), 2);
        let animation = found
            .iter()
            .find(|skill| skill.name == "animation")
            .unwrap();
        assert!(
            animation
                .engines
                .iter()
                .all(|state| state.status == SkillEngineStatus::Native)
        );

        assert_eq!(
            adopt_skill(&homes, &config, &animation.id).unwrap(),
            "animation"
        );
        super::super::reconcile_skills(&homes, &config).unwrap();
        assert!(claude.join(".claude/skills/animation/SKILL.md").exists());
        assert!(!quarantine.join("claude/animation").exists());

        let scratch = skill_catalog(&homes, &config)
            .unwrap()
            .into_iter()
            .find(|skill| skill.name == "scratch")
            .unwrap();
        discard_skill(&homes, &config, &scratch.id).unwrap();
        assert!(!quarantine.join("claude/scratch").exists());
        assert!(
            skill_catalog(&homes, &config)
                .unwrap()
                .iter()
                .all(|skill| skill.origin != SkillOrigin::Discovered)
        );
    }
}
