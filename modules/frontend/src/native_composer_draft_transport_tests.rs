//! A picked image up to 32 MiB crosses the wire in chunks and reads back in
//! windows, verified against its digest.

use artisan_domain::{COMPOSER_ATTACHMENT_MAX_BYTES, ComposerImage, ImageMimeType};

use super::*;

fn picked(len: usize) -> ComposerImage {
    let bytes = (0..len).map(|index| (index % 253) as u8).collect();
    ComposerImage::new("image/png", bytes, "large.png").expect("a 32 MiB image is accepted")
}

fn upload(image: ComposerImage) -> UploadComposerAttachment {
    UploadComposerAttachment {
        request_id: RequestId::parse("native-message-upload").unwrap(),
        upload: ComposerUpload::Image(image),
    }
}

#[test]
fn a_32_mib_image_uploads_as_ordered_chunks_with_stable_request_ids() {
    let image = picked(COMPOSER_ATTACHMENT_MAX_BYTES);
    let digest = ComposerAttachmentDigest::new(Sha256::digest(image.bytes()).into());
    let requests = upload_requests(upload(image.clone())).unwrap();
    assert_eq!(requests.len(), 8);
    let mut joined = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        assert_eq!(
            request.request_id.as_str(),
            format!("native-message-upload-chunk-{index}")
        );
        let ComposerUpload::Chunk(chunk) = &request.upload else {
            panic!("a large image uploads in chunks");
        };
        assert_eq!(chunk.digest(), &digest);
        assert_eq!(chunk.offset() as usize, joined.len());
        assert!(chunk.bytes().len() <= COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES);
        joined.extend_from_slice(chunk.bytes());
    }
    assert_eq!(joined, image.bytes());
    // Planning again (a retransmission) repeats the same requests.
    assert_eq!(upload_requests(upload(image)).unwrap(), requests);
}

#[test]
fn an_image_that_fits_one_chunk_uploads_whole() {
    let command = upload(picked(1024));
    assert_eq!(upload_requests(command.clone()).unwrap(), vec![command]);
}

#[test]
fn windows_join_into_the_verified_image_and_a_wrong_window_fails() {
    let image = picked(COMPOSER_ATTACHMENT_MAX_BYTES);
    let digest = ComposerAttachmentDigest::new(Sha256::digest(image.bytes()).into());
    let total = u32::try_from(image.byte_len()).unwrap();
    let window = |offset: u32, len: u32| ComposerAttachmentResult {
        digest,
        mime_type: ImageMimeType::Png,
        bytes: image.bytes()[offset as usize..(offset + len) as usize].to_vec(),
        total_bytes: total,
        offset,
    };
    let mut readback = AttachmentReadback::new(digest);
    let complete = loop {
        let read = readback.next_read().unwrap();
        assert_eq!(read.max_bytes as usize, COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES);
        let len = read.max_bytes.min(total - read.offset);
        if let Some(complete) = readback.accept(window(read.offset, len)).unwrap() {
            break complete;
        }
    };
    assert_eq!(complete.bytes, image.bytes());
    assert_eq!(complete.total_bytes, total);

    let mut out_of_order = AttachmentReadback::new(digest);
    assert!(out_of_order.accept(window(16, 16)).is_err());
    let mut tampered = AttachmentReadback::new(digest);
    let mut whole = window(0, total);
    whole.bytes[0] ^= 1;
    assert!(tampered.accept(whole).is_err());
}
