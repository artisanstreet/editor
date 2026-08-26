//! Private v1 binary framing between Forge and its internal directory helper.
//!
//! The helper is one explicit mode of the existing Forge executable
//! (`--internal-directory-helper-v1`); it exchanges exactly one request and
//! one response with its parent over inherited standard streams. This module
//! owns that framing so neither side invents a second encoding: an
//! eighteen-byte header followed by at most one bounded payload.
//!
//! # Frame layout
//!
//! ```text
//! offset  size  field
//! 0       4     ASCII magic: `ASDP` request, `ASDR` response
//! 4       1     protocol version (currently 1)
//! 5       1     tag (request or response, per direction)
//! 6       8     operation generation, unsigned little-endian
//! 14      4     payload byte length, unsigned little-endian
//! ```
//!
//! The generation is a private correlation value only: it is echoed exactly,
//! is not an authorization secret, and its non-reuse discipline belongs to
//! the parent controller, not to this codec.
//!
//! Every read here is exact-length and every payload length is checked
//! against [`ROOT_PATH_MAX_BYTES`] *before* any allocation or further read,
//! so bytes and memory stay bounded no matter what the stream declares.
//! These caps bound data, not time: a silent or stalled stream can block a
//! read for an unbounded duration, and limiting that waiting is the parent
//! controller's deadline responsibility, never a property this codec claims.
//!
//! Framing faults are distinct from typed operation outcomes: a malformed
//! frame must end the helper without any response, while a well-framed
//! request whose contents fail validation may still produce a typed failure
//! response.

use std::io::Read;

use artisan_domain::ROOT_PATH_MAX_BYTES;
use thiserror::Error;

/// Fixed header size in bytes for both directions.
pub(crate) const HEADER_LEN: usize = 18;

/// ASCII magic opening every parent-to-helper request frame.
pub(crate) const REQUEST_MAGIC: [u8; 4] = *b"ASDP";

/// ASCII magic opening every helper-to-parent response frame.
pub(crate) const RESPONSE_MAGIC: [u8; 4] = *b"ASDR";

/// The single negotiated protocol version.
pub(crate) const PROTOCOL_VERSION: u8 = 1;

/// Wire tag of the `Pick` request: show one native directory chooser.
pub(crate) const REQUEST_TAG_PICK: u8 = 1;

/// Wire tag of the `Validate` request: validate one supplied path payload.
pub(crate) const REQUEST_TAG_VALIDATE: u8 = 2;

/// Which operation the parent requested, decoded from the wire tag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestKind {
    /// Show one blocking native chooser and return the selection.
    Pick,
    /// Validate the bounded UTF-8 path carried in the payload.
    Validate,
}

/// Decoded header fields available before any payload byte is touched.
///
/// `payload_len` is already proven to fit the shared root-path ceiling, so a
/// caller may allocate for it safely.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RequestPrelude {
    /// Private correlation value to echo verbatim in the response.
    pub(crate) generation: u64,
    /// Payload length in bytes, guaranteed at most [`ROOT_PATH_MAX_BYTES`].
    pub(crate) payload_len: usize,
    /// Operation selected by the wire tag.
    pub(crate) kind: RequestKind,
}

/// One decoded helper command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HelperRequest {
    /// Show one top-level native directory chooser with no owner, default
    /// location, or filename seed.
    Pick,
    /// Validate the parent-supplied absolute filesystem path.
    Validate {
        /// The exact UTF-8 text received; never trimmed or case-folded.
        path_text: String,
    },
}

/// Why a request frame was rejected before it could name an operation.
///
/// Every variant is a framing fault: none carries filesystem text, and any
/// of them obligates the helper to exit nonzero without writing a response.
/// No generation is reported because a rejected header cannot be trusted as
/// a correlation value.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum HeaderFault {
    /// The input was not exactly [`HEADER_LEN`]: too short, or longer than
    /// one header.
    #[error("request header is truncated")]
    Truncated,
    /// The magic bytes were not exactly `ASDP`.
    #[error("request frame does not carry the helper request magic")]
    ForeignMagic,
    /// The version byte named a protocol this build never spoke.
    #[error("request names unsupported protocol version {found}")]
    UnsupportedVersion {
        /// The offending version byte.
        found: u8,
    },
    /// The tag byte is outside the two defined request tags.
    #[error("request names unknown tag {found}")]
    UnsupportedTag {
        /// The offending tag byte.
        found: u8,
    },
    /// The declared payload exceeds [`ROOT_PATH_MAX_BYTES`].
    #[error("request declares {declared} payload bytes beyond the shared bound")]
    PayloadBeyondBound {
        /// The declared payload length in bytes.
        declared: u32,
    },
    /// A `Pick` request declared a payload; `Pick` is always empty.
    #[error("pick request must not carry a payload")]
    PickCarriesPayload,
    /// A `Validate` request declared no payload; its path cannot be empty.
    #[error("validate request must carry a non-empty path payload")]
    EmptyValidatePayload,
}

/// Why reading one complete request failed after the header was accepted.
///
/// `Malformed` covers stream truncation mid-frame and every
/// [`HeaderFault`]: the helper must exit nonzero without a response and must
/// not echo a possibly-bad generation. `PayloadNotUtf8` keeps the validated
/// generation so the helper can answer with the typed
/// [`Response::UnsupportedEncoding`] outcome instead of dying silently; it is
/// a typed operation outcome, not fabricated framing success.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestReadFault {
    /// The frame could not be trusted; emit nothing and exit nonzero.
    Malformed,
    /// The header was valid but the payload was not lossless UTF-8.
    PayloadNotUtf8 {
        /// The validated generation from the accepted header.
        generation: u64,
    },
}

/// Why one response frame could not be encoded at all.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum ResponseEncodeFault {
    /// A caller-constructed [`Response::Selected`] carried a canonical path
    /// beyond [`ROOT_PATH_MAX_BYTES`]. Production construction routes through
    /// validated path resolution, so reaching this means an invariant broke;
    /// the frame is refused instead of emitted out of contract.
    #[error("selected canonical path exceeds the shared root-path byte bound")]
    SelectedBeyondBound {
        /// The offending payload length in UTF-8 bytes.
        length: usize,
    },
}

/// Typed helper outcome carried back to the parent.
///
/// Tags 2 through 6 deliberately have empty payloads; only `Selected`
/// carries bytes, and those are the canonical directory text produced by the
/// helper's own path policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Response {
    /// The chosen or supplied path resolved to this canonical directory.
    Selected {
        /// Canonical directory text within [`ROOT_PATH_MAX_BYTES`].
        canonical_path: String,
    },
    /// The user dismissed the chooser without selecting.
    Cancelled,
    /// The candidate path cannot serve as a project root.
    InvalidPath,
    /// A required string could not cross the boundary losslessly as UTF-8.
    UnsupportedEncoding,
    /// This platform offers no supported chooser implementation.
    UnsupportedPlatform,
    /// The native dialog itself failed before producing a decision.
    DialogFailed,
}

impl Response {
    /// The wire tag for this outcome.
    #[must_use]
    pub(crate) const fn tag(&self) -> u8 {
        match self {
            Self::Selected { .. } => 1,
            Self::Cancelled => 2,
            Self::InvalidPath => 3,
            Self::UnsupportedEncoding => 4,
            Self::UnsupportedPlatform => 5,
            Self::DialogFailed => 6,
        }
    }
}

/// Parses one complete request header without touching any payload byte.
///
/// The input must be exactly the leading [`HEADER_LEN`] bytes of the frame;
/// callers read them with an exact-length read so a short stream surfaces as
/// I/O failure rather than a fabricated verdict. All structural rejection
/// happens here, including the payload-ceiling check, so the caller learns a
/// safe maximum payload size before it allocates anything.
///
/// # Errors
///
/// Returns [`HeaderFault`] for input that is not exactly one header, foreign
/// magic, unsupported versions or tags, payloads beyond
/// [`ROOT_PATH_MAX_BYTES`], and the two impossible tag/payload combinations
/// (`Pick` with bytes, `Validate` empty).
pub(crate) fn parse_request_header(header: &[u8]) -> Result<RequestPrelude, HeaderFault> {
    let (&magic, rest) = header
        .split_first_chunk::<4>()
        .ok_or(HeaderFault::Truncated)?;
    if magic != REQUEST_MAGIC {
        return Err(HeaderFault::ForeignMagic);
    }

    let (&[version], rest) = rest
        .split_first_chunk::<1>()
        .ok_or(HeaderFault::Truncated)?;
    if version != PROTOCOL_VERSION {
        return Err(HeaderFault::UnsupportedVersion { found: version });
    }

    let (&[tag], rest) = rest
        .split_first_chunk::<1>()
        .ok_or(HeaderFault::Truncated)?;
    let kind = match tag {
        REQUEST_TAG_PICK => RequestKind::Pick,
        REQUEST_TAG_VALIDATE => RequestKind::Validate,
        found => return Err(HeaderFault::UnsupportedTag { found }),
    };

    let (&generation_bytes, rest) = rest
        .split_first_chunk::<8>()
        .ok_or(HeaderFault::Truncated)?;
    let (&length_bytes, tail) = rest
        .split_first_chunk::<4>()
        .ok_or(HeaderFault::Truncated)?;
    // The defined header ends exactly here; anything longer was not a
    // header. Payload bytes and trailing input belong to the caller's exact
    // reads, never to this function.
    if !tail.is_empty() {
        return Err(HeaderFault::Truncated);
    }

    let generation = u64::from_le_bytes(generation_bytes);
    let declared = u32::from_le_bytes(length_bytes);
    let Ok(payload_len) = usize::try_from(declared) else {
        return Err(HeaderFault::PayloadBeyondBound { declared });
    };
    if payload_len > ROOT_PATH_MAX_BYTES {
        return Err(HeaderFault::PayloadBeyondBound { declared });
    }

    match kind {
        RequestKind::Pick if declared != 0 => Err(HeaderFault::PickCarriesPayload),
        RequestKind::Validate if declared == 0 => Err(HeaderFault::EmptyValidatePayload),
        RequestKind::Pick | RequestKind::Validate => Ok(RequestPrelude {
            generation,
            payload_len,
            kind,
        }),
    }
}

/// Reads exactly one complete request from an arbitrary byte source.
///
/// Reading is strictly two-phase and exact-length: [`HEADER_LEN`] header
/// bytes first, then—only after [`parse_request_header`] proves the declared
/// payload legal—the declared number of payload bytes. The function never
/// waits for end-of-stream to delimit the request and never consumes a byte
/// beyond the frame, so trailing input stays in the stream for the lifeline
/// watcher to observe. Each exact read completes whenever the operating
/// system delivers the bytes or an error; how long that takes is unbounded
/// here and belongs to the parent's future deadline ownership.
///
/// # Errors
///
/// Returns [`RequestReadFault::Malformed`] when the stream ends or errors
/// mid-frame or the header fails parsing; nothing may be answered then.
/// Returns [`RequestReadFault::PayloadNotUtf8`] when the frame was sound but
/// a `Validate` payload is not lossless UTF-8; the validated generation
/// travels with the fault so a typed [`Response::UnsupportedEncoding`] reply
/// remains possible.
pub(crate) fn read_request<R: Read>(
    reader: &mut R,
) -> Result<(u64, HelperRequest), RequestReadFault> {
    let mut header = [0_u8; HEADER_LEN];
    reader
        .read_exact(&mut header)
        .map_err(|_| RequestReadFault::Malformed)?;
    let prelude = parse_request_header(&header).map_err(|_| RequestReadFault::Malformed)?;

    let mut payload = vec![0_u8; prelude.payload_len];
    reader
        .read_exact(&mut payload)
        .map_err(|_| RequestReadFault::Malformed)?;

    match prelude.kind {
        RequestKind::Pick => Ok((prelude.generation, HelperRequest::Pick)),
        RequestKind::Validate => match std::str::from_utf8(&payload) {
            Ok(path_text) => Ok((
                prelude.generation,
                HelperRequest::Validate {
                    path_text: path_text.to_owned(),
                },
            )),
            Err(_) => Err(RequestReadFault::PayloadNotUtf8 {
                generation: prelude.generation,
            }),
        },
    }
}

/// Encodes one response frame around the exact correlation generation.
///
/// The returned buffer is the full frame: [`RESPONSE_MAGIC`], the current
/// [`PROTOCOL_VERSION`], the outcome tag, the echoed little-endian
/// generation, the little-endian payload length, then the payload bytes for
/// [`Response::Selected`] (empty for every other tag).
///
/// # Errors
///
/// Returns [`ResponseEncodeFault::SelectedBeyondBound`] instead of emitting
/// any frame when a constructed [`Response::Selected`] carries a canonical
/// path beyond [`ROOT_PATH_MAX_BYTES`]; out-of-contract responses are
/// refused, never truncated or panicked into existence.
pub(crate) fn encode_response(
    generation: u64,
    response: &Response,
) -> Result<Vec<u8>, ResponseEncodeFault> {
    let payload: &[u8] = match response {
        Response::Selected { canonical_path } => {
            let length = canonical_path.len();
            if length > ROOT_PATH_MAX_BYTES {
                return Err(ResponseEncodeFault::SelectedBeyondBound { length });
            }
            canonical_path.as_bytes()
        }
        Response::Cancelled
        | Response::InvalidPath
        | Response::UnsupportedEncoding
        | Response::UnsupportedPlatform
        | Response::DialogFailed => &[],
    };

    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&RESPONSE_MAGIC);
    frame.push(PROTOCOL_VERSION);
    frame.push(response.tag());
    frame.extend_from_slice(&generation.to_le_bytes());
    let Ok(payload_len) = u32::try_from(payload.len()) else {
        return Err(ResponseEncodeFault::SelectedBeyondBound {
            length: payload.len(),
        });
    };
    frame.extend_from_slice(&payload_len.to_le_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}
