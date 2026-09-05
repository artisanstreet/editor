//! Application-side scope and retry state for the native model catalog.
//!
//! The service owns authenticated request execution.  This module owns the
//! smaller GPUI-facing part of that boundary: one generation-fenced catalog
//! scope, one durable-favorites read for that scope, and one stable favorite
//! mutation at a time.  It deliberately stores no catalog payload or provider
//! response; the model selector remains the owner of the bounded decoded
//! catalog.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{CatalogRevision, EngineProfileId, ModelFavoriteId, RequestId, ThreadId};

use crate::native_transport_service::{ServiceFailure, ServiceFailureCategory};

/// Monotonic application identity for one catalog/profile read scope.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CatalogLoadGeneration(u64);

impl CatalogLoadGeneration {
    /// Returns the first valid generation.
    #[must_use]
    pub const fn first() -> Self {
        Self(1)
    }

    /// Returns the next generation, or `None` after exhaustion.
    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    /// Returns the finite generation number for correlation tests.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[cfg(test)]
    const fn from_raw_for_test(value: u64) -> Self {
        Self(value)
    }
}

/// Exact application scope for one runtime catalog operation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NativeCatalogScope {
    /// Thread whose authoritative settings selected the profile.
    pub thread_id: ThreadId,
    /// Engine profile whose runtime owns the catalog.
    pub profile_id: EngineProfileId,
    /// Application generation fencing this scope.
    pub generation: CatalogLoadGeneration,
}

impl NativeCatalogScope {
    /// Creates one application-owned scope.
    #[must_use]
    pub const fn new(
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        generation: CatalogLoadGeneration,
    ) -> Self {
        Self {
            thread_id,
            profile_id,
            generation,
        }
    }
}

/// Catalog lifecycle visible to the selector adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCatalogPhase {
    /// No configured thread/profile scope is currently available.
    Offline,
    /// One bounded authenticated discovery is in flight.
    Loading,
    /// A validated runtime snapshot is current for the selected scope.
    Ready,
    /// Discovery failed; an explicit retry may be admitted.
    Failed,
}

/// Result of selecting a new thread/profile scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogScopeSelection {
    /// The existing scope is still current; no duplicate discovery is needed.
    Unchanged(NativeCatalogScope),
    /// A new generation must replace the old scope.
    Changed(NativeCatalogScope),
}

impl CatalogScopeSelection {
    /// Returns the selected scope.
    #[must_use]
    pub const fn scope(&self) -> &NativeCatalogScope {
        match self {
            Self::Unchanged(scope) | Self::Changed(scope) => scope,
        }
    }

    /// Returns whether selecting the scope invalidated prior operations.
    #[must_use]
    pub const fn changed(&self) -> bool {
        matches!(self, Self::Changed(_))
    }
}

/// Failure to mint a new application scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogScopeError {
    /// The monotonic generation cannot be reused after exhaustion.
    GenerationExhausted,
}

/// A favorite request retained across bounded bridge admission or transport
/// failure.  The request id and all input identities remain unchanged on
/// retry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingModelFavorite {
    /// Exact catalog scope that admitted the intent.
    pub scope: NativeCatalogScope,
    /// Stable request identity used by the durable command.
    pub request_id: RequestId,
    /// Stable catalog model identity.
    pub model_id: ModelFavoriteId,
    /// Exact catalog revision used for stale-catalog admission.
    pub catalog_revision: CatalogRevision,
    /// Desired final favorite state, never a toggle operation.
    pub favorite: bool,
    /// Whether the service command was admitted and is awaiting a receipt.
    pub admitted: bool,
}

/// Bounded application-side state for live catalog and favorite operations.
#[derive(Clone, Debug, Default)]
pub struct NativeCatalogController {
    current_scope: Option<NativeCatalogScope>,
    next_generation: Option<CatalogLoadGeneration>,
    catalog_in_flight: bool,
    catalog_phase: NativeCatalogPhase,
    catalog_failure: Option<ServiceFailure>,
    favorites_in_flight: bool,
    favorites_loaded: bool,
    favorites_failure: Option<ServiceFailure>,
    favorite_revision: Option<u64>,
    favorite_ids: Vec<String>,
    pending_favorite: Option<PendingModelFavorite>,
}

impl NativeCatalogController {
    /// Creates offline state with no thread/profile identity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            catalog_phase: NativeCatalogPhase::Offline,
            ..Self::default()
        }
    }

    /// Returns the current thread/profile generation, if any.
    #[must_use]
    pub const fn scope(&self) -> Option<&NativeCatalogScope> {
        self.current_scope.as_ref()
    }

    /// Returns the current catalog phase.
    #[must_use]
    pub const fn catalog_phase(&self) -> NativeCatalogPhase {
        self.catalog_phase
    }

    /// Returns the catalog failure, if discovery or bridge admission failed.
    #[must_use]
    pub const fn catalog_failure(&self) -> Option<ServiceFailure> {
        self.catalog_failure
    }

    /// Returns the favorite-read failure, if one is visible.
    #[must_use]
    pub const fn favorites_failure(&self) -> Option<ServiceFailure> {
        self.favorites_failure
    }

    /// Returns the authoritative favorite revision observed by this scope.
    #[must_use]
    pub const fn favorite_revision(&self) -> Option<u64> {
        self.favorite_revision
    }

    /// Returns the last accepted ordered favorite IDs.
    #[must_use]
    pub fn favorite_ids(&self) -> &[String] {
        &self.favorite_ids
    }

    /// Returns the retained favorite mutation, if any.
    #[must_use]
    pub const fn pending_favorite(&self) -> Option<&PendingModelFavorite> {
        self.pending_favorite.as_ref()
    }

    /// Returns whether a catalog request is currently in flight.
    #[must_use]
    pub const fn catalog_loading(&self) -> bool {
        self.catalog_in_flight
    }

    /// Returns whether the current catalog may be explicitly retried.
    #[must_use]
    pub const fn catalog_retry_available(&self) -> bool {
        matches!(self.catalog_phase, NativeCatalogPhase::Failed)
            && !self.catalog_in_flight
            && self.current_scope.is_some()
    }

    /// Returns whether the current favorite read may be explicitly retried.
    #[must_use]
    pub const fn favorites_retry_available(&self) -> bool {
        self.current_scope.is_some()
            && !self.favorites_in_flight
            && !self.favorites_loaded
            && self.pending_favorite.is_none()
            && self.favorites_failure.is_some()
    }

    /// Selects a thread/profile pair and advances its generation when the
    /// pair changes.  Every old operation becomes stale immediately.
    pub fn select_scope(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
    ) -> Result<CatalogScopeSelection, CatalogScopeError> {
        if let Some(current) = &self.current_scope
            && current.thread_id == thread_id
            && current.profile_id == profile_id
        {
            return Ok(CatalogScopeSelection::Unchanged(current.clone()));
        }

        let generation = match self.next_generation {
            None => CatalogLoadGeneration::first(),
            Some(previous) => previous
                .checked_next()
                .ok_or(CatalogScopeError::GenerationExhausted)?,
        };
        self.next_generation = Some(generation);
        let scope = NativeCatalogScope::new(thread_id, profile_id, generation);
        self.current_scope = Some(scope.clone());
        self.catalog_in_flight = false;
        self.catalog_phase = NativeCatalogPhase::Offline;
        self.catalog_failure = None;
        self.favorites_in_flight = false;
        self.favorites_loaded = false;
        self.favorites_failure = None;
        self.favorite_revision = None;
        self.favorite_ids.clear();
        self.pending_favorite = None;
        Ok(CatalogScopeSelection::Changed(scope))
    }

    /// Clears the active thread/profile scope while retaining the generation
    /// counter so a late response cannot become current after remounting.
    pub fn clear_scope(&mut self) {
        self.current_scope = None;
        self.catalog_in_flight = false;
        self.catalog_phase = NativeCatalogPhase::Offline;
        self.catalog_failure = None;
        self.favorites_in_flight = false;
        self.favorites_loaded = false;
        self.favorites_failure = None;
        self.favorite_revision = None;
        self.favorite_ids.clear();
        self.pending_favorite = None;
    }

    /// Returns whether one catalog read may be admitted for this scope.
    #[must_use]
    pub fn catalog_request_needed(&self, scope: &NativeCatalogScope) -> bool {
        self.current_scope.as_ref() == Some(scope)
            && !self.catalog_in_flight
            && matches!(
                self.catalog_phase,
                NativeCatalogPhase::Offline | NativeCatalogPhase::Failed
            )
    }

    /// Marks a catalog read admitted by the bounded service queue.
    #[must_use]
    pub fn mark_catalog_admitted(&mut self, scope: &NativeCatalogScope) -> bool {
        if !self.catalog_request_needed(scope) {
            return false;
        }
        self.catalog_in_flight = true;
        self.catalog_phase = NativeCatalogPhase::Loading;
        self.catalog_failure = None;
        true
    }

    /// Retains a retryable catalog plan after local command admission failed.
    pub fn on_catalog_admission_failed(
        &mut self,
        scope: &NativeCatalogScope,
        failure: ServiceFailure,
    ) -> bool {
        if self.current_scope.as_ref() != Some(scope) || self.catalog_in_flight {
            return false;
        }
        self.catalog_phase = NativeCatalogPhase::Failed;
        self.catalog_failure = Some(failure);
        true
    }

    /// Returns whether a catalog response belongs to the active request.
    #[must_use]
    pub fn catalog_response_current(&self, scope: &NativeCatalogScope) -> bool {
        self.current_scope.as_ref() == Some(scope) && self.catalog_in_flight
    }

    /// Accepts a validated catalog response for the exact active scope.
    #[must_use]
    pub fn on_catalog_loaded(&mut self, scope: &NativeCatalogScope) -> bool {
        if !self.catalog_response_current(scope) {
            return false;
        }
        self.catalog_in_flight = false;
        self.catalog_phase = NativeCatalogPhase::Ready;
        self.catalog_failure = None;
        true
    }

    /// Records a catalog response failure for the exact active scope.
    #[must_use]
    pub fn on_catalog_failed(
        &mut self,
        scope: &NativeCatalogScope,
        failure: ServiceFailure,
    ) -> bool {
        if !self.catalog_response_current(scope) {
            return false;
        }
        self.catalog_in_flight = false;
        self.catalog_phase = NativeCatalogPhase::Failed;
        self.catalog_failure = Some(failure);
        true
    }

    /// Returns the current failed scope for an explicit retry.
    #[must_use]
    pub fn retry_catalog(&self) -> Option<NativeCatalogScope> {
        self.catalog_retry_available()
            .then(|| self.current_scope.clone())
            .flatten()
    }

    /// Returns whether a durable favorite read may be admitted for this
    /// scope.  A successful read is cached until the scope changes.
    #[must_use]
    pub fn favorites_request_needed(&self, scope: &NativeCatalogScope) -> bool {
        self.current_scope.as_ref() == Some(scope)
            && !self.favorites_in_flight
            && !self.favorites_loaded
            && self.pending_favorite.is_none()
    }

    /// Marks the durable favorite read admitted by the service queue.
    #[must_use]
    pub fn mark_favorites_admitted(&mut self, scope: &NativeCatalogScope) -> bool {
        if !self.favorites_request_needed(scope) {
            return false;
        }
        self.favorites_in_flight = true;
        self.favorites_failure = None;
        true
    }

    /// Retains a favorite-read retry plan after local command admission failed.
    pub fn on_favorites_admission_failed(
        &mut self,
        scope: &NativeCatalogScope,
        failure: ServiceFailure,
    ) -> bool {
        if self.current_scope.as_ref() != Some(scope) || self.favorites_in_flight {
            return false;
        }
        self.favorites_failure = Some(failure);
        true
    }

    /// Returns whether a favorite read response belongs to the active scope.
    #[must_use]
    pub fn favorites_response_current(&self, scope: &NativeCatalogScope) -> bool {
        self.current_scope.as_ref() == Some(scope) && self.favorites_in_flight
    }

    /// Accepts an ordered durable snapshot.  Lower or conflicting equal
    /// revisions are stale and never replace the current authoritative list.
    #[must_use]
    pub fn on_favorites_loaded(
        &mut self,
        scope: &NativeCatalogScope,
        revision: u64,
        model_ids: Vec<String>,
    ) -> bool {
        if !self.favorites_response_current(scope) {
            return false;
        }
        self.favorites_in_flight = false;
        self.favorites_loaded = true;
        self.favorites_failure = None;
        if self
            .favorite_revision
            .is_some_and(|current| revision < current)
            || (self.favorite_revision == Some(revision) && self.favorite_ids != model_ids)
        {
            return false;
        }
        self.favorite_revision = Some(revision);
        self.favorite_ids = model_ids;
        true
    }

    /// Records a failed durable favorite read and leaves it retryable.
    #[must_use]
    pub fn on_favorites_failed(
        &mut self,
        scope: &NativeCatalogScope,
        failure: ServiceFailure,
    ) -> bool {
        if !self.favorites_response_current(scope) {
            return false;
        }
        self.favorites_in_flight = false;
        self.favorites_loaded = false;
        self.favorites_failure = Some(failure);
        true
    }

    /// Returns the current failed favorite-read scope for explicit retry.
    #[must_use]
    pub fn retry_favorites(&self) -> Option<NativeCatalogScope> {
        self.favorites_retry_available()
            .then(|| self.current_scope.clone())
            .flatten()
    }

    /// Starts one stable favorite intent after catalog readiness is known.
    pub fn begin_favorite(
        &mut self,
        scope: &NativeCatalogScope,
        request_id: RequestId,
        model_id: ModelFavoriteId,
        catalog_revision: CatalogRevision,
        favorite: bool,
    ) -> Result<PendingModelFavorite, FavoriteIntentError> {
        if self.current_scope.as_ref() != Some(scope)
            || self.catalog_phase != NativeCatalogPhase::Ready
        {
            return Err(FavoriteIntentError::CatalogUnavailable);
        }
        if self.pending_favorite.is_some() {
            return Err(FavoriteIntentError::AlreadyPending);
        }
        let pending = PendingModelFavorite {
            scope: scope.clone(),
            request_id,
            model_id,
            catalog_revision,
            favorite,
            admitted: false,
        };
        self.pending_favorite = Some(pending.clone());
        self.favorites_failure = None;
        Ok(pending)
    }

    /// Marks the exact favorite command admitted by the service queue.
    #[must_use]
    pub fn mark_favorite_admitted(
        &mut self,
        scope: &NativeCatalogScope,
        request_id: &RequestId,
    ) -> bool {
        let Some(pending) = self.pending_favorite.as_mut() else {
            return false;
        };
        if &pending.scope != scope || &pending.request_id != request_id || pending.admitted {
            return false;
        }
        pending.admitted = true;
        self.favorites_failure = None;
        true
    }

    /// Retains the exact favorite command after local admission failed.
    #[must_use]
    pub fn on_favorite_admission_failed(
        &mut self,
        scope: &NativeCatalogScope,
        request_id: &RequestId,
        failure: ServiceFailure,
    ) -> bool {
        let Some(pending) = self.pending_favorite.as_mut() else {
            return false;
        };
        if &pending.scope != scope || &pending.request_id != request_id {
            return false;
        }
        pending.admitted = false;
        self.favorites_failure = Some(failure);
        true
    }

    /// Returns the exact retained mutation when it is safe to retry.
    #[must_use]
    pub fn retry_favorite(&self) -> Option<PendingModelFavorite> {
        self.pending_favorite
            .as_ref()
            .filter(|pending| !pending.admitted && self.is_current(&pending.scope))
            .cloned()
    }

    /// Settles the exact favorite receipt and adopts its non-stale snapshot.
    #[must_use]
    pub fn on_favorite_succeeded(
        &mut self,
        scope: &NativeCatalogScope,
        request_id: &RequestId,
        model_id: &ModelFavoriteId,
        favorite: bool,
        revision: u64,
        model_ids: Vec<String>,
    ) -> bool {
        let Some(pending) = self.pending_favorite.as_ref() else {
            return false;
        };
        if !pending.admitted
            || &pending.scope != scope
            || &pending.request_id != request_id
            || &pending.model_id != model_id
            || pending.favorite != favorite
            || !self.is_current(scope)
        {
            return false;
        }
        self.pending_favorite = None;
        self.favorites_loaded = true;
        self.favorites_failure = None;
        if self
            .favorite_revision
            .is_none_or(|current| revision >= current)
        {
            self.favorite_revision = Some(revision);
            self.favorite_ids = model_ids;
        }
        true
    }

    /// Retains the exact mutation after a correlated transport failure so a
    /// later explicit retry can reuse its request id and payload.
    #[must_use]
    pub fn on_favorite_failed(
        &mut self,
        scope: &NativeCatalogScope,
        request_id: &RequestId,
        failure: ServiceFailure,
    ) -> bool {
        if !self.is_current(scope) {
            return false;
        }
        let Some(pending) = self.pending_favorite.as_mut() else {
            return false;
        };
        if &pending.scope != scope || &pending.request_id != request_id {
            return false;
        }
        pending.admitted = false;
        self.favorites_failure = Some(failure);
        true
    }

    /// Returns whether the supplied scope is still authoritative.
    #[must_use]
    pub fn is_current(&self, scope: &NativeCatalogScope) -> bool {
        self.current_scope.as_ref() == Some(scope)
    }

    /// Returns a redacted failure suitable for the existing selector status
    /// surface, without exposing provider/catalog values.
    #[must_use]
    pub fn selector_failure(&self) -> Option<ServiceFailure> {
        self.catalog_failure.or(self.favorites_failure)
    }

    /// Returns whether the current scope is configured and ready for policy
    /// interaction.
    #[must_use]
    pub const fn catalog_ready(&self) -> bool {
        matches!(self.catalog_phase, NativeCatalogPhase::Ready)
    }

    /// Returns a stable invalid-configuration failure for UI-only state when
    /// an operation is attempted before a real configured scope exists.
    #[must_use]
    pub const fn unavailable_failure() -> ServiceFailure {
        ServiceFailure {
            stage: crate::native_transport_service::ServiceFailureStage::Request,
            category: ServiceFailureCategory::InvalidConfiguration,
        }
    }
}

impl Default for NativeCatalogPhase {
    fn default() -> Self {
        Self::Offline
    }
}

/// Why a favorite intent was not admitted locally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FavoriteIntentError {
    /// No current validated runtime catalog is available.
    CatalogUnavailable,
    /// Another stable mutation is awaiting its terminal receipt or retry.
    AlreadyPending,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(value: &str) -> ThreadId {
        ThreadId::parse(value).expect("valid thread")
    }

    fn profile(value: &str) -> EngineProfileId {
        EngineProfileId::parse(value).expect("valid profile")
    }

    fn request(value: &str) -> RequestId {
        RequestId::parse(value).expect("valid request")
    }

    fn revision(value: &str) -> CatalogRevision {
        CatalogRevision::parse(value).expect("valid catalog revision")
    }

    fn failure() -> ServiceFailure {
        NativeCatalogController::unavailable_failure()
    }

    fn ready_state() -> (NativeCatalogController, NativeCatalogScope) {
        let mut state = NativeCatalogController::new();
        let scope = match state
            .select_scope(thread("thread-a"), profile("profile-a"))
            .expect("scope")
        {
            CatalogScopeSelection::Changed(scope) => scope,
            CatalogScopeSelection::Unchanged(_) => panic!("first scope must change"),
        };
        assert!(state.mark_catalog_admitted(&scope));
        assert!(state.on_catalog_loaded(&scope));
        (state, scope)
    }

    #[test]
    fn stale_catalog_generation_cannot_replace_new_thread_or_profile() {
        let mut state = NativeCatalogController::new();
        let first = match state
            .select_scope(thread("thread-a"), profile("profile-a"))
            .expect("first scope")
        {
            CatalogScopeSelection::Changed(scope) => scope,
            CatalogScopeSelection::Unchanged(_) => panic!("first scope must change"),
        };
        assert!(state.mark_catalog_admitted(&first));
        let second = match state
            .select_scope(thread("thread-b"), profile("profile-a"))
            .expect("second scope")
        {
            CatalogScopeSelection::Changed(scope) => scope,
            CatalogScopeSelection::Unchanged(_) => panic!("thread change must change"),
        };
        assert_ne!(first.generation, second.generation);
        assert!(!state.on_catalog_loaded(&first));
        assert_eq!(state.catalog_phase(), NativeCatalogPhase::Offline);
        assert!(state.mark_catalog_admitted(&second));
        assert!(state.on_catalog_loaded(&second));
        assert_eq!(state.catalog_phase(), NativeCatalogPhase::Ready);
    }

    #[test]
    fn favorites_accept_only_the_exact_admitted_scope_and_revision() {
        let (mut state, scope) = ready_state();
        assert!(state.favorites_request_needed(&scope));
        assert!(state.mark_favorites_admitted(&scope));
        let stale = NativeCatalogScope::new(
            scope.thread_id.clone(),
            profile("profile-stale"),
            scope.generation,
        );
        assert!(!state.on_favorites_loaded(&stale, 3, vec!["stale".to_owned()]));
        assert!(state.on_favorites_loaded(&scope, 3, vec!["model-a".to_owned()]));
        assert_eq!(state.favorite_revision(), Some(3));
        assert_eq!(state.favorite_ids(), ["model-a"]);
        assert!(!state.favorites_request_needed(&scope));
    }

    #[test]
    fn favorite_failure_retries_the_same_request_and_receipt_cannot_cross_scope() {
        let (mut state, scope) = ready_state();
        let model_id = ModelFavoriteId::parse("model-a").expect("model");
        let pending = state
            .begin_favorite(
                &scope,
                request("favorite-a"),
                model_id.clone(),
                revision("catalog-a"),
                true,
            )
            .expect("intent");
        assert!(state.mark_favorite_admitted(&scope, &pending.request_id));
        assert!(state.on_favorite_failed(&scope, &pending.request_id, failure()));
        let retry = state.retry_favorite().expect("retained retry");
        assert_eq!(retry.request_id, pending.request_id);
        assert_eq!(retry.model_id, pending.model_id);
        assert_eq!(retry.favorite, pending.favorite);

        let other_scope = NativeCatalogScope::new(
            thread("thread-b"),
            scope.profile_id.clone(),
            CatalogLoadGeneration::from_raw_for_test(scope.generation.get() + 1),
        );
        assert!(!state.on_favorite_succeeded(
            &other_scope,
            &pending.request_id,
            &model_id,
            true,
            4,
            vec!["model-a".to_owned()]
        ));
        assert!(state.pending_favorite().is_some());
    }

    #[test]
    fn stale_favorite_read_cannot_clobber_newer_mutation_receipt() {
        let (mut state, scope) = ready_state();
        assert!(state.mark_favorites_admitted(&scope));
        let model_id = ModelFavoriteId::parse("model-a").expect("model");
        let pending = state
            .begin_favorite(
                &scope,
                request("favorite-b"),
                model_id.clone(),
                revision("catalog-a"),
                true,
            )
            .expect("intent");
        assert!(state.mark_favorite_admitted(&scope, &pending.request_id));
        assert!(state.on_favorite_succeeded(
            &scope,
            &pending.request_id,
            &model_id,
            true,
            7,
            vec!["model-a".to_owned()]
        ));
        assert!(!state.on_favorites_loaded(&scope, 6, vec!["stale".to_owned()]));
        assert_eq!(state.favorite_revision(), Some(7));
        assert_eq!(state.favorite_ids(), ["model-a"]);
    }

    #[test]
    fn exhausted_generation_fails_closed() {
        let mut state = NativeCatalogController::new();
        state.next_generation = Some(CatalogLoadGeneration::from_raw_for_test(u64::MAX));
        assert_eq!(
            state.select_scope(thread("thread-a"), profile("profile-a")),
            Err(CatalogScopeError::GenerationExhausted)
        );
    }
}
