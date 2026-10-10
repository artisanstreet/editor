//! The media-file read: one image or audio file a reply names, read from the
//! Forge's machine, and the bytes that answer it.

#[allow(clippy::wildcard_imports)]
use super::*;

use artisan_domain::{MEDIA_FILE_MAX_BYTES, MediaFilePath, MediaPathAnchor, ReadMediaFile};

use crate::artisan_capnp::{media_file_result, read_media_file_request};

/// Encodes one media-file read.
pub(crate) fn encode_read_media_file(builder: request::Builder<'_>, query: &ReadMediaFile) {
    let mut encoded = builder.init_read_media_file();
    encoded.set_thread_id(query.thread_id().as_str());
    encoded.set_path(query.path().text());
    encoded.set_anchor(match query.path().anchor() {
        MediaPathAnchor::Project => artisan_capnp::MediaPathAnchor::Project,
        MediaPathAnchor::Home => artisan_capnp::MediaPathAnchor::Home,
    });
}

/// Decodes one media-file read.
pub(crate) fn decode_read_media_file(
    query: read_media_file_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(query.get_thread_id(), "request.readMediaFile.threadId")?,
        "request.readMediaFile.threadId",
    )?;
    let anchor = match query.get_anchor()? {
        artisan_capnp::MediaPathAnchor::Project => MediaPathAnchor::Project,
        artisan_capnp::MediaPathAnchor::Home => MediaPathAnchor::Home,
    };
    let text = read_text(query.get_path(), "request.readMediaFile.path")?;
    let path =
        MediaFilePath::new(text, anchor).map_err(|source| ProtocolDecodeError::MediaFilePath {
            field: "request.readMediaFile.path",
            source,
        })?;
    Ok(ClientRequest::Query(Query::ReadMediaFile(
        ReadMediaFile::new(thread_id, path),
    )))
}

/// Encodes the bytes of one media file.
pub(crate) fn encode_media_file(builder: response::Builder<'_>, result: &MediaFileResult) {
    builder.init_media_file().set_bytes(&result.bytes);
}

/// Decodes the bytes of one media file, refusing more than one file may hold.
pub(crate) fn decode_media_file(
    result: media_file_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let bytes = result.get_bytes()?;
    if bytes.len() > MEDIA_FILE_MAX_BYTES {
        return Err(ProtocolDecodeError::MediaFileTooLarge {
            field: "response.mediaFile.bytes",
        });
    }
    Ok(ResponsePayload::MediaFile(MediaFileResult {
        bytes: bytes.to_vec(),
    }))
}
