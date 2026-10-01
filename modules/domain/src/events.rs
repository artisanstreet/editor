//! Durable facts emitted by the first native workflow plus finite
//! engine-observation delivery.
//!
//! Each event records a fact that became true only after Forge durably
//! accepted its command: attachment minted a project, creation minted a
//! thread, queueing minted a message. Queuing is the terminal fact of the
//! first milestone; engine observations extend that vocabulary without
//! changing it: every committed observation batch publishes its rows as
//! [`Event::EngineObservation`] in durable sequence order. Responding to an
//! approval or question (the `RespondApproval`/`RespondQuestion` commands)
//! belongs to the later A-approve packet, not to this event.
//!
//! `Eq` is deliberately absent: engine usage rows carry a finite
//! `cost_usd: f64`, so [`Observation`] (and therefore this enum) implements
//! only [`PartialEq`]. Every exhaustive `match` on this enum names the
//! engine arm explicitly; no wildcard may hide it.

use crate::model::{ProjectListing, ProjectSummary, QueuedMessage, ThreadSummary};
use crate::observation::Observation;
use crate::time::UnixMillis;
use crate::{
    EngineUsageSnapshot, MessageOutbox, RecentThreadListing, RunId, RunUsageResult, ThreadId,
    ThreadTitle, TurnId, UserPreferences,
};

/// One directory attach completed and its project identity was minted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectAttached {
    /// Full summary of the attached project, including the minted identity.
    pub project: ProjectSummary,
}

/// One thread came into existence with its Forge-minted identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadCreated {
    /// Summary of the created thread.
    pub thread: ThreadSummary,
}

/// A first message became durably queued on its thread.
///
/// Dispatch is intentionally absent: queued is an explicit durable state of
/// this milestone, not a pending transition into engine territory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirstMessageQueued {
    /// The queued message, including its minted message id.
    pub message: QueuedMessage,
}

/// Every durable fact of the first native workflow plus finite
/// engine-observation delivery.
///
/// `Eq` is absent by necessity: the engine arm carries [`Observation`],
/// whose usage rows hold a finite `f64` cost. See the module docs.
// `allow`, not `expect`: whether the lint fires depends on how the largest
// variant compares with the next, which moves whenever a variant is added.
#[allow(
    clippy::large_enum_variant,
    reason = "boxing would change the public payload type consumed across crates; events are built one at a time, not stored in bulk"
)]
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// See [`ProjectAttached`].
    ProjectAttached(ProjectAttached),
    /// See [`ThreadCreated`].
    ThreadCreated(ThreadCreated),
    /// See [`FirstMessageQueued`].
    FirstMessageQueued(FirstMessageQueued),
    /// One durably committed engine observation for its thread subscribers.
    ///
    /// The observation carries its own durable [`ObservationSequence`](crate::ObservationSequence);
    /// delivery publishes a committed batch in that sequence order and
    /// deduplicates per subscriber cursor. Approval and question rows carry
    /// the provider `approval_id`/`question_id` that the later A-approve
    /// packet answers; this packet never responds to them.
    EngineObservation(EngineObservationEvent),
    /// A newly subscribed thread's observation history has been delivered
    /// through its durable tail (see [`ObservationHistoryCurrent`]).
    ObservationHistoryCurrent(ObservationHistoryCurrent),
    /// The thread's undelivered messages changed; the complete outbox is
    /// pushed to the thread's subscribers (see [`MessageOutbox`]).
    MessageOutbox(MessageOutbox),
    /// Every engine's account usage with the Forge's readiness verdict,
    /// pushed to each connection whenever it changes.
    AccountUsage(EngineUsageSnapshot),
    /// The user's preferences, pushed to each connection whenever they
    /// change.
    UserPreferences(UserPreferences),
    /// A subscribed thread's display title changed.
    ThreadRetitled(ThreadRetitled),
    /// A subscribed thread's live run reported new usage.
    RunUsage(RunUsageResult),
    /// The recent threads across every project changed; pushed to a
    /// connection that read them.
    RecentThreads(RecentThreadListing),
    /// The attached-project catalog changed; pushed to a connection that
    /// listed the projects.
    ProjectCatalog(ProjectListing),
    /// A Forge-managed engine's install status changed; pushed to a
    /// connection that read the engine installs.
    EngineInstalls(crate::EngineInstallSnapshot),
    /// What a subscribed thread's live run is thinking right now; never
    /// stored, so it exists only while the run is thinking.
    LiveThinking(LiveThinking),
    /// The work of a subscribed thread's settled turns that stayed on the
    /// Forge, pushed once the thread's activity replay is complete.
    HeldBackWork(crate::HeldBackWork),
    /// The user messages of a subscribed thread's turns before the loaded
    /// ones, pushed with the held-back work.
    EarlierTurnMarkers(crate::EarlierTurnMarkers),
}

/// A subscribed thread's display title changed: the generated title was
/// recorded, or the first message now stands in for the placeholder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadRetitled {
    /// The thread.
    pub thread_id: ThreadId,
    /// Its display title.
    pub title: ThreadTitle,
}

/// What a subscribed thread's live run is thinking right now.
///
/// Thinking summaries are shown only while they are the newest thing the run
/// produced, so the Forge keeps the current one in memory and never writes it
/// to the observation ledger. `current` is `None` once the run moved on to a
/// tool, to prose, or ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveThinking {
    /// The thread.
    pub thread_id: ThreadId,
    /// The thinking block in progress, if the run is thinking.
    pub current: Option<LiveThinkingBlock>,
}

/// One thinking block of a live run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveThinkingBlock {
    /// Run that is thinking.
    pub run_id: RunId,
    /// Forge turn the run launched from.
    pub turn_id: TurnId,
    /// Provider identity of the thinking block.
    pub item_id: String,
    /// The summary so far, bounded to its newest part.
    pub text: String,
    /// When the block started.
    pub started_at: UnixMillis,
    /// When the block last grew.
    pub updated_at: UnixMillis,
}

/// The end-of-replay marker for one thread subscription.
///
/// Sent once per activation, after every page of the thread's durable
/// observation history has been published and before any live observation.
/// The subscription snapshot carries only the transcript; this marker tells
/// the subscriber the tool and reasoning history that belongs to it is now
/// complete, so the thread can be presented whole instead of filling in page
/// by page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationHistoryCurrent {
    /// The thread whose history is current.
    pub thread_id: ThreadId,
}

/// One committed engine observation routed to its thread subscribers.
///
/// The thread identity scopes delivery: only subscribers of this thread
/// receive the row. The observation itself is the validated, sanitized S1a
/// vocabulary value; no raw provider payload crosses this boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineObservationEvent {
    /// Thread whose subscribers receive the observation.
    pub thread_id: ThreadId,
    /// The committed observation row.
    pub observation: Observation,
    /// Durable delivery attribution of the committed row.
    ///
    /// `None` marks legacy unattributed events only. Every row the
    /// observation ledger appends reads back as `Some`: the producing run,
    /// the Forge turn that run launched from, the caller-injected commit
    /// instant, and the thread-scoped delivery sequence.
    pub attribution: Option<EngineObservationAttribution>,
}

/// Durable delivery attribution of one committed engine observation.
///
/// The Forge turn is resolved from the run scope at commit time, never
/// derived from a provider string. The commit instant is the
/// caller-injected batch operation time; no new wall-clock is sampled.
/// The delivery sequence is a thread-scoped strictly increasing positive
/// counter allocated in the commit transaction, separate from the
/// run-local [`ObservationSequence`](crate::ObservationSequence) the
/// observation itself carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineObservationAttribution {
    /// Run that produced the observation.
    pub run_id: RunId,
    /// Forge turn the producing run launched from.
    pub turn_id: TurnId,
    /// Caller-injected commit instant of the producing batch.
    pub committed_at: UnixMillis,
    /// Thread-scoped delivery sequence, strictly after the read cursor.
    pub delivery_sequence: u64,
}
