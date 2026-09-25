//! Decoders for read-only query arms whose bodies name several identities.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Decodes one history-image read.
pub(crate) fn decode_read_message_image_request(
    query: artisan_capnp::read_message_image_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(ClientRequest::Query(Query::ReadMessageImage(
        artisan_domain::ReadMessageImage::new(
            parse_thread_id(
                read_text(query.get_thread_id(), "request.readMessageImage.threadId")?,
                "request.readMessageImage.threadId",
            )?,
            parse_message_id(
                read_text(query.get_message_id(), "request.readMessageImage.messageId")?,
                "request.readMessageImage.messageId",
            )?,
            query.get_index(),
        ),
    )))
}

/// Decodes one thread/profile catalog read.
pub(crate) fn decode_read_composer_catalog_request(
    query: artisan_capnp::read_composer_catalog_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(ClientRequest::Query(Query::ReadComposerCatalog(
        ReadComposerCatalog::new(
            parse_thread_id(
                read_text(
                    query.get_thread_id(),
                    "request.readComposerCatalog.threadId",
                )?,
                "request.readComposerCatalog.threadId",
            )?,
            parse_profile_id(
                read_text(
                    query.get_profile_id(),
                    "request.readComposerCatalog.profileId",
                )?,
                "request.readComposerCatalog.profileId",
            )?,
        ),
    )))
}
