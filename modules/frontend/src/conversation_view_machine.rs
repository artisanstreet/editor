//! Conversation view state machines.
//!
//! The public controllers own the blocking Statig machines below.  They are
//! synchronous, do not own durable projection, turn state, steering, DOM or
//! GPUI handles, measurements, clocks, or actual scrolling.  Outer adapters
//! supply explicit events and execute the typed effects returned by handlers.

#![allow(clippy::module_name_repetitions)]

use core::fmt;

use artisan_domain::ItemId;
use statig::blocking;

// ---------------------------------------------------------------------------
// Disclosure
// ---------------------------------------------------------------------------

/// The single closed disclosure view exposed to callers.
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

/// The modeled disclosure leaves.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DisclosureState {
    AutoOpen,
    AutoClosed,
    UserOpen,
    UserClosed,
    Retired,
}

impl DisclosureState {
    #[must_use]
    pub const fn disclosure(self) -> Disclosure {
        match self {
            Self::AutoOpen | Self::UserOpen => Disclosure::Open,
            Self::AutoClosed | Self::UserClosed | Self::Retired => Disclosure::Closed,
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

    #[must_use]
    pub const fn is_retired(self) -> bool {
        matches!(self, Self::Retired)
    }

    #[must_use]
    pub const fn is_user_controlled(self) -> bool {
        self.is_user()
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

/// Typed effect emitted by disclosure handlers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DisclosureEffect {
    None,
    Retired,
}

mod disclosure_statig {
    use super::{DisclosureEffect, DisclosureEvent, DisclosureState};
    use statig::blocking::{
        self, IntoStateMachineExt, Outcome,
        Outcome::{Handled, Transition},
    };

    #[derive(Default)]
    pub(super) struct Machine;

    pub(super) enum Event {
        Initialize { is_working: bool },
        Seed(DisclosureState),
        Public(DisclosureEvent),
    }

    pub(super) struct Context {
        effect: DisclosureEffect,
    }

    impl Default for Context {
        fn default() -> Self {
            Self {
                effect: DisclosureEffect::None,
            }
        }
    }

    impl Context {
        fn retire(&mut self) {
            self.effect = DisclosureEffect::Retired;
        }

        pub(super) fn into_effect(self) -> DisclosureEffect {
            self.effect
        }
    }

    #[blocking::state_machine(
        initial = "State::auto_open()",
        state(derive(Debug, PartialEq, Eq)),
        superstate(derive(Debug, PartialEq, Eq))
    )]
    impl Machine {
        #[state]
        fn auto_open(&mut self, context: &mut Context, event: &Event) -> Outcome<State> {
            match event {
                Event::Initialize { is_working: true } | Event::Seed(DisclosureState::AutoOpen) => {
                    Handled
                }
                Event::Initialize { is_working: false }
                | Event::Seed(DisclosureState::AutoClosed) => Transition(State::auto_closed()),
                Event::Seed(DisclosureState::UserOpen)
                | Event::Public(DisclosureEvent::UserOpen) => Transition(State::user_open()),
                Event::Seed(DisclosureState::UserClosed)
                | Event::Public(DisclosureEvent::UserToggle | DisclosureEvent::UserClose) => {
                    Transition(State::user_closed())
                }
                Event::Seed(DisclosureState::Retired) | Event::Public(DisclosureEvent::Removed) => {
                    context.retire();
                    Transition(State::retired())
                }
                Event::Public(DisclosureEvent::WorkBecameActive) => Handled,
                Event::Public(
                    DisclosureEvent::WorkSettledSuccessfully
                    | DisclosureEvent::WorkFailedOrInterrupted,
                ) => Transition(State::auto_closed()),
            }
        }

        #[state]
        fn auto_closed(&mut self, context: &mut Context, event: &Event) -> Outcome<State> {
            match event {
                Event::Initialize { .. } | Event::Seed(DisclosureState::AutoClosed) => Handled,
                Event::Seed(DisclosureState::AutoOpen)
                | Event::Public(DisclosureEvent::WorkBecameActive) => {
                    Transition(State::auto_open())
                }
                Event::Seed(DisclosureState::UserOpen)
                | Event::Public(DisclosureEvent::UserOpen | DisclosureEvent::UserToggle) => {
                    Transition(State::user_open())
                }
                Event::Seed(DisclosureState::UserClosed)
                | Event::Public(DisclosureEvent::UserClose) => Transition(State::user_closed()),
                Event::Seed(DisclosureState::Retired) | Event::Public(DisclosureEvent::Removed) => {
                    context.retire();
                    Transition(State::retired())
                }
                Event::Public(
                    DisclosureEvent::WorkSettledSuccessfully
                    | DisclosureEvent::WorkFailedOrInterrupted,
                ) => Handled,
            }
        }

        #[state]
        fn user_open(&mut self, context: &mut Context, event: &Event) -> Outcome<State> {
            match event {
                Event::Initialize { .. } | Event::Seed(DisclosureState::UserOpen) => Handled,
                Event::Seed(DisclosureState::AutoOpen)
                | Event::Seed(DisclosureState::AutoClosed)
                | Event::Seed(DisclosureState::UserClosed)
                | Event::Public(DisclosureEvent::UserToggle | DisclosureEvent::UserClose) => {
                    Transition(State::user_closed())
                }
                Event::Seed(DisclosureState::Retired) | Event::Public(DisclosureEvent::Removed) => {
                    context.retire();
                    Transition(State::retired())
                }
                Event::Public(
                    DisclosureEvent::WorkBecameActive
                    | DisclosureEvent::WorkSettledSuccessfully
                    | DisclosureEvent::WorkFailedOrInterrupted
                    | DisclosureEvent::UserOpen,
                ) => Handled,
            }
        }

        #[state]
        fn user_closed(&mut self, context: &mut Context, event: &Event) -> Outcome<State> {
            match event {
                Event::Initialize { .. } | Event::Seed(DisclosureState::UserClosed) => Handled,
                Event::Seed(DisclosureState::AutoOpen)
                | Event::Seed(DisclosureState::AutoClosed)
                | Event::Seed(DisclosureState::UserOpen)
                | Event::Public(DisclosureEvent::UserToggle | DisclosureEvent::UserOpen) => {
                    Transition(State::user_open())
                }
                Event::Seed(DisclosureState::Retired) | Event::Public(DisclosureEvent::Removed) => {
                    context.retire();
                    Transition(State::retired())
                }
                Event::Public(
                    DisclosureEvent::WorkBecameActive
                    | DisclosureEvent::WorkSettledSuccessfully
                    | DisclosureEvent::WorkFailedOrInterrupted
                    | DisclosureEvent::UserClose,
                ) => Handled,
            }
        }

        #[state]
        fn retired(&mut self, context: &mut Context, event: &Event) -> Outcome<State> {
            let _ = context;
            let _ = event;
            Handled
        }
    }

    pub(super) fn new_machine() -> blocking::StateMachine<Machine> {
        Machine::default().state_machine()
    }

    pub(super) fn public_state(state: &State) -> DisclosureState {
        match state {
            State::AutoOpen { .. } => DisclosureState::AutoOpen,
            State::AutoClosed { .. } => DisclosureState::AutoClosed,
            State::UserOpen { .. } => DisclosureState::UserOpen,
            State::UserClosed { .. } => DisclosureState::UserClosed,
            State::Retired { .. } => DisclosureState::Retired,
        }
    }
}

/// Public owner for one disclosure view.
pub struct DisclosureController {
    machine: blocking::StateMachine<disclosure_statig::Machine>,
}

impl DisclosureController {
    #[must_use]
    pub fn new(is_working: bool) -> Self {
        let mut controller = Self {
            machine: disclosure_statig::new_machine(),
        };
        let mut context = disclosure_statig::Context::default();
        controller.machine.init_with_context(&mut context);
        let _ = controller.dispatch(disclosure_statig::Event::Initialize { is_working });
        controller
    }

    /// Seeds a settled or user-controlled view through an initialization event.
    #[must_use]
    pub fn from_state(state: DisclosureState) -> Self {
        let mut controller = Self::new(true);
        let _ = controller.dispatch(disclosure_statig::Event::Seed(state));
        controller
    }

    #[must_use]
    pub fn state(&self) -> DisclosureState {
        disclosure_statig::public_state(self.machine.state())
    }

    #[must_use]
    pub fn disclosure(&self) -> Disclosure {
        self.state().disclosure()
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.disclosure().is_open()
    }

    #[must_use]
    pub fn is_retired(&self) -> bool {
        self.state().is_retired()
    }

    #[must_use]
    pub fn is_user_controlled(&self) -> bool {
        self.state().is_user()
    }

    /// Applies one disclosure event and returns its typed effect.
    pub fn handle(&mut self, event: DisclosureEvent) -> DisclosureEffect {
        self.dispatch(disclosure_statig::Event::Public(event))
    }

    fn dispatch(&mut self, event: disclosure_statig::Event) -> DisclosureEffect {
        let mut context = disclosure_statig::Context::default();
        self.machine.handle_with_context(&event, &mut context);
        context.into_effect()
    }
}

impl Clone for DisclosureController {
    fn clone(&self) -> Self {
        Self::from_state(self.state())
    }
}

impl fmt::Debug for DisclosureController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DisclosureController")
            .field("state", &self.state())
            .finish()
    }
}

impl PartialEq for DisclosureController {
    fn eq(&self, other: &Self) -> bool {
        self.state() == other.state()
    }
}

impl Eq for DisclosureController {}

// ---------------------------------------------------------------------------
// Viewport
// ---------------------------------------------------------------------------

/// Monotonic generation used to fence asynchronous viewport effects.
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

/// The exact domain item and visual offset needed to restore an anchor.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ViewportAnchor {
    pub anchor_id: ItemId,
    pub offset: i32,
}

impl ViewportAnchor {
    #[must_use]
    pub fn new(anchor_id: ItemId, offset: i32) -> Self {
        Self { anchor_id, offset }
    }
}

/// The viewport leaves.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ViewportState {
    Following,
    Anchored { anchor_id: ItemId, offset: i32 },
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
    pub const fn is_anchored(&self) -> bool {
        matches!(self, Self::Anchored { .. })
    }

    #[must_use]
    pub const fn is_detached(&self) -> bool {
        matches!(self, Self::Detached)
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

/// Events that drive the viewport controller.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ViewportEvent {
    ExtentChanged,
    UserScrolled { at_bottom: bool },
    AnchorObserved { anchor_id: ItemId, offset: i32 },
    JumpToBottomRequested,
    ProgrammaticScrollStarted { generation: ViewportGeneration },
    ScrollCompleted { generation: ViewportGeneration },
    LayoutSettled,
    AnchorRemoved { anchor_id: ItemId },
    OwnerClosed,
}

impl ViewportEvent {
    #[must_use]
    pub fn anchor_observed(anchor_id: ItemId, offset: i32) -> Self {
        Self::AnchorObserved { anchor_id, offset }
    }

    #[must_use]
    pub fn anchor_removed(anchor_id: ItemId) -> Self {
        Self::AnchorRemoved { anchor_id }
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

/// Why a completion or start was rejected.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CompletionRejection {
    StaleGeneration,
    NoActiveScroll,
    GenerationExhausted,
}

/// Typed viewport effects for an outer adapter to execute.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ViewportEffect {
    None,
    RequestBottomScroll {
        generation: ViewportGeneration,
    },
    RequestAnchorRestore {
        anchor_id: ItemId,
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

mod viewport_statig {
    use super::{
        CompletionRejection, ItemId, ViewportEffect, ViewportEvent, ViewportGeneration,
        ViewportState,
    };
    use statig::blocking::{
        self, IntoStateMachineExt, Outcome,
        Outcome::{Handled, Transition},
    };

    pub(super) enum Event {
        Public(ViewportEvent),
    }

    #[derive(Default)]
    pub(super) struct Context {
        effects: Vec<ViewportEffect>,
    }

    impl Context {
        fn push(&mut self, effect: ViewportEffect) {
            self.effects.push(effect);
        }

        fn none(&mut self) {
            self.push(ViewportEffect::None);
        }

        pub(super) fn into_effects(self) -> Vec<ViewportEffect> {
            self.effects
        }
    }

    #[derive(Default)]
    pub(super) struct Machine {
        last_generation: ViewportGeneration,
    }

    impl Machine {
        fn allocate_generation(&mut self, context: &mut Context) -> Option<ViewportGeneration> {
            let Some(generation) = self.last_generation.next() else {
                context.push(ViewportEffect::GenerationExhausted);
                return None;
            };
            self.last_generation = generation;
            Some(generation)
        }
    }

    #[blocking::state_machine(
        initial = "State::following()",
        state(derive(Debug, PartialEq, Eq)),
        superstate(derive(Debug, PartialEq, Eq))
    )]
    impl Machine {
        #[state]
        fn following(&mut self, context: &mut Context, event: &Event) -> Outcome<State> {
            let event = match event {
                Event::Public(event) => event,
            };
            match event {
                ViewportEvent::ExtentChanged => {
                    let Some(generation) = self.allocate_generation(context) else {
                        return Handled;
                    };
                    context.push(ViewportEffect::RequestBottomScroll { generation });
                    Handled
                }
                ViewportEvent::UserScrolled { at_bottom: true } => {
                    context.push(ViewportEffect::HideJumpToLatest);
                    Handled
                }
                ViewportEvent::UserScrolled { at_bottom: false } => {
                    context.push(ViewportEffect::ShowJumpToLatest);
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::detached())
                }
                ViewportEvent::AnchorObserved { anchor_id, offset } => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::anchored(anchor_id.clone(), *offset))
                }
                ViewportEvent::JumpToBottomRequested => {
                    let Some(generation) = self.allocate_generation(context) else {
                        return Handled;
                    };
                    context.push(ViewportEffect::RequestBottomScroll { generation });
                    context.push(ViewportEffect::HideJumpToLatest);
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::scrolling(generation))
                }
                ViewportEvent::ProgrammaticScrollStarted { generation }
                    if *generation == self.last_generation =>
                {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::scrolling(*generation))
                }
                ViewportEvent::ProgrammaticScrollStarted { generation } => {
                    context.push(ViewportEffect::CompletionRejected {
                        generation: *generation,
                        reason: CompletionRejection::StaleGeneration,
                    });
                    Handled
                }
                ViewportEvent::ScrollCompleted { generation } => {
                    context.push(ViewportEffect::CompletionRejected {
                        generation: *generation,
                        reason: CompletionRejection::NoActiveScroll,
                    });
                    Handled
                }
                ViewportEvent::LayoutSettled | ViewportEvent::AnchorRemoved { .. } => {
                    context.none();
                    Handled
                }
                ViewportEvent::OwnerClosed => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::closed())
                }
            }
        }

        #[state(local_storage("anchor_id: ItemId", "offset: i32"))]
        fn anchored(
            &mut self,
            anchor_id: &mut ItemId,
            offset: &mut i32,
            context: &mut Context,
            event: &Event,
        ) -> Outcome<State> {
            let event = match event {
                Event::Public(event) => event,
            };
            match event {
                ViewportEvent::ExtentChanged => {
                    let Some(generation) = self.allocate_generation(context) else {
                        return Handled;
                    };
                    context.push(ViewportEffect::RequestAnchorRestore {
                        anchor_id: anchor_id.clone(),
                        offset: *offset,
                        generation,
                    });
                    Handled
                }
                ViewportEvent::UserScrolled { at_bottom: true } => {
                    context.push(ViewportEffect::HideJumpToLatest);
                    Transition(State::following())
                }
                ViewportEvent::UserScrolled { at_bottom: false } => {
                    context.push(ViewportEffect::ShowJumpToLatest);
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::detached())
                }
                ViewportEvent::AnchorObserved { anchor_id, offset } => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::anchored(anchor_id.clone(), *offset))
                }
                ViewportEvent::JumpToBottomRequested => {
                    let Some(generation) = self.allocate_generation(context) else {
                        return Handled;
                    };
                    context.push(ViewportEffect::RequestBottomScroll { generation });
                    context.push(ViewportEffect::HideJumpToLatest);
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::scrolling(generation))
                }
                ViewportEvent::ProgrammaticScrollStarted { generation }
                    if *generation == self.last_generation =>
                {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::scrolling(*generation))
                }
                ViewportEvent::ProgrammaticScrollStarted { generation } => {
                    context.push(ViewportEffect::CompletionRejected {
                        generation: *generation,
                        reason: CompletionRejection::StaleGeneration,
                    });
                    Handled
                }
                ViewportEvent::ScrollCompleted { generation } => {
                    context.push(ViewportEffect::CompletionRejected {
                        generation: *generation,
                        reason: CompletionRejection::NoActiveScroll,
                    });
                    Handled
                }
                ViewportEvent::LayoutSettled => {
                    context.none();
                    Handled
                }
                ViewportEvent::AnchorRemoved { anchor_id: removed } if removed == &*anchor_id => {
                    context.push(ViewportEffect::ShowJumpToLatest);
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::detached())
                }
                ViewportEvent::AnchorRemoved { .. } => {
                    context.none();
                    Handled
                }
                ViewportEvent::OwnerClosed => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::closed())
                }
            }
        }

        #[state]
        fn detached(&mut self, context: &mut Context, event: &Event) -> Outcome<State> {
            let event = match event {
                Event::Public(event) => event,
            };
            match event {
                ViewportEvent::ExtentChanged => {
                    context.none();
                    Handled
                }
                ViewportEvent::UserScrolled { at_bottom: true } => {
                    context.push(ViewportEffect::HideJumpToLatest);
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::following())
                }
                ViewportEvent::UserScrolled { at_bottom: false } => {
                    context.none();
                    Handled
                }
                ViewportEvent::AnchorObserved { anchor_id, offset } => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::anchored(anchor_id.clone(), *offset))
                }
                ViewportEvent::JumpToBottomRequested => {
                    let Some(generation) = self.allocate_generation(context) else {
                        return Handled;
                    };
                    context.push(ViewportEffect::RequestBottomScroll { generation });
                    context.push(ViewportEffect::HideJumpToLatest);
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::scrolling(generation))
                }
                ViewportEvent::ProgrammaticScrollStarted { generation }
                    if *generation == self.last_generation =>
                {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::scrolling(*generation))
                }
                ViewportEvent::ProgrammaticScrollStarted { generation } => {
                    context.push(ViewportEffect::CompletionRejected {
                        generation: *generation,
                        reason: CompletionRejection::StaleGeneration,
                    });
                    Handled
                }
                ViewportEvent::ScrollCompleted { generation } => {
                    context.push(ViewportEffect::CompletionRejected {
                        generation: *generation,
                        reason: CompletionRejection::NoActiveScroll,
                    });
                    Handled
                }
                ViewportEvent::LayoutSettled | ViewportEvent::AnchorRemoved { .. } => {
                    context.none();
                    Handled
                }
                ViewportEvent::OwnerClosed => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::closed())
                }
            }
        }

        #[state(local_storage("generation: ViewportGeneration"))]
        fn scrolling(
            &mut self,
            generation: &mut ViewportGeneration,
            context: &mut Context,
            event: &Event,
        ) -> Outcome<State> {
            let event = match event {
                Event::Public(event) => event,
            };
            match event {
                ViewportEvent::ExtentChanged
                | ViewportEvent::UserScrolled { at_bottom: false }
                | ViewportEvent::AnchorRemoved { .. } => {
                    context.none();
                    Handled
                }
                ViewportEvent::UserScrolled { at_bottom: true } => {
                    context.push(ViewportEffect::HideJumpToLatest);
                    Transition(State::following())
                }
                ViewportEvent::AnchorObserved { anchor_id, offset } => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::anchored(anchor_id.clone(), *offset))
                }
                ViewportEvent::JumpToBottomRequested => {
                    let Some(next_generation) = self.allocate_generation(context) else {
                        return Handled;
                    };
                    context.push(ViewportEffect::RequestBottomScroll {
                        generation: next_generation,
                    });
                    context.push(ViewportEffect::HideJumpToLatest);
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::scrolling(next_generation))
                }
                ViewportEvent::ProgrammaticScrollStarted {
                    generation: started,
                } if started == generation => {
                    context.none();
                    Handled
                }
                ViewportEvent::ProgrammaticScrollStarted {
                    generation: started,
                } => {
                    context.push(ViewportEffect::CompletionRejected {
                        generation: *started,
                        reason: CompletionRejection::StaleGeneration,
                    });
                    context.push(ViewportEffect::InvalidateRender);
                    Handled
                }
                ViewportEvent::ScrollCompleted {
                    generation: completed,
                } if completed == generation => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::settling(*completed))
                }
                ViewportEvent::ScrollCompleted {
                    generation: completed,
                } => {
                    context.push(ViewportEffect::CompletionRejected {
                        generation: *completed,
                        reason: CompletionRejection::StaleGeneration,
                    });
                    Handled
                }
                ViewportEvent::OwnerClosed => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::closed())
                }
            }
        }

        #[state(local_storage("generation: ViewportGeneration"))]
        fn settling(
            &mut self,
            generation: &mut ViewportGeneration,
            context: &mut Context,
            event: &Event,
        ) -> Outcome<State> {
            let event = match event {
                Event::Public(event) => event,
            };
            match event {
                ViewportEvent::ExtentChanged
                | ViewportEvent::UserScrolled { at_bottom: false }
                | ViewportEvent::AnchorRemoved { .. } => {
                    context.none();
                    Handled
                }
                ViewportEvent::UserScrolled { at_bottom: true } => {
                    context.push(ViewportEffect::HideJumpToLatest);
                    Transition(State::following())
                }
                ViewportEvent::AnchorObserved { anchor_id, offset } => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::anchored(anchor_id.clone(), *offset))
                }
                ViewportEvent::JumpToBottomRequested => {
                    let Some(next_generation) = self.allocate_generation(context) else {
                        return Handled;
                    };
                    context.push(ViewportEffect::RequestBottomScroll {
                        generation: next_generation,
                    });
                    context.push(ViewportEffect::HideJumpToLatest);
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::scrolling(next_generation))
                }
                ViewportEvent::ProgrammaticScrollStarted {
                    generation: started,
                } if *started == self.last_generation => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::scrolling(*started))
                }
                ViewportEvent::ProgrammaticScrollStarted {
                    generation: started,
                } => {
                    context.push(ViewportEffect::CompletionRejected {
                        generation: *started,
                        reason: CompletionRejection::StaleGeneration,
                    });
                    Handled
                }
                ViewportEvent::ScrollCompleted {
                    generation: completed,
                } => {
                    context.push(ViewportEffect::CompletionRejected {
                        generation: *completed,
                        reason: CompletionRejection::StaleGeneration,
                    });
                    Handled
                }
                ViewportEvent::LayoutSettled => {
                    let _ = generation;
                    context.push(ViewportEffect::HideJumpToLatest);
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::following())
                }
                ViewportEvent::OwnerClosed => {
                    context.push(ViewportEffect::InvalidateRender);
                    Transition(State::closed())
                }
            }
        }

        #[state]
        fn closed(&mut self, context: &mut Context, event: &Event) -> Outcome<State> {
            let _ = self;
            let _ = event;
            context.none();
            Handled
        }
    }

    pub(super) fn new_machine(generation: ViewportGeneration) -> blocking::StateMachine<Machine> {
        Machine {
            last_generation: generation,
        }
        .state_machine()
    }

    pub(super) fn generation(machine: &blocking::StateMachine<Machine>) -> ViewportGeneration {
        machine.inner().last_generation
    }

    pub(super) fn public_state(state: &State) -> ViewportState {
        match state {
            State::Following { .. } => ViewportState::Following,
            State::Anchored {
                anchor_id, offset, ..
            } => ViewportState::Anchored {
                anchor_id: anchor_id.clone(),
                offset: *offset,
            },
            State::Detached { .. } => ViewportState::Detached,
            State::Scrolling { generation, .. } => ViewportState::Scrolling {
                generation: *generation,
            },
            State::Settling { generation, .. } => ViewportState::Settling {
                generation: *generation,
            },
            State::Closed { .. } => ViewportState::Closed,
        }
    }
}

/// Public owner for one rendered thread's viewport state.
pub struct ViewportController {
    machine: blocking::StateMachine<viewport_statig::Machine>,
}

impl Default for ViewportController {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewportController {
    #[must_use]
    pub fn new() -> Self {
        Self::from_machine(viewport_statig::new_machine(ViewportGeneration::INITIAL))
    }

    #[must_use]
    pub fn anchored(anchor_id: ItemId, offset: i32) -> Self {
        let mut controller = Self::new();
        let _ = controller.handle(ViewportEvent::anchor_observed(anchor_id, offset));
        controller
    }

    /// Creates a machine with a bounded pre-machine seed for overflow tests.
    /// The generation cannot be changed after this constructor returns.
    #[doc(hidden)]
    #[must_use]
    pub fn seeded_for_test(generation: ViewportGeneration) -> Self {
        Self::from_machine(viewport_statig::new_machine(generation))
    }

    #[must_use]
    pub fn state(&self) -> ViewportState {
        viewport_statig::public_state(self.machine.state())
    }

    #[must_use]
    pub fn generation(&self) -> ViewportGeneration {
        viewport_statig::generation(&self.machine)
    }

    /// Applies one viewport event and returns typed effects.
    pub fn handle(&mut self, event: ViewportEvent) -> Vec<ViewportEffect> {
        let mut context = viewport_statig::Context::default();
        let statig_event = viewport_statig::Event::Public(event);
        self.machine
            .handle_with_context(&statig_event, &mut context);
        context.into_effects()
    }

    fn from_machine(mut machine: blocking::StateMachine<viewport_statig::Machine>) -> Self {
        let mut context = viewport_statig::Context::default();
        machine.init_with_context(&mut context);
        Self { machine }
    }
}

impl Clone for ViewportController {
    fn clone(&self) -> Self {
        let mut clone = Self::seeded_for_test(self.generation());
        match self.state() {
            ViewportState::Following => {}
            ViewportState::Anchored { anchor_id, offset } => {
                let _ = clone.handle(ViewportEvent::anchor_observed(anchor_id, offset));
            }
            ViewportState::Detached => {
                let _ = clone.handle(ViewportEvent::UserScrolled { at_bottom: false });
            }
            ViewportState::Scrolling { generation } => {
                let _ = clone.handle(ViewportEvent::ProgrammaticScrollStarted { generation });
            }
            ViewportState::Settling { generation } => {
                let _ = clone.handle(ViewportEvent::ProgrammaticScrollStarted { generation });
                let _ = clone.handle(ViewportEvent::ScrollCompleted { generation });
            }
            ViewportState::Closed => {
                let _ = clone.handle(ViewportEvent::OwnerClosed);
            }
        }
        clone
    }
}

impl fmt::Debug for ViewportController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ViewportController")
            .field("state", &self.state())
            .field("generation", &self.generation())
            .finish()
    }
}

impl PartialEq for ViewportController {
    fn eq(&self, other: &Self) -> bool {
        self.state() == other.state() && self.generation() == other.generation()
    }
}

impl Eq for ViewportController {}
