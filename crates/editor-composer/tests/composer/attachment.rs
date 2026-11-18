use super::{Attachments, Source, State};
use artisan_api::tls::Identity;
use editor_core::ForgeClient;
use std::time::{Duration, Instant};

fn noisy_png(edge: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(edge, edge, |x, y| {
        let mix = x
            .wrapping_mul(2_654_435_761)
            .wrapping_add(y.wrapping_mul(40_503));
        let [r, g, b, _] = mix.to_le_bytes();
        image::Rgba([r, g, b, 255])
    });
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("the test image encodes");
    png
}

#[test]
fn a_cancelled_preview_fails_the_attachment_instead_of_encoding_forever() {
    let identity = Identity::generate("attachment-test").expect("identity");
    let client = ForgeClient::new("127.0.0.1:1", &"0".repeat(64), &identity).expect("client");
    let mut attachments = Attachments::new(client);
    assert!(attachments.add(Source::Bytes(noisy_png(1024)), "pasted.png".into()));
    let assets = aqua::images::ImageAssets::default();
    let deadline = Instant::now() + Duration::from_secs(60);
    while attachments.items[0].pending_preview.is_none() {
        attachments.drain(&assets);
        assert!(
            matches!(attachments.items[0].state, State::Encoding),
            "the preview is still decoding when its assets go away"
        );
        assert!(Instant::now() < deadline, "the image converts in time");
        std::thread::yield_now();
    }
    drop(assets);
    attachments.drain(&aqua::images::ImageAssets::default());
    let item = &attachments.items[0];
    assert!(
        matches!(&item.state, State::Failed(reason) if reason == "The preview was cancelled."),
        "a cancelled preview releases the draft for sending"
    );
    assert!(item.encoded.is_none() && item.pending_preview.is_none());
}
