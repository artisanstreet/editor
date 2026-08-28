//! Dependency-free state and action policy for the visible run-usage reading.
//!
//! The legacy controller owns one optional run selection for the application.
//! It publishes a small state machine while a host adapter starts and
//! interrupts the corresponding authoritative subscription. This leaf keeps
//! those two parts explicit without implementing either side effect: update
//! payloads are generic already-decoded values, and subscription work leaves
//! through [`RunUsageHostAction`] descriptions.
//!
//! [`RunUsagePolicy`] takes `&mut self` for every transition. That is the
//! serialization boundary for select and release operations; the caller owns
//! any executor or synchronization needed to make one policy instance
//! available to multiple tasks. No lock, stream, fiber, transport, protocol
//! aggregate, or async runtime is part of this module.

#![allow(clippy::module_name_repetitions)]

/// Monotonically allocated identity of a run-usage lease owner.
pub type RunUsageOwnerId = u64;

/// The one visible run-usage projection.
///
/// `T` is the caller's already-decoded usage update. Keeping it generic lets
/// this policy preserve the legacy ready payload without defining or importing
/// a protocol aggregate in the frontend leaf.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunUsageState<T = ()> {
    /// No run is currently selected.
    None,
    /// A selected run is waiting for its authoritative subscription snapshot.
    Loading {
        /// Run whose snapshot is being awaited.
        run_id: String,
    },
    /// The selected run has an accepted authoritative snapshot.
    Ready {
        /// The already-decoded snapshot payload.
        aggregate: T,
        /// Run that produced the accepted payload.
        run_id: String,
    },
    /// The selected subscription reached its terminal failure path.
    Unavailable {
        /// Run whose authoritative reading is unavailable.
        run_id: String,
    },
}

impl<T> RunUsageState<T> {
    /// Returns the selected run identity represented by this state, if any.
    #[must_use]
    pub fn run_id(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Loading { run_id }
            | Self::Ready { run_id, .. }
            | Self::Unavailable { run_id } => Some(run_id),
        }
    }
}

/// Lifetime scope in which a run-usage subscription is owned.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunUsageSubscriptionScope {
    /// The application/controller scope, matching the legacy controller's
    /// `Effect.scope` rather than a route or component scope.
    App,
}

impl RunUsageSubscriptionScope {
    /// Returns the stable host-adapter label for this scope.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
        }
    }
}

/// Identity and ownership metadata for one authoritative run subscription.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RunUsageSubscriptionRequest {
    /// Lease owner that is allowed to publish this subscription's events.
    pub owner_id: RunUsageOwnerId,
    /// Run passed as the `run` scope to the authoritative host subscription.
    pub run_id: String,
    /// Scope that owns the subscription lifetime.
    pub scope: RunUsageSubscriptionScope,
}

impl RunUsageSubscriptionRequest {
    /// Describes one application-owned authoritative run subscription.
    #[must_use]
    pub fn app(owner_id: RunUsageOwnerId, run_id: impl Into<String>) -> Self {
        Self {
            owner_id,
            run_id: run_id.into(),
            scope: RunUsageSubscriptionScope::App,
        }
    }
}

/// A host-side subscription operation requested by the pure policy.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RunUsageHostAction {
    /// Interrupt the currently owned subscription before a replacement or
    /// release is published.
    InterruptSubscription(RunUsageSubscriptionRequest),
    /// Start one authoritative `run` subscription in application scope.
    StartAuthoritativeSubscription(RunUsageSubscriptionRequest),
}

impl RunUsageHostAction {
    /// Returns the request carried by this host operation.
    #[must_use]
    pub const fn request(&self) -> &RunUsageSubscriptionRequest {
        match self {
            Self::InterruptSubscription(request)
            | Self::StartAuthoritativeSubscription(request) => request,
        }
    }
}

/// One ordered action emitted by a policy transition.
///
/// The action list is a description for the embedding app. In particular, a
/// caller must execute `Publish(Loading)` before the following
/// `StartAuthoritativeSubscription`, and must execute an interrupt before the
/// next subscription operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunUsageAction<T = ()> {
    /// Publish one new visible projection state.
    Publish(RunUsageState<T>),
    /// Ask the application host to perform one subscription operation.
    Host(RunUsageHostAction),
}

/// Why a new lease owner could not be allocated.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunUsageAcquireError {
    /// The finite `u64` owner namespace is exhausted.
    OwnerIdExhausted,
}

impl std::fmt::Display for RunUsageAcquireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OwnerIdExhausted => {
                formatter.write_str("run-usage lease owner IDs are exhausted")
            }
        }
    }
}

impl std::error::Error for RunUsageAcquireError {}

/// A lease handle used to select and release one owner generation.
///
/// A lease remains callable after another owner has acquired the policy. Its
/// release is then stale and ignored, while its select preserves the legacy
/// controller's reselection behavior and may become the current owner again.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RunUsageLease {
    owner_id: RunUsageOwnerId,
}

impl RunUsageLease {
    /// Returns the monotonically allocated owner identity.
    #[must_use]
    pub const fn owner_id(self) -> RunUsageOwnerId {
        self.owner_id
    }
}

#[derive(Debug)]
struct ActiveRunUsage {
    owner_id: RunUsageOwnerId,
    run_id: Option<String>,
}

/// Serialized state/action owner for one application-visible run-usage view.
///
/// The policy deliberately records actions instead of executing them. Callers
/// drain [`Self::take_actions`] and map host actions to their app-scoped
/// subscription runtime. All mutating methods require exclusive `&mut self`,
/// so select/release transitions cannot interleave within this state owner.
#[derive(Debug)]
pub struct RunUsagePolicy<T = ()> {
    next_owner_id: RunUsageOwnerId,
    active: Option<ActiveRunUsage>,
    state: RunUsageState<T>,
    actions: Vec<RunUsageAction<T>>,
}

impl<T> RunUsagePolicy<T> {
    /// Creates an idle policy with no owner, run, or fabricated usage value.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_owner_id: 0,
            active: None,
            state: RunUsageState::None,
            actions: Vec::new(),
        }
    }

    /// Returns the currently published visible projection.
    #[must_use]
    pub const fn state(&self) -> &RunUsageState<T> {
        &self.state
    }

    /// Returns the current owner, including an owner that has deselected its
    /// run and is therefore retaining a lease without a live subscription.
    #[must_use]
    pub const fn active_owner_id(&self) -> Option<RunUsageOwnerId> {
        match &self.active {
            Some(active) => Some(active.owner_id),
            None => None,
        }
    }

    /// Returns the currently subscribed run, if the active owner selected one.
    #[must_use]
    pub fn active_run_id(&self) -> Option<&str> {
        self.active
            .as_ref()
            .and_then(|active| active.run_id.as_deref())
    }

    /// Returns pending actions without draining them.
    #[must_use]
    pub fn actions(&self) -> &[RunUsageAction<T>] {
        &self.actions
    }

    /// Drains pending publication and host actions in emission order.
    pub fn take_actions(&mut self) -> Vec<RunUsageAction<T>> {
        std::mem::take(&mut self.actions)
    }

    /// Allocates a new owner and applies its initial optional run selection.
    ///
    /// Owner allocation happens before selection, matching the legacy
    /// controller. A successful acquire therefore always advances the owner
    /// counter, even when its initial selection is `None`.
    ///
    /// # Errors
    ///
    /// Returns [`RunUsageAcquireError::OwnerIdExhausted`] without changing
    /// state or actions when no newer owner ID can be represented.
    pub fn acquire(&mut self, run_id: Option<&str>) -> Result<RunUsageLease, RunUsageAcquireError>
    where
        T: Clone,
    {
        let owner_id = self
            .next_owner_id
            .checked_add(1)
            .ok_or(RunUsageAcquireError::OwnerIdExhausted)?;
        self.next_owner_id = owner_id;
        let lease = RunUsageLease { owner_id };
        self.select_for_owner(owner_id, run_id);
        Ok(lease)
    }

    /// Selects an optional run for a lease.
    ///
    /// A lease's selection is intentionally not rejected merely because a
    /// newer lease is currently active: the legacy `Select` closure can
    /// reselect and become current again. Only a same-owner, same-run request
    /// is a no-op.
    pub fn select(&mut self, lease: &RunUsageLease, run_id: Option<&str>)
    where
        T: Clone,
    {
        self.select_for_owner(lease.owner_id, run_id);
    }

    /// Selects on behalf of an explicit owner ID.
    ///
    /// This lower-level form is useful to an adapter that stores only the
    /// lease identity. Prefer [`Self::select`] when the lease handle is
    /// available.
    pub fn select_owner(&mut self, owner_id: RunUsageOwnerId, run_id: Option<&str>)
    where
        T: Clone,
    {
        self.select_for_owner(owner_id, run_id);
    }

    /// Accepts one subscription update if both owner and run still match.
    ///
    /// A matching update replaces the ready payload and publishes `Ready`.
    /// A stale update is discarded without changing state or emitting an
    /// action.
    pub fn accept_update(&mut self, owner_id: RunUsageOwnerId, run_id: &str, aggregate: T)
    where
        T: Clone,
    {
        if !self.matches(owner_id, run_id) {
            return;
        }
        self.publish(RunUsageState::Ready {
            aggregate,
            run_id: run_id.to_owned(),
        });
    }

    /// Alias using the shorter event-oriented name.
    pub fn update(&mut self, owner_id: RunUsageOwnerId, run_id: &str, aggregate: T)
    where
        T: Clone,
    {
        self.accept_update(owner_id, run_id, aggregate);
    }

    /// Accepts a terminal subscription failure if both owner and run still
    /// match, publishing `Unavailable` for that run.
    pub fn accept_failure(&mut self, owner_id: RunUsageOwnerId, run_id: &str)
    where
        T: Clone,
    {
        if !self.matches(owner_id, run_id) {
            return;
        }
        self.publish(RunUsageState::Unavailable {
            run_id: run_id.to_owned(),
        });
    }

    /// Alias using the shorter event-oriented name.
    pub fn failure(&mut self, owner_id: RunUsageOwnerId, run_id: &str)
    where
        T: Clone,
    {
        self.accept_failure(owner_id, run_id);
    }

    /// Releases a lease only when it owns the current selection.
    ///
    /// A matching release interrupts its live subscription first, clears all
    /// ownership, and publishes `None`. A stale release emits no action and
    /// cannot disturb a newer owner.
    pub fn release(&mut self, lease: &RunUsageLease)
    where
        T: Clone,
    {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.owner_id != lease.owner_id {
            return;
        }

        let active = self
            .active
            .take()
            .expect("active run usage was checked immediately above");
        if let Some(run_id) = active.run_id {
            self.push_host(RunUsageHostAction::InterruptSubscription(
                RunUsageSubscriptionRequest::app(active.owner_id, run_id),
            ));
        }
        self.publish(RunUsageState::None);
    }

    fn select_for_owner(&mut self, owner_id: RunUsageOwnerId, run_id: Option<&str>)
    where
        T: Clone,
    {
        let requested_run_id = run_id.map(str::to_owned);
        let same_selection = self.active.as_ref().is_some_and(|active| {
            active.owner_id == owner_id && active.run_id.as_deref() == requested_run_id.as_deref()
        });
        if same_selection {
            return;
        }

        if let Some(current) = self.active.take()
            && let Some(current_run_id) = current.run_id
        {
            self.push_host(RunUsageHostAction::InterruptSubscription(
                RunUsageSubscriptionRequest::app(current.owner_id, current_run_id),
            ));
        }

        match requested_run_id {
            Some(run_id) => {
                self.active = Some(ActiveRunUsage {
                    owner_id,
                    run_id: Some(run_id.clone()),
                });
                self.publish(RunUsageState::Loading {
                    run_id: run_id.clone(),
                });
                self.push_host(RunUsageHostAction::StartAuthoritativeSubscription(
                    RunUsageSubscriptionRequest::app(owner_id, run_id),
                ));
            }
            None => {
                self.active = Some(ActiveRunUsage {
                    owner_id,
                    run_id: None,
                });
                self.publish(RunUsageState::None);
            }
        }
    }

    fn matches(&self, owner_id: RunUsageOwnerId, run_id: &str) -> bool {
        self.active.as_ref().is_some_and(|active| {
            active.owner_id == owner_id && active.run_id.as_deref() == Some(run_id)
        })
    }

    fn publish(&mut self, state: RunUsageState<T>)
    where
        T: Clone,
    {
        self.state = state;
        self.actions
            .push(RunUsageAction::Publish(self.state.clone()));
    }

    fn push_host(&mut self, host_action: RunUsageHostAction) {
        self.actions.push(RunUsageAction::Host(host_action));
    }
}

impl<T> Default for RunUsagePolicy<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Name matching the legacy service at the future runtime integration seam.
pub type RunUsageController<T = ()> = RunUsagePolicy<T>;
