//! Server-side dispatch of one client request over one accepted
//! bidirectional stream.
//!
//! This layer sits directly above the owned-envelope seam and below every
//! session concern: it receives exactly one bounded owned envelope, requires
//! [`WireEnvelopeBody::Request`], hands the owned request plus its
//! correlation identity to the caller-supplied async handler exactly once,
//! validates that the handler produced a correlated successful response or
//! correlated typed failure, writes that reply through the same stream, and
//! finishes the server send side. Accept loops, connection supervision,
//! fan-out, retry policy, deadline policy, and backend service behavior stay
//! with callers.
//!
//! Reply validation deliberately completes before any byte is emitted: only
//! a successful response carrying exactly this request's correlation id, or
//! a typed protocol failure carrying exactly this request's correlation id,
//! may cross the stream. A local handler failure is returned as
//! [`ServerDispatchError::Local`] and is never translated into a protocol
//! failure; the caller owns any decision to report it to the peer.
//!
//! Error data owned by this module never copies or formats request payloads,
//! credentials, envelope bodies, or other peer-controlled detail; wrong-input
//! diagnostics identify only the payload-free message family through
//! [`HandshakeMessageKind`](crate::HandshakeMessageKind), the crate's single
//! message-family vocabulary.

use std::future::Future;

use artisan_domain::{IdentifierError, RequestId, UnixMillis};
use artisan_protocol::{ClientRequest, FrameId, ProtocolVersion, WireEnvelope, WireEnvelopeBody};
use quinn::{ClosedStream, RecvStream, SendStream};
use thiserror::Error;

use crate::handshake::kind;
use crate::{
    EnvelopeReceiveError, EnvelopeSendError, HandshakeMessageKind, receive_envelope, send_envelope,
};

/// One accepted client request together with the correlation context its
/// reply must carry.
///
/// The owned [`ClientRequest`] travels alongside every envelope field the
/// caller needs to build and stamp a reply; Quinn stream handles are never
/// exposed here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncomingRequest {
    /// Revision stamped on the request envelope.
    pub protocol_version: ProtocolVersion,
    /// Client-minted frame identity.
    pub frame_id: FrameId,
    /// Client timestamp retained for diagnostics and policy.
    pub sent_at: UnixMillis,
    /// Correlation identity every accepted reply must settle.
    ///
    /// Derived from [`IncomingRequest::frame_id`], so pure reads correlate
    /// their replies exactly like mutations do.
    pub request_id: RequestId,
    /// Owned application request.
    pub request: ClientRequest,
}

/// Failure modes while validating one handler reply against its request.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ReplyValidationError {
    /// The handler replied with a message outside the reply families, so no
    /// byte of it may reach the stream.
    #[error("handler replied with {received} instead of a correlated reply")]
    WrongFamily {
        /// Payload-free family actually produced by the handler.
        received: HandshakeMessageKind,
    },
    /// The handler replied with a protocol failure that carries no request
    /// id, so it correlates to nothing.
    #[error("handler replied with a protocol failure that carries no request id")]
    Uncorrelated,
    /// The handler's reply settles a request other than the one dispatched.
    #[error("handler reply settles a different request")]
    DifferentRequest,
}

/// Failure while dispatching one server-side request stream.
///
/// Receive, handler, reply-validation, send, and stream-finish failures stay
/// distinct so callers can own retry, logging, and peer-notification policy
/// per stage. The generic `E` is the handler's own local failure type; it is
/// preserved verbatim and never converted into wire output.
#[derive(Debug, Error)]
pub enum ServerDispatchError<E> {
    /// The inbound request could not be framed or decoded within bounds.
    #[error("receiving the inbound request failed: {0}")]
    Receive(#[from] EnvelopeReceiveError),
    /// A valid application envelope arrived, but it was not a request.
    #[error("expected Request on the server stream, received {received}")]
    UnexpectedMessage {
        /// Payload-free family actually received from the peer.
        received: HandshakeMessageKind,
    },
    /// The validated frame identity could not be reused as a reply
    /// correlation id. Unreachable for decoded envelopes, which have already
    /// passed identifier validation, but kept typed instead of panicked.
    #[error("request frame id cannot be reused as the reply correlation")]
    Correlation {
        /// Underlying identifier failure, preserved without reformatting.
        #[source]
        source: IdentifierError,
    },
    /// The caller-supplied handler rejected the request locally. The typed
    /// failure belongs to the caller, which owns any policy for reporting it.
    #[error("the request handler failed")]
    Local(E),
    /// The handler produced a reply that is not a correlated response or
    /// correlated protocol failure. Nothing was written to the stream.
    #[error("validating the handler reply failed: {0}")]
    Reply(#[from] ReplyValidationError),
    /// The validated reply could not be encoded or written within bounds.
    #[error("sending the reply failed: {0}")]
    Send(#[from] EnvelopeSendError),
    /// The reply was written but the send side could not be finished.
    #[error("finishing the server send side failed: {0}")]
    Finish(#[from] ClosedStream),
}

/// Dispatches exactly one server-side request over one accepted stream pair
/// and returns a local receipt only after the reply has been written and the
/// send side finished.
///
/// Reads one envelope through [`receive_envelope`], requires its body to be a
/// request, invokes `handle` exactly once with the owned
/// [`IncomingRequest`], validates the produced reply envelope's family and
/// correlation, writes it through [`send_envelope`], and finishes the server
/// send side so the peer observes end-of-stream behind exactly one reply.
/// One request per stream is the whole contract: callers decide everything
/// about accepting streams, deadlines, cancellation, retries, and whether a
/// failed dispatch should be reported to the peer.
///
/// The receipt `R` is local only and never encoded, cloned, logged, inspected,
/// or exposed before successful `send.finish()`. Receipt release proves reply
/// write and send-side finish, not peer application or durable acknowledgement.
/// Any receipt already produced by the handler is dropped on every later
/// validation, send, or finish failure and is never returned on a partial
/// wire outcome.
///
/// # Errors
///
/// Returns [`ServerDispatchError::Receive`] for bounded receive failures,
/// [`ServerDispatchError::UnexpectedMessage`] when the first valid envelope
/// is not a request (the handler is never invoked),
/// [`ServerDispatchError::Correlation`] if the frame identity cannot be
/// reused as a correlation id, [`ServerDispatchError::Local`] when `handle`
/// fails, [`ServerDispatchError::Reply`] when the handler's reply is the
/// wrong family, uncorrelated, or settles a different request (nothing is
/// written), [`ServerDispatchError::Send`] when the validated reply cannot be
/// encoded or written, and [`ServerDispatchError::Finish`] when the send side
/// cannot be finished after the reply.
pub async fn dispatch_server_request_with_receipt<F, Fut, E, R>(
    send: &mut SendStream,
    receive: &mut RecvStream,
    handle: F,
) -> Result<R, ServerDispatchError<E>>
where
    F: FnOnce(IncomingRequest) -> Fut,
    Fut: Future<Output = Result<(WireEnvelope, R), E>>,
{
    let WireEnvelope {
        protocol_version,
        frame_id,
        sent_at,
        body,
    } = receive_envelope(receive).await?;
    let WireEnvelopeBody::Request(request) = body else {
        return Err(ServerDispatchError::UnexpectedMessage {
            received: kind(&body),
        });
    };
    let request_id = frame_id
        .to_request_id()
        .map_err(|source| ServerDispatchError::Correlation { source })?;
    let incoming = IncomingRequest {
        protocol_version,
        frame_id,
        sent_at,
        request_id: request_id.clone(),
        request,
    };

    let (reply, receipt) = handle(incoming).await.map_err(ServerDispatchError::Local)?;
    validate_reply(&reply.body, &request_id)?;
    send_envelope(send, &reply).await?;
    send.finish()?;
    Ok(receipt)
}

/// Dispatches exactly one server-side request over one accepted stream pair.
///
/// Reads one envelope through [`receive_envelope`], requires its body to be a
/// request, invokes `handle` exactly once with the owned
/// [`IncomingRequest`], validates the produced reply envelope's family and
/// correlation, writes it through [`send_envelope`], and finishes the server
/// send side so the peer observes end-of-stream behind exactly one reply.
/// One request per stream is the whole contract: callers decide everything
/// about accepting streams, deadlines, cancellation, retries, and whether a
/// failed dispatch should be reported to the peer.
///
/// # Errors
///
/// Returns [`ServerDispatchError::Receive`] for bounded receive failures,
/// [`ServerDispatchError::UnexpectedMessage`] when the first valid envelope
/// is not a request (the handler is never invoked),
/// [`ServerDispatchError::Correlation`] if the frame identity cannot be
/// reused as a correlation id, [`ServerDispatchError::Local`] when `handle`
/// fails, [`ServerDispatchError::Reply`] when the handler's reply is the
/// wrong family, uncorrelated, or settles a different request (nothing is
/// written), [`ServerDispatchError::Send`] when the validated reply cannot be
/// encoded or written, and [`ServerDispatchError::Finish`] when the send side
/// cannot be finished after the reply.
pub async fn dispatch_server_request<F, Fut, E>(
    send: &mut SendStream,
    receive: &mut RecvStream,
    handle: F,
) -> Result<(), ServerDispatchError<E>>
where
    F: FnOnce(IncomingRequest) -> Fut,
    Fut: Future<Output = Result<WireEnvelope, E>>,
{
    dispatch_server_request_with_receipt(send, receive, |incoming| async move {
        let envelope = handle(incoming).await?;
        Ok((envelope, ()))
    })
    .await
}

/// Validates that one handler-produced envelope is a correlated reply for
/// `request_id`, before any byte reaches the stream.
fn validate_reply(
    body: &WireEnvelopeBody,
    request_id: &RequestId,
) -> Result<(), ReplyValidationError> {
    match body {
        WireEnvelopeBody::Response(response) => {
            if &response.request_id == request_id {
                Ok(())
            } else {
                Err(ReplyValidationError::DifferentRequest)
            }
        }
        WireEnvelopeBody::ProtocolError(failure) => match &failure.request_id {
            Some(id) if id == request_id => Ok(()),
            Some(_) => Err(ReplyValidationError::DifferentRequest),
            None => Err(ReplyValidationError::Uncorrelated),
        },
        body => Err(ReplyValidationError::WrongFamily {
            received: kind(body),
        }),
    }
}
