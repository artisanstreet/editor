//! Dependency-free state and adapter actions for the onboarding setup label.
//!
//! The browser component uses a DOM element, a computed CSS custom property,
//! Svelte's tick boundary, and a real timeout. This module owns none of those
//! runtime facilities. The caller supplies the already-parsed CSS duration and
//! attachment observations, then executes the returned actions in order.
//!
//! A timer callback must pass its opaque token back to
//! [`SetupLabelTransitionController::fire_timer`]. Only the currently pending
//! token can settle the replacement; a late callback is an empty transition.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// The JavaScript fallback used when the parsed CSS duration is absent, zero,
/// non-finite, or otherwise not a usable finite nonzero number.
pub const DEFAULT_TEXT_SWAP_DURATION_MS: f64 = 150.0;

/// The class added while the old setup label exits.
pub const TEXT_SWAP_EXIT_CLASS: &str = "is-exit";

/// The class added after the tick boundary while the new setup label enters.
pub const TEXT_SWAP_ENTER_START_CLASS: &str = "is-enter-start";

/// The owned label and optional email displayed by the setup-label component.
///
/// Neither field is trimmed, folded, decoded, or otherwise normalized. In
/// particular, [`Self::rendered_text`] retains a present-but-empty email's
/// separator space exactly as the Svelte interpolation does.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupLabelDisplayedValue {
    /// The exact setup label text.
    pub label: String,
    /// The exact optional email text.
    pub email: Option<String>,
}

impl SetupLabelDisplayedValue {
    /// Creates an owned displayed value without changing either supplied
    /// string.
    pub fn new(label: impl Into<String>, email: Option<String>) -> Self {
        Self {
            label: label.into(),
            email,
        }
    }

    /// Creates an owned label-only value.
    pub fn label_only(label: impl Into<String>) -> Self {
        Self::new(label, None)
    }

    /// Creates an owned value with a present email.
    pub fn with_email(label: impl Into<String>, email: impl Into<String>) -> Self {
        Self::new(label, Some(email.into()))
    }

    /// Returns the exact label text.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the exact optional email text.
    #[must_use]
    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    /// Concatenates the value exactly as the legacy component renders it.
    ///
    /// A label is always emitted. When `email` is `Some`, one ASCII space is
    /// emitted before the email, including when that email is empty.
    #[must_use]
    pub fn rendered_text(&self) -> String {
        let email_len = self.email.as_ref().map_or(0, String::len);
        let separator_len = usize::from(self.email.is_some());
        let mut rendered = String::with_capacity(self.label.len() + separator_len + email_len);
        rendered.push_str(&self.label);
        if let Some(email) = &self.email {
            rendered.push(' ');
            rendered.push_str(email);
        }
        rendered
    }
}

/// Short alias for callers that use the generic displayed-value vocabulary.
pub type DisplayedValue = SetupLabelDisplayedValue;

/// Short alias for callers that use the setup-label value vocabulary.
pub type SetupLabelValue = SetupLabelDisplayedValue;

/// Applies the legacy `parseFloat(value) || 150` result at the adapter
/// boundary.
///
/// The input is the already-parsed duration in milliseconds. A finite,
/// nonzero value is retained exactly, including a finite negative value. A
/// missing value, zero (including negative zero), `NaN`, or infinity uses the
/// 150 ms fallback. This function does not inspect CSS or query a document.
#[must_use]
pub fn effective_text_swap_duration_ms(css_duration_ms: Option<f64>) -> f64 {
    match css_duration_ms {
        Some(duration) if duration.is_finite() && duration != 0.0 => duration,
        _ => DEFAULT_TEXT_SWAP_DURATION_MS,
    }
}

/// The two classes manipulated by the legacy text-swap lifecycle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SetupLabelTransitionClass {
    /// The old value is leaving.
    Exit,
    /// The new value is beginning its enter transition.
    EnterStart,
}

impl SetupLabelTransitionClass {
    /// Returns the exact DOM class name represented by this typed class.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exit => TEXT_SWAP_EXIT_CLASS,
            Self::EnterStart => TEXT_SWAP_ENTER_START_CLASS,
        }
    }
}

/// An opaque, monotonically allocated timer identity.
///
/// The adapter must return this token from its timer callback. Tokens start at
/// one and never wrap. If the finite token space is exhausted, the controller
/// falls back to an immediate replacement rather than reusing an old token.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SetupLabelTimerToken(u64);

impl SetupLabelTimerToken {
    /// Creates a token from an adapter-owned numeric identity.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric identity carried by this token.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for SetupLabelTimerToken {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

/// The pending replacement and token held while the exit timer is live.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupLabelPendingTimer {
    /// The token the adapter must provide when this timer fires.
    pub token: SetupLabelTimerToken,
    /// The owned value to install when this timer fires.
    pub value: SetupLabelDisplayedValue,
}

/// The controller's durable lifecycle state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SetupLabelTransitionState {
    /// No observation has initialized rendered state.
    Uninitialized,
    /// A value is rendered and no replacement timer is pending.
    Displayed {
        /// The exact currently rendered value.
        value: SetupLabelDisplayedValue,
    },
    /// The old value remains rendered while the exit timer is pending.
    WaitingForTimer {
        /// The exact value still rendered during the exit phase.
        rendered: SetupLabelDisplayedValue,
        /// The timer token and replacement value awaiting the callback.
        pending: SetupLabelPendingTimer,
    },
}

impl SetupLabelTransitionState {
    /// Returns the currently rendered value, if the first observation has
    /// happened.
    #[must_use]
    pub fn rendered_value(&self) -> Option<&SetupLabelDisplayedValue> {
        match self {
            Self::Uninitialized => None,
            Self::Displayed { value } => Some(value),
            Self::WaitingForTimer { rendered, .. } => Some(rendered),
        }
    }

    /// Returns the pending timer, if the state is in its exit phase.
    #[must_use]
    pub fn pending_timer(&self) -> Option<&SetupLabelPendingTimer> {
        match self {
            Self::WaitingForTimer { pending, .. } => Some(pending),
            Self::Uninitialized | Self::Displayed { .. } => None,
        }
    }

    /// Returns whether the first value has initialized rendered state.
    #[must_use]
    pub const fn is_initialized(&self) -> bool {
        !matches!(self, Self::Uninitialized)
    }

    /// Returns whether a replacement timer is pending.
    #[must_use]
    pub const fn is_waiting_for_timer(&self) -> bool {
        matches!(self, Self::WaitingForTimer { .. })
    }
}

/// One ordered effect for the setup-label adapter to execute.
///
/// Actions are returned in execution order. The policy never performs the
/// represented timer, tick, class, layout, or rendered-value operation.
#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub enum SetupLabelTransitionAction {
    /// Cancel the exact pending timer identified by `token`.
    CancelTimer {
        /// The timer identity to cancel.
        token: SetupLabelTimerToken,
    },
    /// Schedule one callback for the exact token and delay.
    ScheduleTimer {
        /// The token the callback must return to the controller.
        token: SetupLabelTimerToken,
        /// The delay supplied to the host timer adapter, in milliseconds.
        delay_ms: f64,
    },
    /// Add one typed transition class to the attached element.
    AddClass {
        /// The class to add.
        class: SetupLabelTransitionClass,
    },
    /// Remove one typed transition class from the attached element.
    RemoveClass {
        /// The class to remove.
        class: SetupLabelTransitionClass,
    },
    /// Request one Svelte-tick-equivalent layout boundary from the host.
    RequestTick,
    /// Request one synchronous layout read from the host.
    RequestLayoutRead,
    /// Replace the value exposed to the renderer with this owned value.
    SetRenderedValue {
        /// The exact value to expose after this action executes.
        value: SetupLabelDisplayedValue,
    },
}

/// An ordered, possibly empty collection of setup-label adapter actions.
#[must_use = "execute the returned setup-label adapter actions"]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetupLabelTransition {
    actions: Vec<SetupLabelTransitionAction>,
}

impl SetupLabelTransition {
    /// Creates a transition from actions that are already in execution order.
    pub fn new(actions: Vec<SetupLabelTransitionAction>) -> Self {
        Self { actions }
    }

    /// Returns the actions without transferring ownership.
    pub fn actions(&self) -> &[SetupLabelTransitionAction] {
        &self.actions
    }

    /// Consumes the transition and returns its actions in execution order.
    #[must_use]
    pub fn into_actions(self) -> Vec<SetupLabelTransitionAction> {
        self.actions
    }

    /// Returns whether no adapter operation was admitted.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Returns the number of ordered adapter actions.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.actions.len()
    }
}

/// Dependency-free controller for one setup-label text-swap lifecycle.
///
/// The controller starts detached and uninitialized. Callers should report
/// element attachment with [`Self::attach_element`], feed each observed value
/// to [`Self::observe`], execute its ordered actions, and return timer tokens
/// through [`Self::fire_timer`].
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupLabelTransitionController {
    state: SetupLabelTransitionState,
    element_attached: bool,
    next_timer_token: u64,
}

impl Default for SetupLabelTransitionController {
    fn default() -> Self {
        Self::new()
    }
}

impl SetupLabelTransitionController {
    /// Creates a detached controller with no observed value.
    pub const fn new() -> Self {
        Self {
            state: SetupLabelTransitionState::Uninitialized,
            element_attached: false,
            next_timer_token: 0,
        }
    }

    /// Returns the complete durable lifecycle state.
    #[must_use]
    pub const fn state(&self) -> &SetupLabelTransitionState {
        &self.state
    }

    /// Returns the currently rendered value, if initialized.
    #[must_use]
    pub fn rendered_value(&self) -> Option<&SetupLabelDisplayedValue> {
        self.state.rendered_value()
    }

    /// Returns the currently rendered concatenated text, if initialized.
    #[must_use]
    pub fn rendered_text(&self) -> Option<String> {
        self.rendered_value()
            .map(SetupLabelDisplayedValue::rendered_text)
    }

    /// Returns whether an element is currently attached.
    #[must_use]
    pub const fn element_attached(&self) -> bool {
        self.element_attached
    }

    /// Returns the current pending timer, if the controller is in its exit
    /// phase.
    #[must_use]
    pub fn pending_timer(&self) -> Option<&SetupLabelPendingTimer> {
        self.state.pending_timer()
    }

    /// Returns the numeric value of the next allocated timer token.
    ///
    /// The initial value is zero; the first scheduled token is one.
    #[must_use]
    pub const fn next_timer_token(&self) -> u64 {
        self.next_timer_token
    }

    /// Reports that the element is attached.
    ///
    /// Attachment itself has no class, timer, layout, or rendered-value
    /// effect. The returned transition is therefore empty.
    pub fn attach_element(&mut self) -> SetupLabelTransition {
        self.element_attached = true;
        SetupLabelTransition::default()
    }

    /// Alias for [`Self::attach_element`].
    pub fn attach(&mut self) -> SetupLabelTransition {
        self.attach_element()
    }

    /// Reports that the element was detached and cancels any pending timer.
    ///
    /// The old rendered value remains rendered. No class-removal action is
    /// fabricated because the legacy cleanup only clears the timeout; the DOM
    /// element is no longer available to mutate.
    pub fn detach_element(&mut self) -> SetupLabelTransition {
        self.element_attached = false;
        let mut actions = Vec::new();
        self.cancel_pending_timer_into(&mut actions);
        SetupLabelTransition::new(actions)
    }

    /// Alias for [`Self::detach_element`].
    pub fn detach(&mut self) -> SetupLabelTransition {
        self.detach_element()
    }

    /// Reports component unmount and cancels any pending timer.
    ///
    /// This is intentionally equivalent to detachment at this boundary. It
    /// remains possible to attach the controller again so a host can model a
    /// later lifecycle without constructing a timer or runtime here.
    pub fn unmount(&mut self) -> SetupLabelTransition {
        self.detach_element()
    }

    /// Sets attachment state, cancelling a pending timer when detaching.
    pub fn set_element_attached(&mut self, attached: bool) -> SetupLabelTransition {
        if attached {
            self.attach_element()
        } else {
            self.detach_element()
        }
    }

    /// Observes one exact label/email pair and returns its ordered effects.
    ///
    /// The first observation immediately sets rendered state. An unchanged
    /// pair is a no-op. A changed pair updates immediately while detached;
    /// while attached it adds `is-exit` and schedules one replacement timer
    /// using [`effective_text_swap_duration_ms`]. Any existing timer is
    /// cancelled first, even when the new observation then becomes a no-op.
    pub fn observe(
        &mut self,
        next: SetupLabelDisplayedValue,
        css_duration_ms: Option<f64>,
    ) -> SetupLabelTransition {
        let mut actions = Vec::new();
        self.cancel_pending_timer_into(&mut actions);

        if matches!(self.state, SetupLabelTransitionState::Uninitialized) {
            self.state = SetupLabelTransitionState::Displayed {
                value: next.clone(),
            };
            actions.push(SetupLabelTransitionAction::SetRenderedValue { value: next });
            return SetupLabelTransition::new(actions);
        }

        let is_unchanged = self
            .rendered_value()
            .is_some_and(|rendered| rendered == &next);
        if is_unchanged {
            return SetupLabelTransition::new(actions);
        }

        if !self.element_attached {
            self.state = SetupLabelTransitionState::Displayed {
                value: next.clone(),
            };
            actions.push(SetupLabelTransitionAction::SetRenderedValue { value: next });
            return SetupLabelTransition::new(actions);
        }

        let Some(rendered) = self.rendered_value().cloned() else {
            return SetupLabelTransition::new(actions);
        };
        let Some(token) = self.allocate_timer_token() else {
            self.state = SetupLabelTransitionState::Displayed {
                value: next.clone(),
            };
            actions.push(SetupLabelTransitionAction::SetRenderedValue { value: next });
            return SetupLabelTransition::new(actions);
        };
        self.state = SetupLabelTransitionState::WaitingForTimer {
            rendered,
            pending: SetupLabelPendingTimer { token, value: next },
        };
        actions.push(SetupLabelTransitionAction::AddClass {
            class: SetupLabelTransitionClass::Exit,
        });
        actions.push(SetupLabelTransitionAction::ScheduleTimer {
            token,
            delay_ms: effective_text_swap_duration_ms(css_duration_ms),
        });
        SetupLabelTransition::new(actions)
    }

    /// Cancels the current timer, if any, while retaining the old rendered
    /// value. This is the cleanup/supersession operation used internally by
    /// observation and detachment, and is also available to an adapter.
    pub fn cancel_pending_timer(&mut self) -> SetupLabelTransition {
        let mut actions = Vec::new();
        self.cancel_pending_timer_into(&mut actions);
        SetupLabelTransition::new(actions)
    }

    /// Applies one timer callback to the controller.
    ///
    /// A callback mutates state only when `token` is the current pending token.
    /// The current callback's actions are ordered exactly as the legacy
    /// lifecycle: set the rendered value, request one tick, remove `is-exit`,
    /// add `is-enter-start`, request one layout read, then remove
    /// `is-enter-start`.
    pub fn fire_timer(&mut self, token: SetupLabelTimerToken) -> SetupLabelTransition {
        let Some(pending) = self.state.pending_timer() else {
            return SetupLabelTransition::default();
        };
        if pending.token != token {
            return SetupLabelTransition::default();
        }

        let value = pending.value.clone();
        self.state = SetupLabelTransitionState::Displayed {
            value: value.clone(),
        };
        SetupLabelTransition::new(vec![
            SetupLabelTransitionAction::SetRenderedValue { value },
            SetupLabelTransitionAction::RequestTick,
            SetupLabelTransitionAction::RemoveClass {
                class: SetupLabelTransitionClass::Exit,
            },
            SetupLabelTransitionAction::AddClass {
                class: SetupLabelTransitionClass::EnterStart,
            },
            SetupLabelTransitionAction::RequestLayoutRead,
            SetupLabelTransitionAction::RemoveClass {
                class: SetupLabelTransitionClass::EnterStart,
            },
        ])
    }

    /// Alias for [`Self::fire_timer`] using callback-oriented naming.
    pub fn on_timer(&mut self, token: SetupLabelTimerToken) -> SetupLabelTransition {
        self.fire_timer(token)
    }

    fn allocate_timer_token(&mut self) -> Option<SetupLabelTimerToken> {
        let next = self.next_timer_token.checked_add(1)?;
        self.next_timer_token = next;
        Some(SetupLabelTimerToken::new(next))
    }

    fn cancel_pending_timer_into(&mut self, actions: &mut Vec<SetupLabelTransitionAction>) {
        let Some((token, rendered)) = (match &self.state {
            SetupLabelTransitionState::WaitingForTimer { rendered, pending } => {
                Some((pending.token, rendered.clone()))
            }
            SetupLabelTransitionState::Uninitialized
            | SetupLabelTransitionState::Displayed { .. } => None,
        }) else {
            return;
        };

        self.state = SetupLabelTransitionState::Displayed { value: rendered };
        actions.push(SetupLabelTransitionAction::CancelTimer { token });
    }
}
