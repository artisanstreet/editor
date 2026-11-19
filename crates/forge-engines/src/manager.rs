use crate::catalog::{self, Listing, Program};
use crate::engines::{Artifact, Feed, host_spec, meets_minimum};
use crate::install::{self, Event, Layout, Machine, Resolved};
use artisan_api::{
    EngineId, EngineStatus, InstallState, Integrity, Model, PublishedVersion, SlashCommand,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::broadcast;

mod accounts;

pub use accounts::NotInstalled;

#[derive(Debug, Clone)]
pub struct Engines {
    inner: Arc<Inner>,
}

#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "a full status is sent whole to every listener; the progress delta is the frequent one and stays small"
)]
pub enum EngineChange {
    Full(EngineStatus),
    Install {
        engine: EngineId,
        install: InstallState,
    },
}

#[derive(Debug)]
struct Inner {
    layout: Layout,
    http: reqwest::Client,
    statuses: broadcast::Sender<EngineChange>,
    slots: [Slot; EngineId::ALL.len()],
    usage: crate::UsageCache,
}

#[derive(Debug)]
struct Slot {
    install: Mutex<Machine>,
    models: Mutex<ModelCache>,
    sign_in: Arc<crate::SignIn>,
    sign_in_follower: Mutex<Option<tokio_util::task::AbortOnDropHandle<()>>>,
    model_reading: tokio::sync::Mutex<()>,
    versions: Mutex<Arc<[PublishedVersion]>>,
    commands: Mutex<Arc<[SlashCommand]>>,
}

#[derive(Debug, Default)]
struct ModelCache {
    discovery: artisan_api::DiscoveryState,
    generation: u64,
    reads: u64,
    listing: Option<CachedListing>,
    error: Option<Arc<catalog::ListModelsError>>,
}

#[derive(Debug, Clone, Default)]
struct CachedListing {
    models: Arc<[Model]>,
    default: Option<String>,
}
impl From<Listing> for CachedListing {
    fn from(listing: Listing) -> Self {
        Self {
            models: listing.models.into(),
            default: listing.default,
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.sign_in.stop_owner();
    }
}

#[derive(Debug)]
pub struct BinaryLease {
    path: PathBuf,
    usage: std::fs::File,
}
impl BinaryLease {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for BinaryLease {
    fn drop(&mut self) {
        if let Err(error) = fs2::FileExt::unlock(&self.usage) {
            tracing::warn!(path = %self.path.display(), %error, "release engine binary lease");
        }
    }
}

enum Target {
    Present,
    Latest,
    Exact(String),
    Previous,
}

#[derive(Debug, thiserror::Error)]
#[error("{} publishes no build for this platform", engine.display_name())]
pub struct NoBuild {
    pub engine: EngineId,
}

#[derive(Debug, thiserror::Error)]
pub enum ValidateVersionError {
    #[error(transparent)]
    Unusable(install::UnusableVersion),
    #[error("{} {version} is older than the oldest supported version {minimum}", engine.display_name())]
    TooOld {
        engine: EngineId,
        version: String,
        minimum: &'static str,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("take the {} install lock", engine.display_name())]
    Lock {
        engine: EngineId,
        #[source]
        source: install::LockError,
    },
    #[error("{} has no previous version to roll back to", engine.display_name())]
    NoPrevious { engine: EngineId },
    #[error(transparent)]
    NoBuild(NoBuild),
    #[error("find the latest {} version", engine.display_name())]
    Latest {
        engine: EngineId,
        #[source]
        source: install::LatestVersionError,
    },
    #[error(transparent)]
    Version(ValidateVersionError),
    #[error("switch {} to {version}", engine.display_name())]
    Switch {
        engine: EngineId,
        version: String,
        #[source]
        source: install::SwitchError,
    },
    #[error("wait for the {} version switch task", engine.display_name())]
    SwitchTask {
        engine: EngineId,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("resolve {} {version}", engine.display_name())]
    Resolve {
        engine: EngineId,
        version: String,
        #[source]
        source: install::ResolveError,
    },
    #[error(transparent)]
    Fetch(install::FetchError),
    #[error("find the current {} version", engine.display_name())]
    Pointer {
        engine: EngineId,
        #[source]
        source: install::ReadPointerError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ListVersionsError {
    #[error(transparent)]
    NoBuild(NoBuild),
    #[error("list {} versions", engine.display_name())]
    Get {
        engine: EngineId,
        #[source]
        source: install::GetError,
    },
    #[error("read the installed {} versions", engine.display_name())]
    Task {
        engine: EngineId,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("find the current {} version", engine.display_name())]
    Pointer {
        engine: EngineId,
        #[source]
        source: install::ReadPointerError,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("list {} models", engine.display_name())]
pub(crate) struct RefreshModelsError {
    engine: EngineId,
    #[source]
    source: Arc<catalog::ListModelsError>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClaudeAiSignInError {
    #[error("that browser is not signed in to claude.ai")]
    NotSignedIn,
    #[error("the session has no user agent")]
    NoUserAgent,
    #[error("build the claude.ai client")]
    Client(#[source] crate::claudeai::BuildClientError),
    #[error("claude.ai did not take the session")]
    Bootstrap(#[source] crate::claudeai::RequestError),
    #[error("claude.ai named no organization for the account")]
    NoOrganization,
    #[error("keeping the claude.ai session")]
    Save(#[source] crate::claudeai::SaveSessionError),
}

fn loaded(layout: &Layout, engine: EngineId) -> (Event, Option<String>) {
    match layout.installed(engine) {
        Ok(version) => (
            Event::Loaded {
                version: version.clone(),
            },
            version,
        ),
        Err(error) => (
            Event::Fail {
                message: diagnostics::chain(&error),
            },
            None,
        ),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Engines {
    #[must_use]
    pub fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }

    #[must_use]
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        let layout = Layout::new(data_dir.as_ref());
        let slots = EngineId::ALL.map(|engine| {
            let mut machine = install::machine();
            machine.handle(&loaded(&layout, engine).0);
            Slot {
                install: Mutex::new(machine),
                models: Mutex::new(ModelCache::default()),
                sign_in: crate::SignIn::new(engine, layout.home(engine)),
                sign_in_follower: Mutex::new(None),
                model_reading: tokio::sync::Mutex::new(()),
                versions: Mutex::new(Arc::from([])),
                commands: Mutex::new(Arc::from([])),
            }
        });
        let http = reqwest::Client::builder()
            .user_agent(concat!("artisan-forge/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .build()
            .expect("invariant: the engine catalog's HTTP client builds with the bundled TLS");
        Self {
            inner: Arc::new(Inner {
                layout,
                http,
                statuses: broadcast::channel(64).0,
                slots,
                usage: crate::UsageCache::new(accounts::USAGE_TTL),
            }),
        }
    }

    fn slot(&self, engine: EngineId) -> &Slot {
        let index = EngineId::ALL
            .iter()
            .position(|candidate| *candidate == engine)
            .unwrap_or_else(|| unreachable!("invariant: EngineId::ALL lists {engine}"));
        &self.inner.slots[index]
    }

    #[must_use]
    pub fn status(&self, engine: EngineId) -> EngineStatus {
        let slot = self.slot(engine);
        let install = self.install_state(engine);
        let listing = lock(&slot.models).listing.clone().unwrap_or_default();
        EngineStatus {
            discovery: lock(&slot.models).discovery.clone(),
            engine,
            install,
            sign_in: slot.sign_in.state(),
            models: listing.models,
            default_model: listing.default,
            commands: self.commands(engine),
            usage: self.inner.usage.cached(engine),
            integrity: integrity(engine),
            versions: self.published(engine),
            claude_ai: (engine == EngineId::Claude)
                .then(|| crate::claudeai::Session::state(&self.home(engine))),
        }
    }

    pub fn changed(&self, engine: EngineId) {
        self.publish(engine);
    }

    pub async fn claude_ai_sign_in(
        &self,
        captured: artisan_api::ClaudeAiSession,
    ) -> Result<artisan_api::ClaudeAiState, ClaudeAiSignInError> {
        use crate::claudeai::{Session, Transport};
        if !captured.signed_in() {
            return Err(ClaudeAiSignInError::NotSignedIn);
        }
        if captured.user_agent.trim().is_empty() {
            return Err(ClaudeAiSignInError::NoUserAgent);
        }
        let home = self.home(EngineId::Claude);
        let org = captured
            .cookie("lastActiveOrg")
            .unwrap_or_default()
            .to_owned();
        let transport = Transport::new(&home, Session::new(captured, org, None))
            .map_err(ClaudeAiSignInError::Client)?;
        let bootstrap = transport
            .get("/api/bootstrap")
            .await
            .map_err(ClaudeAiSignInError::Bootstrap)?;
        let account = &bootstrap["account"];
        let mut session = transport.session();
        session.account = account["email_address"]
            .as_str()
            .or_else(|| account["full_name"].as_str())
            .map(str::to_owned);
        if session.org_uuid.is_empty() {
            let memberships = account["memberships"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let chats = |membership: &&serde_json::Value| {
                membership["organization"]["capabilities"]
                    .as_array()
                    .is_some_and(|capabilities| capabilities.iter().any(|c| c == "chat"))
            };
            memberships
                .iter()
                .find(chats)
                .or_else(|| memberships.first())
                .and_then(|membership| membership["organization"]["uuid"].as_str())
                .ok_or(ClaudeAiSignInError::NoOrganization)?
                .clone_into(&mut session.org_uuid);
        }
        session.save(&home).map_err(ClaudeAiSignInError::Save)?;
        self.publish(EngineId::Claude);
        Ok(session.status())
    }

    pub fn claude_ai_sign_out(&self) -> Result<(), crate::claudeai::RemoveSessionError> {
        crate::claudeai::Session::remove(&self.home(EngineId::Claude))?;
        self.publish(EngineId::Claude);
        Ok(())
    }

    pub fn install_state(&self, engine: EngineId) -> InstallState {
        install::view(lock(&self.slot(engine).install).state())
    }

    fn commands(&self, engine: EngineId) -> Arc<[SlashCommand]> {
        let reported = lock(&self.slot(engine).commands).clone();
        let own: Vec<SlashCommand> = crate::commands::own(engine)
            .into_iter()
            .filter(|own| !reported.iter().any(|command| command.name == own.name))
            .collect();
        if own.is_empty() {
            return reported;
        }
        reported.iter().cloned().chain(own).collect()
    }

    pub fn set_commands(&self, engine: EngineId, commands: Vec<SlashCommand>) {
        let commands: Arc<[SlashCommand]> = crate::commands::reported(commands).into();
        {
            let mut current = lock(&self.slot(engine).commands);
            if *current == commands {
                return;
            }
            *current = commands;
        }
        self.publish(engine);
    }

    fn published(&self, engine: EngineId) -> Arc<[PublishedVersion]> {
        lock(&self.slot(engine).versions).clone()
    }

    pub async fn list_versions(
        &self,
        engine: EngineId,
    ) -> Result<Arc<[PublishedVersion]>, ListVersionsError> {
        let feed = artifact(engine).map_err(ListVersionsError::NoBuild)?.feed;
        let versions = install::published_versions(&self.inner.http, feed)
            .await
            .map_err(|source| ListVersionsError::Get { engine, source })?;
        let layout = self.inner.layout.clone();
        let published = tokio::task::spawn_blocking(move || {
            let current = layout
                .current(engine)
                .map_err(|source| ListVersionsError::Pointer { engine, source })?;
            Ok(versions
                .into_iter()
                .map(|version| PublishedVersion {
                    installed: layout.executable(engine, &version).is_some(),
                    current: current.as_deref() == Some(version.as_str()),
                    unsupported: !meets_minimum(engine, &version),
                    version,
                })
                .collect::<Arc<[PublishedVersion]>>())
        })
        .await
        .map_err(|source| ListVersionsError::Task { engine, source })??;
        *lock(&self.slot(engine).versions) = published;
        self.publish(engine);
        Ok(self.published(engine))
    }

    fn reconcile_install(&self, engine: EngineId) {
        let (loaded, current) = loaded(&self.inner.layout, engine);
        {
            let mut machine = lock(&self.slot(engine).install);
            if install::settled(machine.state()) {
                machine.handle(&loaded);
            }
        }
        let mut versions = lock(&self.slot(engine).versions);
        for version in Arc::make_mut(&mut *versions).iter_mut() {
            version.installed = self
                .inner
                .layout
                .executable(engine, &version.version)
                .is_some();
            version.current = current.as_deref() == Some(version.version.as_str());
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EngineChange> {
        self.inner.statuses.subscribe()
    }

    fn publish(&self, engine: EngineId) {
        let _no_listeners = self
            .inner
            .statuses
            .send(EngineChange::Full(self.status(engine)));
    }

    fn apply(&self, engine: EngineId, event: &Event) {
        lock(&self.slot(engine).install).handle(event);
        if matches!(event, Event::Progress { .. }) {
            let _no_listeners = self.inner.statuses.send(EngineChange::Install {
                engine,
                install: self.install_state(engine),
            });
        } else {
            self.publish(engine);
        }
    }

    #[must_use]
    pub fn binary(&self, engine: EngineId) -> Option<PathBuf> {
        match self.inner.layout.binary(engine) {
            Ok(binary) => binary,
            Err(error) => {
                tracing::warn!(%engine, error = %diagnostics::chain(&error), "the engine's installed version can't be read");
                None
            }
        }
    }

    pub fn lease(&self, engine: EngineId) -> Option<BinaryLease> {
        let usage = match install::usage_lock(&self.home(engine)) {
            Ok(usage) => usage,
            Err(error) => {
                tracing::warn!(%engine, error = %diagnostics::chain(&error), "the engine's binary could not be leased");
                return None;
            }
        };
        if let Err(error) = fs2::FileExt::lock_shared(&usage) {
            tracing::warn!(%engine, %error, "the engine's binary could not be locked for use");
            return None;
        }
        let path = self.binary(engine)?;
        Some(BinaryLease { path, usage })
    }

    #[must_use]
    pub fn home(&self, engine: EngineId) -> PathBuf {
        self.inner.layout.home(engine)
    }

    #[must_use]
    pub fn launch_env_for_binary(&self, engine: EngineId, binary: &Path) -> Vec<(String, String)> {
        crate::engines::environment(engine, Some(binary), &self.home(engine))
    }

    pub fn validate_version(engine: EngineId, version: &str) -> Result<(), ValidateVersionError> {
        install::check_version(version).map_err(ValidateVersionError::Unusable)?;
        if !meets_minimum(engine, version) {
            return Err(ValidateVersionError::TooOld {
                engine,
                version: version.to_owned(),
                minimum: host_spec(engine).minimum.unwrap_or_default(),
            });
        }
        Ok(())
    }

    pub async fn install(&self, engine: EngineId) -> Result<String, InstallError> {
        self.change(engine, Target::Present).await
    }

    pub async fn update(&self, engine: EngineId) -> Result<String, InstallError> {
        self.change(engine, Target::Latest).await
    }

    pub async fn use_version(&self, engine: EngineId, version: &str) -> Result<(), InstallError> {
        self.change(engine, Target::Exact(version.to_owned()))
            .await
            .map(drop)
    }

    pub async fn rollback(&self, engine: EngineId) -> Result<String, InstallError> {
        self.change(engine, Target::Previous).await
    }

    async fn change(&self, engine: EngineId, target: Target) -> Result<String, InstallError> {
        let layout = &self.inner.layout;
        let _lock = match install::lock(layout, engine).await {
            Ok(lock) => lock,
            Err(source) => {
                return self
                    .finish(engine, Err(InstallError::Lock { engine, source }))
                    .await;
            }
        };
        let installed = match layout.installed(engine) {
            Ok(installed) => installed,
            Err(source) => {
                return self
                    .finish(engine, Err(InstallError::Pointer { engine, source }))
                    .await;
            }
        };
        if matches!(target, Target::Present)
            && let Some(current) = installed
        {
            self.apply(
                engine,
                &Event::Loaded {
                    version: Some(current.clone()),
                },
            );
            return Ok(current);
        }
        {
            let mut cache = lock(&self.slot(engine).models);
            cache.generation = cache.generation.wrapping_add(1);
            cache.error = None;
        }
        let result = async {
            let current = layout
                .installed(engine)
                .map_err(|source| InstallError::Pointer { engine, source })?;
            let version = match (target, &current) {
                (Target::Present, Some(current)) => return Ok(current.clone()),
                (Target::Exact(version), _) => version,
                (Target::Previous, _) => layout
                    .previous(engine)
                    .map_err(|source| InstallError::Pointer { engine, source })?
                    .filter(|version| layout.executable(engine, version).is_some())
                    .ok_or(InstallError::NoPrevious { engine })?,
                (Target::Present | Target::Latest, _) => {
                    let feed = artifact(engine).map_err(InstallError::NoBuild)?.feed;
                    install::latest_version(&self.inner.http, feed)
                        .await
                        .map_err(|source| InstallError::Latest { engine, source })?
                }
            };
            Self::validate_version(engine, &version).map_err(InstallError::Version)?;
            if layout.executable(engine, &version).is_some() {
                let switching = self.clone();
                let switching_version = version.clone();
                tokio::task::spawn_blocking(move || {
                    install::switch(&switching.inner.layout, engine, &switching_version)
                })
                .await
                .map_err(|source| InstallError::SwitchTask { engine, source })?
                .map_err(|source| InstallError::Switch {
                    engine,
                    version: version.clone(),
                    source,
                })?;
                return Ok(version);
            }
            let artifact = artifact(engine).map_err(InstallError::NoBuild)?;
            let resolved = install::resolve(&self.inner.http, artifact.feed, &version)
                .await
                .map_err(|source| InstallError::Resolve {
                    engine,
                    version: version.clone(),
                    source,
                })?;
            self.fetch(engine, artifact, &resolved)
                .await
                .map_err(InstallError::Fetch)?;
            Ok(version)
        }
        .await;
        self.finish(engine, result).await
    }

    async fn fetch(
        &self,
        engine: EngineId,
        artifact: Artifact,
        resolved: &Resolved,
    ) -> Result<(), install::FetchError> {
        let report = |event: Event| self.apply(engine, &event);
        install::fetch(
            &self.inner.layout,
            &self.inner.http,
            engine,
            artifact,
            resolved,
            &report,
        )
        .await
    }

    async fn finish(
        &self,
        engine: EngineId,
        result: Result<String, InstallError>,
    ) -> Result<String, InstallError> {
        match &result {
            Ok(version) => {
                let engines = self.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || {
                    if let Err(error) = install::prune(&engines.inner.layout, engine) {
                        tracing::warn!(%engine, error = %diagnostics::chain(&error), "old engine versions were kept");
                    }
                    engines.reconcile_install(engine);
                })
                .await
                {
                    tracing::warn!(%engine, %error, "pruning and reconciling after an install");
                }
                self.apply(
                    engine,
                    &Event::Done {
                        version: version.clone(),
                    },
                );
                self.models_changed(engine);
            }
            Err(error) => {
                let message = diagnostics::chain(error);
                tracing::warn!(%engine, error = message, "engine install failed");
                self.apply(engine, &Event::Fail { message });
            }
        }
        result
    }

    #[cfg(test)]
    pub(crate) async fn install_resolved(
        &self,
        engine: EngineId,
        artifact: Artifact,
        resolved: Resolved,
    ) -> Result<String, InstallError> {
        let _lock = install::lock(&self.inner.layout, engine)
            .await
            .map_err(|source| InstallError::Lock { engine, source })?;
        let result = self
            .fetch(engine, artifact, &resolved)
            .await
            .map(|()| resolved.version.clone())
            .map_err(InstallError::Fetch);
        self.finish(engine, result).await
    }

    async fn refresh_models(&self, engine: EngineId) -> Result<Arc<[Model]>, RefreshModelsError> {
        let slot = self.slot(engine);
        let observed = {
            let cache = lock(&slot.models);
            (cache.generation, cache.reads)
        };
        let observed_account = slot.sign_in.context_generation();
        let _reading = slot.model_reading.lock().await;
        if let Some(cached) = slot
            .sign_in
            .with_context(observed_account, || {
                let cache = lock(&slot.models);
                (cache.generation == observed.0 && cache.reads != observed.1).then(|| {
                    cache.error.as_ref().map_or_else(
                        || {
                            Ok(cache
                                .listing
                                .as_ref()
                                .map(|listing| Arc::clone(&listing.models))
                                .unwrap_or_default())
                        },
                        |error| {
                            Err(RefreshModelsError {
                                engine,
                                source: Arc::clone(error),
                            })
                        },
                    )
                })
            })
            .flatten()
        {
            return cached;
        }
        let account = slot.sign_in.context_generation();
        let generation = lock(&slot.models).generation;
        let last_success_at = lock(&slot.models).discovery.last_success_at();
        lock(&slot.models).discovery = artisan_api::DiscoveryState::Refreshing { last_success_at };
        self.publish(engine);
        let lease = self.lease(engine);
        let binary = lease.as_ref().map(|lease| lease.path().to_owned());
        let home = self.home(engine);
        let env = crate::engines::environment(engine, binary.as_deref(), &home);
        let program = Program {
            binary: binary.as_deref(),
            env: &env,
            home: &home,
        };
        let result = catalog::list(engine, program, &self.inner.http).await;
        let same_binary = self.binary(engine) == binary;
        let committed = slot
            .sign_in
            .with_context(account, || {
                let mut cache = lock(&slot.models);
                if !same_binary || cache.generation != generation {
                    return None;
                }
                cache.reads += 1;
                match result {
                    Ok(mut listing) => {
                        if let Some(commands) = listing.commands.take() {
                            *lock(&slot.commands) = crate::commands::reported(commands).into();
                        }
                        let listing = CachedListing::from(listing);
                        let models = Arc::clone(&listing.models);
                        cache.error = None;
                        cache.discovery = artisan_api::DiscoveryState::Fresh {
                            observed_at: artisan_api::now(),
                        };
                        cache.listing = Some(listing);
                        Some(Ok(models))
                    }
                    Err(error) => {
                        let error = Arc::new(error);
                        cache.discovery = cache.discovery.failed(&diagnostics::chain(&*error));
                        cache.error = Some(Arc::clone(&error));
                        Some(Err(RefreshModelsError {
                            engine,
                            source: error,
                        }))
                    }
                }
            })
            .flatten();
        committed.map_or_else(
            || Ok(Arc::from([])),
            |result| {
                self.publish(engine);
                result
            },
        )
    }

    fn invalidate_models(&self, engine: EngineId) {
        let mut cache = lock(&self.slot(engine).models);
        cache.generation = cache.generation.wrapping_add(1);
        cache.listing = None;
        cache.error = None;
        cache.discovery = artisan_api::DiscoveryState::Unknown;
    }

    fn models_changed(&self, engine: EngineId) {
        self.invalidate_models(engine);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let engines = self.clone();
        runtime.spawn(async move {
            if let Err(error) = engines.refresh_models(engine).await {
                tracing::debug!(%engine, error = diagnostics::chain(&error), "model refresh after install failed");
            }
        });
    }
}

fn integrity(engine: EngineId) -> Option<Integrity> {
    Some(match host_spec(engine).artifact?.feed {
        Feed::Claude { .. } => Integrity::VendorSha256,
        Feed::Npm { .. } => Integrity::NpmSha512,
        Feed::Release { .. } => Integrity::ArtisanSha256,
    })
}

fn artifact(engine: EngineId) -> Result<Artifact, NoBuild> {
    host_spec(engine).artifact.ok_or(NoBuild { engine })
}

#[cfg(test)]
#[cfg(unix)]
#[path = "../tests/manager/context.rs"]
mod context_tests;

#[cfg(test)]
#[path = "../tests/manager/snapshot.rs"]
mod snapshot_tests;

#[cfg(test)]
#[path = "../tests/manager/lease_release.rs"]
mod lease_release_tests;
