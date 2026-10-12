//! `Engines`: installs, versions and models for every engine, and the
//! `EngineStatus` stream the Editor shows.

use crate::catalog::{self, Listing, Program};
use crate::engines::{Artifact, Feed, compare_versions, host_spec, meets_minimum};
use crate::install::{self, Event, Layout, Machine, Resolved};
use anyhow::{Context, Result, bail};
use artisan_api::{EngineId, EngineStatus, Integrity, Model, PublishedVersion};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

mod accounts;

/// How long a model listing is served before asking the engine again.
const MODELS_TTL: Duration = Duration::from_mins(5);

#[derive(Clone)]
pub struct Engines {
    inner: Arc<Inner>,
}

struct Inner {
    layout: Layout,
    http: reqwest::Client,
    statuses: broadcast::Sender<EngineStatus>,
    slots: [Slot; EngineId::ALL.len()],
    usage: crate::UsageCache,
}

/// Everything Forge tracks about one engine. Add a field here and fill it
/// in `Engines::status` to extend `EngineStatus`.
struct Slot {
    install: Mutex<Machine>,
    /// Cache of the last listing; `None` until the first one.
    models: Mutex<Option<(Instant, Listing)>>,
    sign_in: Arc<crate::SignIn>,
    /// The vendor's versions, newest first, once asked for.
    versions: Mutex<Vec<String>>,
}

enum Target {
    /// Install the newest version unless one is installed already.
    Present,
    Latest,
    Exact(String),
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Engines {
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
                models: Mutex::new(None),
                sign_in: crate::SignIn::new(engine, layout.home(engine)),
                versions: Mutex::new(Vec::new()),
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
        let install = {
            let mut machine = lock(&slot.install);
            // `ae` may have installed or switched since we last looked.
            if install::settled(machine.state()) {
                let on_disk = self
                    .inner
                    .layout
                    .current(engine)
                    .filter(|_| self.binary(engine).is_some());
                let ours = match machine.state() {
                    install::State::Installed { version, .. } => Some(version.clone()),
                    install::State::Failed { serving, .. } => serving.clone(),
                    _ => None,
                };
                if on_disk != ours && (on_disk.is_some() || ours.is_some()) {
                    machine.handle(&Event::Loaded { version: on_disk });
                }
            }
            install::view(machine.state())
        };
        let listing = lock(&slot.models)
            .as_ref()
            .map(|(_, listing)| listing.clone())
            .unwrap_or_default();
        EngineStatus {
            engine,
            install,
            sign_in: slot.sign_in.state(),
            models: listing.models,
            default_model: listing.default,
            usage: self.inner.usage.cached(engine),
            integrity: integrity(engine),
            versions: self.published(engine),
        }
    }

    /// The listed versions with where each stands on disk now.
    fn published(&self, engine: EngineId) -> Vec<PublishedVersion> {
        let layout = &self.inner.layout;
        let current = layout.current(engine);
        lock(&self.slot(engine).versions)
            .iter()
            .map(|version| PublishedVersion {
                installed: layout.executable(engine, version).is_some(),
                current: current.as_deref() == Some(version.as_str()),
                unsupported: !meets_minimum(engine, version),
                version: version.clone(),
            })
            .collect()
    }

    /// Asks the vendor which versions it publishes ("Choose version").
    /// Vendors without a list offer their latest release and whatever this
    /// Forge has installed.
    pub async fn list_versions(&self, engine: EngineId) -> Result<Vec<PublishedVersion>> {
        let feed = artifact(engine)?.feed;
        let http = &self.inner.http;
        let listed = install::published_versions(http, feed)
            .await
            .with_context(|| format!("listing {} versions", engine.display_name()))?;
        let mut versions = if let Some(versions) = listed {
            versions
        } else {
            let mut versions = self.inner.layout.installed_versions(engine);
            match install::latest_version(http, feed).await {
                Ok(latest) => versions.push(latest),
                Err(error) => {
                    tracing::debug!(%engine, error = format!("{error:#}"), "no latest version for the list");
                }
            }
            versions
        };
        versions.sort_by(|left, right| compare_versions(right, left));
        versions.dedup();
        versions.truncate(install::MAX_LISTED_VERSIONS);
        *lock(&self.slot(engine).versions) = versions;
        self.publish(engine);
        Ok(self.published(engine))
    }

    /// Every change to any engine's status.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EngineStatus> {
        self.inner.statuses.subscribe()
    }

    fn publish(&self, engine: EngineId) {
        let _ = self.inner.statuses.send(self.status(engine));
    }

    fn apply(&self, engine: EngineId, event: &Event) {
        lock(&self.slot(engine).install).handle(event);
        self.publish(engine);
    }

    /// The engine's executable, if installed.
    #[must_use]
    pub fn binary(&self, engine: EngineId) -> Option<PathBuf> {
        self.inner.layout.binary(engine)
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
        let home = self.home(engine);
        let mut env = vec![("HOME".to_owned(), home.display().to_string())];
        if cfg!(windows) {
            env.push(("USERPROFILE".to_owned(), home.display().to_string()));
        }
        let spec = host_spec(engine);
        if let Some((dir, variable)) = spec.config {
            env.push((variable.to_owned(), home.join(dir).display().to_string()));
        }
        if engine == EngineId::Claude {
            env.push(("DISABLE_UPDATES".to_owned(), "1".to_owned()));
        }
        let version_dir = self
            .inner
            .layout
            .current(engine)
            .map(|v| self.inner.layout.version_dir(engine, &v));
        if let (Some(artifact), Some(version_dir)) = (spec.artifact, version_dir)
            && !artifact.path_dirs.is_empty()
        {
            let dirs = artifact.path_dirs.iter().map(|dir| version_dir.join(dir));
            let inherited = std::env::var_os("PATH").unwrap_or_default();
            if let Ok(path) = std::env::join_paths(dirs.chain(std::env::split_paths(&inherited))) {
                env.push(("PATH".to_owned(), path.to_string_lossy().into_owned()));
            }
        }
        env
    }

    /// Arguments that start the engine's own sign-in.
    #[must_use]
    pub fn login_args(engine: EngineId) -> &'static [&'static str] {
        host_spec(engine).login
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
        let layout = &self.inner.layout;
        let _lock = install::lock(layout, engine).await?;
        let previous = layout
            .previous(engine)
            .filter(|version| layout.executable(engine, version).is_some())
            .with_context(|| {
                format!(
                    "{} has no previous version to roll back to",
                    engine.display_name()
                )
            })?;
        install::switch(layout, engine, &previous)?;
        self.apply(
            engine,
            &Event::Done {
                version: previous.clone(),
            },
        );
        self.models_changed(engine);
        Ok(previous)
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
        let artifact = artifact(engine)?;
        let layout = &self.inner.layout;
        let _lock = install::lock(layout, engine).await?;
        let result = async {
            let current = layout
                .current(engine)
                .filter(|version| layout.executable(engine, version).is_some());
            let version = match (target, &current) {
                (Target::Present, Some(current)) => return Ok(current.clone()),
                (Target::Exact(version), _) => version,
                (Target::Present | Target::Latest, _) => {
                    install::latest_version(&self.inner.http, artifact.feed).await?
                }
            };
            if !meets_minimum(engine, &version) {
                bail!(
                    "{} {version} is older than the oldest supported version {}",
                    engine.display_name(),
                    host_spec(engine).minimum.unwrap_or_default()
                );
            }
            if layout.executable(engine, &version).is_some() {
                install::switch(layout, engine, &version)?;
                return Ok(version);
            }
            let resolved = install::resolve(&self.inner.http, artifact.feed, &version).await?;
            self.fetch(engine, artifact, &resolved).await?;
            Ok(version)
        }
        .await;
        self.finish(engine, result)
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

    fn finish(&self, engine: EngineId, result: Result<String>) -> Result<String> {
        match &result {
            Ok(version) => {
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
        self.finish(engine, result)
    }

    /// Brings engines installed by the old Forge (`<old data>/toolchain`)
    /// into this layout. Returns the engines it imported.
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

    /// The engine's models, from the cache while it's fresh.
    pub async fn models(&self, engine: EngineId) -> Result<Vec<Model>> {
        if let Some((at, listing)) = lock(&self.slot(engine).models).as_ref()
            && at.elapsed() < MODELS_TTL
        {
            return Ok(listing.models.clone());
        }
        self.refresh_models(engine).await
    }

    /// Asks the engine for its models now. On failure the previous listing
    /// stays.
    pub async fn refresh_models(&self, engine: EngineId) -> Result<Vec<Model>> {
        let (binary, home, env) = (
            self.binary(engine),
            self.home(engine),
            self.launch_env(engine),
        );
        let program = Program {
            binary: binary.as_deref(),
            env: &env,
            home: &home,
        };
        let listing = catalog::list(engine, program, &self.inner.http)
            .await
            .with_context(|| format!("listing {} models", engine.display_name()))?;
        let models = listing.models.clone();
        *lock(&self.slot(engine).models) = Some((Instant::now(), listing));
        self.publish(engine);
        Ok(models)
    }

    /// A new binary may list different models.
    fn models_changed(&self, engine: EngineId) {
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
        Feed::Grok { .. } | Feed::Cursor { .. } => Integrity::HttpsOnly,
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
