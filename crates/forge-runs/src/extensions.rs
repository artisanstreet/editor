use crate::Hub;
use crate::Refused;
use crate::error::{self, ErrorClass};
use crate::mcp_auth::ReadMcpServersError;
use artisan_api::{
    CatalogSkill, EngineId, GlobalPrompt, PromptCandidate, PromptEngineState, PromptEngineStatus,
    ResolvePrompt, SavePrompt, SkillOrigin,
};
use forge_engines::Engines;
use forge_engines::{Customisation, PromptApplied};
use forge_store::{
    BumpRevisionError, PromptVersionError, ReadSettingError, SavePromptError, SetSettingError,
    Store,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub(crate) const DISABLED_SKILLS_KEY: &str = "disabled_skills";
const GLOBAL_PROMPT_KEY: &str = "global_prompt";
pub(crate) const PROMPT_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HomesLockError {
    #[error("open the engine homes lock {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("lock the engine homes with {}", path.display())]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum HomesError {
    #[error("wait for the engine homes")]
    Lock(#[source] HomesLockError),
    #[error("run the work on the engine homes")]
    Task(#[source] tokio::task::JoinError),
}

#[derive(Debug, thiserror::Error)]
pub enum SkillCustomisationError {
    #[error("read Forge's {DISABLED_SKILLS_KEY} setting")]
    Read(#[source] ReadSettingError),
    #[error("read the skills turned off in Forge's {DISABLED_SKILLS_KEY} setting")]
    DisabledSkills(#[source] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum CustomisationError {
    #[error("read the global prompt engines launch with")]
    Prompt(#[source] GlobalPromptError),
    #[error("read the MCP servers engines launch with")]
    McpServers(#[source] ReadMcpServersError),
    #[error("read the skills engines launch with")]
    Skills(#[source] SkillCustomisationError),
}

#[derive(Debug, thiserror::Error)]
pub enum SkillCatalogError {
    #[error("read which skills are turned off")]
    Customisation(#[source] SkillCustomisationError),
    #[error("read the skill catalog from the engine homes")]
    Homes(#[source] HomesError),
    #[error("read the skill catalog")]
    Read(#[source] forge_engines::ReadSkillsError),
}

impl SkillCatalogError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Customisation(_) | Self::Homes(_) | Self::Read(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EnabledSkillsError {
    #[error("read which skills are turned off")]
    Customisation(#[source] SkillCustomisationError),
    #[error("read the enabled skills from the engine homes")]
    Homes(#[source] HomesError),
    #[error("read the enabled skills")]
    Read(#[source] forge_engines::ReadSkillsError),
}

#[derive(Debug, thiserror::Error)]
pub enum ReconcileSkillsError {
    #[error("read which skills are turned off")]
    Customisation(#[source] SkillCustomisationError),
    #[error("put the skills into the engine homes")]
    Homes(#[source] HomesError),
    #[error("put the skills into the engines")]
    Reconcile(#[source] forge_engines::ReconcileSkillsError),
}

impl ReconcileSkillsError {
    #[must_use]
    const fn class(&self) -> ErrorClass {
        match self {
            Self::Customisation(_) | Self::Homes(_) | Self::Reconcile(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SetSkillEnabledError {
    #[error("read the skill catalog to turn skill {skill} on or off")]
    Catalog {
        skill: String,
        #[source]
        source: SkillCatalogError,
    },
    #[error("turn skill {skill} on or off")]
    Unmanaged {
        skill: String,
        #[source]
        refused: Refused,
    },
    #[error("save whether skill {skill} is on")]
    Save {
        skill: String,
        #[source]
        source: forge_store::SetSkillEnabledError,
    },
    #[error("put the skills into the engines after turning skill {skill} on or off")]
    Reconcile {
        skill: String,
        #[source]
        source: ReconcileSkillsError,
    },
}

impl SetSkillEnabledError {
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Catalog { source, .. } => source.class(),
            Self::Unmanaged { .. } => ErrorClass::Conflict,
            Self::Save { source, .. } => error::write(&source.cause),
            Self::Reconcile { source, .. } => source.class(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdoptSkillError {
    #[error("read which skills are turned off to adopt skill {skill}")]
    Customisation {
        skill: String,
        #[source]
        source: SkillCustomisationError,
    },
    #[error("adopt skill {skill} in the engine homes")]
    Homes {
        skill: String,
        #[source]
        source: HomesError,
    },
    #[error("Couldn't adopt skill {skill}")]
    Adopt {
        skill: String,
        #[source]
        source: Box<forge_engines::AdoptSkillError>,
    },
    #[error("put the skills into the engines after adopting skill {skill}")]
    Reconcile {
        skill: String,
        #[source]
        source: ReconcileSkillsError,
    },
    #[error("announce adopted skill {skill}")]
    Bump {
        skill: String,
        #[source]
        source: BumpRevisionError,
    },
    #[error("read the skill catalog after adopting skill {skill}")]
    Catalog {
        skill: String,
        #[source]
        source: SkillCatalogError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum DiscardSkillError {
    #[error("read which skills are turned off to discard skill {skill}")]
    Customisation {
        skill: String,
        #[source]
        source: SkillCustomisationError,
    },
    #[error("discard skill {skill} in the engine homes")]
    Homes {
        skill: String,
        #[source]
        source: HomesError,
    },
    #[error("Couldn't discard skill {skill}")]
    Discard {
        skill: String,
        #[source]
        source: forge_engines::DiscardSkillError,
    },
    #[error("announce discarded skill {skill}")]
    Bump {
        skill: String,
        #[source]
        source: BumpRevisionError,
    },
    #[error("read the skill catalog after discarding skill {skill}")]
    Catalog {
        skill: String,
        #[source]
        source: SkillCatalogError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum InstallSkillError {
    #[error("read which skills are turned off to install the skill in {}", from.display())]
    Customisation {
        from: PathBuf,
        #[source]
        source: SkillCustomisationError,
    },
    #[error("install the skill in {} into the engine homes", from.display())]
    Homes {
        from: PathBuf,
        #[source]
        source: HomesError,
    },
    #[error("Couldn't install the skill in {}", from.display())]
    Install {
        from: PathBuf,
        #[source]
        source: forge_engines::InstallSkillError,
    },
    #[error("turn installed skill {skill} on")]
    Enable {
        skill: String,
        #[source]
        source: forge_store::SetSkillEnabledError,
    },
    #[error("put the skills into the engines after installing skill {skill}")]
    Reconcile {
        skill: String,
        #[source]
        source: ReconcileSkillsError,
    },
    #[error("announce installed skill {skill}")]
    Bump {
        skill: String,
        #[source]
        source: BumpRevisionError,
    },
}

impl InstallSkillError {
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Customisation { .. } | Self::Homes { .. } => ErrorClass::Internal,
            Self::Install { .. } => ErrorClass::Conflict,
            Self::Enable { source, .. } => error::write(&source.cause),
            Self::Reconcile { source, .. } => source.class(),
            Self::Bump { source, .. } => error::setting(&source.cause),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlaceSkillsError {
    #[error("read which skills are turned off to place the skills")]
    Customisation(#[source] SkillCustomisationError),
    #[error("place the skills into the engine homes")]
    Homes(#[source] HomesError),
    #[error("Couldn't place the skill in {}", from.display())]
    Place {
        from: PathBuf,
        #[source]
        source: Box<forge_engines::InstallSkillError>,
    },
    #[error("Couldn't place the skill in {} ({placing}), nor take back the skills placed before it", from.display())]
    Stranded {
        from: PathBuf,
        placing: Box<forge_engines::InstallSkillError>,
        #[source]
        source: Box<forge_engines::UndoSkillError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum UndoSkillsError {
    #[error("take the placed skills back out of the engine homes")]
    Homes(#[source] HomesError),
    #[error("take a placed skill back")]
    Undo(#[source] forge_engines::UndoSkillError),
}

#[derive(Debug, thiserror::Error)]
pub enum KeepSkillsError {
    #[error("put the placed skills into the engines")]
    Reconcile(#[source] ReconcileSkillsError),
    #[error("announce the placed skills")]
    Bump(#[source] BumpRevisionError),
}

#[derive(Debug)]
#[must_use = "placed skills are kept or undone"]
pub struct PlacedSkills {
    extensions: Extensions,
    placed: Vec<forge_engines::PlacedSkill>,
}

impl PlacedSkills {
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.placed.iter().map(forge_engines::PlacedSkill::name)
    }

    pub async fn undo(self) -> Result<(), UndoSkillsError> {
        let Self { extensions, placed } = self;
        extensions
            .on_homes(move || undo_placed(placed))
            .await
            .map_err(UndoSkillsError::Homes)?
            .map_err(UndoSkillsError::Undo)
    }

    pub async fn keep(self) -> Result<(), KeepSkillsError> {
        self.extensions
            .reconcile_skills()
            .await
            .map_err(KeepSkillsError::Reconcile)?;
        if !self.placed.is_empty() {
            self.extensions
                .store()
                .bump_revision("skills_revision".into())
                .await
                .map_err(KeepSkillsError::Bump)?;
        }
        Ok(())
    }
}

fn undo_placed(
    placed: Vec<forge_engines::PlacedSkill>,
) -> Result<(), forge_engines::UndoSkillError> {
    placed
        .into_iter()
        .rev()
        .try_for_each(forge_engines::undo_skill)
}

#[derive(Debug, thiserror::Error)]
pub enum MarketplaceSearchError {
    #[error("Couldn't search skills.sh for {query:?}")]
    Search {
        query: String,
        #[source]
        source: forge_engines::SearchSkillsError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum MarketplaceInstallError {
    #[error("make a staging folder in {} for listing {listing}", path.display())]
    Staging {
        listing: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("run the staging folder task for listing {listing}")]
    StagingTask {
        listing: String,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("Couldn't fetch listing {listing}")]
    Fetch {
        listing: String,
        #[source]
        source: Box<forge_engines::FetchSkillError>,
    },
    #[error("install listing {listing}")]
    Install {
        listing: String,
        #[source]
        source: Box<InstallSkillError>,
    },
    #[error("read the skill catalog after installing listing {listing}")]
    Catalog {
        listing: String,
        #[source]
        source: SkillCatalogError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum GlobalPromptError {
    #[error("read Forge's {GLOBAL_PROMPT_KEY} setting")]
    Read(#[source] ReadSettingError),
    #[error("read the engines' global prompt files")]
    Candidates(#[source] HomesError),
    #[error("take over the engines' global prompt")]
    TakeOver(#[source] SetSettingError),
    #[error("read the global prompt's version")]
    Version(#[source] PromptVersionError),
    #[error("compare the global prompt with the engines' files")]
    Engines(#[source] HomesError),
}

impl GlobalPromptError {
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::TakeOver(source) => error::setting(&source.cause),
            Self::Read(_) | Self::Candidates(_) | Self::Version(_) | Self::Engines(_) => {
                ErrorClass::Internal
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SaveGlobalPromptError {
    #[error("save a global prompt of {bytes} bytes")]
    TooLong {
        bytes: usize,
        #[source]
        refused: Refused,
    },
    #[error("save the global prompt over version {base}")]
    Save {
        base: u64,
        #[source]
        source: SavePromptError,
    },
    #[error("save the global prompt over version {base} while version {current} is the latest")]
    Stale {
        base: u64,
        current: u64,
        #[source]
        refused: Refused,
    },
    #[error("write the saved global prompt into the engines")]
    Reconcile(#[source] GlobalPromptError),
}

impl SaveGlobalPromptError {
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::TooLong { .. } | Self::Stale { .. } => ErrorClass::Conflict,
            Self::Save { source, .. } => error::setting(&source.cause),
            Self::Reconcile(source) => source.class(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveGlobalPromptError {
    #[error("read the global prompt to resolve")]
    Current(#[source] GlobalPromptError),
    #[error("resolve {engine}'s global prompt")]
    NotDrifted {
        engine: EngineId,
        #[source]
        refused: Refused,
    },
    #[error("choose prompt {candidate} as the global prompt")]
    NoCandidate {
        candidate: String,
        #[source]
        refused: Refused,
    },
    #[error("save the resolved global prompt over version {version}")]
    Save {
        version: u64,
        #[source]
        source: SavePromptError,
    },
    #[error(
        "save the resolved global prompt over version {version} while version {current} is the latest"
    )]
    Changed {
        version: u64,
        current: u64,
        #[source]
        refused: Refused,
    },
    #[error("write the resolved global prompt into the engines")]
    Reconcile(#[source] GlobalPromptError),
    #[error("announce the resolved global prompt")]
    Bump(#[source] BumpRevisionError),
}

#[derive(Debug, Clone)]
pub struct Extensions {
    store: Store,
    engines: Engines,
    data_dir: PathBuf,
    homes: Arc<Mutex<()>>,
}

impl Hub {
    pub(crate) async fn customisation(&self) -> Result<Customisation, CustomisationError> {
        let prompt = self
            .extensions()
            .launch_prompt()
            .await
            .map_err(CustomisationError::Prompt)?;
        let servers = crate::mcp_auth::read_mcp_servers(self.store())
            .await
            .map_err(CustomisationError::McpServers)?;
        Ok(Customisation {
            prompt,
            mcp_servers: self.mcp().with_tokens(servers).await,
            ..self
                .extensions()
                .skill_customisation()
                .await
                .map_err(CustomisationError::Skills)?
        })
    }
}

impl Extensions {
    pub(crate) fn new(store: Store, engines: Engines, data_dir: PathBuf) -> Self {
        Self {
            store,
            engines,
            data_dir,
            homes: Arc::default(),
        }
    }

    fn store(&self) -> &Store {
        &self.store
    }

    fn engines(&self) -> &Engines {
        &self.engines
    }

    fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub(crate) fn skills_dir(&self) -> PathBuf {
        self.data_dir.join("extensions").join("skills")
    }

    pub(crate) fn homes_lock(&self) -> Result<(MutexGuard<'_, ()>, std::fs::File), HomesLockError> {
        let local = self.homes.lock().unwrap_or_else(PoisonError::into_inner);
        let path = self.data_dir.join("homes.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| HomesLockError::Open {
                path: path.clone(),
                source,
            })?;
        fs2::FileExt::lock_exclusive(&file)
            .map_err(|source| HomesLockError::Lock { path, source })?;
        Ok((local, file))
    }

    fn engine_homes(&self) -> Vec<(EngineId, Option<PathBuf>)> {
        let engines = self.engines();
        EngineId::ALL
            .into_iter()
            .map(|engine| (engine, engines.binary(engine).map(|_| engines.home(engine))))
            .collect()
    }

    async fn disabled_skills(&self) -> Result<BTreeSet<String>, SkillCustomisationError> {
        match self
            .store()
            .setting(DISABLED_SKILLS_KEY)
            .await
            .map_err(SkillCustomisationError::Read)?
        {
            None => Ok(BTreeSet::new()),
            Some(value) => {
                serde_json::from_value(value).map_err(SkillCustomisationError::DisabledSkills)
            }
        }
    }

    async fn skill_customisation(&self) -> Result<Customisation, SkillCustomisationError> {
        Ok(Customisation {
            skills_dir: self.skills_dir(),
            first_party: true,
            disabled_skills: self.disabled_skills().await?,
            ..Customisation::default()
        })
    }

    async fn on_homes<R: Send + 'static>(
        &self,
        job: impl FnOnce() -> R + Send + 'static,
    ) -> Result<R, HomesError> {
        let extensions = self.clone();
        tokio::task::spawn_blocking(move || {
            let _homes = extensions.homes_lock().map_err(HomesError::Lock)?;
            Ok(job())
        })
        .await
        .map_err(HomesError::Task)?
    }

    pub async fn skill_catalog(&self) -> Result<Vec<CatalogSkill>, SkillCatalogError> {
        let customisation = self
            .skill_customisation()
            .await
            .map_err(SkillCatalogError::Customisation)?;
        let homes = self.engine_homes();
        self.on_homes(move || forge_engines::skill_catalog(&homes, &customisation))
            .await
            .map_err(SkillCatalogError::Homes)?
            .map_err(SkillCatalogError::Read)
    }

    pub async fn enabled_skills(&self) -> Result<Vec<artisan_api::Skill>, EnabledSkillsError> {
        let customisation = self
            .skill_customisation()
            .await
            .map_err(EnabledSkillsError::Customisation)?;
        self.on_homes(move || forge_engines::enabled_skills(&customisation))
            .await
            .map_err(EnabledSkillsError::Homes)?
            .map_err(EnabledSkillsError::Read)
    }

    pub async fn set_skill_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<Vec<CatalogSkill>, SetSkillEnabledError> {
        let catalog = |source| SetSkillEnabledError::Catalog {
            skill: id.to_owned(),
            source,
        };
        let known = self
            .skill_catalog()
            .await
            .map_err(catalog)?
            .into_iter()
            .any(|skill| skill.id == id && skill.origin != SkillOrigin::Discovered);
        if !known {
            return Err(SetSkillEnabledError::Unmanaged {
                skill: id.to_owned(),
                refused: Refused("Artisan doesn't manage that skill.".to_owned()),
            });
        }
        self.store()
            .set_skill_enabled(id.to_owned(), enabled)
            .await
            .map_err(|source| SetSkillEnabledError::Save {
                skill: id.to_owned(),
                source,
            })?;
        self.reconcile_skills()
            .await
            .map_err(|source| SetSkillEnabledError::Reconcile {
                skill: id.to_owned(),
                source,
            })?;
        self.skill_catalog().await.map_err(catalog)
    }

    pub async fn adopt_skill(&self, id: &str) -> Result<Vec<CatalogSkill>, AdoptSkillError> {
        let skill = || id.to_owned();
        let customisation =
            self.skill_customisation()
                .await
                .map_err(|source| AdoptSkillError::Customisation {
                    skill: skill(),
                    source,
                })?;
        let homes = self.engine_homes();
        let adopted = id.to_owned();
        self.on_homes(move || forge_engines::adopt_skill(&homes, &customisation, &adopted))
            .await
            .map_err(|source| AdoptSkillError::Homes {
                skill: skill(),
                source,
            })?
            .map_err(|source| AdoptSkillError::Adopt {
                skill: skill(),
                source: Box::new(source),
            })?;
        self.reconcile_skills()
            .await
            .map_err(|source| AdoptSkillError::Reconcile {
                skill: skill(),
                source,
            })?;
        self.store()
            .bump_revision("skills_revision".into())
            .await
            .map_err(|source| AdoptSkillError::Bump {
                skill: skill(),
                source,
            })?;
        self.skill_catalog()
            .await
            .map_err(|source| AdoptSkillError::Catalog {
                skill: skill(),
                source,
            })
    }

    pub async fn marketplace_search(
        &self,
        query: &str,
    ) -> Result<Vec<artisan_api::Listing>, MarketplaceSearchError> {
        forge_engines::search_skills(self.engines().http(), forge_engines::SKILLS_SH_URL, query)
            .await
            .map_err(|source| MarketplaceSearchError::Search {
                query: query.to_owned(),
                source,
            })
    }

    pub async fn marketplace_install(
        &self,
        id: &str,
    ) -> Result<Vec<CatalogSkill>, MarketplaceInstallError> {
        let staging = marketplace_staging(self.data_dir().join("marketplace"), id).await?;
        let scratch = staging.path().to_path_buf();
        let installed = match forge_engines::fetch_skill(self.engines().http(), id, &scratch).await
        {
            Ok(folder) => self
                .install_skill(folder, false)
                .await
                .map(drop)
                .map_err(|source| MarketplaceInstallError::Install {
                    listing: id.to_owned(),
                    source: Box::new(source),
                }),
            Err(source) => Err(MarketplaceInstallError::Fetch {
                listing: id.to_owned(),
                source: Box::new(source),
            }),
        };
        match tokio::task::spawn_blocking(move || staging.close()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(listing = %id, path = %scratch.display(), "remove the marketplace staging folder: {error}");
            }
            Err(error) => {
                tracing::warn!(listing = %id, path = %scratch.display(), "remove the marketplace staging folder task: {error}");
            }
        }
        installed?;
        self.skill_catalog()
            .await
            .map_err(|source| MarketplaceInstallError::Catalog {
                listing: id.to_owned(),
                source,
            })
    }

    pub async fn discard_skill(&self, id: &str) -> Result<Vec<CatalogSkill>, DiscardSkillError> {
        let skill = || id.to_owned();
        let customisation = self.skill_customisation().await.map_err(|source| {
            DiscardSkillError::Customisation {
                skill: skill(),
                source,
            }
        })?;
        let homes = self.engine_homes();
        let discarded = id.to_owned();
        self.on_homes(move || forge_engines::discard_skill(&homes, &customisation, &discarded))
            .await
            .map_err(|source| DiscardSkillError::Homes {
                skill: skill(),
                source,
            })?
            .map_err(|source| DiscardSkillError::Discard {
                skill: skill(),
                source,
            })?;
        self.store()
            .bump_revision("skills_revision".into())
            .await
            .map_err(|source| DiscardSkillError::Bump {
                skill: skill(),
                source,
            })?;
        self.skill_catalog()
            .await
            .map_err(|source| DiscardSkillError::Catalog {
                skill: skill(),
                source,
            })
    }

    pub async fn install_skill(
        &self,
        from: PathBuf,
        replace: bool,
    ) -> Result<String, InstallSkillError> {
        let customisation = self.skill_customisation().await.map_err(|source| {
            InstallSkillError::Customisation {
                from: from.clone(),
                source,
            }
        })?;
        let homes = self.engine_homes();
        let folder = from.clone();
        let name = self
            .on_homes(move || {
                forge_engines::install_skill(&homes, &customisation, &folder, replace)
            })
            .await
            .map_err(|source| InstallSkillError::Homes {
                from: from.clone(),
                source,
            })?
            .map_err(|source| InstallSkillError::Install { from, source })?;
        self.store()
            .set_skill_enabled(format!("local.{name}"), true)
            .await
            .map_err(|source| InstallSkillError::Enable {
                skill: name.clone(),
                source,
            })?;
        self.reconcile_skills()
            .await
            .map_err(|source| InstallSkillError::Reconcile {
                skill: name.clone(),
                source,
            })?;
        self.store()
            .bump_revision("skills_revision".into())
            .await
            .map_err(|source| InstallSkillError::Bump {
                skill: name.clone(),
                source,
            })?;
        Ok(name)
    }

    pub async fn place_skills(
        &self,
        folders: Vec<PathBuf>,
    ) -> Result<PlacedSkills, PlaceSkillsError> {
        let customisation = self
            .skill_customisation()
            .await
            .map_err(PlaceSkillsError::Customisation)?;
        let homes = self.engine_homes();
        let placed = self
            .on_homes(move || {
                let mut placed = Vec::with_capacity(folders.len());
                for from in folders {
                    match forge_engines::place_skill(&homes, &customisation, &from, true) {
                        Ok(skill) => placed.push(skill),
                        Err(placing) => {
                            return Err(match undo_placed(placed) {
                                Ok(()) => PlaceSkillsError::Place {
                                    from,
                                    source: Box::new(placing),
                                },
                                Err(source) => PlaceSkillsError::Stranded {
                                    from,
                                    placing: Box::new(placing),
                                    source: Box::new(source),
                                },
                            });
                        }
                    }
                }
                Ok(placed)
            })
            .await
            .map_err(PlaceSkillsError::Homes)??;
        Ok(PlacedSkills {
            extensions: self.clone(),
            placed,
        })
    }

    pub async fn reconcile_skills(&self) -> Result<(), ReconcileSkillsError> {
        let customisation = self
            .skill_customisation()
            .await
            .map_err(ReconcileSkillsError::Customisation)?;
        let homes = self.engine_homes();
        self.on_homes(move || forge_engines::reconcile_skills(&homes, &customisation))
            .await
            .map_err(ReconcileSkillsError::Homes)?
            .map_err(ReconcileSkillsError::Reconcile)
    }

    async fn launch_prompt(&self) -> Result<Option<String>, GlobalPromptError> {
        if let Some(text) = self.stored_prompt().await? {
            return Ok(Some(text));
        }
        let prompt = self.reconcile_global_prompt().await?;
        Ok(prompt.candidates.is_empty().then_some(prompt.text))
    }

    async fn stored_prompt(&self) -> Result<Option<String>, GlobalPromptError> {
        Ok(self
            .store()
            .setting(GLOBAL_PROMPT_KEY)
            .await
            .map_err(GlobalPromptError::Read)?
            .and_then(|value| value.as_str().map(str::to_owned)))
    }

    pub async fn global_prompt(&self) -> Result<GlobalPrompt, GlobalPromptError> {
        self.prompt_projection(false, None).await
    }

    pub async fn reconcile_global_prompt(&self) -> Result<GlobalPrompt, GlobalPromptError> {
        self.prompt_projection(true, None).await
    }

    async fn prompt_projection(
        &self,
        reconcile: bool,
        force: Option<EngineId>,
    ) -> Result<GlobalPrompt, GlobalPromptError> {
        let homes = self.engine_homes();
        if self.stored_prompt().await?.is_none() {
            let found = homes.clone();
            let mut candidates = self
                .on_homes(move || candidates(&found))
                .await
                .map_err(GlobalPromptError::Candidates)?;
            if !reconcile || candidates.len() > 1 {
                return Ok(GlobalPrompt {
                    version: 0,
                    text: String::new(),
                    candidates,
                    engines: awaiting(&homes),
                });
            }
            let text = candidates
                .pop()
                .map(|candidate| candidate.text)
                .unwrap_or_default();
            self.store()
                .set_setting_if_absent(GLOBAL_PROMPT_KEY.to_owned(), serde_json::json!(text))
                .await
                .map_err(GlobalPromptError::TakeOver)?;
        }
        let text = self.stored_prompt().await?.unwrap_or_default();
        let version = self
            .store()
            .prompt_version()
            .await
            .map_err(GlobalPromptError::Version)?;
        let canonical = text.clone();
        let engines = self
            .on_homes(move || {
                homes
                    .iter()
                    .map(|(engine, home)| {
                        prompt_state(*engine, home.as_ref(), &canonical, reconcile, force)
                    })
                    .collect()
            })
            .await
            .map_err(GlobalPromptError::Engines)?;
        Ok(GlobalPrompt {
            version,
            text,
            candidates: Vec::new(),
            engines,
        })
    }

    pub async fn save_global_prompt(
        &self,
        save: SavePrompt,
    ) -> Result<GlobalPrompt, SaveGlobalPromptError> {
        if save.text.len() > PROMPT_MAX_BYTES {
            return Err(SaveGlobalPromptError::TooLong {
                bytes: save.text.len(),
                refused: Refused("The global prompt is longer than 64 KB.".to_owned()),
            });
        }
        let base = save.base_version;
        self.store()
            .save_prompt_version(base, save.text)
            .await
            .map_err(|source| SaveGlobalPromptError::Save { base, source })?
            .map_err(|current| SaveGlobalPromptError::Stale {
                base,
                current,
                refused: Refused(
                    "The global prompt changed since you started editing. Review the current version and save again.".to_owned(),
                ),
            })?;
        self.prompt_projection(true, None)
            .await
            .map_err(SaveGlobalPromptError::Reconcile)
    }

    pub async fn resolve_global_prompt(
        &self,
        resolution: ResolvePrompt,
    ) -> Result<GlobalPrompt, ResolveGlobalPromptError> {
        let prompt = self.resolve_prompt_inner(resolution).await?;
        self.store()
            .bump_revision("prompt_revision".into())
            .await
            .map_err(ResolveGlobalPromptError::Bump)?;
        Ok(prompt)
    }

    async fn resolve_prompt_inner(
        &self,
        resolution: ResolvePrompt,
    ) -> Result<GlobalPrompt, ResolveGlobalPromptError> {
        let current = self
            .global_prompt()
            .await
            .map_err(ResolveGlobalPromptError::Current)?;
        let drifted = |engine: EngineId| {
            current
                .engines
                .iter()
                .find(|state| state.engine == engine && state.status == PromptEngineStatus::Drifted)
                .and_then(|state| state.external_text.clone())
                .ok_or_else(|| ResolveGlobalPromptError::NotDrifted {
                    engine,
                    refused: Refused(format!(
                        "{}'s global prompt is no longer edited outside Artisan.",
                        engine.display_name()
                    )),
                })
        };
        let version = current.version;
        let changed = |message: &str| {
            let message = message.to_owned();
            move |latest| ResolveGlobalPromptError::Changed {
                version,
                current: latest,
                refused: Refused(message),
            }
        };
        let force = match resolution {
            ResolvePrompt::Choose { candidate_id } => {
                let candidate = current
                    .candidates
                    .iter()
                    .find(|candidate| candidate.id == candidate_id)
                    .ok_or_else(|| ResolveGlobalPromptError::NoCandidate {
                        candidate: candidate_id.clone(),
                        refused: Refused(
                            if current.candidates.is_empty() {
                                "The global prompt is already set up."
                            } else {
                                "That prompt is no longer in your engines."
                            }
                            .to_owned(),
                        ),
                    })?;
                self.store()
                    .save_prompt_version(version, candidate.text.clone())
                    .await
                    .map_err(|source| ResolveGlobalPromptError::Save { version, source })?
                    .map_err(changed("The global prompt was set up meanwhile."))?;
                None
            }
            ResolvePrompt::UseEngine { engine } => {
                let text = drifted(engine)?;
                self.store()
                    .save_prompt_version(version, text)
                    .await
                    .map_err(|source| ResolveGlobalPromptError::Save { version, source })?
                    .map_err(changed(
                        "The global prompt changed meanwhile. Review it and try again.",
                    ))?;
                None
            }
            ResolvePrompt::RestoreEngine { engine } => {
                drifted(engine)?;
                Some(engine)
            }
        };
        self.prompt_projection(true, force)
            .await
            .map_err(ResolveGlobalPromptError::Reconcile)
    }
}

async fn marketplace_staging(
    root: PathBuf,
    listing: &str,
) -> Result<tempfile::TempDir, MarketplaceInstallError> {
    let staging_root = root.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&staging_root)?;
        tempfile::Builder::new()
            .prefix("install-")
            .tempdir_in(&staging_root)
    })
    .await
    .map_err(|source| MarketplaceInstallError::StagingTask {
        listing: listing.to_owned(),
        source,
    })?
    .map_err(|source| MarketplaceInstallError::Staging {
        listing: listing.to_owned(),
        path: root,
        source,
    })
}

fn candidates(homes: &[(EngineId, Option<PathBuf>)]) -> Vec<PromptCandidate> {
    let mut found: Vec<PromptCandidate> = Vec::new();
    for (engine, home) in homes {
        let Some(home) = home else { continue };
        let Some(file) = forge_engines::read_prompt_file(*engine, home) else {
            continue;
        };
        let Ok(Some(text)) = file.text else { continue };
        let text = forge_engines::normalize_prompt(&text);
        if text.is_empty() {
            continue;
        }
        let id = forge_engines::prompt_digest(&text);
        match found.iter_mut().find(|candidate| candidate.id == id) {
            Some(candidate) => candidate.engines.push(*engine),
            None => found.push(PromptCandidate {
                id,
                engines: vec![*engine],
                text,
            }),
        }
    }
    found
}

fn unavailable(engine: EngineId) -> PromptEngineState {
    PromptEngineState {
        engine,
        status: PromptEngineStatus::Unavailable,
        file: None,
        external_text: None,
        reason: Some(format!(
            "{} reads no global prompt file Artisan can manage.",
            engine.display_name()
        )),
    }
}

fn awaiting(homes: &[(EngineId, Option<PathBuf>)]) -> Vec<PromptEngineState> {
    homes
        .iter()
        .map(|(engine, home)| {
            let probe = home.clone().unwrap_or_default();
            let Some(file) = forge_engines::read_prompt_file(*engine, &probe) else {
                return unavailable(*engine);
            };
            PromptEngineState {
                engine: *engine,
                status: if home.is_some() {
                    PromptEngineStatus::AwaitingChoice
                } else {
                    PromptEngineStatus::EngineAbsent
                },
                file: Some(file.display),
                external_text: None,
                reason: None,
            }
        })
        .collect()
}

fn prompt_state(
    engine: EngineId,
    home: Option<&PathBuf>,
    canonical: &str,
    reconcile: bool,
    force: Option<EngineId>,
) -> PromptEngineState {
    let probe = home.cloned().unwrap_or_default();
    let Some(file) = forge_engines::read_prompt_file(engine, &probe) else {
        return unavailable(engine);
    };
    let state = |status, external_text, reason| PromptEngineState {
        engine,
        status,
        file: Some(file.display.clone()),
        external_text,
        reason,
    };
    let Some(home) = home else {
        return state(PromptEngineStatus::EngineAbsent, None, None);
    };
    let applied = if reconcile {
        forge_engines::apply_prompt(engine, home, canonical, force == Some(engine))
    } else {
        forge_engines::inspect_prompt(engine, home, canonical)
    };
    match applied {
        Some(PromptApplied::Synced) | None => state(PromptEngineStatus::Synced, None, None),
        Some(PromptApplied::Drifted(text)) => state(PromptEngineStatus::Drifted, Some(text), None),
        Some(PromptApplied::Failed(reason)) => {
            state(PromptEngineStatus::Failed, None, Some(reason))
        }
    }
}

#[cfg(test)]
#[path = "../tests/extensions/staging.rs"]
mod staging_tests;
