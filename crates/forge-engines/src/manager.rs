//! `Engines`: installs, versions and models for every engine, and the
//! `EngineStatus` stream the Editor shows.

use crate::catalog::{self, Listing, Program};
use crate::engines::{Artifact, Feed, compare_versions, host_spec, meets_minimum};
use crate::install::{self, Event, Layout, Machine, Resolved};
use anyhow::{Context, Result, bail};
use artisan_api::{
    EngineId, EngineStatus, InstallState, Integrity, Model, PublishedVersion, SlashCommand,
};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::broadcast;

mod accounts;

#[derive(Clone)]
pub struct Engines {
    inner: Arc<Inner>,
}

/// Full metadata snapshots and a precise install-progress delta.
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

struct Inner {
    layout: Layout,
    http: reqwest::Client,
    statuses: broadcast::Sender<EngineChange>,
    slots: [Slot; EngineId::ALL.len()],
    usage: crate::UsageCache,
}

/// Everything Forge tracks about one engine. Add a field here and fill it
/// in `Engines::status` to extend `EngineStatus`.
struct Slot {
    install: Mutex<Machine>,
    /// Cache of the last listing; `None` until the first one.
    models: Mutex<ModelCache>,
    sign_in: Arc<crate::SignIn>,
    sign_in_follower: Mutex<Option<tokio_util::task::AbortOnDropHandle<()>>>,
    model_reading: tokio::sync::Mutex<()>,
    /// The vendor's versions, newest first, once asked for.
    versions: Mutex<Arc<[PublishedVersion]>>,
    /// The `/` commands the engine last reported, by a probe or a run.
    commands: Mutex<Arc<[SlashCommand]>>,
}

#[derive(Default)]
struct ModelCache {
    generation: u64,
    reads: u64,
    listing: Option<CachedListing>,
    error: Option<String>,
}

#[derive(Clone, Default)]
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

/// Keeps an installed binary's files available until its run finishes.
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
        // A concurrent fork can briefly inherit this open file description.
        // Release the owner's lock explicitly instead of waiting for every
        // inherited descriptor to close at exec.
        if let Err(error) = fs2::FileExt::unlock(&self.usage) {
            tracing::warn!(path = %self.path.display(), %error, "release engine binary lease");
        }
    }
}

enum Target {
    /// Install the newest version unless one is installed already.
    Present,
    Latest,
    Exact(String),
    Previous,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Engines {
    /// Forge's HTTP client, for what the engines' vendors and directories
    /// serve.
    #[must_use]
    pub fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }

    #[must_use]
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        let layout = Layout::new(data_dir.as_ref());
        let slots = EngineId::ALL.map(|engine| {
            let mut machine = install::machine();
            machine.handle(&Event::Loaded {
                version: layout
                    .current(engine)
                    .filter(|_| layout.binary(engine).is_some()),
            });
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
            .unwrap_or_default();
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
            .unwrap_or_default();
        &self.inner.slots[index]
    }

    /// What Forge knows about `engine` now. Never probes the engine.
    #[must_use]
    pub fn status(&self, engine: EngineId) -> EngineStatus {
        let slot = self.slot(engine);
        let install = self.install_state(engine);
        let listing = lock(&slot.models).listing.clone().unwrap_or_default();
        EngineStatus {
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

    /// Something about the engine changed outside the manager (a run marked
    /// its claude.ai session): tell whoever shows its status.
    pub fn changed(&self, engine: EngineId) {
        self.publish(engine);
    }

    /// Checks a claude.ai session a browser captured, and keeps it for
    /// Native Claude Mode.
    ///
    /// # Errors
    ///
    /// Returns the error when the browser was not signed in or claude.ai
    /// does not take the session.
    pub async fn claude_ai_sign_in(
        &self,
        captured: artisan_api::ClaudeAiSession,
    ) -> Result<artisan_api::ClaudeAiState> {
        use crate::claudeai::{Session, Transport};
        anyhow::ensure!(
            captured.signed_in(),
            "that browser is not signed in to claude.ai"
        );
        anyhow::ensure!(
            !captured.user_agent.trim().is_empty(),
            "the session has no user agent"
        );
        let home = self.home(EngineId::Claude);
        let org = captured
            .cookie("lastActiveOrg")
            .unwrap_or_default()
            .to_owned();
        let transport = Transport::new(&home, Session::new(captured, org, None))?;
        let bootstrap = transport
            .get("/api/bootstrap")
            .await
            .context("claude.ai did not take the session")?;
        let account = &bootstrap["account"];
        let mut session = transport.session();
        session.account = account["email_address"]
            .as_str()
            .or_else(|| account["full_name"].as_str())
            .map(str::to_owned);
        if session.org_uuid.is_empty() {
            // The organization that can chat; the first one otherwise.
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
                .context("claude.ai named no organization for the account")?
                .clone_into(&mut session.org_uuid);
        }
        session
            .save(&home)
            .context("keeping the claude.ai session")?;
        self.publish(EngineId::Claude);
        Ok(session.status())
    }

    /// Forgets the claude.ai session.
    ///
    /// # Errors
    ///
    /// Returns the error if the session cannot be removed.
    pub fn claude_ai_sign_out(&self) -> Result<()> {
        crate::claudeai::Session::remove(&self.home(EngineId::Claude))?;
        self.publish(EngineId::Claude);
        Ok(())
    }

    /// Current scalar installation state without cloning any catalog metadata.
    pub fn install_state(&self, engine: EngineId) -> InstallState {
        install::view(lock(&self.slot(engine).install).state())
    }

    /// The listed versions with where each stands on disk now.
    /// What the engine reported, then what Forge reads for it that the
    /// engine didn't name.
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

    /// The engine said which `/` commands it reads now: a probe or a run
    /// reported them.
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

    /// Asks the vendor which versions it publishes ("Choose version").
    pub async fn list_versions(&self, engine: EngineId) -> Result<Arc<[PublishedVersion]>> {
        let feed = artifact(engine)?.feed;
        let versions = install::published_versions(&self.inner.http, feed)
            .await
            .with_context(|| format!("listing {} versions", engine.display_name()))?;
        let layout = self.inner.layout.clone();
        let published = tokio::task::spawn_blocking(move || {
            let current = layout.current(engine);
            versions
                .into_iter()
                .map(|version| PublishedVersion {
                    installed: layout.executable(engine, &version).is_some(),
                    current: current.as_deref() == Some(version.as_str()),
                    unsupported: !meets_minimum(engine, &version),
                    version,
                })
                .collect::<Arc<[PublishedVersion]>>()
        })
        .await
        .context("read installed engine versions")?;
        *lock(&self.slot(engine).versions) = published;
        self.publish(engine);
        Ok(self.published(engine))
    }

    /// Reconcile external CLI changes only at an explicit refresh boundary.
    fn reconcile_install(&self, engine: EngineId) {
        let current = self
            .inner
            .layout
            .current(engine)
            .filter(|_| self.binary(engine).is_some());
        {
            let mut machine = lock(&self.slot(engine).install);
            if install::settled(machine.state()) {
                machine.handle(&Event::Loaded {
                    version: current.clone(),
                });
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

    /// Every change to any engine's status.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EngineChange> {
        self.inner.statuses.subscribe()
    }

    fn publish(&self, engine: EngineId) {
        // Nobody listening is fine.
        self.inner
            .statuses
            .send(EngineChange::Full(self.status(engine)))
            .ok();
    }

    fn apply(&self, engine: EngineId, event: &Event) {
        lock(&self.slot(engine).install).handle(event);
        if matches!(event, Event::Progress { .. }) {
            // Nobody listening is fine.
            self.inner
                .statuses
                .send(EngineChange::Install {
                    engine,
                    install: self.install_state(engine),
                })
                .ok();
        } else {
            self.publish(engine);
        }
    }

    /// The engine's executable, if installed.
    #[must_use]
    pub fn binary(&self, engine: EngineId) -> Option<PathBuf> {
        self.inner.layout.binary(engine)
    }

    /// Pins installed files across Forge and CLI processes while in use.
    pub fn lease(&self, engine: EngineId) -> Option<BinaryLease> {
        let usage = install::usage_lock(&self.home(engine)).ok()?;
        fs2::FileExt::lock_shared(&usage).ok()?;
        let path = self.binary(engine)?;
        Some(BinaryLease { path, usage })
    }

    /// The engine's `HOME`: `<data>/engines/<engine>/home`.
    #[must_use]
    pub fn home(&self, engine: EngineId) -> PathBuf {
        self.inner.layout.home(engine)
    }

    /// Environment for running the engine: its home, its config directory
    /// and its own tool directories ahead of `PATH`.
    #[must_use]
    pub fn launch_env(&self, engine: EngineId) -> Vec<(String, String)> {
        crate::engines::environment(engine, self.binary(engine).as_deref(), &self.home(engine))
    }

    /// Uses the selected lease for both the executable and its helper PATH.
    #[must_use]
    pub fn launch_env_for_binary(&self, engine: EngineId, binary: &Path) -> Vec<(String, String)> {
        crate::engines::environment(engine, Some(binary), &self.home(engine))
    }

    /// Arguments that start the engine's own sign-in.
    #[must_use]
    pub fn login_args(engine: EngineId) -> &'static [&'static str] {
        host_spec(engine).login
    }

    /// Validates a version command before an asynchronous operation is accepted.
    pub fn validate_version(engine: EngineId, version: &str) -> Result<()> {
        install::check_version(version)?;
        if !meets_minimum(engine, version) {
            bail!(
                "{} {version} is older than the oldest supported version {}",
                engine.display_name(),
                host_spec(engine).minimum.unwrap_or_default()
            );
        }
        Ok(())
    }

    /// Installs the newest version unless one is installed. Returns the
    /// current version.
    pub async fn install(&self, engine: EngineId) -> Result<String> {
        self.change(engine, Target::Present).await
    }

    /// Installs the newest version if it differs from the current one; the
    /// current version keeps serving until the new one is in place.
    pub async fn update(&self, engine: EngineId) -> Result<String> {
        self.change(engine, Target::Latest).await
    }

    /// Makes `version` current, downloading it if it isn't the current or
    /// previous one.
    pub async fn use_version(&self, engine: EngineId, version: &str) -> Result<()> {
        self.change(engine, Target::Exact(version.to_owned()))
            .await
            .map(drop)
    }

    /// Swaps back to the previous version. Returns it.
    pub async fn rollback(&self, engine: EngineId) -> Result<String> {
        self.change(engine, Target::Previous).await
    }

    /// Asks the feed for the newest version and records it as an available
    /// update when it is newer than the current one.
    pub async fn check_update(&self, engine: EngineId) -> Result<Option<String>> {
        let artifact = artifact(engine)?;
        let latest = install::latest_version(&self.inner.http, artifact.feed).await?;
        let current = self.inner.layout.current(engine);
        let newer = current
            .is_some_and(|current| compare_versions(&latest, &current) == Ordering::Greater)
            .then_some(latest);
        if let Some(version) = &newer {
            self.apply(
                engine,
                &Event::UpdateAvailable {
                    version: version.clone(),
                },
            );
        }
        Ok(newer)
    }

    async fn change(&self, engine: EngineId, target: Target) -> Result<String> {
        let layout = &self.inner.layout;
        let _lock = match install::lock(layout, engine).await {
            Ok(lock) => lock,
            Err(error) => return self.finish(engine, Err(error)).await,
        };
        if matches!(target, Target::Present)
            && let Some(current) = self.binary(engine).and_then(|_| layout.current(engine))
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
                .current(engine)
                .filter(|version| layout.executable(engine, version).is_some());
            let version = match (target, &current) {
                (Target::Present, Some(current)) => return Ok(current.clone()),
                (Target::Exact(version), _) => version,
                (Target::Previous, _) => layout
                    .previous(engine)
                    .filter(|version| layout.executable(engine, version).is_some())
                    .with_context(|| {
                        format!(
                            "{} has no previous version to roll back to",
                            engine.display_name()
                        )
                    })?,
                (Target::Present | Target::Latest, _) => {
                    install::latest_version(&self.inner.http, artifact(engine)?.feed).await?
                }
            };
            Self::validate_version(engine, &version)?;
            if layout.executable(engine, &version).is_some() {
                let switching = self.clone();
                let switching_version = version.clone();
                tokio::task::spawn_blocking(move || {
                    install::switch(&switching.inner.layout, engine, &switching_version)
                })
                .await
                .context("engine version switch task")??;
                return Ok(version);
            }
            let resolved =
                install::resolve(&self.inner.http, artifact(engine)?.feed, &version).await?;
            self.fetch(engine, artifact(engine)?, &resolved).await?;
            Ok(version)
        }
        .await;
        self.finish(engine, result).await
    }

    async fn fetch(&self, engine: EngineId, artifact: Artifact, resolved: &Resolved) -> Result<()> {
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

    async fn finish(&self, engine: EngineId, result: Result<String>) -> Result<String> {
        match &result {
            Ok(version) => {
                let engines = self.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || {
                    install::prune(&engines.inner.layout, engine);
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
                tracing::warn!(%engine, error = format!("{error:#}"), "engine install failed");
                self.apply(
                    engine,
                    &Event::Fail {
                        message: format!("{error:#}"),
                    },
                );
            }
        }
        result
    }

    /// Installs an already resolved artifact (tests use local fixtures).
    #[cfg(test)]
    pub(crate) async fn install_resolved(
        &self,
        engine: EngineId,
        artifact: Artifact,
        resolved: Resolved,
    ) -> Result<String> {
        let _lock = install::lock(&self.inner.layout, engine).await?;
        let result = self
            .fetch(engine, artifact, &resolved)
            .await
            .map(|()| resolved.version.clone());
        self.finish(engine, result).await
    }

    /// Brings engines installed by the old Forge (`<old data>/toolchain`)
    /// into this layout. Returns the engines it imported.
    #[cfg(feature = "legacy-import")]
    pub async fn import_legacy(&self, old_toolchain: &Path) -> Result<Vec<EngineId>> {
        let imported = install::import_legacy(&self.inner.layout, old_toolchain).await?;
        for engine in &imported {
            self.apply(
                *engine,
                &Event::Loaded {
                    version: self.inner.layout.current(*engine),
                },
            );
        }
        Ok(imported)
    }

    /// Asks the engine for its models now. On failure the previous listing
    /// stays.
    pub async fn refresh_models(&self, engine: EngineId) -> Result<Arc<[Model]>> {
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
                        |error| Err(anyhow::anyhow!(error.clone())),
                    )
                })
            })
            .flatten()
        {
            return cached;
        }
        let account = slot.sign_in.context_generation();
        let generation = lock(&slot.models).generation;
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
                        cache.listing = Some(listing);
                        Some(Ok(models))
                    }
                    Err(error) => {
                        cache.error = Some(format!("{error:#}"));
                        Some(Err(error.context(format!(
                            "listing {} models",
                            engine.display_name()
                        ))))
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
    }

    /// A new binary may list different models.
    fn models_changed(&self, engine: EngineId) {
        self.invalidate_models(engine);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let engines = self.clone();
        runtime.spawn(async move {
            if let Err(error) = engines.refresh_models(engine).await {
                tracing::debug!(%engine, error = format!("{error:#}"), "model refresh after install failed");
            }
        });
    }
}

/// What the engine's downloads are checked against: whatever digest its
/// feed publishes.
fn integrity(engine: EngineId) -> Option<Integrity> {
    Some(match host_spec(engine).artifact?.feed {
        Feed::Claude { .. } => Integrity::VendorSha256,
        Feed::Npm { .. } => Integrity::NpmSha512,
        Feed::Release { .. } => Integrity::ArtisanSha256,
    })
}

fn artifact(engine: EngineId) -> Result<Artifact> {
    host_spec(engine).artifact.with_context(|| {
        format!(
            "{} publishes no build for this platform",
            engine.display_name()
        )
    })
}

#[cfg(test)]
#[cfg(unix)]
mod context_tests {
    use super::*;
    use artisan_api::SignInState;
    use std::os::unix::fs::PermissionsExt as _;

    /// A Codex stand-in: a shell script answering the CLI calls Forge makes.
    /// `app_server <model>` plays `codex app-server`: it answers
    /// `initialize`, takes `initialized`, and lists `<model>` as the default
    /// once `model/list` arrives.
    fn codex_manager(script: &str) -> (tempfile::TempDir, Engines, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let layout = Layout::new(directory.path());
        let engine = EngineId::Codex;
        let home = layout.home(engine);
        std::fs::create_dir_all(&home).unwrap();
        let version = layout.version_dir(engine, "1.0.0");
        let binary = version.join(host_spec(engine).artifact.unwrap().entry);
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        let script = script.replacen(
            '\n',
            "\napp_server() {\n read -r _; echo '{\"id\":1,\"result\":{}}'\n read -r _\n read -r _; printf '{\"id\":2,\"result\":{\"data\":[{\"id\":\"%s\",\"isDefault\":true}]}}\\n' \"$1\"\n}\n",
            1,
        );
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        install::switch(&layout, engine, "1.0.0").unwrap();
        let engines = Engines::new(directory.path());
        (directory, engines, home)
    }

    async fn wait_until(mut ready: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !ready() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn login_links_are_still_parsed_after_the_retained_log_budget() {
        let (_directory, engines, _home) = codex_manager(
            r#"#!/bin/sh
case "$*" in
 "login --device-auth") yes 'noise' | head -n 15000; echo 'https://login.example.test/login'; sleep 30 ;;
 "login status") echo 'Logged in' ;;
 "app-server --stdio") app_server model1 ;;
esac
"#,
        );
        engines.login(EngineId::Codex).unwrap();
        wait_until(|| matches!(engines.status(EngineId::Codex).sign_in, SignInState::LoggingIn { url: Some(url), .. } if url == "https://login.example.test/login")).await;
        engines.cancel_login(EngineId::Codex);
        wait_until(|| {
            matches!(
                engines.status(EngineId::Codex).sign_in,
                SignInState::SignedIn { .. }
            )
        })
        .await;
    }

    #[tokio::test]
    async fn overlapping_model_refreshes_share_one_native_probe() {
        let (_directory, engines, home) = codex_manager(
            r#"#!/bin/sh
printf 'probe\n' >> "$PWD/calls"
touch "$PWD/started"
while [ ! -e "$PWD/release" ]; do sleep 0.01; done
app_server model1
"#,
        );
        let owner = engines.clone();
        let first =
            tokio::spawn(async move { owner.refresh_models(EngineId::Codex).await.unwrap() });
        wait_until(|| home.join("started").exists()).await;
        let owner = engines.clone();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(async move {
            started.send(()).ok();
            owner.refresh_models(EngineId::Codex).await.unwrap()
        });
        waiting.await.unwrap();
        std::fs::write(home.join("release"), "").unwrap();
        assert_eq!(first.await.unwrap(), second.await.unwrap());
        assert_eq!(
            std::fs::read_to_string(home.join("calls"))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn old_models_cannot_return_after_an_account_aba() {
        let (_directory, engines, home) = codex_manager(
            r#"#!/bin/sh
case "$*" in
 "login status") if [ -e "$PWD/out" ]; then echo 'Not logged in'; else echo 'Logged in'; fi ;;
 logout) touch "$PWD/out" ;;
 "login --device-auth") rm -f "$PWD/out" ;;
 "app-server --stdio") model=$(cat "$PWD/model"); touch "$PWD/started"; while [ ! -e "$PWD/release" ]; do sleep 0.01; done; app_server "$model" ;;
esac
"#,
        );
        std::fs::write(home.join("model"), "old-model1").unwrap();
        assert_eq!(
            engines.check_sign_in(EngineId::Codex).await,
            SignInState::SignedIn { account: None }
        );
        let owner = engines.clone();
        let old = tokio::spawn(async move { owner.refresh_models(EngineId::Codex).await.unwrap() });
        wait_until(|| home.join("started").exists()).await;
        engines.logout(EngineId::Codex).unwrap();
        wait_until(|| engines.status(EngineId::Codex).sign_in == SignInState::SignedOut).await;
        std::fs::write(home.join("model"), "new-model2").unwrap();
        engines.login(EngineId::Codex).unwrap();
        wait_until(|| {
            engines.status(EngineId::Codex).sign_in == SignInState::SignedIn { account: None }
        })
        .await;
        std::fs::write(home.join("release"), "").unwrap();
        assert!(
            old.await.unwrap().is_empty(),
            "old context must not commit even though public account state is identical"
        );
        wait_until(|| {
            engines
                .status(EngineId::Codex)
                .models
                .iter()
                .any(|model| model.id == "new-model2")
        })
        .await;
        assert!(
            !engines
                .status(EngineId::Codex)
                .models
                .iter()
                .any(|model| model.id == "old-model1")
        );
    }

    #[tokio::test]
    async fn login_pins_the_selected_version_until_cancellation_and_probe_finish() {
        let (directory, engines, home) = codex_manager(
            r#"#!/bin/sh
case "$*" in
 "login --device-auth") touch "$PWD/started"; while :; do sleep 1; done ;;
 "login status") echo 'Logged in' ;;
 "app-server --stdio") app_server model1 ;;
esac
"#,
        );
        let old_binary = engines.binary(EngineId::Codex).unwrap();
        engines.login(EngineId::Codex).unwrap();
        wait_until(|| home.join("started").exists()).await;
        let layout = Layout::new(directory.path());
        for version in ["2.0.0", "3.0.0"] {
            let next = layout
                .version_dir(EngineId::Codex, version)
                .join(host_spec(EngineId::Codex).artifact.unwrap().entry);
            std::fs::create_dir_all(next.parent().unwrap()).unwrap();
            std::fs::copy(&old_binary, next).unwrap();
            install::switch(&layout, EngineId::Codex, version).unwrap();
        }
        install::prune(&layout, EngineId::Codex);
        assert!(old_binary.is_file());
        engines.cancel_login(EngineId::Codex);
        wait_until(|| {
            install::prune(&layout, EngineId::Codex);
            !old_binary.exists()
        })
        .await;
    }

    #[tokio::test]
    async fn present_install_reconciles_a_failed_update_without_another_download() {
        let (_directory, engines, _home) = codex_manager("#!/bin/sh\necho 'Logged in'\n");
        engines.apply(
            EngineId::Codex,
            &Event::Start {
                version: "2.0.0".into(),
            },
        );
        engines.apply(
            EngineId::Codex,
            &Event::Fail {
                message: "download failed".into(),
            },
        );
        assert!(matches!(
            engines.status(EngineId::Codex).install,
            artisan_api::InstallState::Failed { .. }
        ));
        assert_eq!(engines.install(EngineId::Codex).await.unwrap(), "1.0.0");
        assert!(matches!(
            engines.status(EngineId::Codex).install,
            artisan_api::InstallState::Installed { .. }
        ));
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    #[tokio::test]
    async fn download_progress_shares_catalog_allocations_and_preserves_prior_snapshots() {
        let folder = tempfile::tempdir().unwrap();
        let engines = Engines::new(folder.path());
        let engine = EngineId::Codex;
        lock(&engines.slot(engine).models).listing = Some(CachedListing::from(catalog::Listing {
            models: vec![artisan_api::Model {
                id: "fixture-model1".into(),
                ..artisan_api::Model::default()
            }],
            default: Some("fixture-model1".into()),
            ..catalog::Listing::default()
        }));
        *lock(&engines.slot(engine).versions) = vec![PublishedVersion {
            version: "1.0.0".into(),
            installed: true,
            current: true,
            unsupported: false,
        }]
        .into();
        let before = engines.status(engine);
        let mut events = engines.subscribe();
        engines.apply(
            engine,
            &Event::Start {
                version: "2.0.0".into(),
            },
        );
        engines.apply(
            engine,
            &Event::Progress {
                received: 100,
                total: Some(1000),
            },
        );
        let EngineChange::Full(started) = events.recv().await.unwrap() else {
            panic!("start must publish metadata");
        };
        let EngineChange::Install {
            engine: progress_engine,
            install: progress_install,
        } = events.recv().await.unwrap()
        else {
            panic!("progress must publish only installation");
        };
        assert_eq!(progress_engine, engine);
        let progress = engines.status(engine);
        assert!(Arc::ptr_eq(&before.models, &started.models));
        assert!(Arc::ptr_eq(&before.models, &progress.models));
        assert!(Arc::ptr_eq(&before.versions, &progress.versions));
        assert!(matches!(
            progress_install,
            artisan_api::InstallState::Downloading { received: 100, .. }
        ));
        // Explicit install reconciliation may replace metadata, while already
        // published snapshots retain their original coherent flags.
        engines.reconcile_install(engine);
        assert!(before.versions[0].installed);
        assert!(!engines.status(engine).versions[0].installed);
    }
}

#[cfg(test)]
mod lease_release_tests {
    use super::*;

    #[test]
    fn dropping_binary_lease_releases_lock_with_an_inherited_descriptor_open() {
        let directory = tempfile::tempdir().unwrap();
        let engine = EngineId::Codex;
        let layout = Layout::new(directory.path());
        let binary = layout
            .version_dir(engine, "0.150.0")
            .join(host_spec(engine).artifact.unwrap().entry);
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, "lease fixture").unwrap();
        install::switch(&layout, engine, "0.150.0").unwrap();
        let engines = Engines::new(directory.path());
        let lease = engines.lease(engine).unwrap();
        // A duplicated handle models the inherited open file description
        // before a concurrently spawned child reaches exec.
        let inherited = lease.usage.try_clone().unwrap();
        let competing = install::usage_lock(&layout.home(engine)).unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&competing).is_err());
        drop(lease);
        fs2::FileExt::try_lock_exclusive(&competing)
            .expect("the ended owner releases its lease even before the inherited handle closes");
        drop(inherited);
    }
}
