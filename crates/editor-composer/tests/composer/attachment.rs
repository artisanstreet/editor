use super::{Attachments, Source, State};
use artisan_api::{BlobHash, BlobRef, tls::Identity};
use axum::http::StatusCode;
use editor_core::ForgeClient;
use std::time::{Duration, Instant};
use test_support::editor::forge::{HeldRequest, StubForge};

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

fn stub_forge() -> (StubForge, Attachments) {
    let forge = editor_core::runtime::runtime().block_on(StubForge::serve(
        test_support::editor::forge::empty_state(),
        axum::Router::new(),
    ));
    let identity = Identity::generate("attachment-test").expect("identity");
    let client = ForgeClient::new(forge.address(), forge.fingerprint(), &identity).expect("client");
    (forge, Attachments::new(client))
}

fn upload_request(
    forge: &StubForge,
    attachments: &mut Attachments,
    assets: &aqua::images::ImageAssets,
) -> Option<HeldRequest> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        attachments.drain(assets);
        if let Some(request) = forge.try_take("PUT", "/blobs") {
            return Some(request);
        }
        if attachments.shake.is_some()
            || attachments
                .items
                .iter()
                .all(|item| matches!(item.state, State::Ready(_) | State::Failed(_)))
        {
            return None;
        }
        assert!(Instant::now() < deadline, "the image reaches Forge in time");
        std::thread::yield_now();
    }
}

fn settle(
    attachments: &mut Attachments,
    assets: &aqua::images::ImageAssets,
    settled: impl Fn(&Attachments) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !settled(attachments) {
        attachments.drain(assets);
        assert!(Instant::now() < deadline, "the attachments settle in time");
        std::thread::yield_now();
    }
}

fn stored(hash: &str) -> BlobRef {
    BlobRef {
        hash: BlobHash(hash.into()),
        bytes: 1,
    }
}

#[test]
fn an_image_uploads_even_when_its_preview_cannot_be_shown() {
    let (forge, mut attachments) = stub_forge();
    assert!(attachments.add(Source::Bytes(noisy_png(1024)), "pasted.png".into()));
    let assets = aqua::images::ImageAssets::default();
    let deadline = Instant::now() + Duration::from_secs(60);
    while attachments.items[0].pending_preview.is_none() {
        attachments.drain(&assets);
        assert!(Instant::now() < deadline, "the image converts in time");
        std::thread::yield_now();
    }
    drop(assets);
    let elsewhere = aqua::images::ImageAssets::default();
    upload_request(&forge, &mut attachments, &elsewhere)
        .unwrap_or_else(|| {
            panic!(
                "the image never reached Forge: {:?}",
                attachments.items[0].state
            )
        })
        .respond(stored("ab"));
    settle(&mut attachments, &elsewhere, |attachments| {
        matches!(attachments.items[0].state, State::Ready(_))
    });
    assert_eq!(
        attachments.ready_hashes(),
        Some(vec![BlobHash("ab".into())]),
        "the image goes out with the message whether or not its preview shows"
    );
}

#[test]
fn pasting_an_image_again_after_its_upload_failed_uploads_it_again() {
    let (forge, mut attachments) = stub_forge();
    let assets = aqua::images::ImageAssets::default();
    let png = noisy_png(256);
    assert!(attachments.add(Source::Bytes(png.clone()), "pasted.png".into()));
    upload_request(&forge, &mut attachments, &assets)
        .expect("the first paste uploads")
        .fail(StatusCode::SERVICE_UNAVAILABLE, "busy", "Forge is busy");
    settle(&mut attachments, &assets, |attachments| {
        matches!(attachments.items[0].state, State::Failed(_))
    });

    assert!(attachments.add(Source::Bytes(png), "pasted.png".into()));
    upload_request(&forge, &mut attachments, &assets)
        .expect("pasting the image again tries the upload again")
        .respond(stored("cd"));
    settle(&mut attachments, &assets, |attachments| {
        attachments
            .items
            .iter()
            .any(|item| matches!(item.state, State::Ready(_)))
    });
    assert_eq!(
        attachments.ready_hashes(),
        Some(vec![BlobHash("cd".into())]),
        "the retried image replaces the failed one"
    );
}

#[test]
fn an_image_still_converting_after_a_second_shows_it_is_waiting() {
    let (_forge, mut attachments) = stub_forge();
    let pasted = Instant::now();
    assert!(attachments.add(Source::Bytes(noisy_png(64)), "pasted.png".into()));
    let deadline = attachments
        .waiting_deadline(Instant::now())
        .expect("a frame is asked for when the spinner is due");
    assert!(
        !attachments.items[0].waiting(pasted),
        "a fresh image is only blurred"
    );
    assert!(
        attachments.items[0].waiting(deadline),
        "the spinner shows once the image has waited a second"
    );
    assert!(deadline >= pasted + Duration::from_secs(1));
    assert_eq!(
        attachments.waiting_deadline(deadline),
        None,
        "no frame is asked for once the spinner shows"
    );
}
