//! Rich-link page metadata response encode and decode.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn encode_rich_link_page_metadata(
    mut encoded: artisan_capnp::rich_link_page_metadata::Builder<'_>,
    result: &RichLinkPageMetadata,
) -> Result<(), ProtocolEncodeError> {
    result.validate()?;
    encoded.set_requested_url(&result.requested_url);
    encoded.set_page_name(&result.page_name);
    encoded.set_favicon(&result.favicon);
    encoded.set_cache_expires_at_ms(result.cache_expires_at_ms);
    Ok(())
}

pub(crate) fn decode_rich_link_page_metadata(
    value: artisan_capnp::rich_link_page_metadata::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    Ok(ResponsePayload::RichLink(
        RichLinkPageMetadata::new(
            read_text(value.get_requested_url(), "response.richLink.requestedUrl")?,
            read_text(value.get_page_name(), "response.richLink.pageName")?,
            value.get_cache_expires_at_ms(),
        )?
        .with_favicon(value.get_favicon()?.to_vec())?,
    ))
}
