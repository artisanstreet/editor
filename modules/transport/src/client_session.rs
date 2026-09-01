//! Caller-driven sequential Artisan client sessions over one private
//! loopback transport.
//!
//! [`ClientSession::connect`] creates exactly one fresh client endpoint
//! and connection, performs the pinned application handshake under
//! whole-stage deadlines and the caller's borrowed cancellation handle,
//! and returns `(ClientSession, ServerWelcome)`. The rotated capability
//! inside that Welcome belongs to the trusted calling coordinator from
//! that moment on — never to the session, any [`Debug`] output, or any
//! cloned state. The session retains only the negotiated non-secret
//! metadata its requests need (protocol revision, connection identity, and
//! the accepted lifecycle feature bit), exposes no raw Quinn getters, spawns
//! no background reader, task, channel, queue, or runtime, and carries one accepted
//! [`ClientRequestLifecycle`] through every successful request of the
//! connection's whole life.
//!
//! # Sequential consuming requests
//!
//! [`ClientSession::request`] consumes the owner and returns
//! `Result<(ClientSession, ResolvedRequest), ClientRequestError>`: the
//! waiter is private and never escapes, pending capacity is fixed at
//! [`PENDING_CAPACITY`] because the API inherently admits one operation,
//! and the caller supplies the explicit nonzero total admission budget
//! delegated unchanged to the accepted correlation registry — there is
//! no hidden budget. The frame identity of the request envelope is
//! authoritative, and only a reply settling exactly that identity may
//! resolve it.
//!
//! # Terminal request errors (conservative tradeoff)
//!
//! EVERY request-local error consumes the session and closes its
//! connection and endpoint: pre-I/O validation, admission rejection,
//! deadline expiry, caller cancellation, malformed frames, wrong version
//! or family or correlation, and wire failures all land here. The
//! conservative tradeoff is deliberate: the session never risks
//! desynchronizing from a peer whose answer may still be in flight, at
//! the cost of discarding a healthy transport after purely local
//! mistakes. A *matched* peer [`ProtocolFailure`](artisan_protocol::ProtocolFailure)
//! is not such an error — it is an ordinary [`ResolvedRequest`] outcome
//! carrying [`RequestOutcome::Failure`](crate::RequestOutcome::Failure),
//! and the same live owner survives it. Abandoning a request locally
//! proves nothing about durable Forge work behind it: no rollback or
//! peer-side cancellation is claimed, observed, or implied.
//! [`ClientSession::request_acknowledging_response`] keeps the same
//! admission and settlement contract but waits, after settlement, for the
//! peer's response stream to finish cleanly before returning the owner.
//!
//! # Whole-stage limits
//!
//! Connect, handshake, request, and shutdown each run under ONE
//! deadline covering the whole composed stage — configuration, binding,
//! dialing; stream open plus Hello/Welcome; stream open plus bounded
//! send plus bounded reply; close plus drain. No timer resets per await.
//! Zero limits follow the existing typed deadline semantics (they expire
//! before the stage future is polled) and unrepresentable limits report
//! [`DeadlineError::InvalidLimit`]; there are no fallback limits. The
//! closest [`OperationKind`] labels the composed stages honestly:
//! `Handshake` for the handshake stage including its open/send/receive,
//! `Receive` for a request stage for the same reason, including the
//! dedicated mode's post-settlement clean-EOF wait.
//!
//! # Guarded abandonment
//!
//! Resource guards are installed before any later await. Dropping a
//! connect future before anything was created has no endpoint or
//! connection to close, but its owned Hello credential is dropped and
//! zeroized; dropping one after creation, dropping a request future that
//! consumed a ready owner, or dropping one while pending stops inbound
//! delivery, resets unfinished outbound streams, and closes the owned
//! connection and endpoint with fixed private non-secret codes. Drop
//! closes synchronously only and promises no asynchronous drain;
//! [`ClientSession::shutdown`] is the sole awaited-drain path.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use artisan_domain::IdentifierError;
use artisan_protocol::{
    ClientRequest, ConnectionId, ProtocolVersion, WireEnvelope, WireEnvelopeBody,
};
use quinn::{Connection, Endpoint, RecvStream};
use rustls_pki_types::CertificateDer;
use thiserror::Error;

use crate::endpoint::{LOOPBACK_SERVER_NAME, client_config};
use crate::handshake::ServerWelcome;
use crate::handshake::kind as message_kind;
use crate::identity::PinnedIdentity;
use crate::request_correlation::RequestCorrelationError;
use crate::request_lifecycle::{ClientRequestLifecycle, ResolvedRequest};
use crate::{
    CancelHandle, DeadlineError, HandshakeMessageKind, OperationKind, TransportError,
    run_with_deadline,
};

mod exchange;
mod link;

pub use exchange::{ExchangeError, HandshakeStageError, ReplyRejection};

use exchange::{
    AcknowledgingResponseError, SettledReply, classify_reply, handshake_stage, request_stage,
    request_stage_acknowledging_response,
};
use link::SessionLink;

/// The one public failure condition for the consuming server-delivery
/// receiver.
///
/// The condition intentionally carries no Quinn error, close code, or peer
/// payload. A lost delivery stream is terminal for this receiver; callers
/// recover by establishing a fresh session and acquiring a fresh receiver.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("delivery stream was lost")]
pub struct DeliveryLost;

/// A consuming owner of the server-initiated delivery stream.
///
/// The receiver retains only a private clone of the session's connection and
/// the one accepted receive stream. It never exposes either Quinn handle.
/// Before the first successful acceptance, [`recv`](Self::recv) waits for one
/// server-opened unidirectional stream; after that, each consuming call reads
/// exactly one bounded application envelope from the same stream.
///
/// The type deliberately implements neither [`Clone`] nor [`Copy`]. A stream
/// that is abandoned while a frame is being read is stopped synchronously and
/// cannot be reused by a later call.
pub struct DeliveryReceiver {
    /// Private shared connection handle used only to accept the one inbound
    /// stream. The session owns the separate connection handle that keeps
    /// request and shutdown custody independent.
    connection: Connection,
    /// Accepted delivery stream, installed before any frame-read await.
    stream: Option<RecvStream>,
}

impl fmt::Debug for DeliveryReceiver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeliveryReceiver")
            .finish_non_exhaustive()
    }
}

impl DeliveryReceiver {
    /// Creates a receiver with no accepted stream.
    fn new(connection: Connection) -> Self {
        Self {
            connection,
            stream: None,
        }
    }

    /// Consumes the receiver to accept or use its delivery stream and read
    /// exactly one bounded envelope.
    ///
    /// The first call awaits exactly one `accept_uni`; subsequent calls use
    /// the already accepted stream. Cancellation is checked before the
    /// operation and wins through a biased selection both while accepting
    /// and while reading. Every cancellation, stream/connection failure,
    /// framing failure, decode failure, or abandoned operation is terminal:
    /// no receiver is returned. If a stream was accepted, dropping the
    /// consumed owner stops it with the private delivery-stop code before
    /// the error is observed by the caller.
    ///
    /// # Errors
    ///
    /// Returns [`DeliveryLost`] for every delivery failure. No Quinn or
    /// protocol-source error crosses this boundary.
    pub async fn recv(
        mut self,
        cancel: &CancelHandle,
    ) -> Result<(DeliveryReceiver, WireEnvelope), DeliveryLost> {
        if self.stream.is_none() {
            if cancel.is_cancelled() {
                return Err(DeliveryLost);
            }
            let stream = tokio::select! {
                biased;
                () = cancel.wait() => return Err(DeliveryLost),
                accepted = self.connection.accept_uni() => {
                    accepted.map_err(|_| DeliveryLost)?
                }
            };
            // Install custody immediately after acceptance and before the
            // first frame await. Any later escape therefore stops this exact
            // stream synchronously through `Drop`.
            self.stream = Some(stream);
        }

        if cancel.is_cancelled() {
            return Err(DeliveryLost);
        }
        let Some(stream) = self.stream.as_mut() else {
            return Err(DeliveryLost);
        };
        let envelope = tokio::select! {
            biased;
            () = cancel.wait() => return Err(DeliveryLost),
            envelope = crate::receive_envelope(stream) => {
                envelope.map_err(|_| DeliveryLost)?
            }
        };
        Ok((self, envelope))
    }
}

impl Drop for DeliveryReceiver {
    fn drop(&mut self) {
        // A receiver drop never closes the shared session connection. It
        // only stops an accepted inbound stream, including one abandoned in
        // the middle of a bounded frame read.
        if let Some(mut stream) = self.stream.take() {
            let _stopped = stream.stop(link::close_code(link::STREAM_STOP_CODE));
        }
    }
}

/// Fixed simultaneous pending-request capacity of the sequential API.
///
/// One [`ClientSession`] admits at most one in-flight request operation
/// by construction — the consuming call shape leaves nothing to overlap
/// — so this constant fixes the lifecycle's capacity instead of exposing
/// a tuning knob.
pub const PENDING_CAPACITY: usize = 1;

/// Caller-supplied bounds for one client session.
///
/// Every duration bounds one WHOLE composed stage, not an individual
/// await; see the [module documentation](self#whole-stage-limits). The
/// admission budget is delegated unchanged to the accepted correlation
/// registry and must be nonzero there; zero is rejected before any
/// network resource exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientSessionLimits {
    /// Whole connect stage: client configuration, private endpoint bind,
    /// and QUIC establishment.
    pub connect: Duration,
    /// Whole application handshake stage, including its stream
    /// open/send/receive under one [`OperationKind::Handshake`] label.
    pub handshake: Duration,
    /// Whole request stage per request, including stream open, the
    /// bounded send, and the bounded reply receive under one
    /// [`OperationKind::Receive`] label.
    pub request: Duration,
    /// Whole awaited shutdown drain.
    pub shutdown: Duration,
    /// Total successful admissions for the session's entire lifetime.
    pub admission_budget: usize,
}

/// Exact loopback dialing target validated before any network attempt.
///
/// The transport leaf supports exactly `127.0.0.1` with a nonzero port,
/// matching the loopback bind primitive; everything else is rejected by
/// [`LoopbackTarget::new`] before a socket can exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoopbackTarget(SocketAddr);

/// Why a candidate session target was rejected before any network
/// attempt.
///
/// The discriminants separate an unsupported address from a zero port:
/// loopback spellings this leaf cannot serve (IPv6 `::1`, other
/// `127.x.x.x` addresses) are unsupported addresses, not remote peers.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SessionTargetError {
    /// The address was not exactly `127.0.0.1`: remote addresses, IPv6
    /// (including `::1`), and every other spelling are unsupported.
    #[error("session target must be exactly 127.0.0.1")]
    UnsupportedAddress,
    /// The loopback address carried port zero.
    #[error("session target port must be nonzero")]
    ZeroPort,
}

impl LoopbackTarget {
    /// Validates `address` as exactly `127.0.0.1` with a nonzero port.
    ///
    /// The address is diagnosed ahead of the port: a non-loopback
    /// address is unsupported however its port reads.
    ///
    /// # Errors
    ///
    /// Returns [`SessionTargetError::UnsupportedAddress`] for every
    /// address other than IPv4 `127.0.0.1` and
    /// [`SessionTargetError::ZeroPort`] for the loopback address with
    /// port zero.
    pub fn new(address: SocketAddr) -> Result<Self, SessionTargetError> {
        let SocketAddr::V4(v4) = &address else {
            return Err(SessionTargetError::UnsupportedAddress);
        };
        // `SocketAddrV4::ip` hands back a reference; compare through it
        // explicitly so the exact-localhost rule is type-evident.
        if *v4.ip() != Ipv4Addr::LOCALHOST {
            return Err(SessionTargetError::UnsupportedAddress);
        }
        if v4.port() == 0 {
            return Err(SessionTargetError::ZeroPort);
        }
        Ok(Self(address))
    }

    /// Returns the validated target address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.0
    }
}

/// Failure while establishing one client session.
///
/// Configuration and binding failures are part of the whole connect
/// stage, so they surface inside
/// [`ClientSessionError::Connect`](Self::Connect) as
/// [`DeadlineError::Peer`] carrying the typed [`TransportError`] — there
/// is no separate route that bypasses the stage wrapper.
#[derive(Debug, Error)]
pub enum ClientSessionError {
    /// The lifecycle rejected its limits — a zero lifetime admission
    /// budget — before any resource was created.
    #[error("session request lifecycle rejected its limits: {0}")]
    Limits(#[from] RequestCorrelationError),
    /// The whole QUIC connect stage failed under its deadline: timeout,
    /// cancellation, unrepresentable limit, or a typed transport failure
    /// from configuration, binding, or dialing.
    #[error("quic connect stage failed: {0}")]
    Connect(#[source] DeadlineError<TransportError>),
    /// The whole application handshake stage failed under its deadline.
    #[error("application handshake stage failed: {0}")]
    Handshake(#[source] DeadlineError<HandshakeStageError>),
    /// The awaited shutdown drain failed under its finite limit.
    #[error("shutdown drain failed: {0}")]
    Shutdown(#[source] DeadlineError<TransportError>),
    /// Delivery acquisition was attempted a second time. The consuming
    /// session is dropped by this terminal error path.
    #[error("delivery receiver was already taken")]
    DeliveryAlreadyTaken,
}

/// Terminal failure of one sequential request; the session is always
/// consumed, and the connection and endpoint are closed on every variant.
#[derive(Debug, Error)]
pub enum ClientRequestError {
    /// The outbound envelope was not a request, diagnosed before any
    /// network attempt.
    #[error("expected a Request envelope, received {received}")]
    NotARequest {
        /// Payload-free family actually supplied.
        received: HandshakeMessageKind,
    },
    /// The outbound envelope's revision disagreed with the session's
    /// negotiated version, diagnosed before any network attempt.
    #[error(
        "request protocol version {envelope_version} disagrees with negotiated version {negotiated_version}"
    )]
    VersionMismatch {
        /// Revision stamped on the outbound envelope.
        envelope_version: u32,
        /// Revision negotiated during the session's handshake.
        negotiated_version: u32,
    },
    /// Native lifecycle control was not negotiated for this session,
    /// diagnosed before any request identity or network admission.
    #[error("native lifecycle control was not negotiated")]
    UnsupportedFeature,
    /// The request's frame identity could not seed its correlation
    /// identity. Unreachable for decoded envelopes, kept typed rather
    /// than panicked.
    #[error("request frame id cannot seed correlation: {0}")]
    Correlation(#[source] IdentifierError),
    /// The correlation registry rejected the admission: duplicate,
    /// retired identity, exhausted lifetime budget, or full capacity.
    /// The registry's rejection stays all-or-nothing; the higher owner
    /// closes instead of surviving or resetting.
    #[error("request admission failed: {0}")]
    Admission(#[source] RequestCorrelationError),
    /// The whole composed request exchange failed under its deadline.
    #[error("request exchange failed: {0}")]
    Exchange(#[source] DeadlineError<ExchangeError>),
    /// The reply failed version, family, correlation validation, or
    /// could not settle despite matching.
    #[error("reply rejected: {0}")]
    Reply(#[from] ReplyRejection),
}

/// Restores the public request error shape after the dedicated stage's
/// single whole-operation deadline finishes. Reply validation remains a
/// direct `Reply` error; only wire and deadline failures remain under the
/// existing `Exchange` boundary.
fn map_acknowledging_response_error(
    error: DeadlineError<AcknowledgingResponseError>,
) -> ClientRequestError {
    match error {
        DeadlineError::Timeout { operation, limit } => {
            ClientRequestError::Exchange(DeadlineError::Timeout { operation, limit })
        }
        DeadlineError::Cancelled { operation } => {
            ClientRequestError::Exchange(DeadlineError::Cancelled { operation })
        }
        DeadlineError::InvalidLimit { operation } => {
            ClientRequestError::Exchange(DeadlineError::InvalidLimit { operation })
        }
        DeadlineError::Peer {
            operation,
            error: AcknowledgingResponseError::Exchange(error),
        } => ClientRequestError::Exchange(DeadlineError::Peer { operation, error }),
        DeadlineError::Peer {
            error: AcknowledgingResponseError::Reply(error),
            ..
        } => ClientRequestError::Reply(error),
    }
}

/// One authenticated client session owning exactly one private loopback
/// endpoint, connection, and request lifecycle.
///
/// Deliberately implements neither [`Clone`] nor [`Copy`]: the session is
/// the single consuming owner of its transport, and duplicating it would
/// let two owners drive one connection. The derived [`Debug`] output
/// covers non-secret state only — the session never stores a credential.
#[derive(Debug)]
pub struct ClientSession {
    /// Privately owned endpoint; closed explicitly by [`Drop::drop`] so
    /// teardown never relies on implicit handle-drop semantics.
    endpoint: Endpoint,
    /// The single established connection behind every request.
    connection: Connection,
    /// Accepted lifecycle carried unchanged through every successful
    /// request of this connection's whole life.
    lifecycle: ClientRequestLifecycle,
    /// Whole-stage limits retained from connect time.
    limits: ClientSessionLimits,
    /// Negotiated non-secret protocol revision.
    negotiated_version: ProtocolVersion,
    /// Negotiated non-secret connection diagnostic identity.
    connection_id: ConnectionId,
    /// Whether native lifecycle control was accepted during this handshake.
    lifecycle_control_supported: bool,
    /// Whether the one server-delivery receiver has already been acquired.
    delivery_taken: bool,
}

impl Drop for ClientSession {
    fn drop(&mut self) {
        // Synchronous teardown only: the connection close resets
        // unfinished outbound streams and stops inbound delivery, and
        // the background driver emits the close frames. No drain is
        // promised here; [`ClientSession::shutdown`] awaits one. The
        // private endpoint is closed explicitly rather than left to
        // handle drop, mirroring the establishment guard.
        self.connection
            .close(link::close_code(link::ABANDON_CODE), link::ABANDON_REASON);
        self.endpoint
            .close(link::close_code(link::ABANDON_CODE), link::ABANDON_REASON);
    }
}

impl ClientSession {
    /// Establishes one pinned session to `target`.
    ///
    /// Builds the pinned client configuration from `trusted_root` and
    /// `pinned_identity`, binds the session's own fresh loopback
    /// endpoint, establishes QUIC, and runs the application handshake
    /// with the owned `hello` envelope — each under its whole-stage
    /// limit and the borrowed `cancel` handle. On success the rotated
    /// capability inside the returned [`ServerWelcome`] belongs to the
    /// trusted caller alone; the session retains only the negotiated
    /// non-secret metadata.
    ///
    /// On any failure — including pre-start cancellation, an
    /// unrepresentable limit, or a dropped future — every resource
    /// already created is closed and the Hello credential is dropped and
    /// zeroized. A connect future dropped before its first poll creates
    /// nothing, so it has no endpoint or connection to close; its Hello
    /// capability is still dropped. A successful handshake tolerates the
    /// server's deliberate STOP after reading one Hello: that intended
    /// post-Hello stop never becomes an authentication failure here.
    ///
    /// # Errors
    ///
    /// Returns [`ClientSessionError::Limits`] before any resource is
    /// created when the admission budget is zero,
    /// [`ClientSessionError::Connect`] when the whole connect stage —
    /// including configuration and binding — fails under its deadline,
    /// and [`ClientSessionError::Handshake`] when the application
    /// handshake stage fails under its deadline.
    pub async fn connect(
        target: LoopbackTarget,
        trusted_root: CertificateDer<'static>,
        pinned_identity: PinnedIdentity,
        hello: WireEnvelope,
        limits: ClientSessionLimits,
        cancel: &CancelHandle,
    ) -> Result<(Self, ServerWelcome), ClientSessionError> {
        // Budget validation precedes every resource: the registry is the
        // sole authority and rejects a zero budget immediately.
        let lifecycle = ClientRequestLifecycle::new(PENDING_CAPACITY, limits.admission_budget)?;

        // Whole connect stage under one guard: from the first bind until
        // the handshake completes, every escape closes what exists.
        let link = run_with_deadline(OperationKind::Connect, limits.connect, cancel, async {
            let config = client_config(trusted_root, pinned_identity)?;
            let mut link = SessionLink::bind(config)?;
            let connection =
                crate::endpoint::connect(link.endpoint(), target.addr(), LOOPBACK_SERVER_NAME)
                    .await?;
            link.install(connection);
            Ok(link)
        })
        .await
        .map_err(ClientSessionError::Connect)?;

        // Whole handshake stage under one Handshake label, honestly
        // covering its stream open/send/receive.
        let welcome = run_with_deadline(
            OperationKind::Handshake,
            limits.handshake,
            cancel,
            handshake_stage(link.connection(), hello),
        )
        .await
        .map_err(ClientSessionError::Handshake)?;

        let (endpoint, connection) = link.disband();
        let negotiated_version = welcome.protocol_version;
        let connection_id = welcome.welcome.connection_id.clone();
        let lifecycle_control_supported = welcome.welcome.lifecycle_control_supported;
        Ok((
            Self {
                endpoint,
                connection,
                lifecycle,
                limits,
                negotiated_version,
                connection_id,
                lifecycle_control_supported,
                delivery_taken: false,
            },
            welcome,
        ))
    }

    /// Returns the negotiated application protocol revision.
    #[must_use]
    pub const fn protocol_version(&self) -> ProtocolVersion {
        self.negotiated_version
    }

    /// Returns whether native lifecycle control was negotiated for this
    /// session.
    #[must_use]
    pub const fn lifecycle_control_supported(&self) -> bool {
        self.lifecycle_control_supported
    }

    /// Returns the server-assigned connection diagnostic identity.
    #[must_use]
    pub fn connection_id(&self) -> &ConnectionId {
        &self.connection_id
    }

    /// Returns the fixed total lifetime admission budget delegated to
    /// the correlation registry at connect time.
    #[must_use]
    pub const fn admission_budget(&self) -> usize {
        self.limits.admission_budget
    }

    /// Returns how many admissions have consumed the lifetime budget so
    /// far. This counts consumed budget, never pending occupancy.
    #[must_use]
    pub fn admitted(&self) -> usize {
        self.lifecycle.admitted()
    }

    /// Returns the fixed simultaneous pending capacity, always
    /// [`PENDING_CAPACITY`].
    #[must_use]
    pub const fn pending_capacity(&self) -> usize {
        PENDING_CAPACITY
    }

    /// Takes the server-initiated delivery receiver without waiting for the
    /// server or performing any network I/O.
    ///
    /// The first call marks this session and returns the same session together
    /// with exactly one receiver holding only a private cloned connection
    /// handle. The server opens its stream only after a real delivery write,
    /// so all waiting belongs to [`DeliveryReceiver::recv`]. A second call is
    /// a terminal typed session error and consumes the session under the same
    /// conservative rule as other session-local errors.
    ///
    /// # Errors
    ///
    /// Returns [`ClientSessionError::DeliveryAlreadyTaken`] when the
    /// receiver was already acquired; the supplied session is dropped on
    /// that path and its endpoint and connection are closed synchronously.
    pub fn take_delivery(self) -> Result<(Self, DeliveryReceiver), ClientSessionError> {
        let mut session = self;
        if session.delivery_taken {
            return Err(ClientSessionError::DeliveryAlreadyTaken);
        }
        session.delivery_taken = true;
        let receiver = DeliveryReceiver::new(session.connection.clone());
        Ok((session, receiver))
    }

    /// Runs exactly one sequential request and returns the same owner
    /// with its settled outcome.
    ///
    /// The operation consumes `self` for its whole duration: the
    /// returned future owns the session, so dropping that future —
    /// before its first poll or while pending — drops the session and
    /// synchronously closes its connection and endpoint. The frame
    /// identity of `envelope` is authoritative: it seeds the single
    /// correlation admitted through the session's lifecycle, and only a
    /// response echoing exactly that identity, or a failure carrying
    /// exactly it, may settle the private waiter. A matched peer failure
    /// settles as an ordinary outcome and the same live owner returns;
    /// every local error consumes the session (see the [module
    /// documentation](self#terminal-request-errors-conservative-tradeoff)).
    ///
    /// # Errors
    ///
    /// Returns [`ClientRequestError::NotARequest`] and
    /// [`ClientRequestError::VersionMismatch`] and
    /// [`ClientRequestError::UnsupportedFeature`] before any network
    /// attempt, [`ClientRequestError::Correlation`] for an unusable
    /// frame identity, [`ClientRequestError::Admission`] when the
    /// registry rejects the identity, [`ClientRequestError::Exchange`]
    /// when the composed wire exchange fails under its deadline, and
    /// [`ClientRequestError::Reply`] when the reply fails validation or
    /// settlement.
    ///
    /// # Panics
    ///
    /// Panics only if the just-settled waiter could not deliver its one
    /// outcome, which would mean lifecycle bookkeeping diverged from its
    /// registry mirror; no decoded input reaches that path.
    pub async fn request(
        mut self,
        envelope: WireEnvelope,
        cancel: &CancelHandle,
    ) -> Result<(Self, ResolvedRequest), ClientRequestError> {
        // Pre-I/O validation, in order: family, negotiated version,
        // negotiated lifecycle feature, correlation seeding, admission.
        // Nothing touches the network before all five pass.
        if !matches!(&envelope.body, WireEnvelopeBody::Request(_)) {
            let received = message_kind(&envelope.body);
            return Err(ClientRequestError::NotARequest { received });
        }
        if envelope.protocol_version != self.negotiated_version {
            return Err(ClientRequestError::VersionMismatch {
                envelope_version: envelope.protocol_version.get(),
                negotiated_version: self.negotiated_version.get(),
            });
        }
        if matches!(
            &envelope.body,
            WireEnvelopeBody::Request(ClientRequest::Lifecycle(_))
        ) && !self.lifecycle_control_supported
        {
            return Err(ClientRequestError::UnsupportedFeature);
        }
        let request_id = envelope
            .frame_id
            .to_request_id()
            .map_err(ClientRequestError::Correlation)?;
        let mut waiter = self
            .lifecycle
            .admit(request_id.clone())
            .map_err(ClientRequestError::Admission)?;

        // Whole request stage under one Receive label, honestly covering
        // its stream open, bounded send, and bounded reply receive.
        let reply = match run_with_deadline(
            OperationKind::Receive,
            self.limits.request,
            cancel,
            request_stage(&self.connection, envelope),
        )
        .await
        {
            Ok(reply) => reply,
            Err(error) => return Err(ClientRequestError::Exchange(error)),
        };

        // Validation strictly precedes lifecycle settlement: version,
        // family, and exact current-operation correlation first.
        let settled = classify_reply(&reply, self.negotiated_version, &request_id)
            .map_err(ClientRequestError::Reply)?;
        match settled {
            SettledReply::Response(response) => {
                self.lifecycle
                    .resolve_on_response(&response)
                    .map_err(ReplyRejection::Settle)
                    .map_err(ClientRequestError::Reply)?;
            }
            SettledReply::Failure(failure) => {
                self.lifecycle
                    .resolve_on_failure(&failure)
                    .map_err(ReplyRejection::Settle)
                    .map_err(ClientRequestError::Reply)?;
            }
        }
        let resolved = waiter
            .take_outcome()
            .expect("the admitted request settled exactly once into its private waiter");
        Ok((self, resolved))
    }

    /// Runs exactly one sequential request, settles its correlated reply,
    /// and then waits for the peer to finish the response stream cleanly.
    ///
    /// This mode is for mutations whose server-side commit is released only
    /// after the peer observes the response FIN. It preserves the ordinary
    /// request's pre-I/O validation, admission, settlement, cancellation,
    /// deadline, and terminal-error policy. The admitted reply is settled
    /// before the inbound stream is read for its end marker. A clean FIN is
    /// the only successful stream completion; trailing data or any other
    /// stream failure consumes the session and emits the private STOP.
    ///
    /// The operation consumes `self` for its whole duration and returns the
    /// same live owner with the settled outcome on success. Dropping the
    /// future, caller cancellation, timeout, malformed input, wrong family,
    /// wrong correlation, or failed settlement all retain the existing
    /// terminal session behavior.
    ///
    /// # Errors
    ///
    /// Returns the same typed pre-I/O, admission, exchange, and reply errors
    /// as [`ClientSession::request`]. The request exchange deadline covers
    /// stream open, send, reply receive, reply settlement, and clean response
    /// EOF as one total budget.
    ///
    /// # Panics
    ///
    /// Panics only if the just-settled waiter cannot deliver its one outcome,
    /// which would mean lifecycle bookkeeping diverged from its registry
    /// mirror; no decoded input reaches that invariant violation.
    pub async fn request_acknowledging_response(
        mut self,
        envelope: WireEnvelope,
        cancel: &CancelHandle,
    ) -> Result<(Self, ResolvedRequest), ClientRequestError> {
        // Keep the ordinary request's pre-I/O order and policy exactly:
        // family, negotiated version, lifecycle feature, correlation
        // seeding, admission. Nothing touches the network before all five
        // pass.
        if !matches!(&envelope.body, WireEnvelopeBody::Request(_)) {
            let received = message_kind(&envelope.body);
            return Err(ClientRequestError::NotARequest { received });
        }
        if envelope.protocol_version != self.negotiated_version {
            return Err(ClientRequestError::VersionMismatch {
                envelope_version: envelope.protocol_version.get(),
                negotiated_version: self.negotiated_version.get(),
            });
        }
        if matches!(
            &envelope.body,
            WireEnvelopeBody::Request(ClientRequest::Lifecycle(_))
        ) && !self.lifecycle_control_supported
        {
            return Err(ClientRequestError::UnsupportedFeature);
        }
        let request_id = envelope
            .frame_id
            .to_request_id()
            .map_err(ClientRequestError::Correlation)?;
        let mut waiter = self
            .lifecycle
            .admit(request_id.clone())
            .map_err(ClientRequestError::Admission)?;

        let negotiated_version = self.negotiated_version;
        let resolved = run_with_deadline(
            OperationKind::Receive,
            self.limits.request,
            cancel,
            request_stage_acknowledging_response(&self.connection, envelope, |reply| {
                // Validation and lifecycle settlement happen before the
                // stage is allowed to observe response EOF/FIN.
                let settled = classify_reply(reply, negotiated_version, &request_id)
                    .map_err(AcknowledgingResponseError::Reply)?;
                match settled {
                    SettledReply::Response(response) => {
                        self.lifecycle
                            .resolve_on_response(&response)
                            .map_err(ReplyRejection::Settle)
                            .map_err(AcknowledgingResponseError::Reply)?;
                    }
                    SettledReply::Failure(failure) => {
                        self.lifecycle
                            .resolve_on_failure(&failure)
                            .map_err(ReplyRejection::Settle)
                            .map_err(AcknowledgingResponseError::Reply)?;
                    }
                }
                Ok(waiter
                    .take_outcome()
                    .expect("the admitted request settled exactly once into its private waiter"))
            }),
        )
        .await
        .map_err(map_acknowledging_response_error)?;
        Ok((self, resolved))
    }

    /// Consumes the session and drains its private endpoint under the
    /// finite shutdown limit and the caller's cancellation signal.
    ///
    /// This is the only path promising asynchronous drain: the accepted
    /// shutdown primitive closes the privately owned endpoint with the
    /// fixed non-secret shutdown code and waits for idle within the same
    /// finite limit that also bounds the whole stage outside. No other
    /// owner's resources are touched. Dropping the session instead closes
    /// synchronously and promises nothing.
    ///
    /// # Errors
    ///
    /// Returns [`ClientSessionError::Shutdown`] when the drain exceeds
    /// its limit, observes cancellation, hits an unrepresentable limit,
    /// or the transport reports a failure while closing.
    pub async fn shutdown(self, cancel: &CancelHandle) -> Result<(), ClientSessionError> {
        let shutdown_limit = self.limits.shutdown;
        let drained = run_with_deadline(
            OperationKind::Shutdown,
            shutdown_limit,
            cancel,
            crate::shutdown(
                &self.endpoint,
                link::close_code(link::SHUTDOWN_CODE),
                link::SHUTDOWN_REASON,
                shutdown_limit,
            ),
        )
        .await;
        // The session drops here regardless of the outcome. After a
        // successful drain its abandon-close finds the endpoint already
        // closed and is ignored; after any failure it is the guaranteed
        // synchronous teardown.
        drained.map_err(ClientSessionError::Shutdown)
    }
}
