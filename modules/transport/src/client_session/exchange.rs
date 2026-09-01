//! Composed whole-stage exchanges over the session's owned connection,
//! with guarded per-stream abandonment.
//!
//! Every stage future opened here is bounded by exactly one
//! [`run_with_deadline`](crate::run_with_deadline) call in the owner:
//! stream opening, sending, and receiving compose into a single stage.
//! The stage labels reuse the closest
//! [`OperationKind`](crate::deadline::OperationKind) honestly, exactly as
//! the server seam does: `Handshake` covers the whole application
//! handshake including its stream open/send/receive, and `Receive` covers
//! one whole request exchange for the same reason — the stage ends by
//! receiving the bounded reply. No timer is reset per await anywhere.
//!
//! # Stream guards
//!
//! [`BidiGuard`] owns one stage's bidirectional stream pair from the
//! moment both halves exist, before any later await. On abandonment — an
//! error return, cancellation, timeout, or a dropped future — it
//! explicitly STOPS inbound delivery with a fixed private code and
//! explicitly RESETS outbound delivery only when that output was not
//! deliberately finished; send-stream drop alone would FIN unfinished
//! output, which is not a reset. A completed stage finishes its output
//! first and marks that state, so cleanup never resets deliberate
//! output. The dedicated response-acknowledging stage marks inbound delivery
//! complete only after it observes clean EOF; every other escape, including
//! ordinary request completion, still stops it. Successful Hello cleanup
//! tolerates Forge's deliberate STOP after reading one Hello: finish attempts
//! against a peer-stopped send side fail harmlessly and never mask the primary
//! result.

use artisan_domain::RequestId;
use artisan_protocol::{
    ProtocolFailure, ProtocolVersion, ServerResponse, WireEnvelope, WireEnvelopeBody,
};
use quinn::{Connection, ConnectionError, RecvStream, SendStream};
use thiserror::Error;

use crate::MAX_FRAME_LEN;
use crate::frame::{FrameOutcome, read_next_frame};
use crate::handshake::kind as message_kind;
use crate::handshake::{HandshakeError, ServerWelcome, client_handshake};
use crate::{
    EnvelopeReceiveError, EnvelopeSendError, HandshakeMessageKind, receive_envelope, send_envelope,
};

use super::link::{STREAM_RESET_CODE, STREAM_STOP_CODE, close_code};

/// Failure of the composed application handshake stage.
#[derive(Debug, Error)]
pub enum HandshakeStageError {
    /// The bidirectional handshake stream could not be opened.
    #[error("opening the handshake stream failed: {0}")]
    Open(#[from] ConnectionError),
    /// The application handshake itself failed.
    #[error("application handshake failed: {0}")]
    Handshake(#[from] HandshakeError),
}

/// Failure of one composed request exchange on the wire.
#[derive(Debug, Error)]
pub enum ExchangeError {
    /// The request's fresh bidirectional stream could not be opened.
    #[error("opening the request stream failed: {0}")]
    Open(#[from] ConnectionError),
    /// The request envelope could not be encoded or written within
    /// bounds.
    #[error("sending the request failed: {0}")]
    Send(#[from] EnvelopeSendError),
    /// The reply could not be framed or decoded within bounds.
    #[error("receiving the reply failed: {0}")]
    Receive(#[from] EnvelopeReceiveError),
    /// The response stream carried another complete frame after the one
    /// reply that was admitted for this operation.
    #[error("response stream carried trailing data")]
    TrailingResponse,
}

/// Failure while running the response-acknowledging request stage.
///
/// Reply validation is deliberately kept separate from wire failures so the
/// public client method can preserve its existing direct `Reply` errors while
/// the whole composed stage remains under one deadline.
#[derive(Debug, Error)]
pub(super) enum AcknowledgingResponseError {
    /// The guarded stream exchange failed before clean response EOF.
    #[error(transparent)]
    Exchange(#[from] ExchangeError),
    /// The decoded reply failed the existing validation or settlement gate.
    #[error(transparent)]
    Reply(#[from] ReplyRejection),
}

/// Guarded owner of one composed stage's bidirectional stream pair.
///
/// Installed immediately after the pair is opened and before the next
/// await, so no escape can leave inbound delivery running or rely on
/// implicit drop semantics for the outbound direction.
///
/// Deliberately implements neither [`Clone`] nor [`Copy`]: the guard is
/// the sole owner of its stream pair.
pub(super) struct BidiGuard {
    /// Outbound half; taken during teardown so only one close action
    /// ever runs.
    send: Option<SendStream>,
    /// Inbound half; stopped on every incomplete teardown path.
    receive: Option<RecvStream>,
    /// Whether the outbound side was deliberately finished, in which
    /// case abandonment must not reset it.
    outbound_finished: bool,
    /// Whether the dedicated response-acknowledging path observed clean
    /// inbound EOF. Ordinary request completion intentionally leaves this
    /// false so its drop still emits the private STOP.
    inbound_finished: bool,
}

impl BidiGuard {
    /// Wraps a freshly opened stream pair.
    pub(super) fn new(send: SendStream, receive: RecvStream) -> Self {
        Self {
            send: Some(send),
            receive: Some(receive),
            outbound_finished: false,
            inbound_finished: false,
        }
    }

    /// Lends both halves to the composed stage body.
    fn lend(&mut self) -> (&mut SendStream, &mut RecvStream) {
        (
            self.send
                .as_mut()
                .expect("send half present until teardown"),
            self.receive
                .as_mut()
                .expect("receive half present until teardown"),
        )
    }

    /// Finishes the outbound side deliberately and marks that state so
    /// later abandonment cannot reset it.
    ///
    /// A peer-stopped send side (Forge's deliberate post-Hello STOP)
    /// makes this attempt fail harmlessly; the failure is discarded so
    /// cleanup never masks the stage's primary result.
    fn finish_outbound(&mut self) {
        if let Some(send) = self.send.as_mut() {
            let _finished = send.finish();
        }
        self.outbound_finished = true;
    }

    /// Awaits exactly the next bounded frame boundary on the response
    /// stream. A clean finish before any next prefix byte is the sole
    /// successful completion; a complete trailing frame and every partial,
    /// oversized, empty, or unreadable continuation are errors.
    async fn await_clean_response_eof(&mut self) -> Result<(), ExchangeError> {
        let outcome = {
            let receive = self
                .receive
                .as_mut()
                .expect("receive half present until teardown");
            read_next_frame(receive, MAX_FRAME_LEN)
                .await
                .map_err(EnvelopeReceiveError::Frame)?
        };
        match outcome {
            FrameOutcome::Finished => {
                self.inbound_finished = true;
                Ok(())
            }
            FrameOutcome::Frame(_) => Err(ExchangeError::TrailingResponse),
        }
    }
}

impl Drop for BidiGuard {
    fn drop(&mut self) {
        // Abandonment resets ONLY unfinished outbound output; a finished
        // side stays finished. Resetting an already-closed stream fails
        // harmlessly and preserves the primary error.
        if let Some(mut send) = self.send.take()
            && !self.outbound_finished
        {
            let _reset = send.reset(close_code(STREAM_RESET_CODE));
        }
        // Inbound delivery always stops, discarding any late peer data,
        // except after the dedicated path has positively observed clean EOF.
        if !self.inbound_finished {
            if let Some(mut receive) = self.receive.take() {
                let _stopped = receive.stop(close_code(STREAM_STOP_CODE));
            }
        }
    }
}

/// Runs the whole application handshake as one composed stage: opens one
/// guarded bidirectional stream, sends the owned Hello, and awaits the
/// Welcome or typed rejection. The streams are finished (tolerating the
/// server's deliberate post-Hello STOP) once the Welcome arrives, and
/// every other escape resets and stops them through [`BidiGuard`].
///
/// # Errors
///
/// Returns [`HandshakeStageError::Open`] when the stream cannot be opened
/// and [`HandshakeStageError::Handshake`] for every typed handshake
/// failure.
pub(super) async fn handshake_stage(
    connection: &Connection,
    hello: WireEnvelope,
) -> Result<ServerWelcome, HandshakeStageError> {
    let (send, receive) = connection.open_bi().await?;
    let mut streams = BidiGuard::new(send, receive);
    let welcome = {
        let (send, receive) = streams.lend();
        client_handshake(send, receive, hello).await?
    };
    streams.finish_outbound();
    drop(streams);
    Ok(welcome)
}

/// Runs one whole request exchange as one composed stage: opens one
/// guarded fresh bidirectional stream, sends exactly one bounded request
/// envelope, receives exactly one bounded reply, and finishes the
/// outbound side. Any earlier escape resets and stops through
/// [`BidiGuard`].
///
/// # Errors
///
/// Returns the typed [`ExchangeError`] variants for stream-open, send,
/// and receive failures respectively.
pub(super) async fn request_stage(
    connection: &Connection,
    envelope: WireEnvelope,
) -> Result<WireEnvelope, ExchangeError> {
    let (send, receive) = connection.open_bi().await?;
    let mut streams = BidiGuard::new(send, receive);
    let reply = {
        let (send, receive) = streams.lend();
        send_envelope(send, &envelope).await?;
        receive_envelope(receive).await?
    };
    streams.finish_outbound();
    drop(streams);
    Ok(reply)
}

/// Runs a request exchange whose caller must settle the one decoded reply
/// before the stage may consume the response stream's clean EOF.
///
/// The callback runs after the same one-request send/receive sequence and
/// after outbound FIN custody is established, but before any read that could
/// acknowledge the peer's response FIN. Only after the callback succeeds does
/// the stage accept a pre-frame [`FrameOutcome::Finished`]. Every other result
/// drops the guard with its private inbound STOP.
///
/// # Errors
///
/// Returns [`AcknowledgingResponseError::Reply`] when the callback rejects or
/// cannot settle the decoded reply, and [`AcknowledgingResponseError::Exchange`]
/// for stream, framing, or response-EOF failures.
pub(super) async fn request_stage_acknowledging_response<T, F>(
    connection: &Connection,
    envelope: WireEnvelope,
    settle: F,
) -> Result<T, AcknowledgingResponseError>
where
    F: FnOnce(&WireEnvelope) -> Result<T, AcknowledgingResponseError>,
{
    let (send, receive) = connection
        .open_bi()
        .await
        .map_err(ExchangeError::from)
        .map_err(AcknowledgingResponseError::from)?;
    let mut streams = BidiGuard::new(send, receive);
    let reply = {
        let (send, receive) = streams.lend();
        send_envelope(send, &envelope)
            .await
            .map_err(ExchangeError::from)
            .map_err(AcknowledgingResponseError::from)?;
        receive_envelope(receive)
            .await
            .map_err(ExchangeError::from)
            .map_err(AcknowledgingResponseError::from)?
    };
    streams.finish_outbound();
    let settled = settle(&reply)?;
    streams.await_clean_response_eof().await?;
    drop(streams);
    Ok(settled)
}

/// A decoded reply that passed version, family, and correlation
/// validation and may settle exactly the current admitted request.
#[derive(Debug)]
pub(super) enum SettledReply {
    /// Successful correlated response.
    Response(ServerResponse),
    /// Correlated typed peer rejection; an ordinary outcome, not a local
    /// error.
    Failure(ProtocolFailure),
}

/// Failure while validating one reply against the negotiated version and
/// the current operation's correlation identity.
///
/// Every variant is terminal for the session: none settles a waiter, and
/// none is ever converted into a fabricated peer failure.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ReplyRejection {
    /// The reply envelope's revision disagreed with the session's
    /// negotiated version. Defense in depth: the codec already rejects
    /// revisions it cannot represent, so a decoded reply currently
    /// cannot reach this arm with a different-but-supported revision;
    /// the check stays typed instead of being assumed away.
    #[error(
        "reply protocol version {envelope_version} disagrees with negotiated version {negotiated_version}"
    )]
    VersionMismatch {
        /// Revision stamped on the reply envelope.
        envelope_version: u32,
        /// Revision negotiated during the session's handshake.
        negotiated_version: u32,
    },
    /// The reply carried a payload-free family that cannot settle a
    /// request (Hello, Welcome, Request, Event, or `PatchBatch`).
    #[error("expected a correlated reply, received {received}")]
    UnexpectedFamily {
        /// Payload-free family actually received.
        received: HandshakeMessageKind,
    },
    /// A protocol failure arrived carrying no request identity, so it
    /// correlates to nothing pending.
    #[error("peer failure carries no request id to correlate")]
    MissingCorrelation,
    /// The reply settled some other request than the current operation —
    /// including an already-retired identity.
    #[error("reply settles a different request")]
    DifferentCorrelation,
    /// A matching reply nonetheless failed lifecycle settlement. This
    /// would mean bookkeeping divergence between waiters and registry;
    /// it is terminal like every other local error and never silently
    /// ignored.
    #[error("settling the matched reply failed: {0}")]
    Settle(#[source] crate::RequestCorrelationError),
}

/// Classifies one decoded reply before any lifecycle mutation.
///
/// Frame-derived identity stays authoritative: only a response echoing
/// exactly `expected`, or a failure carrying exactly `expected`, may
/// settle the admitted request.
///
/// # Errors
///
/// Returns the typed [`ReplyRejection`] variants for version, family,
/// and correlation disagreements.
pub(super) fn classify_reply(
    reply: &WireEnvelope,
    negotiated_version: ProtocolVersion,
    expected: &RequestId,
) -> Result<SettledReply, ReplyRejection> {
    if reply.protocol_version != negotiated_version {
        return Err(ReplyRejection::VersionMismatch {
            envelope_version: reply.protocol_version.get(),
            negotiated_version: negotiated_version.get(),
        });
    }
    match &reply.body {
        WireEnvelopeBody::Response(response) => {
            if &response.request_id == expected {
                Ok(SettledReply::Response(response.clone()))
            } else {
                Err(ReplyRejection::DifferentCorrelation)
            }
        }
        WireEnvelopeBody::ProtocolError(failure) => match &failure.request_id {
            Some(request_id) if request_id == expected => {
                Ok(SettledReply::Failure(failure.clone()))
            }
            Some(_) => Err(ReplyRejection::DifferentCorrelation),
            None => Err(ReplyRejection::MissingCorrelation),
        },
        body => Err(ReplyRejection::UnexpectedFamily {
            received: message_kind(body),
        }),
    }
}
