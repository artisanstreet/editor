//! Owned protocol coverage for the media-file read and its bytes.

use std::error::Error;

use artisan_domain::{
    MEDIA_FILE_MAX_BYTES, MediaFilePath, MediaPathAnchor, Query, ReadMediaFile, RequestId,
    ThreadId, UnixMillis,
};
use artisan_protocol::{
    ClientRequest, FrameId, MediaFileResult, ProtocolVersion, ResponsePayload, ServerResponse,
    WireEnvelope, WireEnvelopeBody, decode_envelope, encode_envelope,
};

fn envelope(body: WireEnvelopeBody) -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("media-file")?,
        sent_at: UnixMillis::from_millis(11),
        body,
    })
}

#[test]
fn media_file_reads_and_bytes_round_trip() -> Result<(), Box<dyn Error>> {
    for (text, anchor) in [
        ("docs/shot.png", MediaPathAnchor::Project),
        ("/home/sander/a b.webp", MediaPathAnchor::Project),
        ("Downloads/clip.mp3", MediaPathAnchor::Home),
        ("", MediaPathAnchor::Home),
    ] {
        let request = envelope(WireEnvelopeBody::Request(ClientRequest::Query(
            Query::ReadMediaFile(ReadMediaFile::new(
                ThreadId::parse("thread-a")?,
                MediaFilePath::new(text.to_owned(), anchor)?,
            )),
        )))?;
        assert!(decode_envelope(&encode_envelope(&request)?)? == request);
    }
    let response = envelope(WireEnvelopeBody::Response(ServerResponse {
        request_id: RequestId::parse("media-file")?,
        payload: ResponsePayload::MediaFile(MediaFileResult {
            bytes: vec![0x89, b'P', b'N', b'G', 0, 1, 2],
        }),
    }))?;
    assert!(decode_envelope(&encode_envelope(&response)?)? == response);
    Ok(())
}

#[test]
fn media_file_bytes_past_the_bound_are_refused() -> Result<(), Box<dyn Error>> {
    let response = envelope(WireEnvelopeBody::Response(ServerResponse {
        request_id: RequestId::parse("media-file")?,
        payload: ResponsePayload::MediaFile(MediaFileResult {
            bytes: vec![0; MEDIA_FILE_MAX_BYTES + 1],
        }),
    }))?;
    assert!(decode_envelope(&encode_envelope(&response)?).is_err());
    Ok(())
}
