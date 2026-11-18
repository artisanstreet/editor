//! Which skills go into the engines, and where each stands in each engine.
//!
//! Three sources (master's `extension_sync`): Artisan's own skills, embedded
//! in Forge from the repository's `skills/`; the user's own, one folder per
//! skill in `<data>/extensions/skills`; and skills an engine has in its own
//! skills folder, which Forge only reports until the user adopts them.

use super::{COPY_MARKER, Entry, inspect, paths, skill_metadata, skill_metadata_in};
use super::{Customisation, ImportError, copy_dir, import_folder, read_skill_header};
use super::{discard_leftover, remove_file_if_present, write_if_changed};
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
    fn admit(&mut self, bytes: usize) -> Result<(), ReadSkillsError> {
        self.records += 1;
        self.bytes = self.bytes.saturating_add(bytes);
        if self.records > MAX_CATALOG_RECORDS || self.bytes > MAX_CATALOG_METADATA_BYTES {
            return Err(ReadSkillsError::Paging {
                records: self.records,
                bytes: self.bytes,
            });
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadSkillsError {
    #[error(
        "skill catalog requires paging: records={records}/{MAX_CATALOG_RECORDS} metadata_bytes={bytes}/{MAX_CATALOG_METADATA_BYTES}"
    )]
    Paging { records: usize, bytes: usize },
    #[error("write Artisan's skills into {}", path.display())]
    WriteFirstParty {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("read first-party skill {folder}")]
    ReadFirstParty {
        folder: String,
        #[source]
        source: io::Error,
    },
    #[error("invalid first-party skill metadata: {folder}")]
    FirstPartyMetadata { folder: String },
    #[error("read managed skills {}", path.display())]
    ReadManaged {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("read {engine} native skills {}", path.display())]
    ReadNative {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("recovering an interrupted skill replacement")]
    Recover(#[source] RecoverSkillError),
    #[error("inspect managed skill {}", path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
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

pub(super) fn quarantine_foreign(
    engine: EngineId,
    engine_skills: &Path,
    skills_dir: &Path,
    owned: &[PathBuf],
) {
    let root = quarantine_root(skills_dir).join(engine.as_str());
    recover_quarantine(engine, &root);
    if let Ok(entries) = fs::read_dir(engine_skills) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if name.starts_with('.') || !matches!(inspect(&path, owned), Entry::Theirs) {
                continue;
            }
            let to = root.join(&name);
            let moved = fs::create_dir_all(&root)
                .and_then(|()| replace_quarantined(&path, &to, &root))
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

fn replace_quarantined(found: &Path, to: &Path, root: &Path) -> io::Result<()> {
    let name = super::link_name(to);
    let incoming = root.join(super::staged_name("incoming", name));
    move_folder(found, &incoming)?;
    let retired = root.join(super::staged_name("retired", name));
    super::swap_in(&incoming, to, &retired).inspect_err(|_| {
        if let Err(error) = move_folder(&incoming, found) {
            tracing::warn!(skill = %found.display(), kept = %incoming.display(), %error, "couldn't put a skill back after a failed quarantine");
        }
    })
}

fn quarantine_leftovers(engine: EngineId, root: &Path, kind: &str) -> Vec<(PathBuf, String)> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            tracing::warn!(%engine, path = %root.display(), %error, "couldn't look for interrupted quarantines");
            return Vec::new();
        }
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let name = super::other_process_leftover(&file_name, kind)?.to_owned();
            Some((entry.path(), name))
        })
        .collect()
}

fn recover_quarantine(engine: EngineId, root: &Path) {
    let leftovers = |kind| quarantine_leftovers(engine, root, kind);
    for (incoming, name) in leftovers("incoming") {
        let to = root.join(&name);
        let retired = root.join(super::staged_name("retired", super::link_name(&to)));
        if let Err(error) = super::swap_in(&incoming, &to, &retired) {
            tracing::warn!(%engine, skill = %name, kept = %incoming.display(), %error, "couldn't finish an interrupted quarantine");
        }
    }
    for (retired, name) in leftovers("retired") {
        let to = root.join(&name);
        let settled = match fs::symlink_metadata(&to) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::rename(&retired, &to),
            Err(error) => Err(error),
            Ok(_) => super::remove_entry(&retired),
        };
        if let Err(error) = settled {
            tracing::warn!(%engine, skill = %name, path = %retired.display(), %error, "couldn't settle an interrupted quarantine");
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
        .map(|root| without_verbatim_prefix(&root))
        .filter(|root| root.is_dir())
}

fn without_verbatim_prefix(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{share}"));
    }
    text.strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
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

fn place_first_party(
    skills_dir: &Path,
    materialize: bool,
) -> Result<Vec<(String, PathBuf)>, ReadSkillsError> {
    first_party(skills_dir, materialize).map_err(|source| ReadSkillsError::WriteFirstParty {
        path: first_party_root(skills_dir),
        source,
    })
}

fn managed(c: &Customisation) -> Result<Vec<Managed>, ReadSkillsError> {
    let mut skills = Vec::new();
    let mut budget = CatalogBudget::default();
    if c.first_party {
        let placed = place_first_party(&c.skills_dir, false)?;
        for (folder_name, folder) in placed {
            let text = match read_skill_header(&folder.join("SKILL.md")) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => FIRST_PARTY
                    .iter()
                    .find(|skill| skill.folder == folder_name)
                    .and_then(|skill| skill.files.iter().find(|(path, _)| *path == "SKILL.md"))
                    .map_or_else(
                        || panic!("invariant: first-party skill {folder_name} embeds its SKILL.md"),
                        |(_, bytes)| String::from_utf8_lossy(bytes).into_owned(),
                    ),
                Err(source) => {
                    return Err(ReadSkillsError::ReadFirstParty {
                        folder: folder_name,
                        source,
                    });
                }
            };
            let (name, description) =
                skill_metadata(&text).ok_or_else(|| ReadSkillsError::FirstPartyMetadata {
                    folder: folder_name.clone(),
                })?;
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
    let read_managed = |source| ReadSkillsError::ReadManaged {
        path: c.skills_dir.clone(),
        source,
    };
    let entries = match fs::read_dir(&c.skills_dir) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(read_managed(error)),
    };
    if let Some(entries) = entries {
        for entry in entries {
            let entry = entry.map_err(read_managed)?;
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

pub(super) fn wanted(c: &Customisation) -> Result<BTreeMap<String, PathBuf>, ReadSkillsError> {
    recover_replacements(&c.skills_dir).map_err(ReadSkillsError::Recover)?;
    if c.first_party {
        place_first_party(&c.skills_dir, true)?;
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

pub fn enabled_skills(c: &Customisation) -> Result<Vec<artisan_api::Skill>, ReadSkillsError> {
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
) -> Result<Vec<Found>, ReadSkillsError> {
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
        let read_native = |source| ReadSkillsError::ReadNative {
            engine: *engine,
            path: skills.clone(),
            source,
        };
        let entries = match fs::read_dir(&skills) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(read_native(error)),
        };
        for entry in entries {
            let entry = entry.map_err(read_native)?;
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

pub fn skill_catalog(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
) -> Result<Vec<CatalogSkill>, ReadSkillsError> {
    let managed = managed(c)?;
    let owned = owned_roots(&c.skills_dir);
    let mut catalog: Vec<CatalogSkill> = managed
        .iter()
        .map(|skill| -> Result<CatalogSkill, ReadSkillsError> {
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
                    .unwrap_or_else(|| {
                        panic!(
                            "invariant: first-party skill {} is embedded",
                            skill.folder.display()
                        )
                    });
                (
                    u32::try_from(source.files.len()).unwrap_or(u32::MAX),
                    source
                        .files
                        .iter()
                        .map(|(_, bytes)| bytes.len() as u64)
                        .sum(),
                )
            } else {
                counts(&skill.folder).map_err(|source| ReadSkillsError::Inspect {
                    path: skill.folder.clone(),
                    source,
                })?
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
        .collect::<Result<_, _>>()?;
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
) -> Result<Vec<CatalogSkill>, ReadSkillsError> {
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

#[derive(Debug, thiserror::Error)]
pub enum FindSkillError {
    #[error("invalid discovered skill identity {id:?}")]
    InvalidId { id: String },
    #[error("look for the skill {id}")]
    Read {
        id: String,
        #[source]
        source: ReadSkillsError,
    },
}

fn found_copies(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
    id: &str,
) -> Result<Vec<Found>, FindSkillError> {
    let (wanted_folder, _digest) = id
        .strip_prefix("discovered.")
        .and_then(|id| id.rsplit_once('.'))
        .filter(|(folder, digest)| {
            !folder.is_empty()
                && digest.len() == 12
                && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .ok_or_else(|| FindSkillError::InvalidId { id: id.to_owned() })?;
    let quarantine = quarantine_root(&c.skills_dir);
    let owned = owned_roots(&c.skills_dir);
    Ok(
        discovered(homes, Some(&quarantine), &owned, false, Some(wanted_folder))
            .map_err(|source| FindSkillError::Read {
                id: id.to_owned(),
                source,
            })?
            .into_iter()
            .filter(|found| discovered_id(&found.folder_name, &found.digest) == id)
            .collect(),
    )
}

#[derive(Debug, thiserror::Error)]
pub enum DiscardSkillError {
    #[error(transparent)]
    Find(FindSkillError),
    #[error("That skill is no longer in quarantine.")]
    Gone { id: String },
    #[error("deleting {}", path.display())]
    Delete {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn forget_quarantine_marker(copy: &Found) {
    if let Some(parent) = copy.folder.parent() {
        let marker = quarantined_at(parent, &copy.folder_name);
        if let Err(error) = remove_file_if_present(&marker) {
            tracing::warn!(marker = %marker.display(), %error, "couldn't remove a quarantine marker");
        }
    }
}

pub fn discard_skill(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
    id: &str,
) -> Result<String, DiscardSkillError> {
    let copies = found_copies(homes, c, id).map_err(DiscardSkillError::Find)?;
    let Some(first) = copies.first() else {
        return Err(DiscardSkillError::Gone { id: id.to_owned() });
    };
    let name = first.name.clone();
    for copy in &copies {
        super::remove_entry(&copy.folder).map_err(|source| DiscardSkillError::Delete {
            path: copy.folder.clone(),
            source,
        })?;
        forget_quarantine_marker(copy);
    }
    Ok(name)
}

#[derive(Debug, thiserror::Error)]
pub enum AdoptSkillError {
    #[error(transparent)]
    Find(FindSkillError),
    #[error("That skill is no longer in quarantine.")]
    Gone { id: String },
    #[error("Artisan already manages a skill named {name}.")]
    Taken { name: String },
    #[error("creating {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("copy the skill {name}")]
    Import {
        name: String,
        #[source]
        source: ImportError,
    },
    #[error("backing up {}'s skill {name}", engine.display_name())]
    Backup {
        engine: EngineId,
        name: String,
        #[source]
        source: io::Error,
    },
    #[error("publishing adopted skill {name}")]
    Publish {
        name: String,
        #[source]
        source: io::Error,
    },
    #[error("{stranded}")]
    Stranded {
        stranded: String,
        #[source]
        source: Box<Self>,
    },
}

pub fn adopt_skill(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
    id: &str,
) -> Result<String, AdoptSkillError> {
    let copies = found_copies(homes, c, id).map_err(AdoptSkillError::Find)?;
    let Some(first) = copies.first() else {
        return Err(AdoptSkillError::Gone { id: id.to_owned() });
    };
    let name = first.folder_name.clone();
    let taken = c.skills_dir.join(&name).exists()
        || (c.first_party && FIRST_PARTY.iter().any(|skill| skill.folder == name));
    if taken {
        return Err(AdoptSkillError::Taken { name });
    }
    fs::create_dir_all(&c.skills_dir).map_err(|source| AdoptSkillError::Create {
        path: c.skills_dir.clone(),
        source,
    })?;
    let staging = c
        .skills_dir
        .join(format!(".adopt-{name}-{}", artisan_api::now()));
    fs::create_dir(&staging).map_err(|source| AdoptSkillError::Create {
        path: staging.clone(),
        source,
    })?;
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let result = (|| -> Result<(), AdoptSkillError> {
        import_folder(&first.folder, &staging, &name).map_err(|source| {
            AdoptSkillError::Import {
                name: name.clone(),
                source,
            }
        })?;
        let backups = backups_root(&c.skills_dir);
        let stamp = artisan_api::now();
        for copy in &copies {
            let to = backups.join(format!("{}-{name}-{stamp}", copy.engine));
            move_folder(&copy.folder, &to).map_err(|source| AdoptSkillError::Backup {
                engine: copy.engine,
                name: name.clone(),
                source,
            })?;
            moved.push((copy.folder.clone(), to));
        }
        fs::rename(staging.join(&name), c.skills_dir.join(&name)).map_err(|source| {
            AdoptSkillError::Publish {
                name: name.clone(),
                source,
            }
        })
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
        discard_leftover(&staging);
        if rollback_errors.is_empty() {
            return Err(error);
        }
        return Err(AdoptSkillError::Stranded {
            stranded: rollback_errors.join("; "),
            source: Box::new(error),
        });
    }
    discard_leftover(&staging);
    for copy in &copies {
        forget_quarantine_marker(copy);
    }
    Ok(name)
}

#[derive(Debug, thiserror::Error)]
pub enum InstallSkillError {
    #[error("{} isn't a folder Forge can read", path.display())]
    Unreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} needs a SKILL.md that starts with YAML front matter", path.display())]
    NoSkillFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("SKILL.md's front matter needs a `name` without spaces or slashes")]
    BadName { path: PathBuf },
    #[error("{name} is one of Artisan's own skills; pick another name.")]
    FirstPartyName { name: String },
    #[error("{name} is already installed.")]
    AlreadyInstalled { name: String },
    #[error(
        "{} is inside an engine's skills folder, where Artisan removes what it didn't put. Write the skill in another folder and install that.",
        path.display()
    )]
    InsideEngine { path: PathBuf },
    #[error("Artisan already has a skill named {name}; pass replace to update it.")]
    Exists { name: String },
    #[error("creating {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("copying {}", path.display())]
    Copy {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("recording the replacement of {name}")]
    Record {
        name: String,
        #[source]
        source: artisan_paths::private::PrivateFileError,
    },
    #[error("backing up the old {name}")]
    Backup {
        name: String,
        #[source]
        source: io::Error,
    },
    #[error("placing the skill {name}")]
    Place {
        name: String,
        #[source]
        source: io::Error,
    },
}

pub fn install_skill(
    homes: &[(EngineId, Option<PathBuf>)],
    c: &Customisation,
    from: &Path,
    replace: bool,
) -> Result<String, InstallSkillError> {
    let from = from
        .canonicalize()
        .map_err(|source| InstallSkillError::Unreadable {
            path: from.to_owned(),
            source,
        })?;
    let text = read_skill_header(&from.join("SKILL.md")).map_err(|source| {
        InstallSkillError::NoSkillFile {
            path: from.clone(),
            source,
        }
    })?;
    let (name, _) =
        skill_metadata(&text).ok_or_else(|| InstallSkillError::BadName { path: from.clone() })?;
    if c.first_party && FIRST_PARTY.iter().any(|skill| skill.folder == name) {
        return Err(InstallSkillError::FirstPartyName { name });
    }
    let inside = |root: &Path| root.canonicalize().is_ok_and(|root| from.starts_with(root));
    if inside(&c.skills_dir) {
        return Err(InstallSkillError::AlreadyInstalled { name });
    }
    let engine_skills = homes
        .iter()
        .filter_map(|(engine, home)| paths(*engine, home.as_deref()?))
        .map(|paths| paths.skills);
    for skills in engine_skills {
        if inside(&skills) {
            return Err(InstallSkillError::InsideEngine { path: from });
        }
    }
    let to = c.skills_dir.join(&name);
    if to.exists() && !replace {
        return Err(InstallSkillError::Exists { name });
    }
    fs::create_dir_all(&c.skills_dir).map_err(|source| InstallSkillError::Create {
        path: c.skills_dir.clone(),
        source,
    })?;
    let _replacing = replacing();
    let stamp = artisan_api::now();
    let staging = c
        .skills_dir
        .join(format!("{INSTALL_STAGING}{name}-{stamp}"));
    let backup_name = format!("{name}-{stamp}");
    let backup = backups_root(&c.skills_dir).join(&backup_name);
    let record = replacement_record(&c.skills_dir, &name);
    let result = (|| -> Result<(), InstallSkillError> {
        copy_dir(&from, &staging).map_err(|source| InstallSkillError::Copy {
            path: from.clone(),
            source,
        })?;
        let replaced = to.exists();
        if replaced {
            artisan_paths::private::write_file(&record, backup_name.as_bytes()).map_err(
                |source| InstallSkillError::Record {
                    name: name.clone(),
                    source,
                },
            )?;
            if let Err(source) = move_folder(&to, &backup) {
                forget_replacement(&record);
                return Err(InstallSkillError::Backup {
                    name: name.clone(),
                    source,
                });
            }
        }
        if let Err(source) = fs::rename(&staging, &to) {
            if replaced {
                match move_folder(&backup, &to) {
                    Ok(()) => forget_replacement(&record),
                    Err(restore) => {
                        tracing::warn!(skill = %name, backup = %backup.display(), %restore, "couldn't restore the old skill; the next launch restores it from the backups");
                    }
                }
            }
            return Err(InstallSkillError::Place {
                name: name.clone(),
                source,
            });
        }
        if replaced {
            forget_replacement(&record);
        }
        Ok(())
    })();
    if result.is_err() {
        discard_leftover(&staging);
    }
    result.map(|()| name)
}

const REPLACING: &str = ".replacing-";
const INSTALL_STAGING: &str = ".install-";

fn replacing() -> std::sync::MutexGuard<'static, ()> {
    static REPLACEMENTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    REPLACEMENTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn replacement_record(skills_dir: &Path, name: &str) -> PathBuf {
    skills_dir.join(format!("{REPLACING}{name}"))
}

fn forget_replacement(record: &Path) {
    if let Err(error) = remove_file_if_present(record) {
        tracing::warn!(record = %record.display(), %error, "couldn't remove a finished skill replacement record");
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RecoverSkillError {
    #[error("reading {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} names no backup folder", path.display())]
    Corrupt { path: PathBuf },
    #[error("restoring {} from {}", skill.display(), backup.display())]
    Restore {
        skill: PathBuf,
        backup: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("removing {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn recover_replacements(skills_dir: &Path) -> Result<(), RecoverSkillError> {
    let _replacing = replacing();
    let read = |path: &Path| {
        let path = path.to_path_buf();
        move |source| RecoverSkillError::Read { path, source }
    };
    let entries = match fs::read_dir(skills_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(read(skills_dir)(error)),
    };
    for entry in entries {
        let entry = entry.map_err(read(skills_dir))?;
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name.starts_with(INSTALL_STAGING) {
            super::remove_entry(&path)
                .map_err(|source| RecoverSkillError::Remove { path, source })?;
        } else if let Some(name) = file_name.strip_prefix(REPLACING) {
            restore_replaced(skills_dir, name, &path)?;
        }
    }
    Ok(())
}

fn restore_replaced(skills_dir: &Path, name: &str, record: &Path) -> Result<(), RecoverSkillError> {
    let read = |path: &Path| {
        let path = path.to_path_buf();
        move |source| RecoverSkillError::Read { path, source }
    };
    let backup_name = fs::read_to_string(record).map_err(read(record))?;
    let backup_name = Path::new(backup_name.trim());
    if !matches!(
        backup_name.components().collect::<Vec<_>>().as_slice(),
        [std::path::Component::Normal(_)]
    ) {
        return Err(RecoverSkillError::Corrupt {
            path: record.to_path_buf(),
        });
    }
    let backup = backups_root(skills_dir).join(backup_name);
    let skill = skills_dir.join(name);
    let missing = |path: &Path| match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(read(path)(error)),
    };
    if missing(&skill)? && !missing(&backup)? {
        move_folder(&backup, &skill).map_err(|source| RecoverSkillError::Restore {
            skill: skill.clone(),
            backup: backup.clone(),
            source,
        })?;
        tracing::info!(skill = %skill.display(), backup = %backup.display(), "restored a skill an interrupted replacement had backed up");
    }
    remove_file_if_present(record).map_err(|source| RecoverSkillError::Remove {
        path: record.to_path_buf(),
        source,
    })
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
        assert!(matches!(error, ReadSkillsError::Inspect { .. }), "{error}");
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
        assert!(matches!(error, ReadSkillsError::Paging { .. }), "{error}");
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

    fn require_enforced_permissions() {
        let status = fs::read_to_string("/proc/self/status").unwrap();
        assert!(
            !status.lines().any(|line| line.starts_with("Uid:\t0\t")),
            "this fixture makes a folder read-only, which root ignores; run it as an unprivileged user"
        );
    }

    fn other_filesystem_than(path: &Path) -> tempfile::TempDir {
        use std::os::unix::fs::MetadataExt as _;
        let other = tempfile::tempdir_in("/dev/shm")
            .expect("this fixture needs /dev/shm for a second filesystem");
        assert_ne!(
            fs::metadata(other.path()).unwrap().dev(),
            fs::metadata(path).unwrap().dev(),
            "this fixture needs /dev/shm on a different filesystem than the temporary directory"
        );
        other
    }

    #[test]
    fn a_later_backup_failure_restores_every_original_and_allows_normal_retry() {
        require_enforced_permissions();
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
        require_enforced_permissions();
        let source_root = tempfile::tempdir().unwrap();
        let backup_root = other_filesystem_than(source_root.path());
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
        assert!(matches!(error, InstallSkillError::Exists { .. }), "{error}");
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
        assert!(
            matches!(error, InstallSkillError::InsideEngine { .. }),
            "{error}"
        );
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

#[cfg(test)]
#[path = "../../tests/customise/skills.rs"]
mod replacement_tests;
