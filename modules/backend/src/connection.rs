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

use artisan_domain::{RequestId, UnixMillis};
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

use crate::conversation_subscription_registry::ActivateError;
use crate::credential_authority::{
    CredentialAuthenticationError, CredentialAuthority, CredentialEntropyError,
    ReconnectRotationError,
};
use crate::lifecycle_control::{LifecycleControlReceipt, LifecycleController, LifecycleDispatch};
use crate::request_handler::{RequestHandler, RequestHandlerReceipt};

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

    /// A prepared conversation subscription could not be activated after its
    /// correlated response was written and the server send side finished.
    #[error("activating the conversation subscription failed")]
    Activate(#[from] ActivateError),
}

/// One admitted, authenticated Forge connection owned exclusively by its
/// caller.
///
/// The value keeps Quinn private, retains the exclusive mutable
/// credential-authority lease for its whole lifetime, and closes the owned
/// connection with the fixed application reason when dropped. The caller
/// services it sequentially with [`Self::respond_next`]; no spawning,
/// concurrent handler tasks, or background work exist inside this leaf.
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
                    let admitted = ForgeConnection {
                        connection,
                        authority,
                        handler,
                        cancel,
                        lifecycle,
                        lifecycle_witness,
                        limits,
                        protocol_version,
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
    /// any local post-write subscription activation completed. A successful
    /// call proves neither peer application nor durable acknowledgement,
    /// patch replay, or delivery.
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
                &mut streams,
                self.cancel,
            ),
        )
        .await;

        match outcome {
            Ok(()) => {
                // The stage completed: hand both directions back for
                // ordinary drops without cleanup actions.
                streams.release();
                Ok(self)
            }
            Err(source) => Err(source),
        }
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
    streams: &mut StageStreams,
    cancel: &CancelHandle,
) -> Result<(), RequestStageError> {
    let (send, recv) = connection
        .accept_bi()
        .await
        .map_err(|source| RequestStageError::Accept { source })?;
    let (send, recv) = streams.install(send, recv);

    // The incoming frame-derived request id stays authoritative. Lifecycle
    // requests are intercepted before the ordinary handler, while every
    // other request follows the existing handler path. Receipts remain opaque
    // until the transport has validated, written, and finished the correlated
    // reply.
    let receipt = dispatch_server_request_with_receipt(send, recv, |incoming| async move {
        let request_id = incoming.request_id;
        let request = incoming.request;
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
                let (answered, receipt) = handler
                    .respond_with_receipt(&request_id, &request)
                    .await
                    .into_parts();
                (answered, PostResponseReceipt::Handler(receipt))
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

    // The dispatcher finishes the server send side only on success, so this
    // record must happen before the local activation await. Never reset a
    // finished send side, even when activation later fails or is cancelled.
    streams.mark_send_finished();
    match receipt {
        PostResponseReceipt::Handler(receipt) => {
            let _activation = handler.activate_after_response(receipt).await?;
        }
        PostResponseReceipt::Lifecycle(receipt) => receipt.commit_after_response(cancel),
    }
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
    Handler(RequestHandlerReceipt),
    Lifecycle(LifecycleControlReceipt),
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
