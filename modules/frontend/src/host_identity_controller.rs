//! Dependency-free host-identity refresh state.
//!
//! This is the value-level counterpart of the legacy
//! `lib/identity/host-identity-controller.ts`. The browser controller keeps
//! one optional identity for both the sidebar shell and the thread context,
//! admits at most one request, and fences every result with the generation
//! that admitted it. This module keeps those decisions explicit without
//! importing a transport, executor, UI toolkit, or storage layer.
//!
//! [`HostIdentityState::apply`] is the only transition point. A
//! [`HostIdentityAction::RefreshAdmitted`] result tells an integration layer
//! to start work and carries the generation that must be returned in either a
//! completion or failure input. The state machine owns no task or caller
//! lifetime: once admission is returned, the integration layer is responsible
//! for running that accepted work in the app scope rather than in a route or
//! menu scope.

#![allow(clippy::module_name_repetitions)]

/// Operating-system family reported by the host identity provider.
///
/// The vocabulary mirrors the protocol snapshot while remaining local to
/// this dependency-light policy module. Transport adapters can map their
/// decoded protocol value onto it at the integration boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostPlatform {
    /// Windows (`win32`).
    Win32,
    /// macOS (`darwin`).
    Darwin,
    /// Linux (`linux`).
    Linux,
    /// A platform not identified by the provider.
    Unknown,
}

impl HostPlatform {
    /// Returns the protocol spelling of this platform family.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Win32 => "win32",
            Self::Darwin => "darwin",
            Self::Linux => "linux",
            Self::Unknown => "unknown",
        }
    }
}

/// The host identity value shared by shell and thread-context consumers.
///
/// The fields are the small decoded snapshot surface read by the legacy
/// callsites. This type contains no validation or I/O; the transport-facing
/// layer remains responsible for supplying a decoded value.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HostIdentitySnapshot {
    /// Human-readable account name when the host provides one.
    pub display_name: Option<String>,
    /// Stable machine name used as the display fallback and avatar seed.
    pub hostname: String,
    /// Operating-system family that produced this value.
    pub platform: HostPlatform,
    /// Raw host account name when available.
    pub username: Option<String>,
}

impl HostIdentitySnapshot {
    /// Creates a snapshot with optional account fields unset.
    #[must_use]
    pub fn new(hostname: impl Into<String>, platform: HostPlatform) -> Self {
        Self {
            display_name: None,
            hostname: hostname.into(),
            platform,
            username: None,
        }
    }
}

/// A correlation identity for one admitted host-identity request.
///
/// Generations start at zero before the first admission and increase without
/// wrapping. The value is public only so an integration layer can carry the
/// returned identity across its asynchronous boundary; callers should use the
/// generation returned by [`HostIdentityAction::RefreshAdmitted`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HostIdentityGeneration(u64);

impl HostIdentityGeneration {
    /// The generation held by a newly created state machine.
    pub const INITIAL: Self = Self(0);

    /// Creates a generation from an externally retained correlation value.
    ///
    /// Normal callers do not need to construct generations: admission returns
    /// the value to use for completion or failure. The constructor keeps the
    /// typed input useful for deterministic callers without adding a transport
    /// dependency here.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric generation for diagnostics or an adapter boundary.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// An input from a shell/context caller or from the admitted request owner.
///
/// The input is deliberately value-only. It does not carry a future, fiber,
/// cancellation token, route scope, or transport error so this state machine
/// cannot accidentally make caller lifetime part of app-scoped custody.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostIdentityInput {
    /// Ask the shared state to admit a refresh if no identity or request is
    /// currently present.
    Refresh,
    /// Report a successful result for an admitted generation.
    Complete {
        /// Generation returned by the corresponding admission action.
        generation: HostIdentityGeneration,
        /// Decoded identity to publish if the generation is still current.
        snapshot: HostIdentitySnapshot,
    },
    /// Report that an admitted generation failed without producing an identity.
    Fail {
        /// Generation returned by the corresponding admission action.
        generation: HostIdentityGeneration,
    },
}

impl HostIdentityInput {
    /// Creates the refresh-admission input.
    #[must_use]
    pub const fn refresh() -> Self {
        Self::Refresh
    }

    /// Creates a successful completion input.
    #[must_use]
    pub fn complete(generation: HostIdentityGeneration, snapshot: HostIdentitySnapshot) -> Self {
        Self::Complete {
            generation,
            snapshot,
        }
    }

    /// Creates a failed-completion input.
    #[must_use]
    pub const fn fail(generation: HostIdentityGeneration) -> Self {
        Self::Fail { generation }
    }
}

/// Why a refresh request was admitted or suppressed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RefreshSuppression {
    /// A snapshot is already available, so refresh is a no-op.
    AlreadyLoaded,
    /// Another request is already in flight.
    InFlight {
        /// Generation currently owned by the admitted request.
        generation: HostIdentityGeneration,
    },
    /// No new generation can be represented without reusing an old one.
    GenerationExhausted,
}

/// Why a successful result was not allowed to publish.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CompletionIgnoredReason {
    /// A newer generation is current, so this result is stale.
    StaleGeneration,
    /// The generation is current but no matching request remains active.
    NoMatchingActiveRefresh,
}

/// Why a failure did not clear an active request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FailureIgnoredReason {
    /// Only the exact active generation may be cleared.
    NoMatchingActiveRefresh,
}

/// The value-level effect of applying one [`HostIdentityInput`].
///
/// Actions report the boundary decision to a caller; they do not execute
/// transport work. In particular, [`Self::RefreshAdmitted`] is an instruction
/// to an outer app-scoped owner to start a request, not an owned task.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum HostIdentityAction {
    /// A new request may be started by the outer app-scoped owner.
    RefreshAdmitted {
        /// Generation that must identify the eventual terminal input.
        generation: HostIdentityGeneration,
    },
    /// The refresh request was intentionally left unchanged.
    RefreshSuppressed {
        /// The precise admission reason.
        reason: RefreshSuppression,
    },
    /// The current request succeeded and the snapshot was published.
    IdentityPublished {
        /// Generation that published the snapshot.
        generation: HostIdentityGeneration,
        /// Snapshot now retained as the shared current identity.
        snapshot: HostIdentitySnapshot,
    },
    /// A success was ignored because it could not publish for its generation.
    CompletionIgnored {
        /// Generation carried by the stale or already-settled result.
        generation: HostIdentityGeneration,
        /// Why the result was rejected.
        reason: CompletionIgnoredReason,
    },
    /// The matching active request failed and was cleared.
    RefreshFailed {
        /// Generation that was cleared.
        generation: HostIdentityGeneration,
    },
    /// A failure for another or already-settled generation was ignored.
    FailureIgnored {
        /// Generation carried by the failure report.
        generation: HostIdentityGeneration,
        /// Why no active request was cleared.
        reason: FailureIgnoredReason,
    },
}

/// Shared host-identity state and its single in-flight refresh generation.
///
/// There is one optional snapshot for all consumers and one optional active
/// generation. A successful completion must match both the current generation
/// and that active generation; a failure may clear only the exact active
/// generation. The last successful snapshot is never cleared by a failure or
/// by a stale completion.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct HostIdentityState {
    snapshot: Option<HostIdentitySnapshot>,
    current_generation: HostIdentityGeneration,
    active_generation: Option<HostIdentityGeneration>,
}

impl HostIdentityState {
    /// Creates an empty state with no request in flight.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            snapshot: None,
            current_generation: HostIdentityGeneration::INITIAL,
            active_generation: None,
        }
    }

    /// Returns the shared identity, if a successful completion has published it.
    #[must_use]
    pub fn snapshot(&self) -> Option<&HostIdentitySnapshot> {
        self.snapshot.as_ref()
    }

    /// Returns the latest admitted generation, including settled generations.
    #[must_use]
    pub const fn current_generation(&self) -> HostIdentityGeneration {
        self.current_generation
    }

    /// Returns the generation whose request is currently in flight, if any.
    #[must_use]
    pub const fn in_flight_generation(&self) -> Option<HostIdentityGeneration> {
        self.active_generation
    }

    /// Whether one admitted refresh still awaits completion or failure.
    #[must_use]
    pub const fn is_refresh_in_flight(&self) -> bool {
        self.active_generation.is_some()
    }

    /// Applies one typed refresh, completion, or failure input.
    ///
    /// Refresh admission first checks the retained snapshot, then the active
    /// request, matching the legacy controller's no-op and duplicate
    /// suppression order. A current completion publishes and clears its active
    /// generation. A stale completion never changes the snapshot or active
    /// request. A matching failure clears only that active generation and
    /// leaves the snapshot untouched.
    #[must_use]
    pub fn apply(&mut self, input: HostIdentityInput) -> HostIdentityAction {
        match input {
            HostIdentityInput::Refresh => self.admit_refresh(),
            HostIdentityInput::Complete {
                generation,
                snapshot,
            } => self.complete_refresh(generation, snapshot),
            HostIdentityInput::Fail { generation } => self.fail_refresh(generation),
        }
    }

    fn admit_refresh(&mut self) -> HostIdentityAction {
        if self.snapshot.is_some() {
            return HostIdentityAction::RefreshSuppressed {
                reason: RefreshSuppression::AlreadyLoaded,
            };
        }
        if let Some(generation) = self.active_generation {
            return HostIdentityAction::RefreshSuppressed {
                reason: RefreshSuppression::InFlight { generation },
            };
        }

        let Some(generation) = self.current_generation.next() else {
            return HostIdentityAction::RefreshSuppressed {
                reason: RefreshSuppression::GenerationExhausted,
            };
        };

        self.current_generation = generation;
        self.active_generation = Some(generation);
        HostIdentityAction::RefreshAdmitted { generation }
    }

    fn complete_refresh(
        &mut self,
        generation: HostIdentityGeneration,
        snapshot: HostIdentitySnapshot,
    ) -> HostIdentityAction {
        if self.current_generation != generation {
            return HostIdentityAction::CompletionIgnored {
                generation,
                reason: CompletionIgnoredReason::StaleGeneration,
            };
        }
        if self.active_generation != Some(generation) {
            return HostIdentityAction::CompletionIgnored {
                generation,
                reason: CompletionIgnoredReason::NoMatchingActiveRefresh,
            };
        }

        self.snapshot = Some(snapshot.clone());
        self.active_generation = None;
        HostIdentityAction::IdentityPublished {
            generation,
            snapshot,
        }
    }

    fn fail_refresh(&mut self, generation: HostIdentityGeneration) -> HostIdentityAction {
        if self.active_generation != Some(generation) {
            return HostIdentityAction::FailureIgnored {
                generation,
                reason: FailureIgnoredReason::NoMatchingActiveRefresh,
            };
        }

        self.active_generation = None;
        HostIdentityAction::RefreshFailed { generation }
    }
}
