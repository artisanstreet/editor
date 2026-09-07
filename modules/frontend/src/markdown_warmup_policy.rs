//! Dependency-free scheduling policy for settled Markdown renderer warmup.
//!
//! The browser implementation owns idle callbacks, delays, dynamic imports,
//! and the actual renderers. This module only models the inputs and typed
//! actions at that boundary. In particular, `LoadChunk` means that a caller
//! may load a module; it never constructs a highlighter, stylesheet, diagram,
//! or rendered result.

/// Starvation timeout supplied to a supported idle callback, in milliseconds.
pub const RENDERER_IDLE_CALLBACK_TIMEOUT_MS: u64 = 10_000;

/// Pacing delay used when idle callbacks are unavailable, in milliseconds.
pub const RENDERER_IDLE_FALLBACK_DELAY_MS: u64 = 1_000;

/// The four renderer chunks warmed in the exact legacy order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererWarmupChunk {
    /// Settled-message code-highlighting support.
    SettledHighlighting,
    /// The conversation math renderer module.
    MathRenderer,
    /// The conversation math renderer stylesheet.
    MathStylesheet,
    /// The conversation Mermaid renderer module.
    MermaidRenderer,
}

impl RendererWarmupChunk {
    /// All chunks in the order in which the legacy warmup loads them.
    pub const ALL: [Self; 4] = [
        Self::SettledHighlighting,
        Self::MathRenderer,
        Self::MathStylesheet,
        Self::MermaidRenderer,
    ];

    /// Returns the stable index used by the policy's fixed-size facts.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::SettledHighlighting => 0,
            Self::MathRenderer => 1,
            Self::MathStylesheet => 2,
            Self::MermaidRenderer => 3,
        }
    }

    /// Returns a diagnostic label for this chunk.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SettledHighlighting => "settled highlighting",
            Self::MathRenderer => "math renderer",
            Self::MathStylesheet => "math stylesheet",
            Self::MermaidRenderer => "Mermaid renderer",
        }
    }
}

/// Public spelling of the exact settled-conversation warmup order.
pub const CONVERSATION_RENDERER_WARMUP_CHUNKS: [RendererWarmupChunk; 4] = RendererWarmupChunk::ALL;

/// Public Markdown-oriented spelling of [`CONVERSATION_RENDERER_WARMUP_CHUNKS`].
pub const MARKDOWN_WARMUP_CHUNKS: [RendererWarmupChunk; 4] = CONVERSATION_RENDERER_WARMUP_CHUNKS;

/// Result of the native scheduler's idle-capability input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdleCapability {
    /// The host can register an idle callback.
    Available,
    /// The host cannot register an idle callback.
    Absent,
    /// Capability detection failed, so the safe paced path is used.
    DetectionFailed,
}

impl From<bool> for IdleCapability {
    fn from(supported: bool) -> Self {
        if supported {
            Self::Available
        } else {
            Self::Absent
        }
    }
}

/// One wait action emitted before a renderer chunk load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererIdleWait {
    /// Ask the host for one idle callback with a starvation timeout.
    IdleCallback {
        /// Starvation timeout passed to the host, in milliseconds.
        timeout_ms: u64,
    },
    /// Ask the host for one paced delay when idle callbacks are unavailable.
    PacedDelay {
        /// Delay requested from the host, in milliseconds.
        duration_ms: u64,
    },
}

impl IdleCapability {
    const fn wait(self) -> RendererIdleWait {
        match self {
            Self::Available => RendererIdleWait::IdleCallback {
                timeout_ms: RENDERER_IDLE_CALLBACK_TIMEOUT_MS,
            },
            Self::Absent | Self::DetectionFailed => RendererIdleWait::PacedDelay {
                duration_ms: RENDERER_IDLE_FALLBACK_DELAY_MS,
            },
        }
    }
}

/// Durable status of one renderer chunk after the warmup attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererWarmupChunkStatus {
    /// The warmup has not reached this chunk, or its load was cancelled.
    NotAttempted,
    /// The warmup load completed successfully.
    Loaded,
    /// The warmup load failed and the first real use may retry it.
    Failed,
}

/// A load failure retained for diagnostics without retaining a browser error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RendererWarmupFailure {
    /// The chunk whose best-effort warmup load failed.
    pub chunk: RendererWarmupChunk,
}

/// Typed result supplied by the host after a `LoadChunk` action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererChunkResult {
    /// The requested module load succeeded.
    Loaded,
    /// The requested module load failed; later chunks still proceed.
    Failed,
}

/// What a real first use should do with a chunk after warmup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererFirstUseDisposition {
    /// The chunk was warmed successfully.
    AlreadyWarmed,
    /// The chunk was never loaded by warmup, so first use is an initial load.
    InitialLoad,
    /// Warmup failed for the chunk, so first use remains a retry opportunity.
    RetryFailedLoad,
}

/// Reducer state at the scheduler boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererWarmupState {
    /// No warmup action has been emitted.
    NotStarted,
    /// One wait is pending before the named chunk's load.
    WaitingForIdle {
        /// Chunk associated with the pending wait.
        chunk: RendererWarmupChunk,
        /// Wait action that was emitted for the chunk.
        wait: RendererIdleWait,
    },
    /// The host may perform the named module load.
    Loading {
        /// Chunk associated with the pending load.
        chunk: RendererWarmupChunk,
    },
    /// Every chunk was attempted and no more warmup work is possible.
    Completed,
    /// Cancellation stopped future warmup actions.
    Cancelled,
}

/// Pure action emitted by the warmup reducer.
#[must_use = "a warmup action must be handed to the host"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererWarmupAction {
    /// Complete one idle wait before loading the named chunk.
    WaitForIdle {
        /// Chunk whose load follows the wait.
        chunk: RendererWarmupChunk,
        /// Exact wait requested by the scheduler.
        wait: RendererIdleWait,
    },
    /// Load the named module chunk; this is not a construction or render.
    LoadChunk {
        /// Chunk to load.
        chunk: RendererWarmupChunk,
    },
    /// All four warmup loads have settled.
    Complete,
    /// Stop any pending future warmup work.
    Cancelled,
}

/// Typed input delivered to the warmup reducer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererWarmupEvent {
    /// Begin the ordered warmup sequence.
    Start,
    /// The current idle callback or paced delay has completed.
    IdleWaitFinished,
    /// The current module load has settled.
    ChunkFinished {
        /// Result supplied by the host for the current load.
        result: RendererChunkResult,
    },
    /// Cancel future waits and loads.
    Cancel,
}

/// Dependency-free reducer for the four settled Markdown renderer chunks.
///
/// The reducer is deliberately tolerant of duplicate or late events. An
/// event that does not match the current state emits no action and leaves all
/// completed facts unchanged. This makes cancellation and terminal delivery
/// idempotent at the policy boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RendererWarmup {
    idle_capability: IdleCapability,
    next_chunk: usize,
    state: RendererWarmupState,
    statuses: [RendererWarmupChunkStatus; 4],
    failures: Vec<RendererWarmupFailure>,
}

impl RendererWarmup {
    /// Creates an unstarted reducer from an idle-capability input.
    ///
    /// A `bool` is accepted as a convenience: `true` means [`IdleCapability::Available`]
    /// and `false` means [`IdleCapability::Absent`].
    #[must_use]
    pub fn new(capability: impl Into<IdleCapability>) -> Self {
        Self::with_idle_capability(capability.into())
    }

    /// Creates an unstarted reducer from an explicit capability result.
    #[must_use]
    pub const fn with_idle_capability(capability: IdleCapability) -> Self {
        Self {
            idle_capability: capability,
            next_chunk: 0,
            state: RendererWarmupState::NotStarted,
            statuses: [RendererWarmupChunkStatus::NotAttempted; 4],
            failures: Vec::new(),
        }
    }

    /// Returns the capability input retained by this reducer.
    #[must_use]
    pub const fn idle_capability(&self) -> IdleCapability {
        self.idle_capability
    }

    /// Returns the current reducer state.
    #[must_use]
    pub const fn state(&self) -> RendererWarmupState {
        self.state
    }

    /// Returns the durable status for one renderer chunk.
    #[must_use]
    pub const fn chunk_status(&self, chunk: RendererWarmupChunk) -> RendererWarmupChunkStatus {
        self.statuses[chunk.index()]
    }

    /// Returns all recorded warmup failures in chunk order.
    #[must_use]
    pub fn failures(&self) -> &[RendererWarmupFailure] {
        &self.failures
    }

    /// Returns whether the reducer has reached a terminal state.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            RendererWarmupState::Completed | RendererWarmupState::Cancelled
        )
    }

    /// Returns whether all four chunks were attempted.
    #[must_use]
    pub const fn is_completed(&self) -> bool {
        matches!(self.state, RendererWarmupState::Completed)
    }

    /// Returns whether cancellation stopped the sequence.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self.state, RendererWarmupState::Cancelled)
    }

    /// Describes the load a real first use should perform for one chunk.
    #[must_use]
    pub const fn first_use_disposition(
        &self,
        chunk: RendererWarmupChunk,
    ) -> RendererFirstUseDisposition {
        match self.chunk_status(chunk) {
            RendererWarmupChunkStatus::NotAttempted => RendererFirstUseDisposition::InitialLoad,
            RendererWarmupChunkStatus::Loaded => RendererFirstUseDisposition::AlreadyWarmed,
            RendererWarmupChunkStatus::Failed => RendererFirstUseDisposition::RetryFailedLoad,
        }
    }

    /// Returns whether a failed warmup chunk remains eligible for first-use retry.
    #[must_use]
    pub const fn is_first_use_retry_eligible(&self, chunk: RendererWarmupChunk) -> bool {
        matches!(
            self.first_use_disposition(chunk),
            RendererFirstUseDisposition::RetryFailedLoad
        )
    }

    /// Emits the first wait, once.
    ///
    /// Repeated starts after the sequence has started or reached a terminal
    /// state emit no action and do not mutate the reducer.
    #[must_use]
    pub fn start(&mut self) -> Option<RendererWarmupAction> {
        if !matches!(self.state, RendererWarmupState::NotStarted) {
            return None;
        }

        Some(self.begin_wait())
    }

    /// Converts a completed wait into the corresponding module-load action.
    ///
    /// A duplicate or late wait completion is ignored.
    #[must_use]
    pub fn on_idle_wait_finished(&mut self) -> Option<RendererWarmupAction> {
        match self.state {
            RendererWarmupState::WaitingForIdle { chunk, .. } => {
                self.state = RendererWarmupState::Loading { chunk };
                Some(RendererWarmupAction::LoadChunk { chunk })
            }
            RendererWarmupState::NotStarted
            | RendererWarmupState::Loading { .. }
            | RendererWarmupState::Completed
            | RendererWarmupState::Cancelled => None,
        }
    }

    /// Applies one module-load result and schedules the next chunk.
    ///
    /// `Failed` is a diagnostic fact, not a sequence error. It remains
    /// eligible for a real first-use retry, and the next chunk still receives
    /// its own wait before its load.
    #[must_use]
    pub fn on_chunk_finished(
        &mut self,
        result: RendererChunkResult,
    ) -> Option<RendererWarmupAction> {
        let RendererWarmupState::Loading { chunk } = self.state else {
            return None;
        };

        let index = chunk.index();
        match result {
            RendererChunkResult::Loaded => {
                self.statuses[index] = RendererWarmupChunkStatus::Loaded;
            }
            RendererChunkResult::Failed => {
                self.statuses[index] = RendererWarmupChunkStatus::Failed;
                self.failures.push(RendererWarmupFailure { chunk });
            }
        }

        self.next_chunk += 1;
        if self.next_chunk == CONVERSATION_RENDERER_WARMUP_CHUNKS.len() {
            self.state = RendererWarmupState::Completed;
            Some(RendererWarmupAction::Complete)
        } else {
            Some(self.begin_wait())
        }
    }

    /// Cancels future actions while preserving all statuses and diagnostics.
    ///
    /// Cancellation is idempotent. Once completed, the reducer remains
    /// `Completed`; there is no future work to cancel and no completed fact is
    /// rewritten.
    #[must_use]
    pub fn cancel(&mut self) -> Option<RendererWarmupAction> {
        match self.state {
            RendererWarmupState::NotStarted
            | RendererWarmupState::WaitingForIdle { .. }
            | RendererWarmupState::Loading { .. } => {
                self.state = RendererWarmupState::Cancelled;
                Some(RendererWarmupAction::Cancelled)
            }
            RendererWarmupState::Completed | RendererWarmupState::Cancelled => None,
        }
    }

    /// Applies one typed scheduler input.
    #[must_use]
    pub fn dispatch(&mut self, event: RendererWarmupEvent) -> Option<RendererWarmupAction> {
        match event {
            RendererWarmupEvent::Start => self.start(),
            RendererWarmupEvent::IdleWaitFinished => self.on_idle_wait_finished(),
            RendererWarmupEvent::ChunkFinished { result } => self.on_chunk_finished(result),
            RendererWarmupEvent::Cancel => self.cancel(),
        }
    }

    fn begin_wait(&mut self) -> RendererWarmupAction {
        let chunk = CONVERSATION_RENDERER_WARMUP_CHUNKS[self.next_chunk];
        let wait = self.idle_capability.wait();
        self.state = RendererWarmupState::WaitingForIdle { chunk, wait };
        RendererWarmupAction::WaitForIdle { chunk, wait }
    }
}
