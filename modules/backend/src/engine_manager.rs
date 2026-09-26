//! The Forge's engine manager: installs, updates, and switches the
//! Forge-managed engine binaries in the background and publishes one status
//! per engine.
//!
//! One dedicated thread owns every install operation (the release transport
//! is blocking and bounded). At start it installs each supported engine's
//! selected version; afterwards it checks the vendor for a newer `latest`
//! every [`UPDATE_INTERVAL`] (only for engines that follow `latest`) and
//! activates pending generations once their engine is idle. Editor requests
//! (select a version, roll back, list versions) are queued to the same
//! thread, so operations never race. Status changes are published through a
//! watch channel that the connection push path observes.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use artisan_domain::{
    EngineInstallPhase, EngineInstallSnapshot, EngineInstallStatus, EngineIntegrity,
    EngineVersionEntry, EngineVersionList,
};
use artisan_native_engine::{
    EngineInspection, EngineOperations, EngineSelection, EngineVersion, HttpsTransport,
    InstallError, InstallProgress, Integrity, ManagedEngine, ManagedEngineAuthority,
    ReleaseTransport, read_trust_records, record_for,
};
use tokio::sync::{oneshot, watch};

/// How often engines that follow `latest` are checked for a newer release.
pub(crate) const UPDATE_INTERVAL: Duration = Duration::from_hours(6);
/// How often pending generations are retried for activation.
const ACTIVATION_POLL: Duration = Duration::from_secs(60);

/// A request the Editor made through the Forge.
enum ManagerCommand {
    Select(ManagedEngine, EngineSelection),
    Rollback(ManagedEngine),
    ListVersions(
        ManagedEngine,
        oneshot::Sender<Result<EngineVersionList, EngineManagerError>>,
    ),
}

/// A refused engine request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EngineManagerError {
    UnknownEngine,
    InvalidSelection,
    Unavailable,
    Operation(InstallError),
}

impl EngineManagerError {
    /// Returns a presentation-ready, path-free reason.
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::UnknownEngine => "unknown engine",
            Self::InvalidSelection => {
                "the selected version is invalid or below the supported floor"
            }
            Self::Unavailable => "the engine manager is unavailable",
            Self::Operation(error) => error.code(),
        }
    }
}

/// What the manager is doing for one engine right now.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Activity {
    Idle,
    Installing(Option<u8>),
    Failed(String),
}

struct Shared {
    database: PathBuf,
    activity: Mutex<BTreeMap<ManagedEngine, Activity>>,
    latest: Mutex<BTreeMap<ManagedEngine, EngineVersion>>,
    snapshot: watch::Sender<EngineInstallSnapshot>,
    /// Wakes every connection's delivery when the snapshot changes.
    on_change: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn set_activity(&self, engine: ManagedEngine, activity: Activity) {
        if let Ok(mut map) = self.activity.lock() {
            map.insert(engine, activity);
        }
        self.publish();
    }

    fn set_latest(&self, engine: ManagedEngine, version: EngineVersion) {
        if let Ok(mut map) = self.latest.lock() {
            map.insert(engine, version);
        }
    }

    fn publish(&self) {
        let activity = self
            .activity
            .lock()
            .map(|map| map.clone())
            .unwrap_or_default();
        let latest = self
            .latest
            .lock()
            .map(|map| map.clone())
            .unwrap_or_default();
        let statuses = ManagedEngine::ALL
            .into_iter()
            .map(|engine| {
                observe(
                    engine,
                    &self.database,
                    activity.get(&engine).unwrap_or(&Activity::Idle),
                    latest.get(&engine),
                )
            })
            .collect();
        if let Ok(snapshot) = EngineInstallSnapshot::new(statuses)
            && self.snapshot.send_if_modified(|current| {
                let changed = *current != snapshot;
                *current = snapshot;
                changed
            })
        {
            (self.on_change)();
        }
    }
}

/// Handle to the running engine manager.
#[derive(Clone)]
pub(crate) struct EngineManager {
    shared: Arc<Shared>,
    commands: mpsc::Sender<ManagerCommand>,
}

impl std::fmt::Debug for EngineManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EngineManager")
            .finish_non_exhaustive()
    }
}

impl EngineManager {
    /// Starts the manager with the production HTTPS transport; `on_change`
    /// runs after every snapshot change.
    pub(crate) fn start(database: &Path, on_change: impl Fn() + Send + Sync + 'static) -> Self {
        Self::start_with(database, None, UPDATE_INTERVAL, on_change)
    }

    /// Starts the manager; `transport` replaces the HTTPS transport in tests.
    pub(crate) fn start_with(
        database: &Path,
        transport: Option<Arc<dyn ReleaseTransport>>,
        update_interval: Duration,
        on_change: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (snapshot, _) = watch::channel(EngineInstallSnapshot::default());
        let shared = Arc::new(Shared {
            database: database.to_path_buf(),
            activity: Mutex::new(BTreeMap::new()),
            latest: Mutex::new(BTreeMap::new()),
            snapshot,
            on_change: Box::new(on_change),
        });
        shared.publish();
        let (commands, receiver) = mpsc::channel();
        let worker = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("artisan-engine-manager".to_owned())
            .spawn(move || {
                let transport: Arc<dyn ReleaseTransport> = match transport {
                    Some(transport) => transport,
                    None => match HttpsTransport::new() {
                        Ok(transport) => Arc::new(transport),
                        Err(error) => {
                            for engine in supported_engines() {
                                worker.set_activity(engine, Activity::Failed(error.code().into()));
                            }
                            return;
                        }
                    },
                };
                run(&worker, transport.as_ref(), &receiver, update_interval);
            });
        if spawned.is_err() {
            for engine in supported_engines() {
                shared.set_activity(
                    engine,
                    Activity::Failed("engine manager unavailable".into()),
                );
            }
        }
        Self { shared, commands }
    }

    /// Returns the current snapshot.
    pub(crate) fn snapshot(&self) -> EngineInstallSnapshot {
        self.shared.snapshot.borrow().clone()
    }

    /// Queues a version selection; progress arrives through the snapshot.
    pub(crate) fn select(
        &self,
        engine_id: &str,
        selection: &artisan_domain::EngineVersionSelection,
    ) -> Result<EngineInstallSnapshot, EngineManagerError> {
        let engine = ManagedEngine::from_id(engine_id).ok_or(EngineManagerError::UnknownEngine)?;
        let selection = match selection {
            artisan_domain::EngineVersionSelection::Latest => EngineSelection::Latest,
            artisan_domain::EngineVersionSelection::Version(version) => {
                EngineVersion::parse(version)
                    .filter(|version| engine.meets_floor(version))
                    .map(EngineSelection::Held)
                    .ok_or(EngineManagerError::InvalidSelection)?
            }
        };
        self.shared.set_activity(engine, Activity::Installing(None));
        self.commands
            .send(ManagerCommand::Select(engine, selection))
            .map_err(|_| EngineManagerError::Unavailable)?;
        Ok(self.snapshot())
    }

    /// Queues a rollback to the previous generation.
    pub(crate) fn rollback(
        &self,
        engine_id: &str,
    ) -> Result<EngineInstallSnapshot, EngineManagerError> {
        let engine = ManagedEngine::from_id(engine_id).ok_or(EngineManagerError::UnknownEngine)?;
        self.commands
            .send(ManagerCommand::Rollback(engine))
            .map_err(|_| EngineManagerError::Unavailable)?;
        Ok(self.snapshot())
    }

    /// Lists the vendor's versions of one engine.
    pub(crate) async fn list_versions(
        &self,
        engine_id: &str,
    ) -> Result<EngineVersionList, EngineManagerError> {
        let engine = ManagedEngine::from_id(engine_id).ok_or(EngineManagerError::UnknownEngine)?;
        let (sender, receiver) = oneshot::channel();
        self.commands
            .send(ManagerCommand::ListVersions(engine, sender))
            .map_err(|_| EngineManagerError::Unavailable)?;
        receiver
            .await
            .map_err(|_| EngineManagerError::Unavailable)?
    }
}

fn supported_engines() -> impl Iterator<Item = ManagedEngine> {
    ManagedEngine::ALL
        .into_iter()
        .filter(|engine| ManagedEngineAuthority::new(*engine).plan().is_ok())
}

fn run(
    shared: &Shared,
    transport: &dyn ReleaseTransport,
    commands: &mpsc::Receiver<ManagerCommand>,
    update_interval: Duration,
) {
    let mut next_update = Instant::now();
    loop {
        if Instant::now() >= next_update {
            for engine in supported_engines() {
                ensure(shared, transport, engine, true);
            }
            next_update = Instant::now() + update_interval;
        }
        match commands.recv_timeout(ACTIVATION_POLL.min(update_interval)) {
            Ok(ManagerCommand::Select(engine, selection)) => {
                let operations = operations(shared, transport, engine);
                let result = operations.select(&selection, &progress(shared, engine));
                finish(shared, engine, result.map(|_| ()));
            }
            Ok(ManagerCommand::Rollback(engine)) => {
                let result = operations(shared, transport, engine).rollback();
                finish(shared, engine, result.map(|_| ()));
            }
            Ok(ManagerCommand::ListVersions(engine, reply)) => {
                let _ = reply.send(list_versions(shared, transport, engine));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                for engine in supported_engines() {
                    if let Ok(Some(_)) = operations(shared, transport, engine).activate_pending() {
                        shared.publish();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn operations<'a>(
    shared: &'a Shared,
    transport: &'a dyn ReleaseTransport,
    engine: ManagedEngine,
) -> EngineOperations<'a> {
    EngineOperations::new(
        ManagedEngineAuthority::new(engine),
        &shared.database,
        transport,
    )
}

/// Installs the selected version (or `latest`) and records the latest
/// release. With `automatic`, an engine held at a version is left alone.
fn ensure(
    shared: &Shared,
    transport: &dyn ReleaseTransport,
    engine: ManagedEngine,
    automatic: bool,
) {
    let operations = operations(shared, transport, engine);
    if let Ok(latest) = operations.latest_version() {
        shared.set_latest(engine, latest);
    }
    let authority = ManagedEngineAuthority::new(engine);
    let held = authority
        .managed_engine_root(&shared.database)
        .ok()
        .and_then(|root| authority.read_selection(&root).ok())
        .is_some_and(|selection| !selection.follows_latest());
    let installed = matches!(
        authority.inspect(&shared.database),
        Ok(EngineInspection::Ready(_))
    );
    if automatic && held && installed {
        shared.publish();
        return;
    }
    let result = operations.ensure_selected(&progress(shared, engine));
    finish(shared, engine, result.map(|_| ()));
}

fn finish(shared: &Shared, engine: ManagedEngine, result: Result<(), InstallError>) {
    let activity = match result {
        // A lock failure means another process holds the install lock (for
        // example a live `OpenCode2` profile launch); the next pass retries.
        Ok(()) | Err(InstallError::Lock(_)) => Activity::Idle,
        Err(error) => Activity::Failed(failure_reason(engine, error)),
    };
    shared.set_activity(engine, activity);
}

fn progress(shared: &Shared, engine: ManagedEngine) -> impl Fn(InstallProgress) + '_ {
    move |progress| {
        let percent = match progress {
            InstallProgress::Downloading {
                received_bytes,
                total_bytes: Some(total),
            } if total > 0 => {
                u8::try_from((received_bytes.saturating_mul(100) / total).min(100)).ok()
            }
            _ => None,
        };
        shared.set_activity(engine, Activity::Installing(percent));
    }
}

fn list_versions(
    shared: &Shared,
    transport: &dyn ReleaseTransport,
    engine: ManagedEngine,
) -> Result<EngineVersionList, EngineManagerError> {
    let listing = operations(shared, transport, engine)
        .list_versions()
        .map_err(EngineManagerError::Operation)?;
    EngineVersionList::new(
        engine.id().to_owned(),
        listing
            .into_iter()
            .take(artisan_domain::engine_install::ENGINE_VERSION_LIST_MAX)
            .map(|entry| EngineVersionEntry {
                version: entry.version.to_string(),
                installed: entry.installed,
                active: entry.active,
                below_floor: entry.below_floor,
            })
            .collect(),
    )
    .map_err(|_| EngineManagerError::Unavailable)
}

fn failure_reason(engine: ManagedEngine, error: InstallError) -> String {
    let what = match error {
        InstallError::Transport(_) | InstallError::Feed(_) => {
            "could not reach the vendor's release feed"
        }
        InstallError::IntegrityMismatch => {
            "the download did not match the vendor's published checksum"
        }
        InstallError::BelowFloor => "the selected version is older than Artisan supports",
        InstallError::NoPreviousGeneration => "there is no previous version to roll back to",
        _ => "the install could not be completed",
    };
    format!(
        "{} update failed: {what} ({}).",
        engine.display_name(),
        error.code()
    )
}

/// Derives one engine's status from its install state on disk plus the
/// manager's in-flight activity.
fn observe(
    engine: ManagedEngine,
    database: &Path,
    activity: &Activity,
    latest: Option<&EngineVersion>,
) -> EngineInstallStatus {
    let authority = ManagedEngineAuthority::new(engine);
    let root = authority.managed_engine_root(database).ok();
    let state = root
        .as_ref()
        .and_then(|root| authority.read_install_state(root).ok().flatten());
    let held_version = root
        .as_ref()
        .and_then(|root| authority.read_selection(root).ok())
        .and_then(|selection| match selection {
            EngineSelection::Latest => None,
            EngineSelection::Held(version) => Some(version.to_string()),
        });
    let inspection = authority.inspect(database);
    let active_version = match &inspection {
        Ok(EngineInspection::Ready(generation)) => Some(generation.version().to_string()),
        _ => None,
    };
    let (integrity, trusted_since) =
        trust_of(authority, root.as_deref(), active_version.as_deref());
    let (phase, reason) = match (&inspection, activity) {
        (Ok(EngineInspection::UnsupportedPlatform(reason)), _) => (
            EngineInstallPhase::Unsupported,
            Some(format!(
                "{} is not managed here: {}.",
                engine.display_name(),
                reason.message()
            )),
        ),
        (_, Activity::Installing(_)) => (EngineInstallPhase::Installing, None),
        (_, Activity::Failed(reason)) => (EngineInstallPhase::Failed, Some(reason.clone())),
        (Ok(EngineInspection::Ready(_)), Activity::Idle) => (EngineInstallPhase::Ready, None),
        (Ok(EngineInspection::NotInstalled), Activity::Idle) => {
            (EngineInstallPhase::NotInstalled, None)
        }
        (Err(error), Activity::Idle) => (
            EngineInstallPhase::Failed,
            Some(format!(
                "{} install is invalid ({}); it will be reinstalled.",
                engine.display_name(),
                error.cli_reason()
            )),
        ),
    };
    EngineInstallStatus {
        engine_id: engine.id().to_owned(),
        phase,
        active_version,
        held_version,
        latest_version: latest.map(ToString::to_string),
        pending_version: state.as_ref().and_then(|state| {
            state
                .pending
                .as_ref()
                .map(|pending| pending.version.clone())
        }),
        rollback_version: state.as_ref().and_then(|state| {
            state
                .previous
                .first()
                .map(|previous| previous.version.clone())
        }),
        progress_percent: match activity {
            Activity::Installing(percent) => *percent,
            Activity::Idle | Activity::Failed(_) => None,
        },
        reason,
        overridden: std::env::var_os(engine.override_env()).is_some_and(|value| !value.is_empty()),
        integrity,
        trusted_since,
        vendor_version_list: authority
            .plan()
            .is_ok_and(|plan| artisan_native_engine::versions_listed(plan.feed)),
    }
}

/// The integrity mode of `authority`'s engine and, for trust on first
/// download, when the active version's hash was recorded.
fn trust_of(
    authority: ManagedEngineAuthority,
    engine_root: Option<&Path>,
    active_version: Option<&str>,
) -> (EngineIntegrity, Option<String>) {
    match authority.plan().map(|plan| plan.integrity) {
        Ok(Integrity::TrustOnFirstDownload) => {
            let since = engine_root.zip(active_version).and_then(|(root, version)| {
                let records = read_trust_records(root, authority.engine()).ok()?;
                let record = record_for(&records, version, authority.platform())?;
                Some(artisan_domain::iso_millis(
                    i64::try_from(record.first_seen_at_ms).unwrap_or(i64::MAX),
                ))
            });
            (EngineIntegrity::TrustOnFirstDownload, since)
        }
        Ok(Integrity::VendorDigest) | Err(_) => (EngineIntegrity::VendorChecksum, None),
    }
}

#[cfg(test)]
#[path = "engine_manager_tests.rs"]
mod tests;
