//! Owned authenticated Forge connection composition.
//!
//! This component composes boundaries the Forge process already owns — the
//! single-use [`CredentialAuthority`], the public [`RequestHandler`] seam,
//! and the established transport handshake and single-request dispatcher —
//! into one admitted QUIC connection driven strictly sequentially. It owns
//! no listener, endpoint, database, correlation registry, scheduler, task
//! pool, or callback store: the caller continues to own Forge storage, the
//! handler, cancellation, and endpoint lifetimes, and hands over one
//! accepted `quinn::Connection` at a time through
//! [`ForgeConnection::authenticate`].
//!
//! Metadata is deliberately injected. This library leaf mints neither
//! connection identities, server frame identities, nor timestamps, and it
//! selects no port, certificate trust, wall clock, launch handoff, engine,
//! or data-path policy. Callers supply the typed Welcome metadata and the
//! finite limits for every stage.
//!
//! # Ownership and cleanup
//!
//! An owned-connection close guard is installed synchronously before the
//! authentication future is returned, so dropping even an unpolled future
//! closes the accepted connection. Each stage's bidirectional stream pair
//! lives in a private owner whose [`Drop`] stops the inbound direction and
//! resets an unfinished outbound direction synchronously, so timeouts,
//! cancellations, and abandoned futures never leave half-open streams
//! behind; a successfully finished send side is never reset, and
//! already-closed resources report through discarded results so the primary
//! typed failure survives.
//!
//! The ready owner keeps Quinn private and retains the exclusive
//! credential-authority lease for its full lifetime;
//! [`ForgeConnection::respond_next`] consumes the owner and returns it only
//! after one complete, deadline-bounded, successful dispatch, including reply
//! write and send-side FIN plus any local post-write subscription activation.
//! An accepted lifecycle stop additionally waits for Quinn to acknowledge its
//! finished response stream before committing the stop.
//! Cancellation of the caller's shared [`CancelHandle`] is observed, never
//! invoked, and this leaf never closes the shared endpoint or database.
//!
//! # Correlation lifetimes
//!
//! This leaf keeps no per-connection correlation registry and performs no
//! automatic retry or rollback. Caller-minted request identities are
//! single-use within one authenticated connection lifetime, and a
//! retry-stable durable command id replays only through a genuinely new
//! connection against a fresh client registry once the old owner has been
//! closed. No identifier or protocol semantics are invented here.

use std::convert::Infallible;
use std::future::Future;
use std::time::Duration;

use artisan_domain::{ConversationRequest, RequestId, ThreadId, UnixMillis};
use artisan_protocol::{
    ClientRequest, ConnectionId, ErrorCode, ErrorDetail, FrameId, ProtocolFailure, ProtocolVersion,
    ResponsePayload, ServerResponse, Welcome, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, DeadlineError, HandshakeError, OperationKind, ServerDispatchError,
    dispatch_server_request_with_receipt, receive_client_hello, run_with_deadline,
    send_server_welcome,
};
use quinn::{ClosedStream, Connection, RecvStream, SendStream, VarInt};
use thiserror::Error;

use crate::conversation_delivery_driver::ConversationDeliveryDriver;
use crate::conversation_subscription_registry::ActivateError;
use crate::credential_authority::{
    CredentialAuthenticationError, CredentialAuthority, CredentialEntropyError,
    ReconnectRotationError,
};
use crate::lifecycle_control::{LifecycleControlReceipt, LifecycleController, LifecycleDispatch};
use crate::request_handler::{
    ActivatedConversationSubscription, ConversationConnectionContext, RequestHandler,
    RequestHandlerReceipt,
};

/// Fixed application close code used whenever this leaf releases a
/// connection it owns.
const CONNECTION_CLOSE_CODE: VarInt = VarInt::from_u32(0x01);

/// Fixed secret-free reason paired with [`CONNECTION_CLOSE_CODE`]. It
/// carries no credential, identifier, or peer-controlled detail.
const CONNECTION_CLOSE_REASON: &[u8] = b"forge connection released";

/// Fixed `STOP_SENDING` code discarding the inbound direction of a stream
/// this leaf stops reading.
const INBOUND_STOP_CODE: VarInt = VarInt::from_u32(0x01);

/// Fixed `RESET_STREAM` code resetting an outbound direction abandoned
/// before it was successfully finished.
const OUTBOUND_RESET_CODE: VarInt = VarInt::from_u32(0x01);

/// Caller-supplied identity of one Forge-originated server frame.
///
/// The connection component mints neither frame identities nor timestamps;
/// the caller stamps every server frame deliberately.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerFrameStamp {
    /// Validated server-minted frame identity for one outgoing frame.
    pub frame_id: FrameId,
    /// Caller-selected timestamp carried on the same frame.
    pub sent_at: UnixMillis,
}

/// Typed Welcome metadata injected by the caller during admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WelcomeMetadata {
    /// Opaque connection-scoped diagnostic identity offered to the client.
    pub connection_id: ConnectionId,
    /// Stamp carried by the outgoing Welcome frame.
    pub frame: ServerFrameStamp,
}

/// Explicit finite durations bounding one connection's stages.
///
/// Both fields must be representable as a future instant: `Duration::MAX`
/// is rejected through the established typed
/// [`DeadlineError::InvalidLimit`] mechanism, and `Duration::ZERO` expires
/// before the bounded operation is polled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionLimits {
    /// Limit covering the complete authentication handshake, including
    /// accepting the control bidirectional stream.
    pub handshake: Duration,

    /// Limit covering one complete request dispatch.
    pub next_request: Duration,
}

/// Failure of one bounded authentication-handshake stage on an admitted
/// connection. Existing typed sources are preserved verbatim.
#[derive(Debug, Error)]
pub enum AuthenticationStageError {
    /// The control bidirectional stream could not be accepted.
    #[error("accepting the control stream failed")]
    Accept {
        /// Underlying connection failure, preserved without reformatting.
        #[source]
        source: quinn::ConnectionError,
    },

    /// The application handshake failed at the established transport seam.
    #[error("application handshake failed")]
    Handshake(#[from] HandshakeError),

    /// The presented credential was rejected by the credential authority.
    #[error("credential authentication failed")]
    Credential(#[from] CredentialAuthenticationError),

    /// Operating-system entropy failed while minting the reconnect pair.
    #[error("reconnect rotation entropy failed")]
    Entropy(#[from] CredentialEntropyError),

    /// Staging or committing the reconnect rotation failed.
    #[error("reconnect rotation failed")]
    Rotation(#[from] ReconnectRotationError),

    /// The Welcome was written but its send side could not be finished.
    #[error("finishing the Welcome send side failed")]
    Finish(#[from] ClosedStream),
}

/// Failure of one bounded conversation-delivery stage on an authenticated
/// connection. Every variant is payload-free and stage-classified.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum DeliveryStageError {
    /// The authoritative conversation replay read failed.
    #[error("conversation delivery replay failed")]
    Replay,

    /// The serialized conversation writer failed or its state was unavailable.
    #[error("conversation delivery writer failed")]
    Writer,

    /// The writer could not record its already-sent batch in the registry.
    #[error("conversation delivery registry update failed")]
    Registry,

    /// The durable cursor requires a fresh snapshot.
    #[error("conversation delivery requires a fresh snapshot")]
    ResnapshotRequired,

    /// The process-wide commit wake source closed.
    #[error("conversation delivery notifier closed")]
    NotifierClosed,

    /// The serialized writer could not finish its output stream.
    #[error("conversation delivery finish failed")]
    Finish,
}

/// Result of one request after its response send side and local receipt work
/// have completed. The delivery driver owns the activation after this point.
#[derive(Debug)]
pub(crate) struct RequestDispatchOutcome {
    pub(crate) activation: Option<ActivatedConversationSubscription>,
    pub(crate) stopped_thread: Option<ThreadId>,
}

/// Terminal result of the connection-owned service loop, retaining the local
/// dispatch count for the listener's existing service report.
#[derive(Debug)]
pub(crate) struct DriveUntilEndError {
    completed_requests: u32,
    source: DeadlineError<RequestStageError>,
}

impl DriveUntilEndError {
    fn new(completed_requests: u32, source: DeadlineError<RequestStageError>) -> Self {
        Self {
            completed_requests,
            source,
        }
    }

    pub(crate) fn into_parts(self) -> (u32, DeadlineError<RequestStageError>) {
        (self.completed_requests, self.source)
    }
}

/// Failure of one bounded request-dispatch stage on an authenticated
/// connection. Existing typed sources are preserved verbatim.
#[derive(Debug, Error)]
pub enum RequestStageError {
    /// The request bidirectional stream could not be accepted.
    #[error("accepting the request stream failed")]
    Accept {
        /// Underlying connection failure, preserved without reformatting.
        #[source]
        source: quinn::ConnectionError,
    },

    /// The single-request dispatcher failed locally. A handler
    /// [`artisan_protocol::ProtocolFailure`] never lands here: it is a
    /// successful wire result mapped onto a correlated `ProtocolError`
    /// envelope, not a dispatcher-local failure.
    #[error("request dispatch failed")]
    Dispatch(#[from] ServerDispatchError<Infallible>),

    /// The peer did not acknowledge all bytes on an accepted lifecycle-stop
    /// response stream.
    #[error("lifecycle response acknowledgement failed")]
    LifecycleResponseAcknowledgement,

    /// A prepared conversation subscription could not be activated after its
    /// correlated response was written and the server send side finished.
    #[error("activating the conversation subscription failed")]
    Activate(#[from] ActivateError),

    /// A fresh server frame stamp could not be acquired for an outgoing frame.
    #[error("stamping the server frame failed")]
    Stamp,

    /// A serialized conversation delivery stage failed.
    #[error("conversation delivery failed")]
    Delivery(#[from] DeliveryStageError),
}

/// One admitted, authenticated Forge connection owned exclusively by its
/// caller.
///
/// The value keeps Quinn private, retains the exclusive mutable
/// credential-authority lease for its whole lifetime, and closes the owned
/// connection with the fixed application reason when dropped. The caller
/// services it sequentially with [`Self::drive_until_end`] for the configured
/// delivery path or [`Self::respond_next`] for the legacy request path; no
/// spawning, concurrent handler tasks, or background work exist inside this
/// leaf.
#[must_use = "an admitted connection must be serviced; dropping it closes the connection"]
pub struct ForgeConnection<'authority, 'handler, 'cancel, 'lifecycle> {
    connection: Connection,
    authority: &'authority mut CredentialAuthority,
    handler: &'handler RequestHandler,
    cancel: &'cancel CancelHandle,
    lifecycle: &'lifecycle LifecycleController,
    lifecycle_witness: LifecycleWitness,
    limits: ConnectionLimits,
    protocol_version: ProtocolVersion,
    conversation_delivery: Option<ConversationDeliveryDriver>,
}

impl ForgeConnection<'_, '_, '_, '_> {
    /// Admits one accepted connection through the full authentication
    /// ordering.
    ///
    /// The owned close guard is installed synchronously before the returned
    /// future exists, and the stage's stream pair is owned by a private
    /// drop guard, so dropping even an unpolled future closes the accepted
    /// connection and cleans up its streams. The future then deadline-bounds
    /// the complete handshake under [`ConnectionLimits::handshake`],
    /// including accepting the control bidirectional stream, and composes
    /// the established seams in the mandated order: the first valid
    /// application envelope must be a Hello whose owned credential is
    /// consumed exactly once by the [`CredentialAuthority`]; the grant
    /// stages an actual system reconnect rotation whose Welcome copy is
    /// taken exactly once; the Welcome is validated and written through the
    /// established transport helper and its send side is explicitly
    /// finished; only after the successful write and finish does the staged
    /// rotation commit, synchronously and without an intervening await. No
    /// request is dispatched before that commit.
    ///
    /// A failure after credential consumption leaves the authority
    /// fail-closed awaiting rotation; a failure after a successful commit
    /// closes this connection while retaining its valid rotated reconnect
    /// credential.
    ///
    /// # Errors
    ///
    /// Returns the established typed deadline decision for the handshake —
    /// including [`DeadlineError::InvalidLimit`] for unrepresentable limits
    /// and [`DeadlineError::Cancelled`] when the caller's handle fires —
    /// with the failing [`AuthenticationStageError`] preserved as the
    /// source of [`DeadlineError::Peer`].
    #[must_use = "dropping the authentication future closes the accepted connection"]
    pub fn authenticate<'authority, 'handler, 'cancel, 'lifecycle>(
        connection: Connection,
        authority: &'authority mut CredentialAuthority,
        handler: &'handler RequestHandler,
        lifecycle: &'lifecycle LifecycleController,
        metadata: WelcomeMetadata,
        limits: ConnectionLimits,
        cancel: &'cancel CancelHandle,
    ) -> impl Future<
        Output = Result<
            ForgeConnection<'authority, 'handler, 'cancel, 'lifecycle>,
            DeadlineError<AuthenticationStageError>,
        >,
    > {
        let mut admission = AdmissionGuard::armed(connection.clone());
        async move {
            let WelcomeMetadata {
                connection_id,
                frame,
            } = metadata;
            let mut streams = StageStreams::new();

            let outcome = run_with_deadline(
                OperationKind::Handshake,
                limits.handshake,
                cancel,
                drive_authentication(
                    &connection,
                    &mut *authority,
                    lifecycle,
                    &mut streams,
                    frame,
                    connection_id,
                ),
            )
            .await;

            match outcome {
                Ok((protocol_version, lifecycle_witness)) => {
                    // The stage completed: hand both directions back for
                    // ordinary drops without cleanup actions.
                    streams.release();
                    let conversation_delivery =
                        handler
                            .new_conversation_connection_context()
                            .map(|context| {
                                ConversationDeliveryDriver::new(
                                    connection.clone(),
                                    protocol_version,
                                    context,
                                )
                            });
                    let admitted = ForgeConnection {
                        connection,
                        authority,
                        handler,
                        cancel,
                        lifecycle,
                        lifecycle_witness,
                        limits,
                        protocol_version,
                        conversation_delivery,
                    };
                    admission.disarm();
                    Ok(admitted)
                }
                Err(source) => Err(source),
            }
        }
    }

    /// Dispatches exactly one deadline-bounded request and returns the
    /// ready owner.
    ///
    /// The owner is consumed for the duration of the dispatch and comes
    /// back only after the established single-request dispatcher completed:
    /// the incoming frame-derived request id stayed authoritative, the
    /// public [`RequestHandler`] answered it, the correlated reply crossed
    /// the wire under the supplied fresh server frame stamp and the
    /// negotiated protocol version, the server send side was finished, and
    /// any local post-write subscription activation completed. An accepted
    /// lifecycle stop also requires Quinn to observe peer acknowledgement of
    /// the finished response stream. No call proves peer application or
    /// durable acknowledgement, patch replay, or delivery.
    /// The caller may loop sequentially; every dispatch accepts the next
    /// bidirectional stream on this connection.
    ///
    /// A handler [`artisan_protocol::ProtocolFailure`] is a wire result: it
    /// is mapped onto a correlated `ProtocolError` envelope like any other
    /// answer. The dispatcher's local failure type stays [`Infallible`]
    /// because this adapter performs no fallible local work. A subscription
    /// activation failure is reported separately after the dispatcher's
    /// successful write and finish.
    ///
    /// On error — or when the returned future is dropped mid-dispatch — the
    /// private stream guard stops the inbound direction and resets the
    /// unfinished outbound direction synchronously, the owner closes the
    /// connection, and neither can ever be admitted again. A successfully
    /// finished send side is never reset.
    ///
    /// # Errors
    ///
    /// Returns the established typed deadline decision for the dispatch,
    /// with the failing [`RequestStageError`] preserved as the source of
    /// [`DeadlineError::Peer`].
    #[must_use = "dropping the in-flight future closes the owned connection"]
    pub async fn respond_next(
        self,
        stamp: ServerFrameStamp,
    ) -> Result<Self, DeadlineError<RequestStageError>> {
        let mut streams = StageStreams::new();

        let outcome = run_with_deadline(
            OperationKind::Receive,
            self.limits.next_request,
            self.cancel,
            drive_request(
                &self.connection,
                self.handler,
                self.lifecycle,
                self.lifecycle_witness,
                self.protocol_version,
                stamp,
                None,
                &mut streams,
                self.cancel,
            ),
        )
        .await;

        match outcome {
            Ok(_) => {
                // The stage completed: hand both directions back for
                // ordinary drops without cleanup actions.
                streams.release();
                Ok(self)
            }
            Err(source) => Err(source),
        }
    }

    /// Drives one authenticated connection until its request budget is
    /// reached or a bounded request/delivery stage fails.
    ///
    /// A configured conversation notifier switches the owner to one
    /// serialized accept-or-wake loop. Requests still use the established
    /// bidirectional dispatcher, while wake scans and initial activation
    /// replay use the same connection-owned writer. A caller cancellation
    /// finishes that writer before registry cleanup; every other terminal
    /// path drops it so its existing guard resets unfinished output.
    pub(crate) async fn drive_until_end<F>(
        mut self,
        capacity: u32,
        mut stamp: F,
    ) -> Result<(Self, u32), DriveUntilEndError>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let mut completed_requests = 0_u32;

        loop {
            if completed_requests >= capacity {
                return self.finish_at_capacity(completed_requests).await;
            }

            if self.conversation_delivery.is_none() {
                let (returned, count) = self
                    .drive_legacy_request(&mut stamp, completed_requests)
                    .await?;
                self = returned;
                completed_requests = count;
                continue;
            }

            let event = match self.next_driver_event().await {
                Ok(event) => event,
                Err(source) => return self.fail_connection(source, completed_requests).await,
            };

            match event {
                DriverEvent::Wake => {
                    if let Err(source) = self.deliver_driver_wake(&mut stamp).await {
                        return self.fail_connection(source, completed_requests).await;
                    }
                }
                DriverEvent::Request { send, recv } => {
                    let mut streams = StageStreams::new();
                    streams.install(send, recv);
                    let frame = match stamp() {
                        Ok(frame) => frame,
                        Err(source) => {
                            drop(streams);
                            return self
                                .fail_connection(
                                    DeadlineError::Peer {
                                        operation: OperationKind::Send,
                                        error: source,
                                    },
                                    completed_requests,
                                )
                                .await;
                        }
                    };
                    let outcome = match self.dispatch_driver_request(frame, &mut streams).await {
                        Ok(outcome) => {
                            streams.release();
                            outcome
                        }
                        Err(source) => {
                            return self.fail_connection(source, completed_requests).await;
                        }
                    };
                    completed_requests += 1;

                    if let Err(source) = self.deliver_driver_request(outcome, &mut stamp).await {
                        return self.fail_connection(source, completed_requests).await;
                    }
                }
            }
        }
    }

    async fn finish_at_capacity(
        mut self,
        completed_requests: u32,
    ) -> Result<(Self, u32), DriveUntilEndError> {
        if let Some(driver) = self.conversation_delivery.as_mut() {
            driver.cleanup(true).await.map_err(|error| {
                DriveUntilEndError::new(completed_requests, delivery_cleanup_source(error))
            })?;
        }
        Ok((self, completed_requests))
    }

    async fn drive_legacy_request<F>(
        self,
        stamp: &mut F,
        completed_requests: u32,
    ) -> Result<(Self, u32), DriveUntilEndError>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let frame = stamp().map_err(|error| {
            DriveUntilEndError::new(
                completed_requests,
                DeadlineError::Peer {
                    operation: OperationKind::Send,
                    error,
                },
            )
        })?;
        let returned = self
            .respond_next(frame)
            .await
            .map_err(|source| DriveUntilEndError::new(completed_requests, source))?;
        Ok((returned, completed_requests + 1))
    }

    async fn next_driver_event(&mut self) -> Result<DriverEvent, DeadlineError<RequestStageError>> {
        let driver = self
            .conversation_delivery
            .as_mut()
            .expect("configured delivery driver remains owned");
        run_with_deadline(
            OperationKind::Receive,
            self.limits.next_request,
            self.cancel,
            wait_for_driver_event(&self.connection, driver),
        )
        .await
    }

    async fn dispatch_driver_request(
        &self,
        frame: ServerFrameStamp,
        streams: &mut StageStreams,
    ) -> Result<RequestDispatchOutcome, DeadlineError<RequestStageError>> {
        let driver = self
            .conversation_delivery
            .as_ref()
            .expect("configured delivery driver remains owned");
        run_with_deadline(
            OperationKind::Receive,
            self.limits.next_request,
            self.cancel,
            drive_request_stream(
                self.handler,
                self.lifecycle,
                self.lifecycle_witness,
                self.protocol_version,
                frame,
                Some(driver.context()),
                streams,
                self.cancel,
            ),
        )
        .await
    }

    async fn deliver_driver_wake<F>(
        &mut self,
        stamp: &mut F,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let driver = self
            .conversation_delivery
            .as_mut()
            .expect("configured delivery driver remains owned");
        driver
            .deliver_wake(stamp, self.limits.next_request, self.cancel)
            .await
    }

    async fn deliver_driver_request<F>(
        &mut self,
        outcome: RequestDispatchOutcome,
        stamp: &mut F,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let driver = self
            .conversation_delivery
            .as_mut()
            .expect("configured delivery driver remains owned");
        driver
            .handle_request(outcome, stamp, self.limits.next_request, self.cancel)
            .await
    }

    async fn fail_connection(
        mut self,
        source: DeadlineError<RequestStageError>,
        completed_requests: u32,
    ) -> Result<(Self, u32), DriveUntilEndError> {
        let graceful = matches!(&source, DeadlineError::Cancelled { .. });
        match (self.conversation_delivery.as_mut(), graceful) {
            (Some(driver), true) => driver.cleanup(true).await.map_err(|error| {
                DriveUntilEndError::new(completed_requests, delivery_cleanup_source(error))
            })?,
            (Some(driver), false) => drop(driver.cleanup(false).await),
            (None, _) => {}
        }
        Err(DriveUntilEndError::new(completed_requests, source))
    }
}

fn delivery_cleanup_source(error: DeliveryStageError) -> DeadlineError<RequestStageError> {
    DeadlineError::Peer {
        operation: OperationKind::Send,
        error: RequestStageError::Delivery(error),
    }
}

impl Drop for ForgeConnection<'_, '_, '_, '_> {
    fn drop(&mut self) {
        // The exclusive credential-authority lease is released together
        // with the owner; touching it here documents that the whole value —
        // lease included — dies with this single close.
        let _ = &self.authority;
        self.connection
            .close(CONNECTION_CLOSE_CODE, CONNECTION_CLOSE_REASON);
    }
}

/// Runs the mandated authentication ordering on one accepted connection.
///
/// Every stage below the deadline boundary is synchronous or awaited here;
/// the Welcome commit deliberately happens with no additional await after
/// the successful Welcome write and finish.
async fn drive_authentication(
    connection: &Connection,
    authority: &mut CredentialAuthority,
    lifecycle: &LifecycleController,
    streams: &mut StageStreams,
    frame: ServerFrameStamp,
    connection_id: ConnectionId,
) -> Result<(ProtocolVersion, LifecycleWitness), AuthenticationStageError> {
    let (send, recv) = connection
        .accept_bi()
        .await
        .map_err(|source| AuthenticationStageError::Accept { source })?;
    let (send, recv) = streams.install(send, recv);

    // Hello must be the first valid application envelope; its version offer
    // is retained for Welcome negotiation.
    let hello = receive_client_hello(recv).await?;
    // The control inbound direction is stopped after its single Hello so
    // trailing bytes can never become requests. The current client
    // handshake does not finish its send side, so no FIN is waited for.
    let _discarded = recv.stop(INBOUND_STOP_CODE);

    // Consume the owned credential exactly once, then stage the actual
    // system reconnect rotation and take its Welcome copy exactly once.
    let lifecycle_control_supported =
        hello.hello.supports_lifecycle_control && lifecycle.implementation_available();
    let grant = authority.authenticate(hello.hello.credential)?;
    let mut pending = grant.prepare_system_reconnect()?;
    let reconnect_capability = pending.take_for_welcome()?;

    // The negotiated revision is the Hello envelope's revision, already
    // validated against the client's own offer; the established helper
    // revalidates the Welcome against the retained offer before writing.
    let negotiated = hello.protocol_version;
    let welcome = WireEnvelope {
        protocol_version: negotiated,
        frame_id: frame.frame_id,
        sent_at: frame.sent_at,
        body: WireEnvelopeBody::Welcome(Welcome {
            negotiated_version: negotiated,
            connection_id,
            reconnect_capability,
            lifecycle_control_supported,
        }),
    };

    send_server_welcome(send, &welcome, &hello.hello.supported_versions).await?;
    send.finish()?;
    // Recorded before the synchronous commit so even a failure after this
    // point never resets the successfully finished Welcome send side.
    streams.mark_send_finished();

    // Commit only after the successful Welcome write and finish, with no
    // additional await between them and this synchronous rotation commit.
    pending.commit()?;

    Ok((
        negotiated,
        LifecycleWitness {
            supported: lifecycle_control_supported,
        },
    ))
}

/// Runs exactly one deadline-bounded request dispatch on the owned
/// connection.
#[allow(clippy::too_many_arguments)]
async fn drive_request(
    connection: &Connection,
    handler: &RequestHandler,
    lifecycle: &LifecycleController,
    lifecycle_witness: LifecycleWitness,
    protocol_version: ProtocolVersion,
    stamp: ServerFrameStamp,
    context: Option<&ConversationConnectionContext>,
    streams: &mut StageStreams,
    cancel: &CancelHandle,
) -> Result<RequestDispatchOutcome, RequestStageError> {
    let (send, recv) = connection
        .accept_bi()
        .await
        .map_err(|source| RequestStageError::Accept { source })?;
    streams.install(send, recv);
    drive_request_stream(
        handler,
        lifecycle,
        lifecycle_witness,
        protocol_version,
        stamp,
        context,
        streams,
        cancel,
    )
    .await
}

/// Dispatches a request on a stream pair already accepted by the connection
/// driver.
#[allow(clippy::too_many_arguments)]
async fn drive_request_stream(
    handler: &RequestHandler,
    lifecycle: &LifecycleController,
    lifecycle_witness: LifecycleWitness,
    protocol_version: ProtocolVersion,
    stamp: ServerFrameStamp,
    context: Option<&ConversationConnectionContext>,
    streams: &mut StageStreams,
    cancel: &CancelHandle,
) -> Result<RequestDispatchOutcome, RequestStageError> {
    let send = streams
        .send
        .as_mut()
        .expect("request send stream is installed before dispatch");
    let recv = streams
        .recv
        .as_mut()
        .expect("request receive stream is installed before dispatch");
    let dispatch_context = context;
    let dispatch_handler = handler;

    // The incoming frame-derived request id stays authoritative. Lifecycle
    // requests are intercepted before the ordinary handler, while every
    // other request follows the existing handler path. Receipts remain opaque
    // until the transport has validated, written, and finished the correlated
    // reply.
    let receipt = dispatch_server_request_with_receipt(send, recv, |incoming| async move {
        let request_id = incoming.request_id;
        let request = incoming.request;
        let stopped_thread = match &request {
            ClientRequest::Conversation(ConversationRequest::Unsubscribe(unsubscribe)) => {
                Some(unsubscribe.thread_id.clone())
            }
            _ => None,
        };
        let (answered, receipt) = match request {
            ClientRequest::Lifecycle(_request) if !lifecycle_witness.supported => (
                Err(unsupported_feature_failure(&request_id)),
                PostResponseReceipt::Lifecycle(LifecycleControlReceipt::none()),
            ),
            ClientRequest::Lifecycle(request) => {
                match lifecycle.dispatch(request_id.clone(), request).await {
                    LifecycleDispatch::Reply { response, receipt } => (
                        Ok(ServerResponse {
                            request_id: request_id.clone(),
                            payload: ResponsePayload::Lifecycle(response),
                        }),
                        PostResponseReceipt::Lifecycle(receipt),
                    ),
                    LifecycleDispatch::Failure(failure) => (
                        Err(failure),
                        PostResponseReceipt::Lifecycle(LifecycleControlReceipt::none()),
                    ),
                }
            }
            request => {
                let (answered, receipt) = match dispatch_context {
                    Some(context) => dispatch_handler
                        .respond_with_receipt_in_context(context, &request_id, &request)
                        .await
                        .into_parts(),
                    None => dispatch_handler
                        .respond_with_receipt(&request_id, &request)
                        .await
                        .into_parts(),
                };
                (
                    answered,
                    PostResponseReceipt::Handler {
                        receipt,
                        stopped_thread,
                    },
                )
            }
        };
        let body = match answered {
            Ok(response) => WireEnvelopeBody::Response(response),
            Err(failure) => WireEnvelopeBody::ProtocolError(failure),
        };
        Ok::<(WireEnvelope, PostResponseReceipt), Infallible>((
            WireEnvelope {
                protocol_version,
                frame_id: stamp.frame_id,
                sent_at: stamp.sent_at,
                body,
            },
            receipt,
        ))
    })
    .await?;

    complete_request_receipt(handler, context, streams, receipt, cancel).await
}

async fn complete_request_receipt(
    handler: &RequestHandler,
    context: Option<&ConversationConnectionContext>,
    streams: &mut StageStreams,
    receipt: PostResponseReceipt,
    cancel: &CancelHandle,
) -> Result<RequestDispatchOutcome, RequestStageError> {
    // The dispatcher finishes the server send side only on success, so this
    // record must happen before the local activation await. Never reset a
    // finished send side, even when activation later fails or is cancelled.
    streams.mark_send_finished();
    match receipt {
        PostResponseReceipt::Handler {
            receipt,
            stopped_thread,
        } => {
            let activation = match context {
                Some(context) => {
                    handler
                        .activate_after_response_in_context(context, receipt)
                        .await?
                }
                None => handler.activate_after_response(receipt).await?,
            };
            Ok(RequestDispatchOutcome {
                activation,
                stopped_thread,
            })
        }
        PostResponseReceipt::Lifecycle(receipt) => {
            complete_lifecycle_receipt(streams, receipt, cancel).await?;
            Ok(RequestDispatchOutcome {
                activation: None,
                stopped_thread: None,
            })
        }
    }
}

async fn complete_lifecycle_receipt(
    streams: &mut StageStreams,
    receipt: LifecycleControlReceipt,
    cancel: &CancelHandle,
) -> Result<(), RequestStageError> {
    if receipt.is_pending() {
        streams.await_finished_send_ack().await?;
    }
    receipt.commit_after_response(cancel);
    Ok(())
}

/// A correlated, bounded failure for a lifecycle request sent without the
/// negotiated per-connection witness.
fn unsupported_feature_failure(request_id: &RequestId) -> ProtocolFailure {
    ProtocolFailure {
        code: ErrorCode::UnsupportedFeature,
        detail: ErrorDetail::parse("native lifecycle control was not negotiated")
            .expect("lifecycle detail is within the protocol bound"),
        retryable: false,
        request_id: Some(request_id.clone()),
    }
}

enum PostResponseReceipt {
    Handler {
        receipt: RequestHandlerReceipt,
        stopped_thread: Option<ThreadId>,
    },
    Lifecycle(LifecycleControlReceipt),
}

enum DriverEvent {
    Request { send: SendStream, recv: RecvStream },
    Wake,
}

async fn wait_for_driver_event(
    connection: &Connection,
    driver: &mut ConversationDeliveryDriver,
) -> Result<DriverEvent, RequestStageError> {
    tokio::select! {
        accepted = connection.accept_bi() => accepted
            .map(|(send, recv)| DriverEvent::Request { send, recv })
            .map_err(|source| RequestStageError::Accept { source }),
        wake = driver.wait_for_wake() => wake
            .map(|()| DriverEvent::Wake)
            .map_err(RequestStageError::Delivery),
    }
}

#[derive(Clone, Copy)]
struct LifecycleWitness {
    supported: bool,
}

/// Private owner of one stage's bidirectional stream pair.
///
/// Dropping the value synchronously stops the inbound direction and resets
/// an outbound direction that was not successfully finished. Because the
/// enclosing future owns this value, abandonment through timeout,
/// cancellation, error, or a plain drop all receive the same
/// cancellation-safe cleanup; already-closed resources report through
/// discarded results so the primary typed failure is preserved.
struct StageStreams {
    send: Option<SendStream>,
    recv: Option<RecvStream>,
    send_finished: bool,
}

impl StageStreams {
    const fn new() -> Self {
        Self {
            send: None,
            recv: None,
            send_finished: false,
        }
    }

    /// Takes joint ownership of an accepted bidirectional pair and lends
    /// both directions back as disjoint borrows.
    fn install(
        &mut self,
        send: SendStream,
        recv: RecvStream,
    ) -> (&mut SendStream, &mut RecvStream) {
        (self.send.insert(send), self.recv.insert(recv))
    }

    /// Records that the outbound direction finished successfully and must
    /// never be reset by cleanup.
    fn mark_send_finished(&mut self) {
        self.send_finished = true;
    }

    /// Waits for Quinn to observe that the peer acknowledged every byte on a
    /// finished lifecycle response stream. A peer STOP or connection loss is
    /// deliberately collapsed to a payload-free request-stage failure.
    async fn await_finished_send_ack(&self) -> Result<(), RequestStageError> {
        let send = self
            .send
            .as_ref()
            .expect("response send stream is installed before acknowledgement");
        match send.stopped().await {
            Ok(None) => Ok(()),
            Ok(Some(_)) | Err(_) => Err(RequestStageError::LifecycleResponseAcknowledgement),
        }
    }

    /// Discharges both directions after a completed stage so the guard's
    /// [`Drop`] performs no cleanup actions on them.
    fn release(&mut self) {
        self.send = None;
        self.recv = None;
    }
}

impl Drop for StageStreams {
    fn drop(&mut self) {
        if let Some(recv) = self.recv.as_mut() {
            let _discarded = recv.stop(INBOUND_STOP_CODE);
        }
        if self.send_finished {
            return;
        }
        if let Some(send) = self.send.as_mut() {
            let _reset = send.reset(OUTBOUND_RESET_CODE);
        }
    }
}

/// Synchronous close guard installed before the authentication future
/// exists.
///
/// The guard duplicates the connection handle so the future can consume the
/// original while retaining a close-capable copy until success is proven:
/// dropping an unpolled, in-flight, or failed admission closes the accepted
/// connection with the fixed secret-free reason. Disarming on success lets
/// the ready owner take over sole responsibility.
struct AdmissionGuard {
    connection: Option<Connection>,
}

impl AdmissionGuard {
    const fn armed(connection: Connection) -> Self {
        Self {
            connection: Some(connection),
        }
    }

    fn disarm(&mut self) {
        self.connection = None;
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.close(CONNECTION_CLOSE_CODE, CONNECTION_CLOSE_REASON);
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/backend/lifecycle_connection.rs"]
pub(crate) mod lifecycle_connection_tests;

#[cfg(test)]
mod lifecycle_response_ack_tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::mpsc::RecvTimeoutError;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use super::{StageStreams, complete_lifecycle_receipt};
    use crate::lifecycle_control::{
        ActivityGateError, ActivityGateImpl, LifecycleControlReceipt, LifecycleController,
        LifecycleDispatch,
    };
    use artisan_domain::RequestId;
    use artisan_protocol::{LifecycleRequest, LifecycleResponse, LifecycleStopDisposition};
    use artisan_transport::CancelHandle;
    use quinn::{Connection, Endpoint, VarInt};
    use tokio::io::AsyncWriteExt;

    const ACK_TIMEOUT: Duration = Duration::from_secs(5);

    struct AckLoopback {
        server_address: SocketAddr,
        client: Option<Endpoint>,
        server_connections: tokio::sync::mpsc::Receiver<Connection>,
        stop_server: Option<tokio::sync::oneshot::Sender<()>>,
        server_thread: Option<JoinHandle<()>>,
    }

    impl AckLoopback {
        fn new() -> Self {
            let pki = super::lifecycle_connection_tests::test_pki();
            let server_config = super::lifecycle_connection_tests::server_config(&pki);
            let client_config = super::lifecycle_connection_tests::client_config(&pki);
            let (address_sender, address_receiver) = std::sync::mpsc::channel();
            let (connections_sender, connections_receiver) = tokio::sync::mpsc::channel(1);
            let (stop_sender, mut stop_receiver) = tokio::sync::oneshot::channel();

            let server_thread = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("acknowledgement test server runtime should build");
                runtime.block_on(async move {
                    let server = artisan_transport::bind_loopback_server(server_config)
                        .expect("acknowledgement test server should bind");
                    address_sender
                        .send(server.local_addr().expect("acknowledgement test address"))
                        .expect("acknowledgement test should receive the server address");

                    loop {
                        let incoming = tokio::select! {
                            _ = &mut stop_receiver => break,
                            incoming = server.accept() => incoming,
                        };
                        let Some(incoming) = incoming else {
                            break;
                        };
                        let established = tokio::time::timeout(ACK_TIMEOUT, incoming)
                            .await
                            .expect("acknowledgement test handshake should settle")
                            .expect("acknowledgement test handshake should succeed");
                        if connections_sender.send(established).await.is_err() {
                            break;
                        }
                    }

                    artisan_transport::shutdown(
                        &server,
                        VarInt::from_u32(0),
                        b"acknowledgement test complete",
                        ACK_TIMEOUT,
                    )
                    .await
                    .expect("acknowledgement test server should shut down");
                });
            });

            let server_address = match address_receiver.recv_timeout(ACK_TIMEOUT) {
                Ok(address) => address,
                Err(RecvTimeoutError::Timeout) => {
                    panic!("acknowledgement test server did not bind")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("acknowledgement test server thread died before binding")
                }
            };
            let client = artisan_transport::bind_loopback_client(client_config)
                .expect("acknowledgement test client should bind");
            Self {
                server_address,
                client: Some(client),
                server_connections: connections_receiver,
                stop_server: Some(stop_sender),
                server_thread: Some(server_thread),
            }
        }

        fn request_server_stop(&mut self) {
            if let Some(stop) = self.stop_server.take() {
                let _sent = stop.send(());
            }
        }

        fn join_server_thread(&mut self) {
            self.request_server_stop();
            if let Some(thread) = self.server_thread.take() {
                thread
                    .join()
                    .expect("acknowledgement test server thread should finish");
            }
        }

        async fn shutdown(mut self) {
            let client = self
                .client
                .take()
                .expect("acknowledgement test client endpoint should remain owned");
            client.close(VarInt::from_u32(0), b"acknowledgement test complete");
            tokio::time::timeout(ACK_TIMEOUT, client.wait_idle())
                .await
                .expect("acknowledgement test client endpoint should become idle");
            drop(client);
            self.join_server_thread();
        }
    }

    impl Drop for AckLoopback {
        fn drop(&mut self) {
            if let Some(client) = self.client.take() {
                client.close(VarInt::from_u32(0), b"acknowledgement test complete");
                drop(client);
            }
            self.request_server_stop();
        }
    }

    async fn connected_pair() -> (AckLoopback, Connection, Connection) {
        let mut loopback = AckLoopback::new();
        let client_connection = {
            let client = loopback
                .client
                .as_ref()
                .expect("acknowledgement test client endpoint should be available");
            tokio::time::timeout(
                ACK_TIMEOUT,
                client
                    .connect(
                        loopback.server_address,
                        artisan_transport::LOOPBACK_SERVER_NAME,
                    )
                    .expect("acknowledgement test client should connect"),
            )
            .await
            .expect("acknowledgement test client connection should settle")
            .expect("acknowledgement test client handshake should succeed")
        };
        let server_connection =
            tokio::time::timeout(ACK_TIMEOUT, loopback.server_connections.recv())
                .await
                .expect("acknowledgement test server connection should arrive")
                .expect("acknowledgement test server should remain accepting");
        (loopback, server_connection, client_connection)
    }

    async fn open_stream_pair(
        server: &Connection,
        client: &Connection,
    ) -> (
        quinn::SendStream,
        quinn::RecvStream,
        quinn::SendStream,
        quinn::RecvStream,
    ) {
        let server_stream = server.accept_bi();
        let client_stream = client.open_bi();
        let (server_stream, client_stream) = tokio::join!(server_stream, client_stream);
        let (server_send, server_recv) = server_stream.expect("server stream should open");
        let (client_send, client_recv) = client_stream.expect("client stream should open");
        (server_send, server_recv, client_send, client_recv)
    }

    async fn accepted_stop(
        controller: &LifecycleController,
        request_id: &str,
    ) -> LifecycleControlReceipt {
        let dispatch = controller
            .dispatch(
                RequestId::parse(request_id).expect("acknowledgement request id should be valid"),
                LifecycleRequest::Stop { require_idle: true },
            )
            .await;
        let LifecycleDispatch::Reply {
            response: LifecycleResponse::Stop(stop),
            receipt,
        } = dispatch
        else {
            panic!("acknowledgement test should admit an idle stop");
        };
        assert_eq!(stop.disposition, LifecycleStopDisposition::Accepted);
        receipt
    }

    #[tokio::test]
    async fn acknowledged_lifecycle_stop_commits_and_cancels_after_peer_receipt() {
        let (loopback, server, client) = connected_pair().await;
        let gate = ActivityGateImpl::new();
        let controller = LifecycleController::with_activity_gate(Arc::new(gate.clone()));
        let cancel = CancelHandle::new();
        let receipt = accepted_stop(&controller, "ack-success").await;
        let (server_send, server_recv, client_send, mut client_recv) =
            open_stream_pair(&server, &client).await;
        let mut streams = StageStreams::new();
        {
            let (server_send, _server_recv) = streams.install(server_send, server_recv);
            server_send
                .write_all(b"lifecycle-stop-response")
                .await
                .expect("acknowledgement response should be written");
            server_send
                .finish()
                .expect("acknowledgement response should finish");
        }
        streams.mark_send_finished();

        let (response, completion) = tokio::join!(
            tokio::time::timeout(ACK_TIMEOUT, client_recv.read_to_end(1024)),
            tokio::time::timeout(
                ACK_TIMEOUT,
                complete_lifecycle_receipt(&mut streams, receipt, &cancel),
            ),
        );
        assert_eq!(
            response
                .expect("peer response read should settle")
                .expect("peer should read the finished response"),
            b"lifecycle-stop-response"
        );
        completion
            .expect("peer acknowledgement should settle")
            .expect("peer acknowledgement should permit commit");
        assert!(cancel.is_cancelled());
        assert!(matches!(
            gate.acquire(),
            Err(ActivityGateError::Unavailable)
        ));

        drop(client_recv);
        drop(client_send);
        drop(streams);
        drop(server);
        drop(client);
        loopback.shutdown().await;
    }

    #[tokio::test]
    async fn peer_stop_returns_typed_failure_and_rolls_back_lifecycle_receipt() {
        let (loopback, server, client) = connected_pair().await;
        let gate = ActivityGateImpl::new();
        let controller = LifecycleController::with_activity_gate(Arc::new(gate.clone()));
        let cancel = CancelHandle::new();
        let receipt = accepted_stop(&controller, "ack-peer-stop").await;
        let (server_send, server_recv, client_send, mut client_recv) =
            open_stream_pair(&server, &client).await;
        let mut streams = StageStreams::new();
        {
            let (server_send, _server_recv) = streams.install(server_send, server_recv);
            server_send
                .write_all(b"lifecycle-stop-response")
                .await
                .expect("acknowledgement response should be written");
            server_send
                .finish()
                .expect("acknowledgement response should finish");
        }
        client_recv
            .stop(VarInt::from_u32(7))
            .expect("peer STOP should be sent");
        streams.mark_send_finished();

        let failure = tokio::time::timeout(
            ACK_TIMEOUT,
            complete_lifecycle_receipt(&mut streams, receipt, &cancel),
        )
        .await
        .expect("peer STOP acknowledgement should settle")
        .expect_err("peer STOP must reject the lifecycle receipt");
        assert!(matches!(
            &failure,
            super::RequestStageError::LifecycleResponseAcknowledgement
        ));
        assert_eq!(
            failure.to_string(),
            "lifecycle response acknowledgement failed"
        );
        assert!(!cancel.is_cancelled());

        let retry = accepted_stop(&controller, "ack-peer-stop-retry").await;
        drop(retry);
        let lease = gate
            .acquire()
            .expect("failed acknowledgement should reopen activity admission");
        drop(lease);

        drop(client_recv);
        drop(client_send);
        drop(streams);
        drop(server);
        drop(client);
        loopback.shutdown().await;
    }
}
