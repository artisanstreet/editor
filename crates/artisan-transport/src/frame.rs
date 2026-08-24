//! Bounded Cap'n Proto frame encoding and decoding for the WebSocket lanes.
//!
//! Every frame on the wire is one serialized Cap'n Proto message. The codec
//! enforces the plan's backpressure rules at this boundary:
//!
//! - encoded output larger than [`FrameCodec::max_frame_bytes`] is rejected
//!   instead of silently shipped;
//! - decoded input is length-checked before parsing and traversal-limited
//!   during parsing, so a hostile peer cannot force unbounded allocation;
//! - all failures are typed; external input never panics.

use std::io::Cursor;

/// Hard ceiling on one serialized frame, 4 MiB. Stream chunks and handshake
/// frames sit far below this; transcript blocks arrive as events in later
/// schema families and are re-evaluated there if they ever need more.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Failures that can occur while framing one Cap'n Proto message.
///
/// Note that Cap'n Proto reports premature input exhaustion as an ordinary
/// parse failure (`capnp::ErrorKind::Failed`) rather than a distinct kind;
/// this crate does not second-guess that classification into a separate
/// "truncated" case.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// An encoded frame exceeded the configured wire bound.
    #[error("frame of {actual} bytes exceeds the {limit}-byte wire bound")]
    TooLarge {
        /// The configured maximum frame size.
        limit: usize,
        /// The size of the frame that was rejected.
        actual: usize,
    },

    /// The bytes are not a well-formed, complete Cap'n Proto message.
    #[error("malformed or truncated frame: {0}")]
    Malformed(#[from] capnp::Error),

    /// A union discriminant outside the schema was encountered.
    #[error("frame carries a union variant not present in the schema: {0}")]
    UnknownVariant(#[from] capnp::NotInSchema),
}

/// Encodes one message with the default builder and returns its bytes,
/// rejecting output beyond `max_frame_bytes`.
///
/// # Errors
/// Returns [`FrameError::TooLarge`] when the built message exceeds the
/// configured bound. Building itself is infallible by construction.
pub fn encode_frame(
    max_frame_bytes: usize,
    build: impl FnOnce(&mut capnp::message::Builder<capnp::message::HeapAllocator>),
) -> Result<Vec<u8>, FrameError> {
    let mut message = capnp::message::Builder::new_default();
    build(&mut message);
    let bytes = capnp::serialize::write_message_to_words(&message);
    if bytes.len() > max_frame_bytes {
        return Err(FrameError::TooLarge {
            limit: max_frame_bytes,
            actual: bytes.len(),
        });
    }
    Ok(bytes)
}

/// Decodes one frame from `bytes`, enforcing both the byte bound and the
/// Cap'n Proto traversal limit before any structured access happens.
///
/// # Errors
/// Returns [`FrameError::TooLarge`] when the input exceeds the configured
/// bound and [`FrameError::Malformed`] when the message is incomplete or does
/// not parse; hostile input is rejected, never panicked on.
pub fn decode_frame(
    codec: &FrameCodec,
    bytes: &[u8],
) -> Result<capnp::message::Reader<capnp::serialize::OwnedSegments>, FrameError> {
    if bytes.len() > codec.max_frame_bytes {
        return Err(FrameError::TooLarge {
            limit: codec.max_frame_bytes,
            actual: bytes.len(),
        });
    }
    let options = {
        let mut options = capnp::message::ReaderOptions::new();
        options.traversal_limit_in_words(Some(codec.traversal_limit_in_words));
        options.nesting_limit(codec.nesting_limit);
        options
    };
    let mut cursor = Cursor::new(bytes);
    let reader = capnp::serialize::read_message(&mut cursor, options)?;
    Ok(reader)
}

/// Named bounds shared by every lane using this codec.
#[derive(Debug, Clone)]
pub struct FrameCodec {
    max_frame_bytes: usize,
    traversal_limit_in_words: usize,
    nesting_limit: i32,
}

impl Default for FrameCodec {
    fn default() -> Self {
        Self {
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            traversal_limit_in_words: DEFAULT_MAX_FRAME_BYTES / 8,
            nesting_limit: 32,
        }
    }
}

impl FrameCodec {
    /// A codec with [`DEFAULT_MAX_FRAME_BYTES`] and matching traversal bounds.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A codec with an explicit frame bound; traversal limits derive from it.
    #[must_use]
    pub fn with_max_frame_bytes(max_frame_bytes: usize) -> Self {
        Self {
            max_frame_bytes,
            traversal_limit_in_words: (max_frame_bytes / 8).max(1),
            nesting_limit: 32,
        }
    }

    /// The configured maximum serialized frame size.
    #[must_use]
    pub fn max_frame_bytes(&self) -> usize {
        self.max_frame_bytes
    }

    /// The configured Cap'n Proto traversal limit in words.
    #[must_use]
    pub fn traversal_limit_in_words(&self) -> usize {
        self.traversal_limit_in_words
    }

    /// The configured struct nesting limit.
    #[must_use]
    pub fn nesting_limit(&self) -> i32 {
        self.nesting_limit
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameCodec, FrameError, decode_frame, encode_frame};
    use artisan_protocol::handshake_capnp;
    use artisan_protocol::stream_capnp::stream_frame;

    fn codec() -> FrameCodec {
        FrameCodec::new()
    }

    fn sample_chunk_text(message_id: &str, text: &str) -> Result<Vec<u8>, FrameError> {
        encode_frame(codec().max_frame_bytes(), |message| {
            let mut frame = message.init_root::<stream_frame::Builder>();
            frame.set_message_id(message_id);
            frame.set_origin(artisan_protocol::common_capnp::Origin::Backend);
            frame.set_protocol_version(2);
            frame.set_schema_version(1);
            frame.set_sent_at("2026-08-24T10:00:00.000Z");
            frame.set_channel_id("channel-7");
            frame.set_channel_sequence(1);
            frame.set_stream_id("terminal-out");
            let mut chunk = frame.init_chunk();
            chunk.set_text(text);
        })
    }

    #[test]
    fn stream_chunk_text_round_trips() -> Result<(), FrameError> {
        let bytes = sample_chunk_text("msg-2", "hello")?;
        let reader = decode_frame(&codec(), &bytes)?;
        let frame = reader.get_root::<stream_frame::Reader>()?;
        assert_eq!(frame.get_message_id()?, "msg-2");
        assert_eq!(
            frame.get_origin()?,
            artisan_protocol::common_capnp::Origin::Backend
        );
        assert_eq!(frame.get_channel_id()?, "channel-7");
        assert_eq!(frame.get_stream_id()?, "terminal-out");
        match frame.which()? {
            stream_frame::Which::Chunk(chunk) => match chunk.which()? {
                stream_frame::chunk::WhichReader::Text(text) => {
                    assert_eq!(text?, "hello");
                }
                stream_frame::chunk::WhichReader::Bytes(_) => panic!("expected the text variant"),
            },
            _ => panic!("expected the chunk variant"),
        }
        Ok(())
    }

    #[test]
    fn stream_chunk_bytes_round_trips_natively() -> Result<(), FrameError> {
        let bytes = encode_frame(codec().max_frame_bytes(), |message| {
            let mut frame = message.init_root::<stream_frame::Builder>();
            frame.set_message_id("msg-bin");
            frame.set_origin(artisan_protocol::common_capnp::Origin::Frontend);
            frame.set_protocol_version(2);
            frame.set_schema_version(1);
            frame.set_sent_at("2026-08-24T10:00:01.000Z");
            frame.set_channel_id("channel-7");
            frame.set_channel_sequence(2);
            frame.set_stream_id("terminal-out");
            frame.init_chunk().set_bytes(&[0xDE, 0xAD, 0xBE, 0xEF]);
        })?;
        let reader = decode_frame(&codec(), &bytes)?;
        let frame = reader.get_root::<stream_frame::Reader>()?;
        match frame.which()? {
            stream_frame::Which::Chunk(chunk) => match chunk.which()? {
                stream_frame::chunk::WhichReader::Bytes(bytes) => {
                    assert_eq!(bytes?, &[0xDE, 0xAD, 0xBE, 0xEF]);
                }
                stream_frame::chunk::WhichReader::Text(_) => panic!("expected the bytes variant"),
            },
            _ => panic!("expected the chunk variant"),
        }
        Ok(())
    }

    #[test]
    fn stream_end_reason_is_genuinely_optional() -> Result<(), FrameError> {
        let absent = encode_frame(codec().max_frame_bytes(), |message| {
            let mut frame = message.init_root::<stream_frame::Builder>();
            frame.set_message_id("end-a");
            frame.set_origin(artisan_protocol::common_capnp::Origin::Frontend);
            frame.set_protocol_version(2);
            frame.set_schema_version(1);
            frame.set_sent_at("2026-08-24T10:00:02.000Z");
            frame.set_channel_id("channel-7");
            frame.set_channel_sequence(3);
            frame.set_stream_id("terminal-out");
            frame.init_end();
        })?;
        let reader = decode_frame(&codec(), &absent)?;
        let frame = reader.get_root::<stream_frame::Reader>()?;
        match frame.which()? {
            stream_frame::Which::End(end) => {
                assert!(!end.has_reason(), "absent reason must read as null");
            }
            _ => panic!("expected the end variant"),
        }
        Ok(())
    }

    #[test]
    fn hello_event_cursors_round_trip_through_the_list() -> Result<(), FrameError> {
        let bytes = encode_frame(codec().max_frame_bytes(), |message| {
            let mut hello = message.init_root::<handshake_capnp::hello_envelope::Builder>();
            hello.set_message_id("hello-1");
            hello.set_origin(artisan_protocol::common_capnp::Origin::Frontend);
            hello.set_schema_version(1);
            hello.set_sent_at("2026-08-24T10:00:03.000Z");
            let mut payload = hello.init_payload();
            payload.set_last_journal_sequence(41);
            payload.set_resume_mode("resume");
            let mut cursors = payload.reborrow().init_event_cursors(2);
            let mut first = cursors.reborrow().get(0);
            first.set_sequence(7);
            first.set_stream_id("stream-a1");
            let mut second = cursors.get(1);
            second.set_sequence(8);
            second.set_stream_id("stream-b2");
            let mut versions = payload.init_supported_protocol_versions(1);
            versions.set(0, 2);
        })?;
        let reader = decode_frame(&codec(), &bytes)?;
        let hello = reader.get_root::<handshake_capnp::hello_envelope::Reader>()?;
        let payload = hello.get_payload();
        assert_eq!(payload.get_last_journal_sequence(), 41);
        assert_eq!(payload.get_resume_mode()?, "resume");
        let cursors = payload.get_event_cursors()?;
        assert_eq!(cursors.len(), 2);
        assert_eq!(cursors.get(0).get_stream_id()?, "stream-a1");
        assert_eq!(cursors.get(1).get_sequence(), 8);
        let versions = payload.get_supported_protocol_versions()?;
        assert_eq!(versions.len(), 1);
        assert_eq!(versions.get(0), 2);
        Ok(())
    }

    #[test]
    fn control_frame_discriminates_the_protocol_error_variant() -> Result<(), FrameError> {
        let bytes = encode_frame(codec().max_frame_bytes(), |message| {
            let mut control = message.init_root::<handshake_capnp::control_frame::Builder>();
            let mut error = control.reborrow().init_protocol_error();
            error.set_message_id("err-1");
            error.set_origin(artisan_protocol::common_capnp::Origin::Backend);
            error.set_protocol_version(2);
            error.set_schema_version(1);
            error.set_sent_at("2026-08-24T10:00:04.000Z");
            error.set_correlation_id("corr-9");
            let mut detail = error.init_payload();
            detail.set_code("version_unsupported");
            detail.set_message("no overlapping protocol version");
            detail.set_retryable(false);
        })?;
        let reader = decode_frame(&codec(), &bytes)?;
        let control = reader.get_root::<handshake_capnp::control_frame::Reader>()?;
        match control.which()? {
            handshake_capnp::control_frame::Which::ProtocolError(error) => {
                let error = error?;
                assert_eq!(error.get_correlation_id()?, "corr-9");
                let detail = error.get_payload()?;
                assert_eq!(detail.get_code()?, "version_unsupported");
                assert!(!detail.get_retryable());
            }
            _ => panic!("expected the protocol.error variant"),
        }
        Ok(())
    }

    #[test]
    fn oversized_encoding_is_rejected_not_shipped() {
        let oversized = "x".repeat(64 * 1024);
        let error = encode_frame(4096, |message| {
            let mut frame = message.init_root::<stream_frame::Builder>();
            frame.set_message_id("too-big");
            frame.set_origin(artisan_protocol::common_capnp::Origin::Frontend);
            frame.set_protocol_version(2);
            frame.set_schema_version(1);
            frame.set_sent_at("2026-08-24T10:00:05.000Z");
            frame.set_channel_id("channel-7");
            frame.set_channel_sequence(4);
            frame.set_stream_id("terminal-out");
            frame.init_chunk().set_text(&oversized);
        })
        .expect_err("encoding beyond the bound must fail");
        assert!(
            matches!(error, FrameError::TooLarge { limit: 4096, actual } if actual > 4096),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn oversized_decoding_is_rejected_before_allocation() {
        let hostile = vec![0u8; 5 * 1024 * 1024];
        let Err(error) = decode_frame(&codec(), &hostile) else {
            panic!("decoding beyond the bound must fail");
        };
        assert!(
            matches!(error, FrameError::TooLarge { .. }),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn truncated_frames_are_rejected_without_panicking() {
        let complete = sample_chunk_text("msg-trunc", "payload").expect("sample encodes");
        for cut in [1usize, 8, 16, complete.len() / 2, complete.len() - 1] {
            let result = decode_frame(&codec(), &complete[..cut]);
            assert!(
                result.is_err(),
                "a frame cut to {cut} bytes must not decode"
            );
        }
    }

    #[test]
    fn garbage_bytes_are_rejected_without_panicking() {
        // All-zero bytes form a valid empty Cap'n Proto message, so exercise
        // structurally hostile patterns instead.
        for pattern in [
            [0xFFu8, 0x00, 0x11, 0x80],
            [0xA5, 0xA5, 0x00, 0x00],
            [0x01, 0x02, 0x03, 0x04],
            [0xFF, 0xFF, 0xFF, 0xFF],
        ] {
            let hostile: Vec<u8> = pattern.repeat(128);
            assert!(
                decode_frame(&codec(), &hostile).is_err(),
                "pattern {pattern:?} must not decode"
            );
        }
    }
}
