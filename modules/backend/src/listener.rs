//! Owning loopback Forge listener composing the accepted seams.
//!
//! [`ForgeListener`] exclusively owns one Quinn endpoint bound on
//! `127.0.0.1:0`, the single-use [`CredentialAuthority`], one existing
//! [`CommandOrigin`] implementation acquired for server Welcome/request
//! metadata, explicit finite [`ListenerLimits`], and two caller-supplied
//! [`NonZeroU32`] capacities. It never owns the [`ForgeApp`](crate::app::ForgeApp),
//! storage, repository, [`RequestHandler`], Tokio runtime, or spawned tasks;
//! callers own those lifetimes and close them through their own boundaries.
//!
//! # Linear service custody
//!
//! [`ForgeListener::serve_one`] consumes ownership exactly like the accepted
//! [`ForgeConnection`] work: it returns a future owning the entire listener,
//! and only a successfully authenticated **and finished** connection hands
//! custody back together with a typed [`ServiceReport`]. Every pre-ready
//! failure is conservatively terminal: the typed error is returned while the
//! owned listener drops, closing its endpoint. Post-ready request-stage
//! failures instead return reusable custody plus the preserved typed failure
//! so the caller may deliberately reconnect using the rotated secret. A
//! dropped serve future owns the listener and is therefore terminal too.
//!
//! Because the listener implements [`Drop`], its fields are never moved out;
//! every phase works through disjoint borrows of the one owned value, and
//! custody returns only after the connection lease has been released.
//!
//! [`Drop`] explicitly closes the owned endpoint with a fixed payload-free
//! code and reason, so dropping even an unpolled owning serve or drain future
//! closes the endpoint locally. That proves local closure only — never peer
//! receipt, awaited endpoint idle, database shutdown, or hard real-time
//! cleanup; those require the awaited boundaries below.
//!
//! # Long-lived production loop
//!
//! [`ForgeListener::serve_until_cancel`] owns one listener for its entire
//! lifetime and serves sequential connections until cancellation or a terminal
//! failure. It reuses the same private borrowed attempt as [`Self::serve_one`]
//! without rebinding, cloning the endpoint, detaching a task, or adding a
//! second runtime. Cancellation is checked before each admission: on
//! cancellation the owned endpoint is closed and awaited via [`Self::drain`].
//! An idle [`OperationKind::Connect`] timeout is nonterminal and continues
//! with the same endpoint without consuming admission capacity. A
//! connection-local authentication failure after the accepted guard closes the
//! peer is nonterminal. All other pre-ready exhaustion, metadata, and
//! non-timeout admission failures, plus non-cancellation request-stage
//! failures, are terminal service failures. A terminal primary remains
//! classified as a service failure even when the best-effort drain also
//! fails; both typed causes are preserved.
//!
//! [`Drop`] remains the synchronous local-close proof; [`Self::drain`] is the
//! awaited idle proof.
//!
//! # Metadata provenance
//!
//! No new identity trait, encoder, or clock exists here. Server metadata is
//! minted through the injected [`CommandOrigin`]: `mint_identity` text is
//! validated through the existing [`ConnectionId`] and [`FrameId`] types, and
//! `acceptance_instant` stamps every outgoing frame. Identity text is opaque
//! routing data; credential material never enters it, and no error in this
//! module formats secret payload bytes.

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use artisan_protocol::{ConnectionId, FrameId, LocalCapability, ProtocolValueError};
use artisan_transport::{
    CancelHandle, DeadlineError, OperationKind, TransportError, bind_loopback_server,
    run_with_deadline, shutdown,
};
use quinn::{Connection, ConnectionError, Endpoint, ServerConfig, VarInt};
use thiserror::Error;

use crate::command_admission::{CommandOrigin, CommandOriginClockError, CommandOriginEntropyError};
use crate::connection::{
    AuthenticationStageError, ConnectionLimits, ForgeConnection, RequestStageError,
    ServerFrameStamp, WelcomeMetadata,
};
use crate::credential_authority::{CredentialAuthenticationError, CredentialAuthority};
use crate::lifecycle_control::LifecycleController;
use crate::request_handler::RequestHandler;

/// Fixed application close code used whenever this listener releases its own
/// endpoint or an admitted connection it guards.
const LISTENER_CLOSE_CODE: VarInt = VarInt::from_u32(0x01);

/// Fixed secret-free reason paired with [`LISTENER_CLOSE_CODE`].
const LISTENER_CLOSE_REASON: &[u8] = b"forge listener released";

#[cfg(test)]
#[path = "../../../tests/backend/listener_configuration.rs"]
mod listener_configuration;

#[cfg(test)]
#[path = "../../../tests/backend/lifecycle_listener.rs"]
pub(crate) mod lifecycle_listener_tests;

/// Caller-supplied finite bounds for one listener. There is deliberately no
/// `Default`: assembly selects every value.
///
/// Zero durations are valid and mean the established precedence-only decision
/// (`DeadlineError::Timeout` before the bounded operation is ever polled);
/// they are never evidence about in-flight stages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListenerLimits {
    /// Combined bound covering waiting for one incoming peer and completing
    /// its QUIC/TLS handshake, applied through one deadline decision.
    pub admission: Duration,

    /// Bound covering one complete authentication handshake after admission.
    pub handshake: Duration,

    /// Bound covering one complete request dispatch on a ready connection.
    pub next_request: Duration,

    /// Bound granted to the awaited endpoint drain during teardown.
    pub drain: Duration,
}

impl ListenerLimits {
    /// Returns whether every limit can produce a future instant under the
    /// same instant-plus-duration overflow rule the transport deadline
    /// mechanism applies before polling a bounded operation.
    ///
    /// `Duration::MAX`, for example, is rejected here instead of surfacing
    /// later as [`DeadlineError::InvalidLimit`].
    #[must_use]
    pub fn representable(&self) -> bool {
        let reference = Instant::now();
        [
            self.admission,
            self.handshake,
            self.next_request,
            self.drain,
        ]
        .iter()
        .all(|limit| reference.checked_add(*limit).is_some())
    }
}

/// Failure of the owning Forge listener lifecycle.
#[derive(Debug, Error)]
pub enum ListenerError {
    /// At least one configured limit cannot produce a future instant.
    #[error("listener limits are not representable as future instants")]
    UnrepresentableLimits,

    /// Binding the loopback endpoint failed before any listener existed.
    #[error("binding the loopback listener failed")]
    Bind(#[source] TransportError),

    /// The lifetime admission capacity was exhausted before accepting another
    /// peer. Nothing was dequeued, and returning this error drops the
    /// listener.
    #[error("admission capacity was exhausted")]
    AdmissionCapacityExhausted,

    /// Waiting for a peer, completing its QUIC/TLS handshake, or the bounded
    /// decision itself failed. Returning this error drops the listener.
    #[error("admitting the next connection failed")]
    Admission {
        /// Typed combined accept-and-TLS failure preserved verbatim.
        #[source]
        source: DeadlineError<AdmissionCause>,
    },

    /// Validated Welcome or stamp metadata could not be acquired from the
    /// injected origin. Terminal at any stage; returning this error drops the
    /// listener.
    #[error("acquiring validated server metadata failed")]
    Metadata {
        /// Typed acquisition or validation failure.
        #[source]
        source: MetadataError,
    },

    /// Any authentication failure after admission. Conservatively terminal:
    /// the authority state is never probed, reused, or rolled back, and
    /// returning this error drops the listener.
    #[error("authenticating the accepted connection failed")]
    Authentication {
        /// Typed handshake-stage failure preserved verbatim.
        #[source]
        source: DeadlineError<AuthenticationStageError>,
    },
}

/// Typed cause of one combined accept-and-establishment failure.
#[derive(Debug, Error)]
pub enum AdmissionCause {
    /// The endpoint stopped offering incoming connections (`None`).
    #[error("the endpoint stopped offering incoming connections")]
    EndpointClosed,

    /// The dequeued peer failed during QUIC/TLS establishment.
    #[error("the incoming QUIC/TLS handshake failed")]
    Tls {
        /// Underlying Quinn connection failure, preserved verbatim.
        #[source]
        source: ConnectionError,
    },
}

/// Failure to acquire or validate one piece of server-originated metadata.
#[derive(Debug, Error)]
pub enum MetadataError {
    /// Operating-system entropy failed behind a fresh identity mint.
    #[error("metadata identity acquisition failed")]
    Entropy {
        /// Typed platform entropy failure, preserved verbatim.
        #[source]
        source: CommandOriginEntropyError,
    },

    /// The acceptance instant could not be represented.
    #[error("metadata acceptance instant acquisition failed")]
    Clock {
        /// Typed clock failure, preserved verbatim.
        #[source]
        source: CommandOriginClockError,
    },

    /// Minted text failed existing [`ConnectionId`] validation.
    #[error("minted connection identity failed validation")]
    ConnectionId {
        /// Typed protocol-value failure, preserved verbatim.
        #[source]
        source: ProtocolValueError,
    },

    /// Minted text failed existing [`FrameId`] validation.
    #[error("minted frame identity failed validation")]
    FrameId {
        /// Typed protocol-value failure, preserved verbatim.
        #[source]
        source: ProtocolValueError,
    },
}

/// Why one served connection stopped receiving requests.
#[derive(Debug)]
pub enum RequestTermination {
    /// The explicit per-connection dispatch capacity was reached and the
    /// ready connection was closed deliberately. This counts local completed
    /// dispatches only; it is not proof the peer consumed any response.
    BudgetReached,

    /// A typed request-stage failure, deadline, or caller cancellation ended
    /// the connection. The full typed decision is preserved verbatim.
    Failed {
        /// Typed request-stage decision, preserved verbatim.
        source: DeadlineError<RequestStageError>,
    },
}

/// Typed outcome report for one successfully authenticated connection that
/// has ended, returned together with reusable listener custody.
///
/// [`Self::completed_requests`] counts local dispatches whose correlated
/// reply was accepted locally and whose send side finished (`FIN` issued);
/// handler protocol failures are wire results and count here. This is never
/// proof of peer delivery or consumption, and reaching the budget closes the
/// connection without proving the peer consumed its last reply.
#[derive(Debug)]
pub struct ServiceReport {
    /// Completed sequential dispatches as defined above.
    pub completed_requests: u32,

    /// Whether the connection ended by reaching its explicit budget or by a
    /// preserved typed request-stage failure.
    pub termination: RequestTermination,
}

/// Typed cause for a terminal service failure inside the production loop.
#[derive(Debug, Error)]
pub enum ServiceCause {
    /// A listener-stage failure that is terminal for the production loop.
    #[error(transparent)]
    Listener(#[from] ListenerError),

    /// A non-cancellation request-stage failure that is terminal.
    #[error(transparent)]
    Request(#[from] DeadlineError<RequestStageError>),
}

/// Terminal error for the long-lived production loop
/// [`ForgeListener::serve_until_cancel`].
///
/// The type hides its fields and correlated booleans; callers distinguish
/// outcomes through the typed methods:
///
/// * `is_service_failure()` — primary non-cancellation service failure
///   (future exit 72), optionally carrying a drain failure without replacing
///   the primary classification;
/// * `is_drain_failure()` — cleanup-only listener drain failure after
///   cancellation (future exit 73).
///
/// No raw capability, credential, request payload, or frame bytes appear in
/// `Debug` or `Display`.
#[derive(Debug)]
pub struct ServeUntilCancelError {
    kind: Box<Kind>,
}

#[derive(Debug)]
enum Kind {
    Service {
        cause: ServiceCause,
        drain: Option<TransportError>,
    },
    Drain {
        drain: TransportError,
    },
}

impl ServeUntilCancelError {
    fn service(cause: ServiceCause, drain: Option<TransportError>) -> Self {
        Self {
            kind: Box::new(Kind::Service { cause, drain }),
        }
    }

    fn drain(drain: TransportError) -> Self {
        Self {
            kind: Box::new(Kind::Drain { drain }),
        }
    }

    /// Returns `true` when this is a primary non-cancellation service failure.
    #[must_use]
    pub fn is_service_failure(&self) -> bool {
        matches!(&*self.kind, Kind::Service { .. })
    }

    /// Returns `true` when this is a cleanup-only drain failure after cancellation.
    #[must_use]
    pub fn is_drain_failure(&self) -> bool {
        matches!(&*self.kind, Kind::Drain { .. })
    }

    /// Returns the primary service cause when this is a service failure.
    #[must_use]
    pub fn service_cause(&self) -> Option<&ServiceCause> {
        match &*self.kind {
            Kind::Service { cause, .. } => Some(cause),
            Kind::Drain { .. } => None,
        }
    }

    /// Returns the drain failure when present.
    ///
    /// For a service failure this is the optional best-effort drain error
    /// preserved alongside the primary; for a drain-only failure it is the
    /// primary drain error.
    #[must_use]
    pub fn drain_error(&self) -> Option<&TransportError> {
        match &*self.kind {
            Kind::Service { drain, .. } => drain.as_ref(),
            Kind::Drain { drain } => Some(drain),
        }
    }

    /// Convenience: the listener-stage service error, if any.
    #[must_use]
    pub fn as_listener_error(&self) -> Option<&ListenerError> {
        match self.service_cause() {
            Some(ServiceCause::Listener(error)) => Some(error),
            _ => None,
        }
    }

    /// Convenience: the request-stage service error, if any.
    #[must_use]
    pub fn as_request_error(&self) -> Option<&DeadlineError<RequestStageError>> {
        match self.service_cause() {
            Some(ServiceCause::Request(error)) => Some(error),
            _ => None,
        }
    }
}

impl std::fmt::Display for ServeUntilCancelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &*self.kind {
            Kind::Service { cause, drain } => {
                if drain.is_some() {
                    write!(
                        formatter,
                        "forge listener service failure ({cause}) with additional drain failure"
                    )
                } else {
                    write!(formatter, "forge listener service failure: {cause}")
                }
            }
            Kind::Drain { drain } => write!(
                formatter,
                "forge listener drain failure after cancellation: {drain}"
            ),
        }
    }
}

impl std::error::Error for ServeUntilCancelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &*self.kind {
            Kind::Service { cause, .. } => Some(cause),
            Kind::Drain { drain } => Some(drain),
        }
    }
}

/// One owning loopback Forge listener over the accepted seams.
///
/// The value implements neither `Clone` nor `Debug`. Dropping it closes the
/// owned endpoint synchronously with the fixed listener close code and
/// reason; awaiting [`Self::drain`] additionally proves endpoint idle.
pub struct ForgeListener {
    endpoint: Endpoint,
    authority: CredentialAuthority,
    origin: Box<dyn CommandOrigin>,
    limits: ListenerLimits,
    admission_remaining: u32,
    requests_per_connection: NonZeroU32,
    lifecycle: LifecycleController,
}

impl ForgeListener {
    /// Validates limits, applies the approved pending-peer bounds, and binds
    /// the loopback endpoint.
    ///
    /// The supplied configuration keeps every caller-selected TLS and
    /// established-transport setting; before binding, exactly three approved
    /// pending-peer mutations are applied (`max_incoming(8)`,
    /// `incoming_buffer_size(65_536)`,
    /// `incoming_buffer_size_total(524_288)` — methods verified against the
    /// pinned Quinn sources). They bound outstanding queued peers and their
    /// buffered bytes; each queued peer's first packet and all other
    /// allocation overhead are excluded, so this is not a total
    /// endpoint-memory cap.
    ///
    /// # Errors
    ///
    /// Returns [`ListenerError::UnrepresentableLimits`] before binding when
    /// any limit cannot produce a future instant, and
    /// [`ListenerError::Bind`] when the loopback socket cannot be bound.
    pub fn bind(
        server_config: ServerConfig,
        bootstrap: LocalCapability,
        origin: Box<dyn CommandOrigin>,
        limits: ListenerLimits,
        admission_capacity: NonZeroU32,
        requests_per_connection: NonZeroU32,
    ) -> Result<Self, ListenerError> {
        Self::bind_with_lifecycle(
            server_config,
            bootstrap,
            origin,
            limits,
            admission_capacity,
            requests_per_connection,
            LifecycleController::new(),
        )
    }

    /// Binds a listener with a crate-local lifecycle controller.
    pub(crate) fn bind_with_lifecycle(
        server_config: ServerConfig,
        bootstrap: LocalCapability,
        origin: Box<dyn CommandOrigin>,
        limits: ListenerLimits,
        admission_capacity: NonZeroU32,
        requests_per_connection: NonZeroU32,
        lifecycle: LifecycleController,
    ) -> Result<Self, ListenerError> {
        if !limits.representable() {
            return Err(ListenerError::UnrepresentableLimits);
        }

        let bounded_config = apply_approved_pending_peer_limits(server_config);

        let endpoint = bind_loopback_server(bounded_config).map_err(ListenerError::Bind)?;

        Ok(Self {
            endpoint,
            authority: CredentialAuthority::new(bootstrap),
            origin,
            limits,
            admission_remaining: admission_capacity.get(),
            requests_per_connection,
            lifecycle,
        })
    }

    /// Reads back the bound loopback address honestly.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O failure when the socket address can no
    /// longer be observed.
    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.endpoint.local_addr()
    }

    /// Serves exactly one admitted client to the end of its connection.
    ///
    /// The returned future owns the whole listener linearly: dropping even an
    /// unpolled future drops the listener and closes its endpoint. Custody
    /// comes back only after a successfully authenticated connection ended,
    /// paired with its [`ServiceReport`]; every pre-ready failure is
    /// terminal and drops the listener instead.
    ///
    /// Phases: one admission from remaining capacity (typed exhaustion before
    /// touching the endpoint); one deadline-bounded accept-and-TLS operation;
    /// guard the accepted connection before any fallible metadata call;
    /// transfer it into the existing authentication seam; then drive
    /// sequential consuming dispatches up to the configured per-connection
    /// capacity with freshly acquired stamps.
    ///
    /// Work cancellation is only observed through the supplied handle and is
    /// never triggered or reset here.
    ///
    /// # Errors
    ///
    /// See [`ListenerError`]: exhaustion, admission, metadata, and
    /// authentication failures consume the listener; only post-ready
    /// request-stage endings return reusable custody.
    #[must_use = "the owning service future must be driven; dropping it closes the listener"]
    pub async fn serve_one(
        self,
        handler: &RequestHandler,
        cancel: &CancelHandle,
    ) -> Result<(Self, ServiceReport), ListenerError> {
        let mut listener = self;
        let report = listener.serve_attempt(handler, cancel).await?;
        Ok((listener, report))
    }

    /// Long-lived production loop owning one listener for its whole lifetime.
    ///
    /// Sequentially serves connections until cancellation or a terminal
    /// failure. The same private borrowed attempt as [`Self::serve_one`] is
    /// reused without rebinding, cloning the endpoint, detaching a task, or
    /// adding a second runtime. Cancellation is checked before each admission;
    /// on cancellation the owned endpoint is closed and drained. An idle
    /// [`OperationKind::Connect`] timeout with no peer is nonterminal and
    /// continues with the same endpoint without consuming admission capacity.
    /// Authentication recoverability is fenced exactly: only
    /// `DeadlineError::Peer` containing `Accept` or `Handshake`, or
    /// `Credential(FamilyMismatch|Rejected)`, is retryable; every other
    /// authentication failure — `AwaitingRotation`, timeout, invalid limit,
    /// entropy, rotation, finish, and all other variants — is a terminal
    /// service failure because mutation may have occurred or recoverability is
    /// unproven. Admission-capacity exhaustion, local metadata/origin failure,
    /// non-timeout admission transport failure, and non-cancellation
    /// request-stage failure are terminal service failures; a terminal
    /// primary remains a service failure even when the best-effort drain also
    /// fails. Cancellation with a drain failure is the cleanup-only error.
    ///
    /// Preserves lifetime admission counting and per-connection request
    /// capacity exactly; idle timeouts neither consume nor refund an
    /// admission — only a proven accepted admission consumes capacity via
    /// checked arithmetic. Sequential dispatch, capability rotation,
    /// accepted-connection guards, endpoint close code/reason, and the
    /// honest synchronous-`Drop` versus awaited-`drain` documentation are
    /// preserved.
    ///
    /// # Errors
    ///
    /// Returns [`ServeUntilCancelError`] on terminal service or cleanup-only
    /// drain failure; `Ok(())` means cancellation with successful drain.
    pub async fn serve_until_cancel(
        self,
        handler: &RequestHandler,
        cancel: &CancelHandle,
    ) -> Result<(), ServeUntilCancelError> {
        let mut listener = self;
        loop {
            if cancel.is_cancelled() {
                return match listener.drain().await {
                    Ok(()) => Ok(()),
                    Err(drain) => Err(ServeUntilCancelError::drain(drain)),
                };
            }

            match listener.serve_attempt(handler, cancel).await {
                Ok(report) => match report.termination {
                    RequestTermination::BudgetReached => {}
                    RequestTermination::Failed { source } => {
                        if matches!(&source, DeadlineError::Cancelled { .. }) {
                            return match listener.drain().await {
                                Ok(()) => Ok(()),
                                Err(drain) => Err(ServeUntilCancelError::drain(drain)),
                            };
                        }
                        let cause = ServiceCause::Request(source);
                        return match listener.drain().await {
                            Ok(()) => Err(ServeUntilCancelError::service(cause, None)),
                            Err(drain) => Err(ServeUntilCancelError::service(cause, Some(drain))),
                        };
                    }
                },
                Err(error) => {
                    let is_cancelled = match &error {
                        ListenerError::Admission { source } => {
                            matches!(source, &DeadlineError::Cancelled { .. })
                        }
                        ListenerError::Authentication { source } => {
                            matches!(source, &DeadlineError::Cancelled { .. })
                        }
                        _ => false,
                    };
                    if is_cancelled {
                        return match listener.drain().await {
                            Ok(()) => Ok(()),
                            Err(drain) => Err(ServeUntilCancelError::drain(drain)),
                        };
                    }

                    let is_idle_timeout = match &error {
                        ListenerError::Admission { source } => matches!(
                            source,
                            &DeadlineError::Timeout {
                                operation: OperationKind::Connect,
                                ..
                            }
                        ),
                        _ => false,
                    };
                    if is_idle_timeout {
                        continue;
                    }

                    if let ListenerError::Authentication { source } = &error
                        && is_authentication_retryable(source)
                    {
                        continue;
                    }

                    let cause = ServiceCause::Listener(error);
                    return match listener.drain().await {
                        Ok(()) => Err(ServeUntilCancelError::service(cause, None)),
                        Err(drain) => Err(ServeUntilCancelError::service(cause, Some(drain))),
                    };
                }
            }
        }
    }

    /// Single-admission borrowed attempt reused by both `serve_one` and
    /// `serve_until_cancel`.
    ///
    /// Checks capacity without consuming it before touching the endpoint;
    /// only a proven accepted connection consumes capacity via checked
    /// arithmetic. One deadline-bounded accept-and-TLS operation, guard
    /// before metadata, authentication seam, and sequential dispatches up to
    /// the per-connection capacity follow. Idle timeouts therefore neither
    /// consume nor refund admission capacity.
    async fn serve_attempt(
        &mut self,
        handler: &RequestHandler,
        cancel: &CancelHandle,
    ) -> Result<ServiceReport, ListenerError> {
        if self.admission_remaining == 0 {
            return Err(ListenerError::AdmissionCapacityExhausted);
        }

        let guarded = match run_with_deadline(
            OperationKind::Connect,
            self.limits.admission,
            cancel,
            establish(&self.endpoint),
        )
        .await
        {
            Ok(connection) => {
                let Some(remaining) = self.admission_remaining.checked_sub(1) else {
                    let guard = AcceptedGuard::armed(connection);
                    drop(guard);
                    return Err(ListenerError::AdmissionCapacityExhausted);
                };
                self.admission_remaining = remaining;
                AcceptedGuard::armed(connection)
            }
            Err(source) => return Err(ListenerError::Admission { source }),
        };

        let metadata = welcome_metadata(self.origin.as_ref())
            .map_err(|source| ListenerError::Metadata { source })?;

        let connection = guarded.release();
        let connection_limits = ConnectionLimits {
            handshake: self.limits.handshake,
            next_request: self.limits.next_request,
        };
        let connection = match ForgeConnection::authenticate(
            connection,
            &mut self.authority,
            handler,
            &self.lifecycle,
            metadata,
            connection_limits,
            cancel,
        )
        .await
        {
            Ok(owner) => owner,
            Err(source) => return Err(ListenerError::Authentication { source }),
        };

        let capacity = self.requests_per_connection.get();
        let mut metadata_error = None;
        match connection
            .drive_until_end(capacity, || match frame_stamp(self.origin.as_ref()) {
                Ok(stamp) => Ok(stamp),
                Err(source) => {
                    metadata_error = Some(source);
                    Err(RequestStageError::Stamp)
                }
            })
            .await
        {
            Ok((connection, completed_requests)) => {
                drop(connection);
                Ok(ServiceReport {
                    completed_requests,
                    termination: RequestTermination::BudgetReached,
                })
            }
            Err(failure) => {
                let (completed_requests, source) = failure.into_parts();
                if let Some(source) = metadata_error {
                    return Err(ListenerError::Metadata { source });
                }
                Ok(ServiceReport {
                    completed_requests,
                    termination: RequestTermination::Failed { source },
                })
            }
        }
    }

    /// Consuming teardown: closes every connection and waits until the owned
    /// endpoint reaches idle state under this listener's drain limit.
    ///
    /// Deliberately takes no sticky work-cancel handle, so cancellation of
    /// earlier work can never skip cleanup. Only this awaited boundary proves
    /// endpoint idle; plain [`Drop`] proves local closure only.
    ///
    /// # Errors
    ///
    /// Returns the underlying typed transport failure when connections stay
    /// open past the drain limit.
    pub async fn drain(self) -> Result<(), TransportError> {
        shutdown(
            &self.endpoint,
            LISTENER_CLOSE_CODE,
            LISTENER_CLOSE_REASON,
            self.limits.drain,
        )
        .await
    }
}

impl Drop for ForgeListener {
    fn drop(&mut self) {
        // Explicit close, not reliance on raw handle drops: this proves local
        // closure of the owned endpoint and nothing more.
        self.endpoint
            .close(LISTENER_CLOSE_CODE, LISTENER_CLOSE_REASON);
    }
}

/// Returns whether an authentication `DeadlineError` is retryable without
/// risking a fail-closed `CredentialAuthority`.
///
/// Only `Peer` containing `Accept`, `Handshake`, or
/// `Credential(FamilyMismatch|Rejected)` is retryable per the
/// `ForgeConnection::authenticate` contract; every other variant — including
/// `AwaitingRotation`, timeout, invalid limit, entropy, rotation, finish, and
/// all other failures — may have consumed the credential and left rotation
/// pending, so it is terminal.
fn is_authentication_retryable(source: &DeadlineError<AuthenticationStageError>) -> bool {
    match source {
        DeadlineError::Peer { error, .. } => match error {
            AuthenticationStageError::Accept { .. } | AuthenticationStageError::Handshake(_) => {
                true
            }
            AuthenticationStageError::Credential(inner) => matches!(
                *inner,
                CredentialAuthenticationError::FamilyMismatch { .. }
                    | CredentialAuthenticationError::Rejected { .. }
            ),
            _ => false,
        },
        _ => false,
    }
}

/// Applies exactly the three approved pending-peer bounds to the supplied
/// server configuration and returns it otherwise unchanged.
///
/// These byte bounds exclude each queued peer's first packet and all other
/// allocation overhead; this is production wiring for the approved values,
/// not a statement about total endpoint memory.
fn apply_approved_pending_peer_limits(mut server_config: ServerConfig) -> ServerConfig {
    // Applied as plain statements so neither unit nor builder return shapes
    // matter across the pinned Quinn sources.
    server_config.max_incoming(8);
    server_config.incoming_buffer_size(65_536);
    server_config.incoming_buffer_size_total(524_288);
    server_config
}

/// Bounded accept-and-establishment stage driven by one deadline decision.
async fn establish(endpoint: &Endpoint) -> Result<Connection, AdmissionCause> {
    // `Endpoint::accept` itself is a future yielding the queued incoming
    // peer; awaiting it and then awaiting that peer are both covered by the
    // caller's single deadline decision.
    let Some(incoming) = endpoint.accept().await else {
        return Err(AdmissionCause::EndpointClosed);
    };
    incoming
        .await
        .map_err(|source| AdmissionCause::Tls { source })
}

/// Synchronous guard holding one freshly established connection.
///
/// Armed immediately after TLS completes and released only once metadata
/// succeeded, it closes the real peer connection with the fixed listener
/// code/reason across any metadata-failure drop window.
struct AcceptedGuard {
    connection: Option<Connection>,
}

impl AcceptedGuard {
    const fn armed(connection: Connection) -> Self {
        Self {
            connection: Some(connection),
        }
    }

    /// Takes joint ownership out exactly once, after fallible setup succeeded.
    fn release(mut self) -> Connection {
        self.connection
            .take()
            .expect("guarded connection is released exactly once")
    }
}

impl Drop for AcceptedGuard {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.close(LISTENER_CLOSE_CODE, LISTENER_CLOSE_REASON);
        }
    }
}

/// Mints and validates the Welcome connection identity through the existing
/// [`ConnectionId`] type.
fn connection_id(origin: &dyn CommandOrigin) -> Result<ConnectionId, MetadataError> {
    let text = origin
        .mint_identity()
        .map_err(|source| MetadataError::Entropy { source })?;
    ConnectionId::parse(text).map_err(|source| MetadataError::ConnectionId { source })
}

/// Mints and validates one frame stamp through the existing [`FrameId`] type
/// and the origin's acceptance instant.
fn frame_stamp(origin: &dyn CommandOrigin) -> Result<ServerFrameStamp, MetadataError> {
    let text = origin
        .mint_identity()
        .map_err(|source| MetadataError::Entropy { source })?;
    let frame_id = FrameId::parse(text).map_err(|source| MetadataError::FrameId { source })?;
    let sent_at = origin
        .acceptance_instant()
        .map_err(|source| MetadataError::Clock { source })?;
    Ok(ServerFrameStamp { frame_id, sent_at })
}

/// Mints the complete validated Welcome metadata for one admission.
fn welcome_metadata(origin: &dyn CommandOrigin) -> Result<WelcomeMetadata, MetadataError> {
    Ok(WelcomeMetadata {
        connection_id: connection_id(origin)?,
        frame: frame_stamp(origin)?,
    })
}

#[cfg(test)]
mod auth_retry_tests {
    use super::*;
    use artisan_transport::{DeadlineError, OperationKind};
    use quinn::ConnectionError;

    fn peer_error(error: AuthenticationStageError) -> DeadlineError<AuthenticationStageError> {
        DeadlineError::Peer {
            operation: OperationKind::Handshake,
            error,
        }
    }

    #[test]
    fn accept_and_handshake_are_retryable() {
        let accept: DeadlineError<AuthenticationStageError> =
            peer_error(AuthenticationStageError::Accept {
                source: ConnectionError::VersionMismatch,
            });
        assert!(is_authentication_retryable(&accept));

        let handshake: DeadlineError<AuthenticationStageError> =
            peer_error(AuthenticationStageError::Handshake(
                artisan_transport::HandshakeError::UnexpectedMessage {
                    expected: artisan_transport::HandshakeMessageKind::Hello,
                    received: artisan_transport::HandshakeMessageKind::Welcome,
                },
            ));
        assert!(is_authentication_retryable(&handshake));
    }

    #[test]
    fn family_mismatch_and_rejected_are_retryable() {
        let family = peer_error(AuthenticationStageError::Credential(
            CredentialAuthenticationError::FamilyMismatch {
                expected: crate::credential_authority::CredentialKind::Initial,
                presented: crate::credential_authority::CredentialKind::Reconnect,
            },
        ));
        assert!(is_authentication_retryable(&family));

        let rejected = peer_error(AuthenticationStageError::Credential(
            CredentialAuthenticationError::Rejected {
                kind: crate::credential_authority::CredentialKind::Initial,
            },
        ));
        assert!(is_authentication_retryable(&rejected));
    }

    #[test]
    fn awaiting_rotation_is_terminal() {
        let awaiting = peer_error(AuthenticationStageError::Credential(
            CredentialAuthenticationError::AwaitingRotation,
        ));
        assert!(!is_authentication_retryable(&awaiting));
    }

    #[test]
    fn timeout_is_terminal() {
        let timeout: DeadlineError<AuthenticationStageError> = DeadlineError::Timeout {
            operation: OperationKind::Handshake,
            limit: Duration::from_millis(10),
        };
        assert!(!is_authentication_retryable(&timeout));
    }

    #[test]
    fn invalid_limit_is_terminal() {
        let invalid: DeadlineError<AuthenticationStageError> = DeadlineError::InvalidLimit {
            operation: OperationKind::Handshake,
        };
        assert!(!is_authentication_retryable(&invalid));
    }

    #[test]
    fn rotation_is_terminal() {
        let rotation = peer_error(AuthenticationStageError::Rotation(
            crate::credential_authority::ReconnectRotationError::AlreadyTaken,
        ));
        assert!(!is_authentication_retryable(&rotation));
    }

    #[test]
    fn cancelled_is_not_retryable_via_helper() {
        let cancelled: DeadlineError<AuthenticationStageError> = DeadlineError::Cancelled {
            operation: OperationKind::Handshake,
        };
        assert!(!is_authentication_retryable(&cancelled));
    }
}
