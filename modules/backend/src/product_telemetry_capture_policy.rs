//! Vendor-neutral product-telemetry capture admission and containment.
//!
//! The TypeScript boundary owns event decoding and the concrete adapter. This
//! leaf receives only their observations and returns a typed dispatch decision;
//! it does not validate event fields, call an adapter, or retain runtime state.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// The caller-supplied canonical identity for one product-telemetry event.
///
/// The policy treats the value as opaque. In particular, it does not trim,
/// normalize, validate, or otherwise rewrite the ID before dispatching it.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CanonicalEventId(String);

impl CanonicalEventId {
    /// Wraps a caller-supplied canonical event ID without changing its bytes.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrows the exact canonical spelling supplied by the caller.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the exact owned canonical spelling supplied by the caller.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for CanonicalEventId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for CanonicalEventId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// An opaque event token produced by the already-existing observability
/// decoder.
///
/// No event name or property is represented here. The decoder remains the
/// owner of that schema, while this token proves only that decoding succeeded.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DecodedProductTelemetryEvent;

impl DecodedProductTelemetryEvent {
    /// Creates the opaque success token supplied by a decoder observation.
    pub const fn new() -> Self {
        Self
    }
}

impl Default for DecodedProductTelemetryEvent {
    fn default() -> Self {
        Self::new()
    }
}

/// The outcome observed at the existing product-telemetry decoder boundary.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DecoderObservation {
    /// Decoding produced a valid event token.
    Decoded(DecodedProductTelemetryEvent),
    /// The supplied value was invalid for the decoder's event schema.
    Invalid,
    /// The decoder failed while producing an event.
    Failed,
}

impl DecoderObservation {
    /// Returns a successful decoder observation without event-schema details.
    pub const fn decoded() -> Self {
        Self::Decoded(DecodedProductTelemetryEvent::new())
    }

    /// Returns whether this observation contains a decoded event token.
    #[must_use]
    pub const fn is_decoded(self) -> bool {
        matches!(self, Self::Decoded(_))
    }
}

/// The outcome observed at the existing adapter capture boundary.
///
/// The adapter's ordinary error and exceptional/cause error values are
/// deliberately not carried across this policy seam. Both are containment
/// cases and have the same caller-facing completion.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AdapterCaptureObservation {
    /// The adapter reported successful capture.
    Succeeded,
    /// The adapter reported an ordinary failure.
    OrdinaryFailure,
    /// The adapter failed with an exceptional or cause-level failure.
    ExceptionalCauseFailure,
}

/// Selects whether product telemetry is admitted to the adapter boundary.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProductTelemetryCaptureMode {
    /// Product telemetry is inert and never dispatches.
    Noop,
    /// Valid decoded events are admitted for one adapter dispatch.
    Live,
}

/// One complete, typed input observation for a capture attempt.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProductTelemetryCaptureInput {
    /// The canonical event ID supplied by the caller.
    pub canonical_event_id: CanonicalEventId,
    /// The result observed from the existing event decoder.
    pub decoder: DecoderObservation,
    /// The result observed from the existing adapter boundary.
    pub adapter: AdapterCaptureObservation,
}

impl ProductTelemetryCaptureInput {
    /// Creates a typed capture observation without changing any input.
    pub const fn new(
        canonical_event_id: CanonicalEventId,
        decoder: DecoderObservation,
        adapter: AdapterCaptureObservation,
    ) -> Self {
        Self {
            canonical_event_id,
            decoder,
            adapter,
        }
    }
}

/// The typed request that would be sent to the existing adapter exactly once.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AdapterCaptureRequest {
    event: DecodedProductTelemetryEvent,
    canonical_event_id: CanonicalEventId,
}

impl AdapterCaptureRequest {
    /// Returns the opaque event token admitted by the decoder.
    pub const fn event(&self) -> DecodedProductTelemetryEvent {
        self.event
    }

    /// Borrows the exact canonical event ID forwarded to the adapter.
    pub fn canonical_event_id(&self) -> &CanonicalEventId {
        &self.canonical_event_id
    }
}

/// Describes whether this capture attempt dispatches to the adapter.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum CaptureDispatch {
    /// No adapter dispatch is admitted.
    None,
    /// Exactly one adapter dispatch is admitted.
    Once(AdapterCaptureRequest),
}

impl CaptureDispatch {
    /// Returns the number of adapter dispatches admitted by this decision.
    #[must_use]
    pub const fn count(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Once(_) => 1,
        }
    }

    /// Returns the one adapter request, when dispatch was admitted.
    #[must_use]
    pub const fn request(&self) -> Option<&AdapterCaptureRequest> {
        match self {
            Self::None => None,
            Self::Once(request) => Some(request),
        }
    }

    /// Returns whether exactly one adapter dispatch was admitted.
    #[must_use]
    pub const fn is_once(&self) -> bool {
        matches!(self, Self::Once(_))
    }
}

/// The caller-facing completion after capture handling.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CaptureCompletion {
    /// The capture attempt completed successfully, including containment of
    /// decode, ordinary adapter, and exceptional adapter failures.
    Succeeded,
}

/// The dispatch decision and contained completion for one capture attempt.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CaptureDecision {
    /// The adapter dispatch admitted by the policy, if any.
    pub dispatch: CaptureDispatch,
    /// The successful completion visible to the caller.
    pub completion: CaptureCompletion,
}

impl CaptureDecision {
    /// Returns whether this decision admits one adapter dispatch.
    #[must_use]
    pub const fn dispatched_once(&self) -> bool {
        self.dispatch.is_once()
    }
}

/// Pure product-telemetry capture policy.
///
/// The policy is intentionally stateless: each supplied input is an
/// independent capture attempt. It emits a dispatch decision but never calls
/// an adapter, reads a clock, schedules work, or persists data.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProductTelemetryCapturePolicy {
    mode: ProductTelemetryCaptureMode,
}

impl ProductTelemetryCapturePolicy {
    /// Creates a policy with the supplied explicit capture mode.
    pub const fn new(mode: ProductTelemetryCaptureMode) -> Self {
        Self { mode }
    }

    /// Creates the inert no-op policy mode.
    pub const fn noop() -> Self {
        Self::new(ProductTelemetryCaptureMode::Noop)
    }

    /// Creates the live policy mode.
    pub const fn live() -> Self {
        Self::new(ProductTelemetryCaptureMode::Live)
    }

    /// Returns the explicit mode owned by this policy.
    pub const fn mode(self) -> ProductTelemetryCaptureMode {
        self.mode
    }

    /// Decides admission and contains every supplied failure observation.
    ///
    /// In [`ProductTelemetryCaptureMode::Noop`] mode, no input can dispatch.
    /// In live mode, only [`DecoderObservation::Decoded`] admits exactly one
    /// request carrying the untouched canonical event ID. Invalid and failed
    /// decoder observations are silently dropped. The adapter observation is
    /// already an observation of the existing boundary; all three outcomes
    /// complete successfully and do not alter the dispatch decision.
    pub fn capture(self, input: ProductTelemetryCaptureInput) -> CaptureDecision {
        let dispatch = match (self.mode, input.decoder) {
            (ProductTelemetryCaptureMode::Live, DecoderObservation::Decoded(event)) => {
                CaptureDispatch::Once(AdapterCaptureRequest {
                    event,
                    canonical_event_id: input.canonical_event_id,
                })
            }
            (ProductTelemetryCaptureMode::Noop, _)
            | (
                ProductTelemetryCaptureMode::Live,
                DecoderObservation::Invalid | DecoderObservation::Failed,
            ) => CaptureDispatch::None,
        };

        CaptureDecision {
            dispatch,
            completion: CaptureCompletion::Succeeded,
        }
    }
}
