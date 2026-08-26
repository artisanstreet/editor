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
use crate::credential_authority::CredentialAuthority;
use crate::request_handler::RequestHandler;

/// Fixed application close code used whenever this listener releases its own
/// endpoint or an admitted connection it guards.
const LISTENER_CLOSE_CODE: VarInt = VarInt::from_u32(0x01);

/// Fixed secret-free reason paired with [`LISTENER_CLOSE_CODE`].
const LISTENER_CLOSE_REASON: &[u8] = b"forge listener released";

#[cfg(test)]
#[path = "../../../tests/backend/listener_configuration.rs"]
mod listener_configuration;

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
    /// Phases: reserve one admission from remaining capacity (typed
    /// exhaustion before touching the endpoint); one deadline-bounded
    /// accept-and-TLS operation; guard the accepted connection before any
    /// fallible metadata call; transfer it into the existing authentication
    /// seam; then drive sequential consuming dispatches up to the configured
    /// per-connection capacity with freshly acquired stamps.
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
        // The listener stays one owning local throughout (it implements
        // Drop, so its fields are never moved out); phases work through
        // disjoint field borrows of this single value.
        let mut listener = self;

        // Reserve exactly once, with checked arithmetic, before touching
        // the endpoint.
        let Some(reserved) = listener.admission_remaining.checked_sub(1) else {
            return Err(ListenerError::AdmissionCapacityExhausted);
        };
        listener.admission_remaining = reserved;

        // One bounded decision covers waiting for a peer and completing
        // its QUIC/TLS handshake; there is no unbounded gap between them.
        let guarded = match run_with_deadline(
            OperationKind::Connect,
            listener.limits.admission,
            cancel,
            establish(&listener.endpoint),
        )
        .await
        {
            Ok(connection) => AcceptedGuard::armed(connection),
            Err(source) => return Err(ListenerError::Admission { source }),
        };

        // Guard first, fallible metadata second: a metadata failure drops
        // the guard and closes the actual peer connection synchronously.
        let metadata = welcome_metadata(listener.origin.as_ref())
            .map_err(|source| ListenerError::Metadata { source })?;

        // Transfer into the accepted seam, whose own synchronous guard
        // takes over eager closure for the whole authentication stage.
        let connection = guarded.release();
        let connection_limits = ConnectionLimits {
            handshake: listener.limits.handshake,
            next_request: listener.limits.next_request,
        };
        let mut connection = match ForgeConnection::authenticate(
            connection,
            &mut listener.authority,
            handler,
            metadata,
            connection_limits,
            cancel,
        )
        .await
        {
            Ok(owner) => owner,
            Err(source) => return Err(ListenerError::Authentication { source }),
        };

        // Sequential consuming dispatches up to the explicit capacity.
        let capacity = listener.requests_per_connection.get();
        let mut completed_requests: u32 = 0;
        let termination = loop {
            if completed_requests >= capacity {
                // Budget reached: explicitly close the ready owner here.
                drop(connection);
                break RequestTermination::BudgetReached;
            }
            let stamp = frame_stamp(listener.origin.as_ref())
                .map_err(|source| ListenerError::Metadata { source })?;
            match connection.respond_next(stamp).await {
                Ok(returned) => {
                    connection = returned;
                    completed_requests += 1;
                }
                Err(source) => break RequestTermination::Failed { source },
            }
        };

        // A failed consuming call already consumed and closed its
        // connection; the budget path dropped the ready owner above.
        // Either way the lease is released before custody returns.
        Ok((
            listener,
            ServiceReport {
                completed_requests,
                termination,
            },
        ))
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
