//! On-demand parts of a thread's history.
//!
//! A subscription opens on the newest turns only, and the work rows of a
//! settled turn stay on the Forge until its section is opened. These types
//! carry what is read afterwards: older turns as the reader scrolls toward
//! them, and one turn's held-back work when its section opens.

use crate::events::EngineObservationEvent;
use crate::time::UnixMillis;
use crate::{ConversationSnapshot, ItemId, QueryTurnCount, RunId, ThreadId, TurnId, TurnOrdinal};

/// Most work rows one [`ConversationHistoryPart::TurnWork`] answer carries;
/// the reader asks again from [`ConversationHistoryPage::next_after_sequence`].
pub const TURN_WORK_PAGE_MAX_ROWS: usize = 256;

/// Request for one on-demand part of a thread's history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationHistoryRequest {
    /// Thread whose history is read.
    pub thread_id: ThreadId,
    /// Which part.
    pub part: ConversationHistoryPart,
}

/// One on-demand part of a thread's history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationHistoryPart {
    /// Older turns before the loaded ones, with the activity that shows while
    /// their sections are closed.
    EarlierTurns {
        /// Exclusive upper ordinal: the oldest loaded turn.
        before_turn_ordinal: TurnOrdinal,
        /// Inclusive floor when the reader jumps to one turn; every turn from
        /// it up to `before_turn_ordinal` is then wanted.
        minimum_turn_ordinal: Option<TurnOrdinal>,
        /// Maximum turns to include.
        maximum_turn_count: QueryTurnCount,
    },
    /// The held-back work rows of one settled turn, in delivery order.
    TurnWork {
        /// The turn whose section was opened.
        turn_id: TurnId,
        /// Delivery sequence to continue after; zero starts at the first row.
        after_sequence: u64,
    },
}

/// One answered part of a thread's history.
#[derive(Clone, Debug, PartialEq)]
pub struct ConversationHistoryPage {
    /// Thread the page belongs to.
    pub thread_id: ThreadId,
    /// The older turns and their items; present for
    /// [`ConversationHistoryPart::EarlierTurns`] only. A snapshot without
    /// turns means the thread has none before the requested ordinal.
    pub snapshot: Option<ConversationSnapshot>,
    /// Observation rows of the page, in delivery order.
    pub observations: Vec<EngineObservationEvent>,
    /// Work of the page's settled turns that stayed on the Forge.
    pub held_back: Vec<HeldBackTurnWork>,
    /// For [`ConversationHistoryPart::TurnWork`]: the delivery sequence to
    /// continue after when more rows remain, else `None`.
    pub next_after_sequence: Option<u64>,
}

// Every float an observation carries is validated finite when it is built,
// so equality is reflexive and the page can sit in an `Eq` response payload.
impl Eq for ConversationHistoryPage {}

/// Work rows of one settled turn that the Forge has not sent.
///
/// The first row's run, instant and delivery sequence place the turn's work
/// section where its first step would be, so the closed section looks the
/// same before and after its rows are read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeldBackTurnWork {
    /// The settled turn.
    pub turn_id: TurnId,
    /// Run that produced the first held-back row.
    pub run_id: RunId,
    /// Rows held back.
    pub row_count: u32,
    /// Commit instant of the first held-back row.
    pub first_committed_at: UnixMillis,
    /// Delivery sequence of the first held-back row.
    pub first_delivery_sequence: u64,
}

/// The held-back work of a subscribed thread's loaded turns, pushed once the
/// thread's activity replay is complete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeldBackWork {
    /// The thread.
    pub thread_id: ThreadId,
    /// Its settled turns with held-back work.
    pub turns: Vec<HeldBackTurnWork>,
}

/// Most user messages one [`EarlierTurnMarkers`] push names, newest kept.
pub const EARLIER_TURN_MARKERS_MAX: usize = 512;

/// Most UTF-8 bytes of a user message a marker carries as its label.
pub const EARLIER_TURN_MARKER_LABEL_MAX_BYTES: usize = 480;

/// The user messages of a subscribed thread's turns that lie before the
/// loaded ones, oldest first.
///
/// A subscription opens on the newest turns only, but a reader still jumps
/// to any question they asked: these name the ones not loaded, so the turn
/// navigator lists them and a jump reads the turns in between.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EarlierTurnMarkers {
    /// The thread.
    pub thread_id: ThreadId,
    /// One marker per user message before the loaded turns.
    pub markers: Vec<EarlierTurnMarker>,
}

/// One user message in a turn that is not loaded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EarlierTurnMarker {
    /// The user message.
    pub item_id: ItemId,
    /// Ordinal of the turn that holds it.
    pub turn_ordinal: TurnOrdinal,
    /// The start of its text.
    pub label: String,
}
