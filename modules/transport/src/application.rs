//! Bounded Quinn-stream conversion for owned application envelopes.
//!
//! Production session controllers use this as their only wire seam. Reading
//! always applies the transport byte ceiling before Cap'n Proto traversal or
//! owned conversion; writing validates the owned envelope before any bytes are
//! emitted. Quinn stream ownership and session policy stay with callers.

use artisan_protocol::{
    ProtocolDecodeError, ProtocolEncodeError, WireEnvelope, decode_envelope, encode_envelope,
};
use quinn::{RecvStream, SendStream};
use thiserror::Error;

use crate::{FrameError, MAX_FRAME_LEN, read_frame, write_frame};

/// Failure while sending one owned application envelope.
#[derive(Debug, Error)]
pub enum EnvelopeSendError {
    /// Owned application metadata or correlation failed before stream output.
    #[error("application envelope encoding failed: {0}")]
    Encode(#[from] ProtocolEncodeError),
    /// The encoded envelope violated framing policy or the stream write failed.
    #[error("application envelope write failed: {0}")]
    Frame(#[from] FrameError),
}

/// Failure while receiving one owned application envelope.
#[derive(Debug, Error)]
pub enum EnvelopeReceiveError {
    /// The bounded transport frame was incomplete, oversized, or unreadable.
    #[error("application envelope read failed: {0}")]
    Frame(#[from] FrameError),
    /// Cap'n Proto framing or owned application validation failed.
    #[error("application envelope decoding failed: {0}")]
    Decode(#[from] ProtocolDecodeError),
}

/// Encodes and writes one owned application envelope.
///
/// Encoding and correlation validation complete before [`write_frame`] emits
/// the length prefix. The encoded message then receives the same absolute
/// [`MAX_FRAME_LEN`] ceiling as every other transport frame.
///
/// # Errors
///
/// Returns [`EnvelopeSendError::Encode`] when the owned envelope violates
/// protocol rules, or [`EnvelopeSendError::Frame`] when its encoded form is
/// empty, oversized, or cannot be written to the QUIC stream.
pub async fn send_envelope(
    stream: &mut SendStream,
    envelope: &WireEnvelope,
) -> Result<(), EnvelopeSendError> {
    let encoded = encode_envelope(envelope)?;
    write_frame(stream, &encoded).await?;
    Ok(())
}

/// Reads and decodes one owned application envelope.
///
/// The frame length is validated before allocation, and the protocol decoder
/// independently applies its Cap'n Proto traversal and nesting limits.
///
/// # Errors
///
/// Returns [`EnvelopeReceiveError::Frame`] when the bounded frame cannot be
/// read, or [`EnvelopeReceiveError::Decode`] when the bytes do not form one
/// valid owned application envelope.
pub async fn receive_envelope(
    stream: &mut RecvStream,
) -> Result<WireEnvelope, EnvelopeReceiveError> {
    let encoded = read_frame(stream, MAX_FRAME_LEN).await?;
    Ok(decode_envelope(&encoded)?)
}
