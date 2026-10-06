//! The Forge's authority over agent extensions and the global prompt.
//!
//! The Forge decides what every managed engine holds: Artisan's own skills
//! and the skills the user adopted are linked into each engine's skills
//! folder, and the user's global prompt is written to each engine's global
//! file. Reconciliation runs at startup, after every change the user makes,
//! and periodically, so a newly installed engine catches up and an edit made
//! outside Artisan is noticed. Every change to what the Editor sees wakes
//! the connections that read it.

mod first_party;
mod prompt;
mod skills;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use artisan_database::{GlobalPromptSave, Repository, StoredExtension, StoredExtensionOrigin};
use artisan_domain::error_chain::ErrorChain;
use artisan_domain::{
    ExtensionCatalog, ExtensionLinkSupport, ExtensionOrigin, GLOBAL_PROMPT_CANDIDATES_MAX,
    GlobalPrompt, GlobalPromptEngineStatus, GlobalPromptResolution, GlobalPromptRevision,
    GlobalPromptSetup, UnixMillis,
};
use artisan_native_engine::ManagedEngine;
use artisan_native_engine::extensions::{
    ExtensionStore, engine_slots, links_supported, move_to_backup, read_skill_dir,
};
use artisan_transport::CancelHandle;

use self::first_party::{first_party_id, first_party_packages, repository_skills_root};
use self::skills::{DiscoveredSkill, ManagedSkill, sync_skills};

/// How often the keeper reconciles without a user change.
const KEEP_CURRENT_INTERVAL: Duration = Duration::from_secs(30);

/// A request the Forge refuses, with a presentation-ready message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SyncRefusal {
    pub(crate) message: String,
    /// Whether retrying the same request may succeed.
    pub(crate) retryable: bool,
}

impl SyncRefusal {
    fn final_(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }
}

/// What the Editor last saw, plus where discovered skills live.
#[derive(Clone, Debug, Default)]
struct Snapshots {
    catalog: ExtensionCatalog,
    prompt: GlobalPrompt,
    discovered: Vec<DiscoveredSkill>,
}

/// Keeps every managed engine's skills and global prompt in line with what
/// the user chose in Artisan.
pub(crate) struct ExtensionSync {
    state_root: PathBuf,
    repository: Repository,
    publish: Arc<dyn Fn() + Send + Sync>,
    current: Mutex<Snapshots>,
    reconciling: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for ExtensionSync {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionSync")
            .field("state_root", &self.state_root)
            .finish_non_exhaustive()
    }
}

impl ExtensionSync {
    /// Creates the sync for the Forge whose database lives in `state_root`.
    /// `publish` wakes the connections after a visible change.
    pub(crate) fn new(
        state_root: PathBuf,
        repository: Repository,
        publish: impl Fn() + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            state_root,
            repository,
            publish: Arc::new(publish),
            current: Mutex::new(Snapshots::default()),
            reconciling: tokio::sync::Mutex::new(()),
        })
    }

    /// Starts the sync for the Forge whose database is `database`, keeps it
    /// current until `cancel` fires, and hands it to `handler`. Returns the
    /// handler and the keeper task to abort when serving ends.
    pub(crate) fn start(
        handler: crate::RequestHandler,
        database: &std::path::Path,
        notifier: &crate::conversation_commit_notifier::ConversationCommitNotifier,
        cancel: &Arc<CancelHandle>,
    ) -> (crate::RequestHandler, tokio::task::JoinHandle<()>) {
        let state_root = database
            .parent()
            .map_or_else(PathBuf::new, std::path::Path::to_path_buf);
        let notifier = notifier.clone();
        let sync = Self::new(state_root, handler.repository().clone(), move || {
            notifier.publish_host_state();
        });
        let keeper = tokio::spawn(Arc::clone(&sync).keep_current(Arc::clone(cancel)));
        (handler.with_extension_sync(sync), keeper)
    }

    /// The extension catalog as last reconciled.
    pub(crate) fn catalog(&self) -> ExtensionCatalog {
        self.snapshots().catalog
    }

    /// The global prompt as last reconciled.
    pub(crate) fn global_prompt(&self) -> GlobalPrompt {
        self.snapshots().prompt
    }

    fn snapshots(&self) -> Snapshots {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Reconciles until `cancel` fires: once now, then periodically.
    pub(crate) async fn keep_current(self: Arc<Self>, cancel: Arc<CancelHandle>) {
        loop {
            if let Err(refusal) = self.reconcile().await {
                eprintln!("extension sync failed: {}", refusal.message);
            }
            tokio::select! {
                () = cancel.wait() => return,
                () = tokio::time::sleep(KEEP_CURRENT_INTERVAL) => {}
            }
        }
    }

    /// Brings every engine in line and publishes what changed.
    ///
    /// # Errors
    ///
    /// Returns [`SyncRefusal`] when the stored state cannot be read or the
    /// store cannot be prepared.
    pub(crate) async fn reconcile(&self) -> Result<(), SyncRefusal> {
        self.reconcile_forcing(None).await
    }

    async fn reconcile_forcing(&self, force: Option<ManagedEngine>) -> Result<(), SyncRefusal> {
        let _guard = self.reconciling.lock().await;
        let stored = self
            .repository
            .read_extensions()
            .await
            .map_err(|error| storage_refusal("read managed extensions", &error))?;
        let state_root = self.state_root.clone();
        let observed = tokio::task::spawn_blocking(move || observe_and_place(&state_root, &stored))
            .await
            .map_err(|_| SyncRefusal::retryable("Artisan could not update the engines."))??;
        let prompt = self.reconcile_prompt(&observed, force).await?;
        let mut discovered = observed.skills.discovered;
        let mut entries = observed.skills.managed;
        entries.extend(discovered.iter().map(|skill| skill.extension.clone()));
        let catalog = ExtensionCatalog::new(entries, observed.links).unwrap_or_else(|error| {
            eprintln!("extension catalog is invalid: {}", ErrorChain(&error));
            discovered.clear();
            ExtensionCatalog::default()
        });
        let changed = {
            let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
            let changed = current.catalog != catalog || current.prompt != prompt;
            *current = Snapshots {
                catalog,
                prompt,
                discovered,
            };
            changed
        };
        if changed {
            (self.publish)();
        }
        Ok(())
    }

    async fn reconcile_prompt(
        &self,
        observed: &Observed,
        force: Option<ManagedEngine>,
    ) -> Result<GlobalPrompt, SyncRefusal> {
        let mut stored = self
            .repository
            .read_global_prompt()
            .await
            .map_err(|error| storage_refusal("read the global prompt", &error))?;
        if !stored.established {
            let mut candidates = prompt::candidates(&observed.homes);
            if candidates.len() > 1 {
                candidates.truncate(GLOBAL_PROMPT_CANDIDATES_MAX);
                let states = prompt::engine_states(&observed.slots, &[], true);
                return Ok(validated_prompt(
                    stored.revision,
                    stored.text,
                    GlobalPromptSetup::ChoiceRequired(candidates),
                    states,
                ));
            }
            let text = candidates
                .pop()
                .map(|candidate| candidate.text)
                .unwrap_or_default();
            stored = self
                .repository
                .establish_global_prompt(&text, now())
                .await
                .map_err(|error| storage_refusal("establish the global prompt", &error))?;
        }
        let records = self
            .repository
            .read_prompt_files()
            .await
            .map_err(|error| storage_refusal("read global prompt files", &error))?;
        let homes = observed.homes.clone();
        let canonical = stored.text.clone();
        let backups = ExtensionStore::new(&self.state_root).backups_dir();
        let applied_records = records.clone();
        let outcomes = tokio::task::spawn_blocking(move || {
            prompt::apply(&homes, &canonical, &applied_records, force, &backups)
        })
        .await
        .map_err(|_| SyncRefusal::retryable("Artisan could not write the global prompt."))?;
        for outcome in &outcomes {
            let Some(digest) = &outcome.recorded_digest else {
                continue;
            };
            let current = records.iter().find(|record| {
                record.engine_id == outcome.engine.id() && record.home_key == outcome.home_key
            });
            if current.is_some_and(|record| &record.written_digest == digest) {
                continue;
            }
            let record = artisan_database::StoredPromptFile {
                engine_id: outcome.engine.id().to_owned(),
                home_key: outcome.home_key.clone(),
                written_digest: digest.clone(),
            };
            if let Err(error) = self.repository.record_prompt_file(&record, now()).await {
                eprintln!(
                    "global prompt write for {} ({}) was not recorded: {}",
                    record.engine_id,
                    record.home_key,
                    ErrorChain(&error)
                );
            }
        }
        let states = prompt::engine_states(&observed.slots, &outcomes, false);
        Ok(validated_prompt(
            stored.revision,
            stored.text,
            GlobalPromptSetup::Ready,
            states,
        ))
    }

    /// Turns a managed extension on or off in every engine.
    ///
    /// # Errors
    ///
    /// Returns [`SyncRefusal`] for an unknown extension or a storage failure.
    pub(crate) async fn set_enabled(
        &self,
        extension_id: &str,
        enabled: bool,
    ) -> Result<ExtensionCatalog, SyncRefusal> {
        let entry = self
            .catalog()
            .extension(extension_id)
            .filter(|entry| entry.origin != ExtensionOrigin::Discovered)
            .cloned()
            .ok_or_else(|| SyncRefusal::final_("Artisan does not manage that extension."))?;
        let origin = match entry.origin {
            ExtensionOrigin::FirstParty => StoredExtensionOrigin::FirstParty,
            ExtensionOrigin::Adopted | ExtensionOrigin::Discovered => {
                StoredExtensionOrigin::Adopted
            }
        };
        self.repository
            .save_extension(
                &StoredExtension {
                    extension_id: entry.id,
                    origin,
                    name: entry.name,
                    digest: entry.digest,
                    enabled,
                },
                now(),
            )
            .await
            .map_err(|error| storage_refusal("save an extension", &error))?;
        self.reconcile().await?;
        Ok(self.catalog())
    }

    /// Adopts a discovered skill: stores it, moves each engine's identical
    /// copy into the backups, and places Artisan's copy everywhere.
    ///
    /// # Errors
    ///
    /// Returns [`SyncRefusal`] when the skill is gone or changed, its name is
    /// already managed, or storing it fails.
    pub(crate) async fn adopt(&self, extension_id: &str) -> Result<ExtensionCatalog, SyncRefusal> {
        let snapshots = self.snapshots();
        let discovered = snapshots
            .discovered
            .iter()
            .find(|skill| skill.extension.id == extension_id)
            .cloned()
            .ok_or_else(|| {
                SyncRefusal::final_("That skill is no longer in your engines' skills folders.")
            })?;
        let name = discovered.extension.name.clone();
        if snapshots
            .catalog
            .extensions()
            .iter()
            .any(|entry| entry.origin != ExtensionOrigin::Discovered && entry.name == name)
        {
            return Err(SyncRefusal::final_(format!(
                "Artisan already manages a skill named {name}."
            )));
        }
        let store = ExtensionStore::new(&self.state_root);
        let digest = discovered.extension.digest.clone();
        let copies = discovered.copies.clone();
        let stored_digest =
            tokio::task::spawn_blocking(move || adopt_copies(&store, &digest, &copies))
                .await
                .map_err(|_| SyncRefusal::retryable("Artisan could not adopt the skill."))??;
        self.repository
            .save_extension(
                &StoredExtension {
                    extension_id: format!("local.{name}"),
                    origin: StoredExtensionOrigin::Adopted,
                    name,
                    digest: stored_digest,
                    enabled: true,
                },
                now(),
            )
            .await
            .map_err(|error| storage_refusal("save an adopted skill", &error))?;
        self.reconcile().await?;
        Ok(self.catalog())
    }

    /// Saves a new version of the global prompt and writes it everywhere.
    ///
    /// # Errors
    ///
    /// Returns [`SyncRefusal`] for an invalid text, a stale base revision,
    /// or a storage failure.
    pub(crate) async fn save_prompt(
        &self,
        base: GlobalPromptRevision,
        text: &str,
    ) -> Result<GlobalPrompt, SyncRefusal> {
        artisan_domain::validate_prompt_text(text).map_err(|_| {
            SyncRefusal::final_(
                "The global prompt is too long or holds characters engines cannot read.",
            )
        })?;
        let saved = self
            .repository
            .save_global_prompt(base.get(), text, now())
            .await
            .map_err(|error| storage_refusal("save the global prompt", &error))?;
        if let GlobalPromptSave::Stale(_) = saved {
            self.reconcile().await?;
            return Err(SyncRefusal::final_(
                "The global prompt changed since you started editing. Review the current version and save again.",
            ));
        }
        self.reconcile().await?;
        Ok(self.global_prompt())
    }

    /// Settles setup or a drifted engine.
    ///
    /// # Errors
    ///
    /// Returns [`SyncRefusal`] when the candidate or drift no longer exists,
    /// or a storage failure.
    pub(crate) async fn resolve_prompt(
        &self,
        resolution: &GlobalPromptResolution,
    ) -> Result<GlobalPrompt, SyncRefusal> {
        let current = self.global_prompt();
        match resolution {
            GlobalPromptResolution::ChooseCandidate { candidate_id } => {
                let GlobalPromptSetup::ChoiceRequired(candidates) = current.setup() else {
                    return Err(SyncRefusal::final_("The global prompt is already set up."));
                };
                let candidate = candidates
                    .iter()
                    .find(|candidate| &candidate.candidate_id == candidate_id)
                    .ok_or_else(|| {
                        SyncRefusal::final_("That prompt is no longer in your engines.")
                    })?;
                self.repository
                    .establish_global_prompt(&candidate.text, now())
                    .await
                    .map_err(|error| storage_refusal("establish the global prompt", &error))?;
                self.reconcile().await?;
            }
            GlobalPromptResolution::ImportEngine { engine_id } => {
                let external = drifted_text(&current, engine_id)?;
                let saved = self
                    .repository
                    .save_global_prompt(current.revision().get(), &external, now())
                    .await
                    .map_err(|error| storage_refusal("import a drifted prompt", &error))?;
                if let GlobalPromptSave::Stale(_) = saved {
                    self.reconcile().await?;
                    return Err(SyncRefusal::final_(
                        "The global prompt changed meanwhile. Review it and try again.",
                    ));
                }
                self.reconcile().await?;
            }
            GlobalPromptResolution::OverwriteEngine { engine_id } => {
                drifted_text(&current, engine_id)?;
                let engine = ManagedEngine::from_id(engine_id)
                    .ok_or_else(|| SyncRefusal::final_("That engine is unknown."))?;
                self.reconcile_forcing(Some(engine)).await?;
            }
        }
        Ok(self.global_prompt())
    }
}

/// What one pass over the engine homes found and placed.
struct Observed {
    slots: Vec<artisan_native_engine::extensions::EngineSlot>,
    skills: skills::SkillSync,
    homes: Vec<prompt::HomePrompt>,
    links: ExtensionLinkSupport,
}

fn observe_and_place(
    state_root: &std::path::Path,
    stored: &[StoredExtension],
) -> Result<Observed, SyncRefusal> {
    let store = ExtensionStore::new(state_root);
    store
        .prepare()
        .map_err(|_| SyncRefusal::retryable("Artisan could not prepare its extension store."))?;
    let links_available = links_supported(&store.root().join("probe"));
    let slots = engine_slots(state_root);
    let mut managed = Vec::new();
    let mut keep = std::collections::HashSet::new();
    let first_party = first_party_packages().unwrap_or_else(|error| {
        eprintln!("first-party skills are invalid: {}", ErrorChain(&error));
        Vec::new()
    });
    for skill in &first_party {
        let id = first_party_id(skill.package.name());
        let enabled = stored
            .iter()
            .find(|row| row.extension_id == id)
            .is_none_or(|row| row.enabled);
        let target = if let Some(folder) = &skill.linked_folder {
            Ok(folder.clone())
        } else {
            keep.insert(skill.package.digest().to_owned());
            store
                .put(&skill.package)
                .map_err(|_| "Artisan could not store the skill.")
        };
        managed.push(ManagedSkill::from_package(
            id,
            ExtensionOrigin::FirstParty,
            &skill.package,
            enabled,
            target,
        ));
    }
    for row in stored
        .iter()
        .filter(|row| row.origin == StoredExtensionOrigin::Adopted)
    {
        keep.insert(row.digest.clone());
        let folder = store.skill_dir(&row.digest);
        match read_skill_dir(&folder) {
            Ok(package) => managed.push(ManagedSkill::from_package(
                row.extension_id.clone(),
                ExtensionOrigin::Adopted,
                &package,
                row.enabled,
                Ok(folder),
            )),
            Err(_) => managed.push(ManagedSkill {
                id: row.extension_id.clone(),
                origin: ExtensionOrigin::Adopted,
                name: row.name.clone(),
                description: String::new(),
                digest: row.digest.clone(),
                file_count: 0,
                byte_count: 0,
                enabled: row.enabled,
                target: Err("The adopted skill's files are missing from Artisan's store."),
            }),
        }
    }
    managed.sort_by(|left, right| left.name.cmp(&right.name));
    let mut owned_roots = vec![store.store_dir()];
    owned_roots.extend(repository_skills_root());
    let skills = sync_skills(&slots, &managed, &owned_roots, links_available);
    if let Err(error) = store.collect_garbage(&keep) {
        eprintln!("extension store cleanup failed: {}", ErrorChain(&error));
    }
    let homes = prompt::observe(&slots, state_root);
    Ok(Observed {
        slots,
        skills,
        homes,
        links: if links_available {
            ExtensionLinkSupport::Available
        } else {
            ExtensionLinkSupport::DeveloperModeRequired
        },
    })
}

/// Stores the adopted skill from its first copy and moves every identical
/// copy into the backups. Returns the stored digest.
fn adopt_copies(
    store: &ExtensionStore,
    digest: &str,
    copies: &[(ManagedEngine, PathBuf)],
) -> Result<String, SyncRefusal> {
    let changed =
        || SyncRefusal::final_("The skill changed in your engines. Review it and adopt it again.");
    let (_, first) = copies.first().ok_or_else(changed)?;
    let package = read_skill_dir(first).map_err(|_| changed())?;
    if package.digest() != digest {
        return Err(changed());
    }
    store
        .prepare()
        .and_then(|()| store.put(&package))
        .map_err(|_| SyncRefusal::retryable("Artisan could not store the skill."))?;
    for (engine, folder) in copies {
        if read_skill_dir(folder).is_ok_and(|copy| copy.digest() == digest) {
            let label = format!("{}-{}-skill", engine.id(), package.name());
            move_to_backup(folder, &store.backups_dir(), &label).map_err(|_| {
                SyncRefusal::retryable(format!(
                    "Artisan could not move {}'s copy of the skill aside.",
                    engine.display_name()
                ))
            })?;
        }
    }
    Ok(package.digest().to_owned())
}

fn drifted_text(prompt: &GlobalPrompt, engine_id: &str) -> Result<String, SyncRefusal> {
    prompt
        .engine(engine_id)
        .filter(|state| state.status == GlobalPromptEngineStatus::Drifted)
        .and_then(|state| state.external_text.clone())
        .ok_or_else(|| SyncRefusal::final_("That engine's global prompt is no longer edited."))
}

fn validated_prompt(
    revision: u64,
    text: String,
    setup: GlobalPromptSetup,
    engines: Vec<artisan_domain::GlobalPromptEngineState>,
) -> GlobalPrompt {
    GlobalPrompt::new(GlobalPromptRevision::new(revision), text, setup, engines).unwrap_or_else(
        |error| {
            eprintln!("global prompt state is invalid: {}", ErrorChain(&error));
            GlobalPrompt::default()
        },
    )
}

fn storage_refusal(step: &str, error: &artisan_database::RepositoryError) -> SyncRefusal {
    eprintln!("extension sync could not {step}: {}", ErrorChain(error));
    SyncRefusal::retryable("Artisan could not read or save its extension settings.")
}

fn now() -> UnixMillis {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    UnixMillis::from_millis(millis)
}

#[cfg(test)]
#[path = "../../../tests/backend/extension_sync.rs"]
mod tests;
