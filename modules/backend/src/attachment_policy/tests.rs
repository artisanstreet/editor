use artisan_domain::ComposerAttachmentDigest;

use super::*;

fn picked(mime_type: ImageMimeType, bytes: Vec<u8>) -> ComposerAttachmentResult {
    ComposerAttachmentResult {
        digest: ComposerAttachmentDigest::new([1; 32]),
        mime_type,
        bytes,
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    encode(&DynamicImage::new_rgba8(width, height), ImageMediaType::Png).expect("png")
}

fn dimensions_of(image: &ImageAttachment) -> (u32, u32) {
    let decoded = image::load_from_memory(image.bytes()).expect("fitted image decodes");
    (decoded.width(), decoded.height())
}

#[test]
fn an_oversized_picture_is_rescaled_to_the_long_edge_for_every_engine() {
    let source = picked(ImageMimeType::Png, png(2577, 3));
    for engine in ["codex", "claude", "grok", "cursor", "opencode2"] {
        let fitted =
            fit_draft_images(engine, &[(source.clone(), "wide.png".to_owned())]).expect("fits");
        assert_eq!(dimensions_of(&fitted[0]), (2576, 3), "{engine}");
    }
}

#[test]
fn the_engine_decides_the_encoding_of_a_rescaled_image() {
    let source = picked(ImageMimeType::Png, png(3000, 10));
    let codex = fit_draft_images("codex", &[(source.clone(), "shot.png".to_owned())]).unwrap();
    assert_eq!(codex[0].mime_type(), ImageMimeType::Webp);
    assert_eq!(codex[0].name(), "shot.webp");
    let grok = fit_draft_images("grok", &[(source, "shot.png".to_owned())]).unwrap();
    assert_eq!(grok[0].mime_type(), ImageMimeType::Png);
    assert_eq!(grok[0].name(), "shot.png");
}

#[test]
fn a_gif_passes_through_untouched() {
    let mut gif = Vec::new();
    DynamicImage::new_rgba8(4000, 2)
        .write_to(&mut Cursor::new(&mut gif), ImageFormat::Gif)
        .expect("gif");
    let fitted = fit_draft_images(
        "claude",
        &[(
            picked(ImageMimeType::Gif, gif.clone()),
            "loop.gif".to_owned(),
        )],
    )
    .unwrap();
    assert_eq!(fitted[0].bytes(), gif.as_slice());
    assert_eq!(fitted[0].mime_type(), ImageMimeType::Gif);
}

#[test]
fn images_the_engine_cannot_take_are_refused_with_a_reason() {
    let undecodable = fit_draft_images(
        "codex",
        &[(
            picked(ImageMimeType::Png, b"not an image".to_vec()),
            "broken.png".to_owned(),
        )],
    )
    .unwrap_err();
    assert_eq!(undecodable, "broken.png: That image could not be decoded.");

    // An untouched GIF over the message bound cannot be sent.
    let huge_gif = picked(
        ImageMimeType::Gif,
        vec![0; MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES + 1],
    );
    assert_eq!(
        fit_draft_images("codex", &[(huge_gif, "huge.gif".to_owned())]).unwrap_err(),
        "huge.gif: That image exceeds the 5 MiB limit."
    );

    // Together the fitted images must fit one message.
    let four_mib = picked(ImageMimeType::Gif, vec![0; 4 * 1024 * 1024 + 512 * 1024]);
    let three = vec![
        (four_mib.clone(), "a.gif".to_owned()),
        (four_mib.clone(), "b.gif".to_owned()),
        (four_mib, "c.gif".to_owned()),
    ];
    assert_eq!(
        fit_draft_images("codex", &three).unwrap_err(),
        "Attached images together cannot exceed 12 MiB."
    );
}
