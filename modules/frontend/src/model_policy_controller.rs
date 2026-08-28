//! Dependency-free model-policy mutation and reconciliation state.
//!
//! This is the native counterpart of
//! `routes/components/model-selector/policy-controller.ts`. It deliberately
//! stops at an owned value and a synchronous persistence seam: there is no
//! protocol decoding, Effect runtime, transport, storage, or UI behavior in
//! this module.

#![forbid(unsafe_code)]

use std::fmt;
use std::sync::{Mutex, MutexGuard};

/// A compact, owned selection with independent policy axes.
///
/// The native boundary intentionally carries only the axes needed to prove
/// replacement and partial-update behavior. A later protocol adapter may
/// project the complete `ThreadSessionPolicy` onto this value without making
/// the state machine depend on protocol types.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SessionPolicy {
    /// The selected execution engine or harness.
    pub engine_id: String,
    /// The selected model within the engine.
    pub model_id: String,
    /// The model's reasoning-effort selection.
    pub reasoning_effort: String,
    /// The coarse permission compatibility mode.
    pub permission_mode: String,
    /// Whether web search is enabled for the session.
    pub web_search_enabled: bool,
}

impl SessionPolicy {
    /// Creates an owned policy from the five independent selection axes.
    #[must_use]
    pub fn new(
        engine_id: impl Into<String>,
        model_id: impl Into<String>,
        reasoning_effort: impl Into<String>,
        permission_mode: impl Into<String>,
        web_search_enabled: bool,
    ) -> Self {
        Self {
            engine_id: engine_id.into(),
            model_id: model_id.into(),
            reasoning_effort: reasoning_effort.into(),
            permission_mode: permission_mode.into(),
            web_search_enabled,
        }
    }

    /// Applies a partial update while retaining every omitted axis.
    #[must_use]
    pub fn apply_patch(mut self, patch: SessionPolicyPatch) -> Self {
        if let Some(engine_id) = patch.engine_id {
            self.engine_id = engine_id;
        }
        if let Some(model_id) = patch.model_id {
            self.model_id = model_id;
        }
        if let Some(reasoning_effort) = patch.reasoning_effort {
            self.reasoning_effort = reasoning_effort;
        }
        if let Some(permission_mode) = patch.permission_mode {
            self.permission_mode = permission_mode;
        }
        if let Some(web_search_enabled) = patch.web_search_enabled {
            self.web_search_enabled = web_search_enabled;
        }
        self
    }

    /// Returns the opaque, deterministic key used for repair deduplication.
    ///
    /// Length-prefixed segments make the key unambiguous without a serializer
    /// dependency. The field order is part of this local policy boundary; the
    /// key is not a wire representation.
    #[must_use]
    pub fn key(&self) -> String {
        let mut key = String::new();
        append_key_segment(&mut key, &self.engine_id);
        append_key_segment(&mut key, &self.model_id);
        append_key_segment(&mut key, &self.reasoning_effort);
        append_key_segment(&mut key, &self.permission_mode);
        key.push_str(if self.web_search_enabled {
            "true"
        } else {
            "false"
        });
        key
    }
}

/// Independent optional replacements for [`SessionPolicy`] axes.
///
/// `None` means that the property was omitted, matching the TypeScript
/// `Partial<ThreadSessionPolicy>` spread. It is not a request to reset the
/// corresponding owned value.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SessionPolicyPatch {
    /// Replacement for the engine identifier, when supplied.
    pub engine_id: Option<String>,
    /// Replacement for the model identifier, when supplied.
    pub model_id: Option<String>,
    /// Replacement for reasoning effort, when supplied.
    pub reasoning_effort: Option<String>,
    /// Replacement for permission mode, when supplied.
    pub permission_mode: Option<String>,
    /// Replacement for web-search admission, when supplied.
    pub web_search_enabled: Option<bool>,
}

impl SessionPolicyPatch {
    /// Creates an empty patch.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            engine_id: None,
            model_id: None,
            reasoning_effort: None,
            permission_mode: None,
            web_search_enabled: None,
        }
    }

    /// Creates a patch for the engine axis only.
    #[must_use]
    pub fn for_engine_id(engine_id: impl Into<String>) -> Self {
        Self {
            engine_id: Some(engine_id.into()),
            ..Self::empty()
        }
    }

    /// Creates a patch for the model axis only.
    #[must_use]
    pub fn for_model_id(model_id: impl Into<String>) -> Self {
        Self {
            model_id: Some(model_id.into()),
            ..Self::empty()
        }
    }

    /// Creates a patch for the reasoning-effort axis only.
    #[must_use]
    pub fn for_reasoning_effort(reasoning_effort: impl Into<String>) -> Self {
        Self {
            reasoning_effort: Some(reasoning_effort.into()),
            ..Self::empty()
        }
    }

    /// Creates a patch for the permission axis only.
    #[must_use]
    pub fn for_permission_mode(permission_mode: impl Into<String>) -> Self {
        Self {
            permission_mode: Some(permission_mode.into()),
            ..Self::empty()
        }
    }

    /// Creates a patch for the web-search axis only.
    #[must_use]
    pub fn for_web_search_enabled(web_search_enabled: bool) -> Self {
        Self {
            web_search_enabled: Some(web_search_enabled),
            ..Self::empty()
        }
    }

    /// Returns whether every policy property was omitted.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.engine_id.is_none()
            && self.model_id.is_none()
            && self.reasoning_effort.is_none()
            && self.permission_mode.is_none()
            && self.web_search_enabled.is_none()
    }

    /// Adds or replaces the engine axis in this patch.
    #[must_use]
    pub fn with_engine_id(mut self, engine_id: impl Into<String>) -> Self {
        self.engine_id = Some(engine_id.into());
        self
    }

    /// Adds or replaces the model axis in this patch.
    #[must_use]
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }

    /// Adds or replaces the reasoning-effort axis in this patch.
    #[must_use]
    pub fn with_reasoning_effort(mut self, reasoning_effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(reasoning_effort.into());
        self
    }

    /// Adds or replaces the permission axis in this patch.
    #[must_use]
    pub fn with_permission_mode(mut self, permission_mode: impl Into<String>) -> Self {
        self.permission_mode = Some(permission_mode.into());
        self
    }

    /// Adds or replaces the web-search axis in this patch.
    #[must_use]
    pub fn with_web_search_enabled(mut self, web_search_enabled: bool) -> Self {
        self.web_search_enabled = Some(web_search_enabled);
        self
    }
}

/// The four pieces of mutable reconciliation state owned by one controller.
///
/// The fields are public for an adapter or test harness that needs to inspect
/// the state transition. Mutations should go through
/// [`ModelPolicyController`], which preserves the precedence and locking
/// rules.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct PolicyControllerState {
    /// The latest policy confirmed by persistence or an authoritative event.
    pub authoritative: Option<SessionPolicy>,
    /// The newest local intent waiting to be persisted.
    pub desired: Option<SessionPolicy>,
    /// The intent currently handed to the persistence closure.
    pub in_flight: Option<SessionPolicy>,
    /// The opaque key of the most recently requested repair, if any.
    pub repair_key: Option<String>,
}

impl PolicyControllerState {
    /// Resolves the effective policy using the legacy precedence order.
    #[must_use]
    pub fn current(&self) -> Option<SessionPolicy> {
        self.desired
            .clone()
            .or_else(|| self.in_flight.clone())
            .or_else(|| self.authoritative.clone())
    }
}

/// The result of a successful serialized flush.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct PolicyFlushResult {
    /// Every authoritative policy returned by a persistence invocation, in
    /// invocation order.
    pub confirmed: Vec<SessionPolicy>,
    /// The effective policy after the desired queue has been drained.
    pub current: Option<SessionPolicy>,
}

impl PolicyFlushResult {
    /// Borrows all authoritative policies confirmed by this flush.
    #[must_use]
    pub fn confirmed(&self) -> &[SessionPolicy] {
        &self.confirmed
    }

    /// Borrows the final effective policy, if one exists.
    #[must_use]
    pub fn current(&self) -> Option<&SessionPolicy> {
        self.current.as_ref()
    }
}

/// A persistence failure wrapped without erasing its typed cause.
#[must_use = "a model-policy mutation failure should be handled"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModelPolicyMutationError<Cause> {
    /// The exact error returned by the injected persistence closure.
    pub cause: Cause,
}

impl<Cause> ModelPolicyMutationError<Cause> {
    /// Wraps a persistence cause.
    #[must_use]
    pub const fn new(cause: Cause) -> Self {
        Self { cause }
    }

    /// Borrows the exact persistence cause.
    #[must_use]
    pub const fn cause(&self) -> &Cause {
        &self.cause
    }

    /// Returns the exact persistence cause.
    #[must_use]
    pub fn into_cause(self) -> Cause {
        self.cause
    }
}

impl<Cause: fmt::Display> fmt::Display for ModelPolicyMutationError<Cause> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.cause.fmt(formatter)
    }
}

impl<Cause: std::error::Error + 'static> std::error::Error for ModelPolicyMutationError<Cause> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

/// Component-local model policy state and mutation/reconciliation operations.
///
/// State mutation is synchronous and independent of any executor. The state
/// lock is released before the injected persistence closure is called, so the
/// closure can queue a newer `replace`, `patch`, or repair request. The flush
/// lock remains held for the whole drain, making concurrent flush calls
/// serialize while still allowing those ordinary mutations.
#[derive(Debug, Default)]
pub struct ModelPolicyController {
    state: Mutex<PolicyControllerState>,
    flush_lock: Mutex<()>,
}

impl ModelPolicyController {
    /// Creates an empty controller with no authoritative policy.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a snapshot of all controller state.
    #[must_use]
    pub fn state(&self) -> PolicyControllerState {
        lock_unpoisoned(&self.state).clone()
    }

    /// Returns the effective policy using `desired > in_flight > authoritative`.
    #[must_use]
    pub fn current(&self) -> Option<SessionPolicy> {
        lock_unpoisoned(&self.state).current()
    }

    /// Replaces the authoritative policy and returns the effective result.
    ///
    /// A changed authoritative value clears the repair key. Repeating the
    /// exact same authoritative value preserves that key, matching the
    /// source controller's structural comparison.
    pub fn set_authoritative(&self, policy: SessionPolicy) -> SessionPolicy {
        let mut state = lock_unpoisoned(&self.state);
        let changed = state.authoritative.as_ref() != Some(&policy);
        state.authoritative = Some(policy);
        if changed {
            state.repair_key = None;
        }
        state
            .current()
            .expect("an authoritative replacement always supplies a current policy")
    }

    /// Replaces the whole desired policy without merging it with prior state.
    pub fn replace(&self, policy: SessionPolicy) -> SessionPolicy {
        let mut state = lock_unpoisoned(&self.state);
        state.desired = Some(policy.clone());
        policy
    }

    /// Applies a partial patch to the current effective policy.
    ///
    /// When no authoritative, in-flight, or desired policy exists, the patch
    /// has no base and returns `None` without queuing an intent.
    pub fn patch(&self, patch: SessionPolicyPatch) -> Option<SessionPolicy> {
        let mut state = lock_unpoisoned(&self.state);
        let base = state.current()?;
        let desired = base.apply_patch(patch);
        state.desired = Some(desired.clone());
        Some(desired)
    }

    /// Requests one repair unless its key or the effective policy already
    /// represents the same value.
    pub fn request_repair(&self, policy: SessionPolicy) -> bool {
        let mut state = lock_unpoisoned(&self.state);
        let repair_key = policy.key();
        if state.repair_key.as_deref() == Some(repair_key.as_str())
            || state.current().as_ref() == Some(&policy)
        {
            return false;
        }

        state.desired = Some(policy);
        state.repair_key = Some(repair_key);
        true
    }

    /// Drains desired policies through a synchronous persistence closure.
    ///
    /// The closure receives each desired policy by value and returns the
    /// authoritative policy after persistence/normalization. Newly queued
    /// desired policies are consumed in the same call, with the latest queued
    /// value winning. On the first failure, both desired and in-flight state
    /// are cleared exactly as the legacy controller does; the authoritative
    /// policy and repair key are retained.
    pub fn flush<Persist, Cause>(
        &self,
        mut persist: Persist,
    ) -> Result<PolicyFlushResult, ModelPolicyMutationError<Cause>>
    where
        Persist: FnMut(SessionPolicy) -> Result<SessionPolicy, Cause>,
    {
        let _flush_guard = lock_unpoisoned(&self.flush_lock);
        let mut confirmed = Vec::new();

        loop {
            let desired = {
                let mut state = lock_unpoisoned(&self.state);
                let Some(desired) = state.desired.take() else {
                    break;
                };
                state.in_flight = Some(desired.clone());
                desired
            };

            match persist(desired) {
                Ok(authoritative) => {
                    let mut state = lock_unpoisoned(&self.state);
                    state.authoritative = Some(authoritative.clone());
                    state.in_flight = None;
                    confirmed.push(authoritative);
                }
                Err(cause) => {
                    let mut state = lock_unpoisoned(&self.state);
                    state.desired = None;
                    state.in_flight = None;
                    return Err(ModelPolicyMutationError::new(cause));
                }
            }
        }

        Ok(PolicyFlushResult {
            confirmed,
            current: self.current(),
        })
    }
}

fn append_key_segment(key: &mut String, value: &str) {
    key.push_str(&value.len().to_string());
    key.push(':');
    key.push_str(value);
    key.push('|');
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
