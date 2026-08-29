//! Pure policy for the thread-retention settings surface.
//!
//! The legacy settings component owns an Effect controller, a change stream,
//! and the rendered controls. This leaf keeps only the decoded policy, the
//! local input and save state, and the typed intents that an adapter can hand
//! to those boundaries. It performs no transport, stream subscription,
//! rendering, or asynchronous work.

#![allow(clippy::module_name_repetitions)]

use std::fmt;

/// Inclusive lower bound accepted by the thread-retention protocol.
pub const THREAD_RETENTION_MIN_INACTIVITY_DAYS: u16 = 1;

/// Inclusive upper bound accepted by the thread-retention protocol.
pub const THREAD_RETENTION_MAX_INACTIVITY_DAYS: u16 = 3650;

/// The exact title shown after a retention-policy refresh cannot be verified.
pub const THREAD_RETENTION_SETTINGS_LOAD_FAILURE_TITLE: &str = "Could not load retention policy";

/// The exact title shown after a retention-policy save cannot be verified.
pub const THREAD_RETENTION_SETTINGS_SAVE_FAILURE_TITLE: &str = "Could not save retention policy";

/// The native scalar used for the protocol's bounded inactivity-day value.
pub type ThreadRetentionInactivityDays = u16;

/// The two fields of the decoded thread-retention policy.
///
/// Values received from a controller are expected to have passed the
/// protocol's inclusive `1..=3650` check. [`Self::try_new`] and
/// [`Self::validate`] are available at a boundary that constructs a value
/// from untrusted data.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadRetentionPolicy {
    /// Whether inactive threads are erased at all.
    pub enabled: bool,
    /// Number of untouched days before an inactive thread is erased.
    pub inactivity_days: ThreadRetentionInactivityDays,
}

impl ThreadRetentionPolicy {
    /// Creates a policy without performing I/O, coercion, or validation.
    #[must_use]
    pub const fn new(enabled: bool, inactivity_days: ThreadRetentionInactivityDays) -> Self {
        Self {
            enabled,
            inactivity_days,
        }
    }

    /// Creates a policy after enforcing the protocol's inclusive day bound.
    ///
    /// No value is clamped or normalized.
    ///
    /// # Errors
    ///
    /// Returns [`ThreadRetentionPolicyValidationError::InactivityDaysOutOfRange`]
    /// when `inactivity_days` is outside the inclusive protocol range.
    pub const fn try_new(
        enabled: bool,
        inactivity_days: ThreadRetentionInactivityDays,
    ) -> Result<Self, ThreadRetentionPolicyValidationError> {
        let policy = Self::new(enabled, inactivity_days);
        match policy.validate() {
            Ok(()) => Ok(policy),
            Err(error) => Err(error),
        }
    }

    /// Returns whether this policy satisfies the protocol's day bound.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.inactivity_days >= THREAD_RETENTION_MIN_INACTIVITY_DAYS
            && self.inactivity_days <= THREAD_RETENTION_MAX_INACTIVITY_DAYS
    }

    /// Validates this policy without changing it.
    ///
    /// # Errors
    ///
    /// Returns [`ThreadRetentionPolicyValidationError::InactivityDaysOutOfRange`]
    /// when the inactivity threshold is outside the inclusive protocol range.
    pub const fn validate(self) -> Result<(), ThreadRetentionPolicyValidationError> {
        if self.is_valid() {
            Ok(())
        } else {
            Err(
                ThreadRetentionPolicyValidationError::InactivityDaysOutOfRange {
                    value: self.inactivity_days,
                },
            )
        }
    }
}

/// Validation failure for one constructed retention-policy value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadRetentionPolicyValidationError {
    /// The inactivity threshold was outside the protocol's inclusive range.
    InactivityDaysOutOfRange {
        /// The offending threshold.
        value: ThreadRetentionInactivityDays,
    },
}

impl fmt::Display for ThreadRetentionPolicyValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InactivityDaysOutOfRange { value } => write!(
                formatter,
                "thread retention inactivity days {value} are outside {THREAD_RETENTION_MIN_INACTIVITY_DAYS}..={THREAD_RETENTION_MAX_INACTIVITY_DAYS}"
            ),
        }
    }
}

impl std::error::Error for ThreadRetentionPolicyValidationError {}

/// The state tags emitted by the retention-policy controller.
///
/// `Unverified` carries no error payload because the controller exposes only
/// whether durable truth was confirmed. The human-facing failure title lives
/// in [`ThreadRetentionSettingsState`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ThreadRetentionPolicyState {
    /// No retention policy has been confirmed yet.
    #[default]
    Loading,
    /// A refresh or save stream update confirmed a policy.
    Ready {
        /// The exact policy currently exposed by the controller.
        policy: ThreadRetentionPolicy,
    },
    /// The most recent controller operation failed to confirm durable truth.
    Unverified,
}

impl ThreadRetentionPolicyState {
    /// Returns the confirmed policy, if one is currently available.
    #[must_use]
    pub const fn policy(self) -> Option<ThreadRetentionPolicy> {
        match self {
            Self::Ready { policy } => Some(policy),
            Self::Loading | Self::Unverified => None,
        }
    }

    /// Returns whether this is the confirmed state.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// Returns whether no confirmed durable policy is available.
    #[must_use]
    pub const fn is_unavailable(self) -> bool {
        matches!(self, Self::Loading | Self::Unverified)
    }
}

/// The typed refresh intent emitted by an admitted retry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadRetentionSettingsRefreshAction {
    /// Re-read the retention policy from the controller boundary.
    RefreshPolicy,
}

impl ThreadRetentionSettingsRefreshAction {
    /// Returns the protocol query kind used by the refresh adapter.
    #[must_use]
    pub const fn operation(self) -> &'static str {
        match self {
            Self::RefreshPolicy => "thread.retention.query",
        }
    }
}

/// The typed save intent emitted by an admitted toggle or day commit.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadRetentionSettingsSaveAction {
    /// Persist the exact policy requested by the settings control.
    SavePolicy {
        /// The policy to submit to the retention controller.
        policy: ThreadRetentionPolicy,
    },
}

impl ThreadRetentionSettingsSaveAction {
    /// Returns the exact policy carried by this save intent.
    #[must_use]
    pub const fn policy(self) -> ThreadRetentionPolicy {
        match self {
            Self::SavePolicy { policy } => policy,
        }
    }

    /// Returns the exact policy carried by this save intent.
    #[must_use]
    pub const fn requested_policy(self) -> ThreadRetentionPolicy {
        self.policy()
    }

    /// Returns the protocol update kind used by the save adapter.
    #[must_use]
    pub const fn operation(self) -> &'static str {
        match self {
            Self::SavePolicy { .. } => "thread.retention.update",
        }
    }
}

/// Alias for callers that name controller-bound intents commands.
pub type ThreadRetentionSettingsPersistenceCommand = ThreadRetentionSettingsSaveAction;

/// The result observed when an admitted retention-policy save completes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadRetentionSettingsSaveOutcome {
    /// The controller confirmed the requested save.
    Succeeded,
    /// The controller failed to confirm the requested save.
    Failed,
}

/// Complete deterministic state for the thread-retention settings surface.
///
/// `policy_state` is replaced by controller stream observations. The local
/// `days_text`, `saving`, and `failure_title` fields model the Svelte state
/// variables and are changed only by the transitions below. A save intent
/// does not optimistically replace the policy; a later `Ready` stream update
/// is authoritative.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadRetentionSettingsState {
    /// The latest state observed from the retention-policy controller.
    pub policy_state: ThreadRetentionPolicyState,
    /// The exact text currently held by the number input.
    pub days_text: String,
    /// Whether an admitted save is currently in flight.
    pub saving: bool,
    /// The exact title shown for an unverified policy state.
    pub failure_title: String,
}

/// Short alias for the complete retention settings state.
pub type ThreadRetentionSettings = ThreadRetentionSettingsState;

impl Default for ThreadRetentionSettingsState {
    fn default() -> Self {
        Self::new(ThreadRetentionPolicyState::Loading)
    }
}

impl ThreadRetentionSettingsState {
    /// Creates local settings state from one controller state observation.
    ///
    /// The legacy component initializes its failure title before inspecting
    /// the initial controller state. Consequently a `Ready` initial state has
    /// populated days text but still carries the initial title until a
    /// streamed `Ready` application clears it; the title is not rendered in
    /// that ready state.
    #[must_use]
    pub fn new(policy_state: ThreadRetentionPolicyState) -> Self {
        let days_text = match policy_state {
            ThreadRetentionPolicyState::Ready { policy } => policy.inactivity_days.to_string(),
            ThreadRetentionPolicyState::Loading | ThreadRetentionPolicyState::Unverified => {
                String::new()
            }
        };

        Self {
            policy_state,
            days_text,
            saving: false,
            failure_title: THREAD_RETENTION_SETTINGS_LOAD_FAILURE_TITLE.to_owned(),
        }
    }

    /// Creates initial settings state from a confirmed policy.
    #[must_use]
    pub fn from_policy(policy: ThreadRetentionPolicy) -> Self {
        Self::new(ThreadRetentionPolicyState::Ready { policy })
    }

    /// Returns the current controller state by shared reference.
    #[must_use]
    pub const fn state(&self) -> &ThreadRetentionPolicyState {
        &self.policy_state
    }

    /// Returns the current controller state by shared reference.
    #[must_use]
    pub const fn policy_state(&self) -> &ThreadRetentionPolicyState {
        &self.policy_state
    }

    /// Returns the confirmed policy, if one is currently available.
    #[must_use]
    pub const fn policy(&self) -> Option<ThreadRetentionPolicy> {
        self.policy_state.policy()
    }

    /// Returns the exact number-input text.
    #[must_use]
    pub fn days_text(&self) -> &str {
        &self.days_text
    }

    /// Returns whether a save is currently in flight.
    #[must_use]
    pub const fn is_saving(&self) -> bool {
        self.saving
    }

    /// Returns the exact failure title currently held by the local state.
    #[must_use]
    pub fn failure_title(&self) -> &str {
        &self.failure_title
    }

    /// Alias matching the legacy local variable's vocabulary.
    #[must_use]
    pub fn policy_failure_title(&self) -> &str {
        self.failure_title()
    }

    /// Returns whether the retention switch is enabled for interaction.
    ///
    /// The switch requires a confirmed policy and no save in flight.
    #[must_use]
    pub const fn toggle_enabled(&self) -> bool {
        self.policy_state.is_ready() && !self.saving
    }

    /// Returns the switch's derived disabled state.
    #[must_use]
    pub const fn toggle_disabled(&self) -> bool {
        !self.toggle_enabled()
    }

    /// Alias for [`Self::toggle_disabled`] using the rendered-control name.
    #[must_use]
    pub const fn switch_disabled(&self) -> bool {
        self.toggle_disabled()
    }

    /// Returns whether the source toggle guard will admit a save.
    #[must_use]
    pub const fn can_admit_toggle(&self) -> bool {
        self.toggle_enabled()
    }

    /// Returns whether the number input is enabled by the rendered policy.
    ///
    /// The input is disabled while the policy is unavailable, retention is
    /// off, or a save is in flight.
    #[must_use]
    pub const fn days_input_enabled(&self) -> bool {
        match self.policy_state {
            ThreadRetentionPolicyState::Ready { policy } => policy.enabled && !self.saving,
            ThreadRetentionPolicyState::Loading | ThreadRetentionPolicyState::Unverified => false,
        }
    }

    /// Returns the number input's derived disabled state.
    #[must_use]
    pub const fn days_input_disabled(&self) -> bool {
        !self.days_input_enabled()
    }

    /// Alias for [`Self::days_input_disabled`].
    #[must_use]
    pub const fn days_disabled(&self) -> bool {
        self.days_input_disabled()
    }

    /// Returns whether a day commit's source guard will admit a save.
    ///
    /// This intentionally does not include `policy.enabled`: the legacy
    /// `CommitDays` handler checks only for a policy and an idle save, while
    /// the disabled number input normally prevents a disabled-control event.
    #[must_use]
    pub const fn can_admit_days_commit(&self) -> bool {
        self.policy_state.is_ready() && !self.saving
    }

    /// Returns whether the rendered retry button is enabled.
    #[must_use]
    pub const fn retry_enabled(&self) -> bool {
        !self.saving
    }

    /// Returns the retry button's derived disabled state.
    #[must_use]
    pub const fn retry_disabled(&self) -> bool {
        !self.retry_enabled()
    }

    /// Returns whether an adapter may admit a retry through the disabled
    /// control.
    #[must_use]
    pub const fn can_admit_retry(&self) -> bool {
        self.retry_enabled()
    }

    /// Replaces the number-input text exactly as a bound input would.
    pub fn set_days_text(&mut self, text: impl Into<String>) {
        self.days_text = text.into();
    }

    /// Applies one controller change-stream state.
    ///
    /// Every state tag replaces `policy_state`. Only `Ready` also clears the
    /// failure title and replaces the input text with the policy's canonical
    /// decimal representation. `saving` is preserved, so a stream update
    /// arriving during a save cannot accidentally settle that save.
    pub fn apply_streamed_state(&mut self, next: ThreadRetentionPolicyState) {
        self.policy_state = next;
        if let ThreadRetentionPolicyState::Ready { policy } = next {
            self.failure_title.clear();
            self.days_text = policy.inactivity_days.to_string();
        }
    }

    /// Alias for [`Self::apply_streamed_state`].
    pub fn apply_policy_state(&mut self, next: ThreadRetentionPolicyState) {
        self.apply_streamed_state(next);
    }

    /// Applies a successful refresh as a controller `Ready` observation.
    pub fn refresh_succeeded(&mut self, policy: ThreadRetentionPolicy) {
        self.apply_streamed_state(ThreadRetentionPolicyState::Ready { policy });
    }

    /// Records the local refresh failure catch branch.
    ///
    /// The controller separately publishes [`ThreadRetentionPolicyState::Unverified`]
    /// on failure; an adapter can apply that stream state independently with
    /// [`Self::apply_streamed_state`]. This method therefore changes only the
    /// local title, exactly as the legacy `Effect.catch` branch does.
    pub fn refresh_failed(&mut self) {
        THREAD_RETENTION_SETTINGS_LOAD_FAILURE_TITLE.clone_into(&mut self.failure_title);
    }

    /// Applies the controller's unverified refresh result and its local title
    /// in one convenience transition.
    pub fn apply_refresh_failure(&mut self) {
        self.apply_streamed_state(ThreadRetentionPolicyState::Unverified);
        self.refresh_failed();
    }

    /// Admits the source retry handler and immediately enters `Loading`.
    ///
    /// The legacy handler itself does not inspect `saving`; the rendered retry
    /// button is disabled while saving. Callers that model control admission
    /// should use [`Self::admit_retry`].
    #[must_use]
    pub fn retry(&mut self) -> ThreadRetentionSettingsRefreshAction {
        self.policy_state = ThreadRetentionPolicyState::Loading;
        self.days_text.clear();
        ThreadRetentionSettingsRefreshAction::RefreshPolicy
    }

    /// Admits a retry only when the rendered retry control is enabled.
    #[must_use]
    pub fn admit_retry(&mut self) -> Option<ThreadRetentionSettingsRefreshAction> {
        if self.retry_enabled() {
            Some(self.retry())
        } else {
            None
        }
    }

    /// Admits the requested switch value and emits a save intent.
    ///
    /// The policy remains authoritative until a later streamed `Ready`
    /// state. Starting a save does not clear the existing title or input,
    /// matching the legacy `SavePolicy` ordering.
    #[must_use]
    pub fn admit_toggle(&mut self, enabled: bool) -> Option<ThreadRetentionSettingsSaveAction> {
        let policy = self.policy()?;
        if self.saving {
            return None;
        }

        self.saving = true;
        Some(ThreadRetentionSettingsSaveAction::SavePolicy {
            policy: ThreadRetentionPolicy { enabled, ..policy },
        })
    }

    /// Admits the inverse of the current switch value.
    #[must_use]
    pub fn toggle(&mut self) -> Option<ThreadRetentionSettingsSaveAction> {
        let policy = self.policy()?;
        self.admit_toggle(!policy.enabled)
    }

    /// Alias for [`Self::admit_toggle`].
    #[must_use]
    pub fn start_toggle(&mut self, enabled: bool) -> Option<ThreadRetentionSettingsSaveAction> {
        self.admit_toggle(enabled)
    }

    /// Parses and commits the current number-input text.
    ///
    /// A non-ready state or an in-flight save is a no-op. An invalid parsed
    /// value reverts the text to the current policy. A valid unchanged value
    /// emits no action. A valid changed value starts a save without
    /// optimistically changing the policy or input text.
    #[must_use]
    pub fn commit_days(&mut self) -> Option<ThreadRetentionSettingsSaveAction> {
        let policy = self.policy()?;
        if self.saving {
            return None;
        }

        let Some(days) = parse_inactivity_days(&self.days_text) else {
            self.days_text = policy.inactivity_days.to_string();
            return None;
        };
        if days == policy.inactivity_days {
            return None;
        }

        self.saving = true;
        Some(ThreadRetentionSettingsSaveAction::SavePolicy {
            policy: ThreadRetentionPolicy {
                inactivity_days: days,
                ..policy
            },
        })
    }

    /// Commits days only for the exact browser key string `Enter`.
    #[must_use]
    pub fn commit_days_on_key(&mut self, key: &str) -> Option<ThreadRetentionSettingsSaveAction> {
        if key == "Enter" {
            self.commit_days()
        } else {
            None
        }
    }

    /// Alias for [`Self::commit_days_on_key`].
    #[must_use]
    pub fn commit_days_on_enter(&mut self, key: &str) -> Option<ThreadRetentionSettingsSaveAction> {
        self.commit_days_on_key(key)
    }

    /// Settles a save, always clearing the local in-flight bit.
    ///
    /// A failure also sets the exact save-failure title and clears the input,
    /// while a success changes no other local field. The controller's
    /// authoritative `Ready` or `Unverified` state remains a separate stream
    /// observation.
    pub fn finish_save(&mut self, outcome: ThreadRetentionSettingsSaveOutcome) {
        self.saving = false;
        if matches!(outcome, ThreadRetentionSettingsSaveOutcome::Failed) {
            THREAD_RETENTION_SETTINGS_SAVE_FAILURE_TITLE.clone_into(&mut self.failure_title);
            self.days_text.clear();
        }
    }

    /// Settles a successful save.
    pub fn save_succeeded(&mut self) {
        self.finish_save(ThreadRetentionSettingsSaveOutcome::Succeeded);
    }

    /// Settles a failed save with the exact title and cleared input.
    pub fn save_failed(&mut self) {
        self.finish_save(ThreadRetentionSettingsSaveOutcome::Failed);
    }

    /// Applies a failed save's controller state and local failure transition.
    pub fn apply_save_failure(&mut self) {
        self.apply_streamed_state(ThreadRetentionPolicyState::Unverified);
        self.save_failed();
    }
}

const JAVASCRIPT_MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Converts number-input text using the relevant JavaScript `Number` rules
/// and the protocol's inclusive inactivity-day bounds.
///
/// Empty and whitespace-only strings therefore become JavaScript `0` and are
/// rejected by the protocol range. Decimal and exponent notation are parsed
/// as JavaScript numbers; only finite safe integers in `1..=3650` are
/// returned. The hexadecimal, binary, and octal forms accepted by
/// JavaScript `Number` are handled as well.
#[must_use]
pub fn parse_inactivity_days(input: &str) -> Option<ThreadRetentionInactivityDays> {
    let number = javascript_number(input)?;
    if !is_javascript_safe_integer(number)
        || number < f64::from(THREAD_RETENTION_MIN_INACTIVITY_DAYS)
        || number > f64::from(THREAD_RETENTION_MAX_INACTIVITY_DAYS)
    {
        return None;
    }

    // The checks above prove that this conversion is integral, non-negative,
    // and within the destination type's range.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(number as ThreadRetentionInactivityDays)
}

fn javascript_number(input: &str) -> Option<f64> {
    let trimmed = input.trim_matches(is_javascript_whitespace);
    if trimmed.is_empty() {
        return Some(0.0);
    }

    let (digits, radix) = if let Some(digits) = trimmed.strip_prefix("0x") {
        (digits, 16)
    } else if let Some(digits) = trimmed.strip_prefix("0X") {
        (digits, 16)
    } else if let Some(digits) = trimmed.strip_prefix("0b") {
        (digits, 2)
    } else if let Some(digits) = trimmed.strip_prefix("0B") {
        (digits, 2)
    } else if let Some(digits) = trimmed.strip_prefix("0o") {
        (digits, 8)
    } else if let Some(digits) = trimmed.strip_prefix("0O") {
        (digits, 8)
    } else {
        return trimmed.parse::<f64>().ok();
    };

    if digits.is_empty() {
        return None;
    }

    // Any radix value above u32::MAX is already outside the accepted day
    // range, so retaining it is unnecessary for this bounded parser.
    u32::from_str_radix(digits, radix).ok().map(f64::from)
}

fn is_javascript_whitespace(character: char) -> bool {
    character.is_whitespace() || character == '\u{feff}'
}

#[allow(clippy::float_cmp)]
fn is_javascript_safe_integer(number: f64) -> bool {
    if !number.is_finite() || number.abs() > JAVASCRIPT_MAX_SAFE_INTEGER {
        return false;
    }

    // This is deliberately an exact zero comparison: Number.isSafeInteger
    // rejects values whose represented fractional part is non-zero.
    number.fract() == 0.0
}
