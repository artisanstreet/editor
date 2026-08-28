//! Private v1 framing tests for the internal directory helper codec:
//! roundtrips, every rejection bound, and typed response encoding.
//!
//! This contract-named file is compiled as a private `cfg(test)` module of
//! the backend crate through the supplied backend-crate `rust_test` wiring,
//! so it reaches `pub(crate)` codec items via `crate::` paths. All reads run
//! against deterministic in-memory readers; real child-process stream and
//! lifeline behavior is covered separately by the fixture module.

use std::io::{self, Cursor, Read};

use artisan_domain::ROOT_PATH_MAX_BYTES;

use crate::directory_helper_codec::{
    HEADER_LEN, HeaderFault, HelperRequest, PROTOCOL_VERSION, REQUEST_MAGIC, REQUEST_TAG_PICK,
    REQUEST_TAG_VALIDATE, RESPONSE_MAGIC, RequestKind, RequestPrelude, RequestReadFault, Response,
    ResponseEncodeFault, encode_response, parse_request_header, read_request,
};

/// Reader that records how many `read` calls it served.
struct CountingReader {
    data: Cursor<Vec<u8>>,
    reads: usize,
}

impl CountingReader {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data: Cursor::new(data),
            reads: 0,
        }
    }

    fn consumed(&self) -> u64 {
        self.data.position()
    }
}

impl Read for CountingReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reads += 1;
        self.data.read(buf)
    }
}

/// Deterministic reader serving a fixed prefix, then a non-EOF failure.
struct FailingAfterPrefix {
    data: Vec<u8>,
    served: usize,
    calls: usize,
}

impl FailingAfterPrefix {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            served: 0,
            calls: 0,
        }
    }
}

impl Read for FailingAfterPrefix {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.served < self.data.len() {
            let count = buf.len().min(self.data.len() - self.served);
            buf[..count].copy_from_slice(&self.data[self.served..self.served + count]);
            self.served += count;
            return Ok(count);
        }
        Err(io::Error::other("deterministic non-eof read failure"))
    }
}

/// Reader failing once with `Interrupted`, then serving its bytes faithfully.
struct InterruptedOnceThenData {
    data: Vec<u8>,
    served: usize,
    interrupted: bool,
}

impl InterruptedOnceThenData {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            served: 0,
            interrupted: false,
        }
    }
}

impl Read for InterruptedOnceThenData {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        if self.served < self.data.len() {
            let count = buf.len().min(self.data.len() - self.served);
            buf[..count].copy_from_slice(&self.data[self.served..self.served + count]);
            self.served += count;
            return Ok(count);
        }
        Ok(0)
    }
}

/// Builds one exact request header from raw fields.
fn request_header(tag: u8, generation: u64, payload_len: u32) -> [u8; HEADER_LEN] {
    let mut header = [0_u8; HEADER_LEN];
    header[..4].copy_from_slice(&REQUEST_MAGIC);
    header[4] = PROTOCOL_VERSION;
    header[5] = tag;
    header[6..14].copy_from_slice(&generation.to_le_bytes());
    header[14..18].copy_from_slice(&payload_len.to_le_bytes());
    header
}

#[test]
fn response_tags_match_the_v1_wire_contract() {
    assert_eq!(
        Response::Selected {
            canonical_path: "x".to_owned()
        }
        .tag(),
        1
    );
    assert_eq!(Response::Cancelled.tag(), 2);
    assert_eq!(Response::InvalidPath.tag(), 3);
    assert_eq!(Response::UnsupportedEncoding.tag(), 4);
    assert_eq!(Response::UnsupportedPlatform.tag(), 5);
    assert_eq!(Response::DialogFailed.tag(), 6);
}

#[test]
fn pick_request_roundtrips_through_exact_reads() {
    let generation = 0x0102_0304_0506_0708_u64;
    let frame = request_header(REQUEST_TAG_PICK, generation, 0);
    let mut reader = Cursor::new(frame.to_vec());

    let decoded = read_request(&mut reader).expect("well-formed pick frame should decode");
    assert_eq!(decoded, (generation, HelperRequest::Pick));
    // Exactly the frame was consumed; no EOF-delimiting or over-read happened.
    assert_eq!(reader.position(), u64::try_from(HEADER_LEN).expect("small"));
}

#[test]
fn validate_request_roundtrips_path_text_exactly() {
    let generation = 7_u64;
    let path_text = r"C:\temp dir\héllo 🦈 project";
    let mut frame = request_header(
        REQUEST_TAG_VALIDATE,
        generation,
        u32::try_from(path_text.len()).expect("small"),
    )
    .to_vec();
    frame.extend_from_slice(path_text.as_bytes());
    let mut reader = Cursor::new(frame);

    let decoded = read_request(&mut reader).expect("well-formed validate frame should decode");
    assert_eq!(
        decoded,
        (
            generation,
            HelperRequest::Validate {
                path_text: path_text.to_owned(),
            }
        )
    );
}

#[test]
fn maximum_payload_length_is_accepted_inclusively() {
    let generation = 9_u64;
    let payload = vec![b'a'; ROOT_PATH_MAX_BYTES];
    let declared = u32::try_from(ROOT_PATH_MAX_BYTES).expect("shared bound fits u32");
    let mut frame = request_header(REQUEST_TAG_VALIDATE, generation, declared).to_vec();
    frame.extend_from_slice(&payload);

    let decoded =
        read_request(&mut Cursor::new(frame)).expect("payload exactly at the bound must decode");
    assert!(matches!(decoded.1, HelperRequest::Validate { .. }));
}

#[test]
fn response_frames_encode_the_documented_layout() {
    let generation = 0x0102_0304_0506_0708_u64;

    let cancelled = encode_response(generation, &Response::Cancelled)
        .expect("empty-payload responses always encode");
    let mut expected = RESPONSE_MAGIC.to_vec();
    expected.push(PROTOCOL_VERSION);
    expected.push(2);
    expected.extend_from_slice(&generation.to_le_bytes());
    expected.extend_from_slice(&0_u32.to_le_bytes());
    assert_eq!(cancelled, expected);

    let canonical_path = r"\\?\C:\some dir";
    let selected = encode_response(
        generation,
        &Response::Selected {
            canonical_path: canonical_path.to_owned(),
        },
    )
    .expect("bounded selected responses encode");
    assert_eq!(&selected[..4], &RESPONSE_MAGIC);
    assert_eq!(selected[4], PROTOCOL_VERSION);
    assert_eq!(selected[5], 1);
    assert_eq!(&selected[6..14], &generation.to_le_bytes());
    assert_eq!(
        &selected[14..18],
        &u32::try_from(canonical_path.len())
            .expect("small")
            .to_le_bytes()
    );
    assert_eq!(&selected[18..], canonical_path.as_bytes());
    assert_eq!(selected.len(), HEADER_LEN + canonical_path.len());
}

#[test]
fn header_faults_are_classified_individually() {
    let complete = request_header(REQUEST_TAG_PICK, 1, 0);

    assert_eq!(parse_request_header(&[]), Err(HeaderFault::Truncated));
    assert_eq!(
        parse_request_header(&complete[..17]),
        Err(HeaderFault::Truncated)
    );
    assert_eq!(
        parse_request_header(complete.as_slice()),
        Ok(RequestPrelude {
            generation: 1,
            payload_len: 0,
            kind: RequestKind::Pick,
        })
    );
    // Longer than one header is also not a header.
    let mut overlong = complete.to_vec();
    overlong.push(0);
    assert_eq!(parse_request_header(&overlong), Err(HeaderFault::Truncated));

    let mut foreign = complete;
    foreign[0] = b'X';
    assert_eq!(
        parse_request_header(&foreign),
        Err(HeaderFault::ForeignMagic)
    );

    let mut version_zero = complete;
    version_zero[4] = 0;
    assert_eq!(
        parse_request_header(&version_zero),
        Err(HeaderFault::UnsupportedVersion { found: 0 })
    );

    let mut version_two = complete;
    version_two[4] = 2;
    assert_eq!(
        parse_request_header(&version_two),
        Err(HeaderFault::UnsupportedVersion { found: 2 })
    );

    let mut unknown_tag = complete;
    unknown_tag[5] = 3;
    assert_eq!(
        parse_request_header(&unknown_tag),
        Err(HeaderFault::UnsupportedTag { found: 3 })
    );

    assert_eq!(
        parse_request_header(&request_header(REQUEST_TAG_PICK, 1, 1)),
        Err(HeaderFault::PickCarriesPayload)
    );
    assert_eq!(
        parse_request_header(&request_header(REQUEST_TAG_VALIDATE, 1, 0)),
        Err(HeaderFault::EmptyValidatePayload)
    );
    assert_eq!(
        parse_request_header(&request_header(
            REQUEST_TAG_VALIDATE,
            1,
            u32::try_from(ROOT_PATH_MAX_BYTES + 1).expect("small")
        )),
        Err(HeaderFault::PayloadBeyondBound {
            declared: u32::try_from(ROOT_PATH_MAX_BYTES + 1).expect("small")
        })
    );
    assert_eq!(
        parse_request_header(&request_header(REQUEST_TAG_VALIDATE, 1, u32::MAX)),
        Err(HeaderFault::PayloadBeyondBound { declared: u32::MAX })
    );
}

#[test]
fn oversized_declared_payload_is_rejected_before_any_further_read() {
    // The header alone declares a beyond-bound payload; no payload bytes even
    // exist behind it in the stream.
    let declared = u32::try_from(ROOT_PATH_MAX_BYTES + 1).expect("small");
    let frame = request_header(REQUEST_TAG_VALIDATE, 5, declared);
    let mut reader = CountingReader::new(frame.to_vec());

    let outcome = read_request(&mut reader);
    assert_eq!(outcome, Err(RequestReadFault::Malformed));
    // Only the header read was attempted: nothing was allocated or consumed
    // for the impossible payload.
    assert_eq!(reader.reads, 1);
    assert_eq!(reader.consumed(), u64::try_from(HEADER_LEN).expect("small"));
}

#[test]
fn trailing_input_after_one_frame_is_never_consumed() {
    let mut stream = request_header(REQUEST_TAG_PICK, 3, 0).to_vec();
    stream.extend_from_slice(b"trailing!");
    let position_after_frame = stream.len() - b"trailing!".len();
    let mut reader = Cursor::new(stream);

    let decoded = read_request(&mut reader).expect("frame must decode without waiting on EOF");
    assert_eq!(decoded, (3, HelperRequest::Pick));
    assert_eq!(
        reader.position(),
        u64::try_from(position_after_frame).expect("small")
    );
}

#[test]
fn stream_truncation_mid_payload_is_malformed_not_encoding() {
    // The header promises eight payload bytes; only three actually follow.
    let mut frame = request_header(REQUEST_TAG_VALIDATE, 11, 8).to_vec();
    frame.extend_from_slice(b"abc");

    let outcome = read_request(&mut Cursor::new(frame));
    assert_eq!(outcome, Err(RequestReadFault::Malformed));
}

#[test]
fn non_utf8_validate_payload_keeps_generation_for_typed_reply() {
    let generation = 0xDEAD_BEEF_u64;
    let mut frame = request_header(REQUEST_TAG_VALIDATE, generation, 4).to_vec();
    frame.extend_from_slice(&[0xFF, 0xFE, 0xFF, 0xFE]);

    let outcome = read_request(&mut Cursor::new(frame));
    assert_eq!(
        outcome,
        Err(RequestReadFault::PayloadNotUtf8 { generation })
    );
}

#[test]
fn encoding_refuses_selected_payloads_beyond_the_shared_bound() {
    let beyond_bound = "x".repeat(ROOT_PATH_MAX_BYTES + 1);
    let response = Response::Selected {
        canonical_path: beyond_bound,
    };

    assert_eq!(
        encode_response(42, &response),
        Err(ResponseEncodeFault::SelectedBeyondBound {
            length: ROOT_PATH_MAX_BYTES + 1
        })
    );
}

#[test]
fn encoding_accepts_selected_payloads_exactly_at_the_bound() {
    let at_bound = "y".repeat(ROOT_PATH_MAX_BYTES);
    let response = Response::Selected {
        canonical_path: at_bound.clone(),
    };

    let frame = encode_response(43, &response).expect("payload exactly at the bound encodes");
    assert_eq!(frame.len(), HEADER_LEN + ROOT_PATH_MAX_BYTES);
    assert_eq!(&frame[HEADER_LEN..], at_bound.as_bytes());
}

#[test]
fn empty_and_every_truncated_pick_prefix_is_malformed_through_reads() {
    // The completely empty stream fails before any header byte exists.
    let outcome = read_request(&mut Cursor::new(Vec::new()));
    assert_eq!(outcome, Err(RequestReadFault::Malformed));

    // Every truncated header prefix, zero bytes through seventeen.
    let complete = request_header(REQUEST_TAG_PICK, 77, 0);
    for length in 0..HEADER_LEN {
        let truncated = complete[..length].to_vec();
        let outcome = read_request(&mut Cursor::new(truncated));
        assert_eq!(
            outcome,
            Err(RequestReadFault::Malformed),
            "prefix length {length} must stay malformed"
        );
    }
}

#[test]
fn validate_declared_payload_without_supplied_bytes_is_malformed() {
    let declared = 6_u32;
    let frame = request_header(REQUEST_TAG_VALIDATE, 12, declared).to_vec();

    // A positive declared payload with zero supplied bytes cannot complete.
    let outcome = read_request(&mut Cursor::new(frame.clone()));
    assert_eq!(outcome, Err(RequestReadFault::Malformed));

    // Short supplied prefixes are equally incomplete and malformed.
    let declared_len = usize::try_from(declared).expect("small");
    for supplied in 0..declared_len {
        let mut partial = frame.clone();
        partial.truncate(HEADER_LEN + supplied);
        let outcome = read_request(&mut Cursor::new(partial));
        assert_eq!(
            outcome,
            Err(RequestReadFault::Malformed),
            "supplied prefix {supplied} must stay malformed"
        );
    }
}

#[test]
fn non_eof_read_errors_at_frame_boundaries_stay_malformed() {
    // Header start: the very first read fails; no generation is trusted.
    let mut reader = FailingAfterPrefix::new(Vec::new());
    assert_eq!(read_request(&mut reader), Err(RequestReadFault::Malformed));
    assert_eq!(reader.calls, 1);

    // Mid-header: nine faithful header bytes arrive, then failure.
    let full_header = request_header(REQUEST_TAG_VALIDATE, 21, 6);
    let mut mid_header = FailingAfterPrefix::new(full_header[..9].to_vec());
    assert_eq!(
        read_request(&mut mid_header),
        Err(RequestReadFault::Malformed)
    );

    // Payload start: a fully sound header, then failure on payload reads.
    let mut payload_start = FailingAfterPrefix::new(full_header.to_vec());
    assert_eq!(
        read_request(&mut payload_start),
        Err(RequestReadFault::Malformed)
    );

    // Mid-payload: three of six promised bytes arrive, then failure. This is
    // never a typed encoding reply and echoes no correlation generation.
    let mut partial = full_header.to_vec();
    partial.extend_from_slice(b"abc");
    let mut mid_payload = FailingAfterPrefix::new(partial);
    assert_eq!(
        read_request(&mut mid_payload),
        Err(RequestReadFault::Malformed)
    );
}

#[test]
fn interrupted_once_reader_retries_through_read_exact() {
    let frame = request_header(REQUEST_TAG_PICK, 34, 0).to_vec();
    let mut reader = InterruptedOnceThenData::new(frame);

    let decoded = read_request(&mut reader);
    assert_eq!(decoded, Ok((34, HelperRequest::Pick)));
    assert!(reader.interrupted, "the transient interruption was served");
}
