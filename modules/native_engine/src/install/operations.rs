//! Managed engine version operations: follow `latest`, hold or select a
//! version, roll back, activate pending generations, and list versions.
//!
//! A generation switch requires both the exclusive install lock and proof
//! that the engine is idle ([`EngineIdle`]). When a managed process of the
//! engine is running, the newly installed generation is recorded as pending
//! and activated by [`EngineOperations::activate_pending`] once idle; running
//! processes never see their generation replaced.

use std::{fmt, path::Path};

use super::{
    archive::ArchiveError,
    authority::ManagedEngineAuthority,
    feed::{
        FeedError, ReleaseArtifact, direct_release, latest_request, parse_latest, parse_release,
        parse_versions, release_request, versions_request,
    },
    pipeline::{install_artifact, prune_generations},
    selection::EngineSelection,
    spec::{EngineIdle, ManagedInstallLock, ManagedInstallLockError, ManagedInstallPaths},
    state::{ManagedGeneration, ManagedToolchainState},
    transport::{ReleaseTransport, TransportError},
    trust::read_trust_records,
    version::EngineVersion,
};

/// Progress of one install, reported to status observers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallProgress {
    Resolving,
    Downloading {
        received_bytes: u64,
        total_bytes: Option<u64>,
    },
    Extracting,
    Verifying,
    Activating,
}

/// Bounded, path- and URL-free failures of managed engine operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallError {
    UnsupportedPlatform,
    BelowFloor,
    InvalidRoot,
    RootUnavailable,
    Lock(ManagedInstallLockError),
    StateInvalid,
    Feed(FeedError),
    Transport(TransportError),
    IntegrityMismatch,
    TrustMismatch,
    Archive(ArchiveError),
    ExecutableInvalid,
    GenerationUnavailable,
    GenerationCollision,
    StatePublicationFailed,
    RandomUnavailable,
    CleanupFailed,
    NoPreviousGeneration,
}

impl InstallError {
    /// Returns the stable classification.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::BelowFloor => "below_floor",
            Self::InvalidRoot => "invalid_root",
            Self::RootUnavailable => "root_unavailable",
            Self::Lock(ManagedInstallLockError::Timeout) => "lock_timeout",
            Self::Lock(ManagedInstallLockError::IdentityChanged) => "lock_identity_changed",
            Self::Lock(_) => "lock_unavailable",
            Self::StateInvalid => "state_invalid",
            Self::Feed(error) => error.code(),
            Self::Transport(error) => error.code(),
            Self::IntegrityMismatch => "integrity_mismatch",
            Self::TrustMismatch => "trust_mismatch",
            Self::Archive(error) => error.code(),
            Self::ExecutableInvalid => "executable_invalid",
            Self::GenerationUnavailable => "generation_unavailable",
            Self::GenerationCollision => "generation_collision",
            Self::StatePublicationFailed => "state_publication_failed",
            Self::RandomUnavailable => "random_unavailable",
            Self::CleanupFailed => "cleanup_failed",
            Self::NoPreviousGeneration => "no_previous_generation",
        }
    }
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "managed engine operation failed: {}",
            self.code()
        )
    }
}

impl std::error::Error for InstallError {}

/// What a switch did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SwitchOutcome {
    /// The target version was already active.
    AlreadyActive(EngineVersion),
    /// The target version is now active.
    Activated(EngineVersion),
    /// The target version is installed and activates once the engine is idle.
    Pending(EngineVersion),
}

/// One listed vendor version with its local status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionListing {
    pub version: EngineVersion,
    pub installed: bool,
    pub active: bool,
    pub below_floor: bool,
}

/// Operations over one engine's managed installation.
pub struct EngineOperations<'a> {
    authority: ManagedEngineAuthority,
    database_path: &'a Path,
    transport: &'a dyn ReleaseTransport,
}

impl fmt::Debug for EngineOperations<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EngineOperations")
            .field("engine", &self.authority.engine())
            .finish_non_exhaustive()
    }
}

impl<'a> EngineOperations<'a> {
    /// Binds the operations to one engine, database, and transport.
    pub const fn new(
        authority: ManagedEngineAuthority,
        database_path: &'a Path,
        transport: &'a dyn ReleaseTransport,
    ) -> Self {
        Self {
            authority,
            database_path,
            transport,
        }
    }

    /// Resolves the vendor's current version.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError`] when the feed is unreachable or invalid.
    pub fn latest_version(&self) -> Result<EngineVersion, InstallError> {
        let plan = self.plan()?;
        let bytes = self
            .transport
            .fetch(&latest_request(plan.feed))
            .map_err(InstallError::Transport)?;
        parse_latest(plan.feed, &bytes).map_err(InstallError::Feed)
    }

    /// Lists the vendor's versions, newest first, with local status. A
    /// vendor without a version listing yields its current release plus every
    /// version this Forge downloaded before.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError`] when the listing is unreachable or invalid.
    pub fn list_versions(&self) -> Result<Vec<VersionListing>, InstallError> {
        let plan = self.plan()?;
        let state = self.read_state()?;
        let versions = match versions_request(plan.feed) {
            Some(request) => {
                let bytes = self
                    .transport
                    .fetch(&request)
                    .map_err(InstallError::Transport)?;
                parse_versions(plan.feed, &bytes).map_err(InstallError::Feed)?
            }
            None => self.seen_versions(state.as_ref())?,
        };
        Ok(versions
            .into_iter()
            .map(|version| VersionListing {
                installed: state
                    .as_ref()
                    .is_some_and(|state| state.generation_for(version.as_str()).is_some()),
                active: state
                    .as_ref()
                    .is_some_and(|state| state.active.version == version.as_str()),
                below_floor: !self.authority.engine().meets_floor(&version),
                version,
            })
            .collect())
    }

    /// Makes the persisted selection active: installs `latest` or the held
    /// version when it is not retained, then switches (or queues the switch
    /// until the engine is idle).
    ///
    /// # Errors
    ///
    /// Returns [`InstallError`] when the target cannot be resolved,
    /// installed, or activated.
    pub fn ensure_selected(
        &self,
        progress: &dyn Fn(InstallProgress),
    ) -> Result<SwitchOutcome, InstallError> {
        let paths = self.paths()?;
        let selection = self
            .authority
            .read_selection(paths.engine_root())
            .map_err(|_| InstallError::StateInvalid)?;
        progress(InstallProgress::Resolving);
        let target = match selection {
            EngineSelection::Latest => self.latest_version()?,
            EngineSelection::Held(version) => version,
        };
        self.switch_to(&paths, &target, progress)
    }

    /// Persists `selection` and makes it active.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::BelowFloor`] for a held version below the
    /// engine floor, or any failure of [`Self::ensure_selected`].
    pub fn select(
        &self,
        selection: &EngineSelection,
        progress: &dyn Fn(InstallProgress),
    ) -> Result<SwitchOutcome, InstallError> {
        if let EngineSelection::Held(version) = selection
            && !self.authority.engine().meets_floor(version)
        {
            return Err(InstallError::BelowFloor);
        }
        let paths = self.paths()?;
        paths.prepare().map_err(|_| InstallError::RootUnavailable)?;
        self.persist_selection(&paths, selection)?;
        self.ensure_selected(progress)
    }

    /// Switches back to the most recently active previous generation and
    /// holds the engine at that version, so automatic updates do not
    /// immediately undo the rollback.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::NoPreviousGeneration`] when nothing can be
    /// rolled back to.
    pub fn rollback(&self) -> Result<SwitchOutcome, InstallError> {
        let paths = self.paths()?;
        let lock = Self::lock(&paths)?;
        let state = self
            .read_state()?
            .ok_or(InstallError::NoPreviousGeneration)?;
        let previous = state
            .previous
            .iter()
            .find(|generation| {
                generation
                    .parsed_version()
                    .is_some_and(|version| self.authority.engine().meets_floor(&version))
            })
            .cloned()
            .ok_or(InstallError::NoPreviousGeneration)?;
        let version = previous
            .parsed_version()
            .ok_or(InstallError::StateInvalid)?;
        self.persist_selection(&paths, &EngineSelection::Held(version))?;
        self.activate(&paths, &lock, Some(&state), previous)
    }

    /// Activates a pending generation when the engine is idle.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError`] when state cannot be read or published.
    pub fn activate_pending(&self) -> Result<Option<SwitchOutcome>, InstallError> {
        let paths = self.paths()?;
        let Some(state) = self.read_state()? else {
            return Ok(None);
        };
        let Some(pending) = state.pending.clone() else {
            return Ok(None);
        };
        let lock = Self::lock(&paths)?;
        let state = self.read_state()?.ok_or(InstallError::StateInvalid)?;
        if state.pending.as_ref() != Some(&pending) {
            return Ok(None);
        }
        self.activate(&paths, &lock, Some(&state), pending)
            .map(Some)
    }

    fn switch_to(
        &self,
        paths: &ManagedInstallPaths,
        target: &EngineVersion,
        progress: &dyn Fn(InstallProgress),
    ) -> Result<SwitchOutcome, InstallError> {
        if !self.authority.engine().meets_floor(target) {
            return Err(InstallError::BelowFloor);
        }
        paths.prepare().map_err(|_| InstallError::RootUnavailable)?;
        let lock = Self::lock(paths)?;
        let state = self.read_state()?;
        if let Some(state) = &state {
            if state.active.version == target.as_str() {
                return Ok(SwitchOutcome::AlreadyActive(target.clone()));
            }
            if let Some(retained) = state.generation_for(target.as_str()).cloned()
                && self.verify(paths, &retained)
            {
                return self.activate(paths, &lock, Some(state), retained);
            }
        }
        let artifact = self.release(target)?;
        let generation = install_artifact(
            self.authority,
            paths,
            &lock,
            self.transport,
            &artifact,
            progress,
        )?;
        progress(InstallProgress::Activating);
        self.activate(paths, &lock, state.as_ref(), generation)
    }

    fn activate(
        &self,
        paths: &ManagedInstallPaths,
        lock: &ManagedInstallLock,
        current: Option<&ManagedToolchainState>,
        generation: ManagedGeneration,
    ) -> Result<SwitchOutcome, InstallError> {
        let version = generation
            .parsed_version()
            .ok_or(InstallError::StateInvalid)?;
        let idle = match EngineIdle::try_acquire(paths) {
            Ok(idle) => Some(idle),
            Err(ManagedInstallLockError::Busy) => None,
            Err(error) => return Err(InstallError::Lock(error)),
        };
        let (next, outcome) = match (current, &idle) {
            (None, _) => (
                ManagedToolchainState::new(generation),
                SwitchOutcome::Activated(version),
            ),
            (Some(state), Some(_)) => (
                state.activated(generation),
                SwitchOutcome::Activated(version),
            ),
            (Some(state), None) => (
                state.with_pending(generation),
                SwitchOutcome::Pending(version),
            ),
        };
        lock.fence(paths).map_err(InstallError::Lock)?;
        self.persist_state(paths, &next)?;
        prune_generations(paths, lock, next.directories())?;
        drop(idle);
        Ok(outcome)
    }

    /// Publishes the selection and reads it back, so an unverified atomic
    /// replace is never mistaken for a persisted choice.
    fn persist_selection(
        &self,
        paths: &ManagedInstallPaths,
        selection: &EngineSelection,
    ) -> Result<(), InstallError> {
        let _outcome = self
            .authority
            .write_selection(paths.engine_root(), selection)
            .map_err(|_| InstallError::StatePublicationFailed)?;
        match self.authority.read_selection(paths.engine_root()) {
            Ok(stored) if stored == *selection => Ok(()),
            _ => Err(InstallError::StatePublicationFailed),
        }
    }

    /// Publishes the install state and reads it back.
    fn persist_state(
        &self,
        paths: &ManagedInstallPaths,
        state: &ManagedToolchainState,
    ) -> Result<(), InstallError> {
        let _outcome = self
            .authority
            .write_install_state(paths.engine_root(), state)
            .map_err(|_| InstallError::StatePublicationFailed)?;
        match self.authority.read_install_state(paths.engine_root()) {
            Ok(Some(stored)) if stored == *state => Ok(()),
            _ => Err(InstallError::StatePublicationFailed),
        }
    }

    fn verify(&self, paths: &ManagedInstallPaths, generation: &ManagedGeneration) -> bool {
        self.authority.plan().is_ok_and(|plan| {
            self.authority
                .verify_generation(paths, &plan, generation)
                .is_ok()
        })
    }

    /// The current release plus every version downloaded before, newest
    /// first, for a vendor without a version listing.
    fn seen_versions(
        &self,
        state: Option<&ManagedToolchainState>,
    ) -> Result<Vec<EngineVersion>, InstallError> {
        let paths = self.paths()?;
        let records = read_trust_records(paths.engine_root(), self.authority.engine())
            .map_err(|_| InstallError::StateInvalid)?;
        let mut versions: Vec<EngineVersion> = records
            .iter()
            .filter(|record| record.platform == self.authority.platform().label())
            .map(|record| record.version.as_str())
            .chain(
                state
                    .into_iter()
                    .flat_map(ManagedToolchainState::directories_versions),
            )
            .filter_map(EngineVersion::parse)
            .collect();
        if let Ok(latest) = self.latest_version() {
            versions.push(latest);
        }
        versions.sort_by(|left, right| right.cmp(left));
        versions.dedup();
        versions.truncate(super::feed::MAX_LISTED_VERSIONS);
        Ok(versions)
    }

    fn release(&self, version: &EngineVersion) -> Result<ReleaseArtifact, InstallError> {
        let plan = self.plan()?;
        let Some(request) = release_request(plan.feed, version) else {
            return direct_release(plan.feed, version)
                .ok_or(InstallError::Feed(FeedError::PlatformMissing));
        };
        let bytes = self
            .transport
            .fetch(&request)
            .map_err(InstallError::Transport)?;
        parse_release(plan.feed, version, &bytes).map_err(InstallError::Feed)
    }

    fn plan(&self) -> Result<super::catalog::ArtifactPlan, InstallError> {
        self.authority
            .plan()
            .map_err(|_| InstallError::UnsupportedPlatform)
    }

    fn paths(&self) -> Result<ManagedInstallPaths, InstallError> {
        self.plan()?;
        self.authority
            .install_paths(self.database_path)
            .map_err(|_| InstallError::InvalidRoot)
    }

    fn lock(paths: &ManagedInstallPaths) -> Result<ManagedInstallLock, InstallError> {
        paths.prepare().map_err(|_| InstallError::RootUnavailable)?;
        ManagedInstallLock::acquire(paths).map_err(InstallError::Lock)
    }

    fn read_state(&self) -> Result<Option<ManagedToolchainState>, InstallError> {
        let paths = self.paths()?;
        self.authority
            .read_install_state(paths.engine_root())
            .map_err(|_| InstallError::StateInvalid)
    }
}

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;
