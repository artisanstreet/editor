//! Bounded length-prefixed framing over Quinn streams.
//!
//! Every frame travels as one `u32` little-endian length prefix followed by
//! exactly that many payload bytes. QUIC streams deliver ordered, reliable
//! bytes without message boundaries, so the prefix is the only thing that
//! makes one message distinguishable from the next. Bounds are enforced
//! before any payload allocation: a hostile length prefix costs four bytes
//! of buffering, never its announced size.

use std::mem::size_of;

use quinn::{ReadError, ReadExactError, RecvStream, SendStream, WriteError};
use thiserror::Error;

/// Largest frame the production transport accepts. The bound mirrors the
/// legacy codec so a Phase 1 peer can never be pushed into allocating more
/// than four mebibytes per message.
pub const MAX_FRAME_LEN: u32 = 4 * 1024 * 1024;

const PREFIX_LEN: usize = size_of::<u32>();

/// Failures while reading or writing one framed message.
#[derive(Debug, Clone, Eq, Error, PartialEq)]
pub enum FrameError {
    /// A length prefix announced more bytes than the configured bound.
    #[error("announced frame of {length} bytes exceeds the {bound}-byte bound")]
    TooLarge {
        /// Length carried by the offending prefix.
        length: u32,
        /// Bound the reader or writer enforces.
        bound: u32,
    },

    /// A zero-length prefix carries no message and is always invalid.
    #[error("zero-length frames are not valid transport messages")]
    Empty,

    /// The stream ended before the pending unit was complete.
    ///
    /// `expected` counts every byte of the pending unit (the four-byte
    /// prefix or the announced body) and `received` counts how many of
    /// them arrived before the stream finished.
    #[error("stream ended after {received} of {expected} expected bytes")]
    Truncated {
        /// Bytes required by the pending read.
        expected: usize,
        /// Bytes that arrived before the stream finished.
        received: usize,
    },

    /// The receive side of the stream failed.
    #[error(transparent)]
    Read(#[from] ReadError),

    /// The send side of the stream failed.
    #[error(transparent)]
    Write(#[from] WriteError),
}

/// Reads one framed message from `stream`, refusing anything above `bound`.
///
/// Callers pass an explicit bound at every site so the limit stays visible
/// where it is enforced; production readers use [`MAX_FRAME_LEN`] unless a
/// tighter application bound applies. The announced length is validated
/// before the payload buffer is allocated, and zero-length prefixes are
/// rejected because a frame always carries exactly one non-empty message.
///
/// # Errors
///
/// Returns [`FrameError::Empty`] for zero-length prefixes,
/// [`FrameError::TooLarge`] when the announced length exceeds `bound`,
/// [`FrameError::Truncated`] when the stream finishes mid-frame, and
/// [`FrameError::Read`] when the QUIC receive side fails.
pub async fn read_frame(stream: &mut RecvStream, bound: u32) -> Result<Vec<u8>, FrameError> {
    let mut prefix = [0u8; PREFIX_LEN];
    match stream.read_exact(&mut prefix).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(received)) => {
            return Err(FrameError::Truncated {
                expected: PREFIX_LEN,
                received,
            });
        }
        Err(ReadExactError::ReadError(source)) => return Err(source.into()),
    }

    let length = u32::from_le_bytes(prefix);
    if length == 0 {
        return Err(FrameError::Empty);
    }
    if length > bound {
        return Err(FrameError::TooLarge { length, bound });
    }

    let mut body = vec![0u8; length as usize];
    match stream.read_exact(&mut body).await {
        Ok(()) => Ok(body),
        Err(ReadExactError::FinishedEarly(received)) => Err(FrameError::Truncated {
            expected: body.len(),
            received,
        }),
        Err(ReadExactError::ReadError(source)) => Err(source.into()),
    }
}

/// Writes one framed message onto `stream` as a prefix plus body.
///
/// Empty bodies are rejected and the absolute [`MAX_FRAME_LEN`] ceiling is
/// enforced before any byte reaches the wire; a receiver may still apply a
/// tighter bound of its own through [`read_frame`].
///
/// # Errors
///
/// Returns [`FrameError::Empty`] for empty bodies, [`FrameError::TooLarge`]
/// when the body exceeds [`MAX_FRAME_LEN`], and [`FrameError::Write`] when
/// the QUIC send side fails. The stream is left untouched when validation
/// fails, and partially written frames surface [`FrameError::Write`]
/// carrying the underlying send failure.
pub async fn write_frame(stream: &mut SendStream, body: &[u8]) -> Result<(), FrameError> {
    if body.is_empty() {
        return Err(FrameError::Empty);
    }

    let length = match u32::try_from(body.len()) {
        Ok(length) if length <= MAX_FRAME_LEN => length,
        Ok(length) => {
            return Err(FrameError::TooLarge {
                length,
                bound: MAX_FRAME_LEN,
            });
        }
        // The body cannot even be addressed by a `u32` prefix, so it is
        // necessarily above the configured bound.
        Err(_) => {
            return Err(FrameError::TooLarge {
                length: u32::MAX,
                bound: MAX_FRAME_LEN,
            });
        }
    };

    stream.write_all(&length.to_le_bytes()).await?;
    stream.write_all(body).await?;
    Ok(())
}
