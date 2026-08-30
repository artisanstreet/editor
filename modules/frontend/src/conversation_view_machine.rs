//! Conversation view state machines.
//!
//! Two pure synchronous owners replace ad-hoc booleans and racing
//! scroll-settlement callbacks. The legacy TypeScript/Svelte
//! implementation is a parity oracle only.
//!
//! * Disclosure controller — one per expandable work/compaction/change-set card.
//! * Viewport controller — one per rendered thread.
//!
//! Both owners are synchronous, do not own durable projection, turn
//! state, steering, DOM/GPUI handles, measurements, clocks, or actual
//! scrolling. Outer adapters supply explicit events and execute typed
//! effects. Generated Statig internals are hidden behind small public
//! controller APIs.

#![allow(clippy::module_name_repetitions)]

use statig::blocking;

// ---------------------------------------------------------------------------
// Disclosure
// ---------------------------------------------------------------------------

/// The single closed disclosure view exposed to callers.
///
/// Never `default_open` plus `user_override` booleans.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Disclosure {
    Open,
    Closed,
}

impl Disclosure {
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Closed)
    }
}

/// The four modeled disclosure states.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DisclosureState {
    AutoOpen,
    AutoClosed,
    UserOpen,
    UserClosed,
}

impl DisclosureState {
    #[must_use]
    pub const fn disclosure(self) -> Disclosure {
        match self {
            Self::AutoOpen | Self::UserOpen => Disclosure::Open,
            Self::AutoClosed | Self::UserClosed => Disclosure::Closed,
        }
    }

    #[must_use]
    pub const fn is_user(self) -> bool {
        matches!(self, Self::UserOpen | Self::UserClosed)
    }

    #[must_use]
    pub const fn is_auto(self) -> bool {
        matches!(self, Self::AutoOpen | Self::AutoClosed)
    }
}

/// Events that drive the disclosure controller.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DisclosureEvent {
    WorkBecameActive,
    WorkSettledSuccessfully,
    WorkFailedOrInterrupted,
    UserToggle,
    UserOpen,
    UserClose,
    Removed,
}

/// Typed retirement effect for optional item removal.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DisclosureEffect {
    None,
    Retired,
}

// Hidden Statig machine for disclosure. The public controller below is the
// only API that callers use; this module demonstrates correct 0.4.1
// blocking macro usage without leaking generated types.
#[allow(dead_code)]
mod disclosure_statig {
    use super::DisclosureEvent as Event;
    use super::DisclosureState as PublicState;
    use statig::blocking::{
        self,
        Outcome::{Handled, Transition},
    };

    #[derive(Default)]
    pub(super) struct Machine {
        pub state: PublicState,
        pub retired: bool,
    }

    #[blocking::state_machine(
        initial = "State::auto_open()",
        state(derive(Debug)),
        superstate(derive(Debug))
    )]
    impl Machine {
        #[state]
        fn auto_open(&mut self, event: &Event) -> Outcome<State> {
            match event {
                Event::WorkSettledSuccessfully | Event::WorkFailedOrInterrupted => {
                    Transition(State::auto_closed())
                }
                Event::UserToggle => Transition(State::user_closed()),
                Event::UserOpen => Transition(State::user_open()),
                Event::UserClose => Transition(State::user_closed()),
                Event::Removed => Transition(State::auto_open()),
                Event::WorkBecameActive => Handled,
            }
        }

        #[state]
        fn auto_closed(&mut self, event: &Event) -> Outcome<State> {
            match event {
                Event::WorkBecameActive => Transition(State::auto_open()),
                Event::UserToggle => Transition(State::user_open()),
                Event::UserOpen => Transition(State::user_open()),
                Event::UserClose => Transition(State::user_closed()),
                Event::Removed => Transition(State::auto_closed()),
                Event::WorkSettledSuccessfully | Event::WorkFailedOrInterrupted => Handled,
            }
        }

        #[state]
        fn user_open(&mut self, event: &Event) -> Outcome<State> {
            match event {
                Event::UserToggle => Transition(State::user_closed()),
                Event::UserClose => Transition(State::user_closed()),
                Event::UserOpen => Handled,
                Event::Removed => Transition(State::user_open()),
                Event::WorkBecameActive
                | Event::WorkSettledSuccessfully
                | Event::WorkFailedOrInterrupted => Handled,
            }
        }

        #[state]
        fn user_closed(&mut self, event: &Event) -> Outcome<State> {
            match event {
                Event::UserToggle => Transition(State::user_open()),
                Event::UserOpen => Transition(State::user_open()),
                Event::UserClose => Handled,
                Event::Removed => Transition(State::user_closed()),
                Event::WorkBecameActive
                | Event::WorkSettledSuccessfully
                | Event::WorkFailedOrInterrupted => Handled,
            }
        }
    }
}

/// Pure synchronous disclosure controller.
///
/// Frozen rules:
/// * active work defaults open;
/// * terminal work defaults closed only while still under automatic policy;
/// * any explicit user choice wins over later automatic lifecycle changes;
/// * user choice changes only on another explicit user action;
/// * removal is terminal or emits a typed retirement effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisclosureController {
    state: DisclosureState,
    retired: bool,
}

impl DisclosureController {
    /// Creates a disclosure controller seeded from liveness.
    ///
    /// Active work starts [`DisclosureState::AutoOpen`]; settled history
    /// starts [`DisclosureState::AutoClosed`].
    #[must_use]
    pub const fn new(is_working: bool) -> Self {
        let state = if is_working {
            DisclosureState::AutoOpen
        } else {
            DisclosureState::AutoClosed
        };
        Self {
            state,
            retired: false,
        }
    }

    /// Creates a controller from an explicit state (useful for tests).
    #[must_use]
    pub const fn from_state(state: DisclosureState) -> Self {
        Self {
            state,
            retired: false,
        }
    }

    #[must_use]
    pub const fn state(&self) -> DisclosureState {
        self.state
    }

    #[must_use]
    pub const fn disclosure(&self) -> Disclosure {
        self.state.disclosure()
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(
            self.state,
            DisclosureState::AutoOpen | DisclosureState::UserOpen
        )
    }

    #[must_use]
    pub const fn is_retired(&self) -> bool {
        self.retired
    }

    #[must_use]
    pub const fn is_user_controlled(&self) -> bool {
        self.state.is_user()
    }

    /// Applies one disclosure event and returns a typed effect.
    pub fn handle(&mut self, event: DisclosureEvent) -> DisclosureEffect {
        if self.retired {
            return DisclosureEffect::None;
        }
        match event {
            DisclosureEvent::Removed => {
                self.retired = true;
                return DisclosureEffect::Retired;
            }
            DisclosureEvent::WorkBecameActive => {
                if self.state == DisclosureState::AutoClosed {
                    self.state = DisclosureState::AutoOpen;
                }
            }
            DisclosureEvent::WorkSettledSuccessfully | DisclosureEvent::WorkFailedOrInterrupted => {
                if self.state == DisclosureState::AutoOpen {
                    self.state = DisclosureState::AutoClosed;
                }
            }
            DisclosureEvent::UserToggle => {
                self.state = match self.state {
                    DisclosureState::AutoOpen | DisclosureState::UserOpen => {
                        DisclosureState::UserClosed
                    }
                    DisclosureState::AutoClosed | DisclosureState::UserClosed => {
                        DisclosureState::UserOpen
                    }
                };
            }
            DisclosureEvent::UserOpen => {
                self.state = DisclosureState::UserOpen;
            }
            DisclosureEvent::UserClose => {
                self.state = DisclosureState::UserClosed;
            }
        }
        DisclosureEffect::None
    }
}

// ---------------------------------------------------------------------------
// Viewport
// ---------------------------------------------------------------------------

/// Checked monotonic command generation for fenced scroll commands.
///
/// Never wraps; overflow is a typed refusal.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ViewportGeneration(pub u64);

impl ViewportGeneration {
    pub const INITIAL: Self = Self(0);

    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// The exact anchor preserved by [`ViewportState::Anchored`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ViewportAnchor {
    pub anchor_id: String,
    pub offset: i32,
}

impl ViewportAnchor {
    #[must_use]
    pub fn new(anchor_id: impl Into<String>, offset: i32) -> Self {
        Self {
            anchor_id: anchor_id.into(),
            offset,
        }
    }
}

/// Explicit viewport states.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ViewportState {
    Following,
    Anchored { anchor_id: String, offset: i32 },
    Detached,
    Scrolling { generation: ViewportGeneration },
    Settling { generation: ViewportGeneration },
    Closed,
}

impl ViewportState {
    #[must_use]
    pub const fn is_following(&self) -> bool {
        matches!(self, Self::Following)
    }

    #[must_use]
    pub const fn is_detached(&self) -> bool {
        matches!(self, Self::Detached)
    }

    #[must_use]
    pub const fn is_anchored(&self) -> bool {
        matches!(self, Self::Anchored { .. })
    }

    #[must_use]
    pub const fn is_scrolling(&self) -> bool {
        matches!(self, Self::Scrolling { .. })
    }

    #[must_use]
    pub const fn is_settling(&self) -> bool {
        matches!(self, Self::Settling { .. })
    }

    #[must_use]
    pub const fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }
}

/// Viewport lifecycle events.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ViewportEvent {
    ExtentChanged,
    UserScrolled { at_bottom: bool },
    AnchorObserved { anchor_id: String, offset: i32 },
    JumpToBottomRequested,
    ProgrammaticScrollStarted { generation: ViewportGeneration },
    ScrollCompleted { generation: ViewportGeneration },
    LayoutSettled,
    AnchorRemoved { anchor_id: String },
    OwnerClosed,
}

impl ViewportEvent {
    #[must_use]
    pub fn anchor_observed(anchor_id: impl Into<String>, offset: i32) -> Self {
        Self::AnchorObserved {
            anchor_id: anchor_id.into(),
            offset,
        }
    }

    #[must_use]
    pub fn anchor_removed(anchor_id: impl Into<String>) -> Self {
        Self::AnchorRemoved {
            anchor_id: anchor_id.into(),
        }
    }

    #[must_use]
    pub const fn scroll_completed(generation: ViewportGeneration) -> Self {
        Self::ScrollCompleted { generation }
    }

    #[must_use]
    pub const fn programmatic_scroll_started(generation: ViewportGeneration) -> Self {
        Self::ProgrammaticScrollStarted { generation }
    }
}

/// Why a scroll completion was refused.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CompletionRejection {
    StaleGeneration,
    NoActiveScroll,
    GenerationExhausted,
}

/// Typed viewport effects.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ViewportEffect {
    None,
    RequestBottomScroll {
        generation: ViewportGeneration,
    },
    RequestAnchorRestore {
        anchor_id: String,
        offset: i32,
        generation: ViewportGeneration,
    },
    ShowJumpToLatest,
    HideJumpToLatest,
    InvalidateRender,
    CompletionRejected {
        generation: ViewportGeneration,
        reason: CompletionRejection,
    },
    GenerationExhausted,
}

// Hidden Statig machine for viewport. Demonstrates hierarchical states and
// generation fencing without leaking generated types. The public
// `ViewportController` is the only caller-visible API.
#[allow(dead_code)]
mod viewport_statig {
    use super::ViewportEvent as Event;
    use super::ViewportGeneration;
    use super::ViewportState as PublicState;
    use statig::blocking::{
        self,
        Outcome::{Handled, Transition},
    };

    #[derive(Default)]
    pub(super) struct Machine {
        pub state: PublicState,
        pub generation: ViewportGeneration,
    }

    #[blocking::state_machine(
        initial = "State::following()",
        state(derive(Debug)),
        superstate(derive(Debug))
    )]
    impl Machine {
        #[state]
        fn following(&mut self, event: &Event) -> Outcome<State> {
            match event {
                Event::UserScrolled { at_bottom: false } => Transition(State::detached()),
                Event::AnchorObserved { anchor_id, offset } => {
                    let _ = (anchor_id, offset);
                    Transition(State::anchored())
                }
                Event::JumpToBottomRequested => Transition(State::scrolling()),
                Event::OwnerClosed => Transition(State::closed()),
                _ => Handled,
            }
        }

        #[state]
        fn anchored(&mut self, event: &Event) -> Outcome<State> {
            match event {
                Event::AnchorRemoved { .. } => Transition(State::detached()),
                Event::UserScrolled { at_bottom: false } => Transition(State::detached()),
                Event::JumpToBottomRequested => Transition(State::scrolling()),
                Event::OwnerClosed => Transition(State::closed()),
                _ => Handled,
            }
        }

        #[state]
        fn detached(&mut self, event: &Event) -> Outcome<State> {
            match event {
                Event::UserScrolled { at_bottom: true } => Transition(State::following()),
                Event::JumpToBottomRequested => Transition(State::scrolling()),
                Event::OwnerClosed => Transition(State::closed()),
                _ => Handled,
            }
        }

        #[state]
        fn scrolling(&mut self, event: &Event) -> Outcome<State> {
            match event {
                Event::ScrollCompleted { .. } => Transition(State::settling()),
                Event::OwnerClosed => Transition(State::closed()),
                _ => Handled,
            }
        }

        #[state]
        fn settling(&mut self, event: &Event) -> Outcome<State> {
            match event {
                Event::LayoutSettled => Transition(State::following()),
                Event::OwnerClosed => Transition(State::closed()),
                _ => Handled,
            }
        }

        #[state]
        fn closed(&mut self, _event: &Event) -> Outcome<State> {
            Handled
        }
    }
}

/// Pure synchronous conversation viewport controller.
///
/// Frozen rules:
/// * only `Following` automatically follows streaming/appended content;
/// * `Detached` never jumps because a token, work card, or changed-files block arrived;
/// * `Anchored` restores the exact item and offset across compaction/virtualization
///   changes when it still exists;
/// * if the anchor is removed, transition deterministically to detached and show
///   jump-to-latest rather than selecting a neighboring message;
/// * jump-to-bottom begins one fenced scroll command and eventually returns to
///   following only after matching completion plus settlement;
/// * duplicate/stale completion does not clear an active newer scroll;
/// * closed is terminal if modeled;
/// * no viewport calculation reads a clock or actual GPUI geometry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewportController {
    state: ViewportState,
    generation: ViewportGeneration,
}

impl Default for ViewportController {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewportController {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: ViewportState::Following,
            generation: ViewportGeneration::INITIAL,
        }
    }

    /// Creates a controller anchored to an exact item and signed offset.
    #[must_use]
    pub fn anchored(anchor_id: impl Into<String>, offset: i32) -> Self {
        Self {
            state: ViewportState::Anchored {
                anchor_id: anchor_id.into(),
                offset,
            },
            generation: ViewportGeneration::INITIAL,
        }
    }

    #[must_use]
    pub fn state(&self) -> &ViewportState {
        &self.state
    }

    #[must_use]
    pub const fn generation(&self) -> ViewportGeneration {
        self.generation
    }

    /// Test-only helper to set the monotonic generation directly.
    #[must_use]
    pub fn with_generation(mut self, generation: ViewportGeneration) -> Self {
        self.generation = generation;
        self
    }

    pub fn set_generation(&mut self, generation: ViewportGeneration) {
        self.generation = generation;
    }

    fn next_generation(&mut self) -> Result<ViewportGeneration, ViewportEffect> {
        let Some(next) = self.generation.next() else {
            return Err(ViewportEffect::GenerationExhausted);
        };
        self.generation = next;
        Ok(next)
    }

    /// Applies one viewport event and returns typed effects.
    pub fn handle(&mut self, event: ViewportEvent) -> Vec<ViewportEffect> {
        if matches!(self.state, ViewportState::Closed) {
            return vec![ViewportEffect::None];
        }

        match event {
            ViewportEvent::OwnerClosed => {
                self.state = ViewportState::Closed;
                return vec![ViewportEffect::InvalidateRender];
            }
            ViewportEvent::ExtentChanged => return self.handle_extent_changed(),
            ViewportEvent::UserScrolled { at_bottom } => {
                return self.handle_user_scrolled(at_bottom);
            }
            ViewportEvent::AnchorObserved { anchor_id, offset } => {
                self.state = ViewportState::Anchored { anchor_id, offset };
                return vec![ViewportEffect::InvalidateRender];
            }
            ViewportEvent::JumpToBottomRequested => return self.handle_jump_requested(),
            ViewportEvent::ProgrammaticScrollStarted { generation } => {
                return self.handle_programmatic_started(generation);
            }
            ViewportEvent::ScrollCompleted { generation } => {
                return self.handle_completion(generation);
            }
            ViewportEvent::LayoutSettled => return self.handle_layout_settled(),
            ViewportEvent::AnchorRemoved { anchor_id } => {
                return self.handle_anchor_removed(anchor_id);
            }
        }
    }

    fn handle_extent_changed(&mut self) -> Vec<ViewportEffect> {
        match &self.state {
            ViewportState::Following => {
                let generation_value = match self.next_generation() {
                    Ok(g) => g,
                    Err(e) => return vec![e],
                };
                vec![ViewportEffect::RequestBottomScroll {
                    generation: generation_value,
                }]
            }
            ViewportState::Anchored { anchor_id, offset } => {
                let anchor_id = anchor_id.clone();
                let offset = *offset;
                let generation_value = match self.next_generation() {
                    Ok(g) => g,
                    Err(e) => return vec![e],
                };
                vec![ViewportEffect::RequestAnchorRestore {
                    anchor_id,
                    offset,
                    generation: generation_value,
                }]
            }
            ViewportState::Detached
            | ViewportState::Scrolling { .. }
            | ViewportState::Settling { .. }
            | ViewportState::Closed => vec![ViewportEffect::None],
        }
    }

    fn handle_user_scrolled(&mut self, at_bottom: bool) -> Vec<ViewportEffect> {
        if at_bottom {
            let was_detached = matches!(self.state, ViewportState::Detached);
            self.state = ViewportState::Following;
            if was_detached {
                return vec![
                    ViewportEffect::HideJumpToLatest,
                    ViewportEffect::InvalidateRender,
                ];
            }
            return vec![ViewportEffect::HideJumpToLatest];
        }
        // User intentionally scrolled away.
        match &self.state {
            ViewportState::Following | ViewportState::Anchored { .. } => {
                self.state = ViewportState::Detached;
                vec![
                    ViewportEffect::ShowJumpToLatest,
                    ViewportEffect::InvalidateRender,
                ]
            }
            ViewportState::Detached
            | ViewportState::Scrolling { .. }
            | ViewportState::Settling { .. }
            | ViewportState::Closed => vec![ViewportEffect::None],
        }
    }

    fn handle_jump_requested(&mut self) -> Vec<ViewportEffect> {
        let generation_value = match self.next_generation() {
            Ok(g) => g,
            Err(e) => return vec![e],
        };
        self.state = ViewportState::Scrolling {
            generation: generation_value,
        };
        vec![
            ViewportEffect::RequestBottomScroll {
                generation: generation_value,
            },
            ViewportEffect::HideJumpToLatest,
            ViewportEffect::InvalidateRender,
        ]
    }

    fn handle_programmatic_started(
        &mut self,
        generation: ViewportGeneration,
    ) -> Vec<ViewportEffect> {
        // Only accept if it matches the current scrolling generation or if we
        // are not yet scrolling we adopt it as a fenced command.
        match &self.state {
            ViewportState::Scrolling { generation: active } if *active == generation => {
                vec![ViewportEffect::None]
            }
            ViewportState::Scrolling { generation: active } => vec![
                ViewportEffect::CompletionRejected {
                    generation,
                    reason: CompletionRejection::StaleGeneration,
                },
                ViewportEffect::InvalidateRender,
            ],
            _ => {
                // Allow explicit start from detached/following/anchored by
                // adopting the generation if it is the expected next.
                if generation == self.generation {
                    self.state = ViewportState::Scrolling { generation };
                    vec![ViewportEffect::InvalidateRender]
                } else {
                    vec![ViewportEffect::CompletionRejected {
                        generation,
                        reason: CompletionRejection::StaleGeneration,
                    }]
                }
            }
        }
    }

    fn handle_completion(&mut self, generation: ViewportGeneration) -> Vec<ViewportEffect> {
        match &self.state {
            ViewportState::Scrolling { generation: active } if *active == generation => {
                self.state = ViewportState::Settling { generation };
                vec![ViewportEffect::InvalidateRender]
            }
            ViewportState::Scrolling { .. } | ViewportState::Settling { .. } => {
                vec![ViewportEffect::CompletionRejected {
                    generation,
                    reason: CompletionRejection::StaleGeneration,
                }]
            }
            _ => vec![ViewportEffect::CompletionRejected {
                generation,
                reason: CompletionRejection::NoActiveScroll,
            }],
        }
    }

    fn handle_layout_settled(&mut self) -> Vec<ViewportEffect> {
        match &self.state {
            ViewportState::Settling { .. } => {
                self.state = ViewportState::Following;
                vec![
                    ViewportEffect::HideJumpToLatest,
                    ViewportEffect::InvalidateRender,
                ]
            }
            _ => vec![ViewportEffect::None],
        }
    }

    fn handle_anchor_removed(&mut self, anchor_id: String) -> Vec<ViewportEffect> {
        let should_detach = match &self.state {
            ViewportState::Anchored {
                anchor_id: current, ..
            } => current == &anchor_id,
            _ => false,
        };
        if should_detach {
            self.state = ViewportState::Detached;
            return vec![
                ViewportEffect::ShowJumpToLatest,
                ViewportEffect::InvalidateRender,
            ];
        }
        vec![ViewportEffect::None]
    }
}
