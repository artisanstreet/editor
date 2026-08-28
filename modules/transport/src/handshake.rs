//! Application-handshake ordering over the bounded envelope stream.
//!
//! This layer consumes the client's outbound Hello, verifies Hello-first and
//! Welcome-first ordering, and checks version agreement. The server side
//! deliberately returns the owned credential to Forge: the credential
//! authority consumes it and mints the rotated reconnect capability before
//! calling [`send_server_welcome`]. Transport never stores authentication
//! secrets or decides whether they are valid.

use std::fmt;

use artisan_domain::UnixMillis;
use artisan_protocol::{
    FrameId, Hello, ProtocolFailure, ProtocolVersion, VersionOffer, Welcome, WireEnvelope,
    WireEnvelopeBody,
};
use quinn::{RecvStream, SendStream};
use thiserror::Error;

use crate::{EnvelopeReceiveError, EnvelopeSendError, receive_envelope, send_envelope};

/// Message family observed while enforcing the application handshake.
///
/// This carries no payload data, so errors can identify a protocol-ordering
/// failure without formatting credentials, request bodies, or peer details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandshakeMessageKind {
    /// Client negotiation offer.
    Hello,
    /// Successful server negotiation response.
    Welcome,
    /// Application request before establishment.
    Request,
    /// Application response before establishment.
    Response,
    /// Server event before establishment.
    Event,
    /// Typed peer rejection.
    ProtocolError,
    /// Conversation patch delivery before establishment.
    PatchBatch,
}

impl fmt::Display for HandshakeMessageKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Hello => "Hello",
            Self::Welcome => "Welcome",
            Self::Request => "Request",
            Self::Response => "Response",
            Self::Event => "Event",
            Self::ProtocolError => "ProtocolError",
            Self::PatchBatch => "PatchBatch",
        };
        formatter.write_str(name)
    }
}

/// Failure while establishing the application session on a QUIC stream.
#[derive(Debug, Error)]
pub enum HandshakeError {
    /// An outbound handshake envelope could not be encoded or written.
    #[error("sending application handshake failed: {0}")]
    Send(#[from] EnvelopeSendError),
    /// An inbound handshake envelope could not be framed or decoded.
    #[error("receiving application handshake failed: {0}")]
    Receive(#[from] EnvelopeReceiveError),
    /// One side sent a valid application message in the wrong state.
    #[error("expected {expected} during application handshake, received {received}")]
    UnexpectedMessage {
        /// Sole message family valid in this state.
        expected: HandshakeMessageKind,
        /// Payload-free family actually received.
        received: HandshakeMessageKind,
    },
    /// A peer selected or stamped a revision absent from the Hello offer.
    #[error("application protocol version {version} was not offered by the client")]
    VersionNotOffered {
        /// Invalid selected or stamped revision.
        version: u32,
    },
    /// The Welcome envelope revision disagreed with its selected revision.
    #[error(
        "Welcome envelope protocol version {envelope_version} disagrees with negotiated version {negotiated_version}"
    )]
    WelcomeVersionMismatch {
        /// Revision stamped on the envelope.
        envelope_version: u32,
        /// Revision selected inside Welcome.
        negotiated_version: u32,
    },
    /// The server returned an explicit typed rejection instead of Welcome.
    #[error("peer rejected the application handshake")]
    Rejected {
        /// Complete bounded rejection for caller policy and presentation.
        failure: ProtocolFailure,
    },
}

/// Owned client Hello received by the server before authentication.
///
/// This type deliberately derives neither `Clone` nor `Debug` because its
/// [`Hello`] contains a single-use credential.
pub struct ClientHello {
    /// Revision stamped on the Hello envelope.
    pub protocol_version: ProtocolVersion,
    /// Client-minted frame identity.
    pub frame_id: FrameId,
    /// Client timestamp retained for diagnostics and policy.
    pub sent_at: UnixMillis,
    /// Version offer and single-use credential for Forge authentication.
    pub hello: Hello,
}

/// Owned successful server answer returned to the client.
///
/// This type deliberately derives neither `Clone` nor `Debug` because its
/// [`Welcome`] contains the next single-use reconnect capability.
pub struct ServerWelcome {
    /// Revision stamped on the Welcome envelope.
    pub protocol_version: ProtocolVersion,
    /// Server-minted frame identity.
    pub frame_id: FrameId,
    /// Server timestamp retained for diagnostics and policy.
    pub sent_at: UnixMillis,
    /// Negotiated revision, connection identity, and rotated credential.
    pub welcome: Welcome,
}

pub(crate) fn kind(body: &WireEnvelopeBody) -> HandshakeMessageKind {
    match body {
        WireEnvelopeBody::Hello(_) => HandshakeMessageKind::Hello,
        WireEnvelopeBody::Welcome(_) => HandshakeMessageKind::Welcome,
        WireEnvelopeBody::Request(_) => HandshakeMessageKind::Request,
        WireEnvelopeBody::Response(_) => HandshakeMessageKind::Response,
        WireEnvelopeBody::Event(_) => HandshakeMessageKind::Event,
        WireEnvelopeBody::ProtocolError(_) => HandshakeMessageKind::ProtocolError,
        WireEnvelopeBody::PatchBatch(_) => HandshakeMessageKind::PatchBatch,
    }
}

fn ensure_offered(offer: &VersionOffer, version: ProtocolVersion) -> Result<(), HandshakeError> {
    if offer.versions().contains(&version) {
        Ok(())
    } else {
        Err(HandshakeError::VersionNotOffered {
            version: version.get(),
        })
    }
}

fn validate_welcome(
    envelope_version: ProtocolVersion,
    welcome: &Welcome,
    offer: &VersionOffer,
) -> Result<(), HandshakeError> {
    ensure_offered(offer, welcome.negotiated_version)?;
    if envelope_version == welcome.negotiated_version {
        Ok(())
    } else {
        Err(HandshakeError::WelcomeVersionMismatch {
            envelope_version: envelope_version.get(),
            negotiated_version: welcome.negotiated_version.get(),
        })
    }
}

/// Sends one client Hello and waits for one server Welcome or rejection.
///
/// Taking the Hello envelope by value makes credential ownership explicit: it
/// is dropped and zeroized after the send attempt rather than retained by a
/// reconnect loop. The returned Welcome owns the newly rotated credential.
///
/// # Errors
///
/// Returns [`HandshakeError::UnexpectedMessage`] when the outbound frame is
/// not Hello or the peer answers with neither Welcome nor `ProtocolError`,
/// [`HandshakeError::VersionNotOffered`] or
/// [`HandshakeError::WelcomeVersionMismatch`] for invalid negotiation,
/// [`HandshakeError::Rejected`] for a typed peer rejection, and the typed
/// send/receive variants for wire failures.
pub async fn client_handshake(
    send: &mut SendStream,
    receive: &mut RecvStream,
    envelope: WireEnvelope,
) -> Result<ServerWelcome, HandshakeError> {
    let offered = match &envelope.body {
        WireEnvelopeBody::Hello(hello) => {
            ensure_offered(&hello.supported_versions, envelope.protocol_version)?;
            hello.supported_versions.clone()
        }
        body => {
            return Err(HandshakeError::UnexpectedMessage {
                expected: HandshakeMessageKind::Hello,
                received: kind(body),
            });
        }
    };
    send_envelope(send, &envelope).await?;
    drop(envelope);

    let response = receive_envelope(receive).await?;
    let WireEnvelope {
        protocol_version,
        frame_id,
        sent_at,
        body,
    } = response;
    match body {
        WireEnvelopeBody::Welcome(welcome) => {
            validate_welcome(protocol_version, &welcome, &offered)?;
            Ok(ServerWelcome {
                protocol_version,
                frame_id,
                sent_at,
                welcome,
            })
        }
        WireEnvelopeBody::ProtocolError(failure) => Err(HandshakeError::Rejected { failure }),
        body => Err(HandshakeError::UnexpectedMessage {
            expected: HandshakeMessageKind::Welcome,
            received: kind(&body),
        }),
    }
}

/// Receives the first server-side application message and requires Hello.
///
/// The returned credential remains owned and opaque so Forge can consume it
/// exactly once against its launch/reconnect authority before responding.
///
/// # Errors
///
/// Returns [`HandshakeError::UnexpectedMessage`] unless the first valid
/// envelope is Hello, [`HandshakeError::VersionNotOffered`] when the outer
/// envelope revision is absent from its own offer, or a typed receive error.
pub async fn receive_client_hello(receive: &mut RecvStream) -> Result<ClientHello, HandshakeError> {
    let envelope = receive_envelope(receive).await?;
    let WireEnvelope {
        protocol_version,
        frame_id,
        sent_at,
        body,
    } = envelope;
    let WireEnvelopeBody::Hello(hello) = body else {
        return Err(HandshakeError::UnexpectedMessage {
            expected: HandshakeMessageKind::Hello,
            received: kind(&body),
        });
    };
    ensure_offered(&hello.supported_versions, protocol_version)?;
    Ok(ClientHello {
        protocol_version,
        frame_id,
        sent_at,
        hello,
    })
}

/// Validates and sends the server Welcome after credential acceptance.
///
/// Forge supplies the original offer retained from [`ClientHello`] so a
/// caller cannot accidentally select an unoffered version.
///
/// # Errors
///
/// Returns [`HandshakeError::UnexpectedMessage`] unless `envelope` is Welcome,
/// a typed negotiation error for version disagreement, or a typed send error.
pub async fn send_server_welcome(
    send: &mut SendStream,
    envelope: &WireEnvelope,
    offer: &VersionOffer,
) -> Result<(), HandshakeError> {
    let WireEnvelopeBody::Welcome(welcome) = &envelope.body else {
        return Err(HandshakeError::UnexpectedMessage {
            expected: HandshakeMessageKind::Welcome,
            received: kind(&envelope.body),
        });
    };
    validate_welcome(envelope.protocol_version, welcome, offer)?;
    send_envelope(send, envelope).await?;
    Ok(())
}
