use editor_aqua::composer::encode::{placeholder, to_webp};
use std::io::Cursor;

const HEIC: &[u8] = include_bytes!("fixtures/quad.heic");
const AVIF: &[u8] = include_bytes!("fixtures/quad.avif");
const QUADRANTS: [[u8; 3]; 4] = [[220, 40, 30], [30, 180, 60], [40, 70, 210], [240, 240, 240]];

fn quad(width: u32, height: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(width, height, |x, y| {
        let [red, green, blue] =
            QUADRANTS[usize::from(x >= width / 2) + 2 * usize::from(y >= height / 2)];
        image::Rgba([red, green, blue, 255])
    })
}

fn encoded(image: &image::RgbaImage, format: image::ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    match format {
        image::ImageFormat::Jpeg => image::DynamicImage::ImageRgba8(image.clone())
            .into_rgb8()
            .write_to(&mut bytes, format),
        _ => image.write_to(&mut bytes, format),
    }
    .unwrap();
    bytes.into_inner()
}

fn with_orientation(jpeg: &[u8], orientation: u8) -> Vec<u8> {
    let mut exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    exif.extend_from_slice(&[orientation, 0, 0, 0, 0, 0, 0, 0]);
    let length = u16::try_from(exif.len() + 2).unwrap().to_be_bytes();
    [&jpeg[..2], &[0xff, 0xe1], &length, &exif, &jpeg[2..]].concat()
}

fn sent(bytes: &[u8]) -> image::RgbaImage {
    let webp = to_webp(bytes).unwrap();
    assert_eq!((&webp[..4], &webp[8..12]), (&b"RIFF"[..], &b"WEBP"[..]));
    image::load_from_memory_with_format(&webp, image::ImageFormat::WebP)
        .unwrap()
        .into_rgba8()
}

fn assert_quadrants(image: &image::RgbaImage, order: [usize; 4]) {
    let (width, height) = image.dimensions();
    for (index, (x, y)) in [(1, 1), (3, 1), (1, 3), (3, 3)].into_iter().enumerate() {
        let actual = image.get_pixel(width * x / 4, height * y / 4).0;
        let expected = QUADRANTS[order[index]];
        assert!(
            actual[..3]
                .iter()
                .zip(expected)
                .all(|(actual, expected)| actual.abs_diff(expected) <= 12),
            "{actual:?} != {expected:?} in quadrant {index}"
        );
    }
}

#[test]
fn attachments_in_every_accepted_format_are_sent_as_webp() {
    let tiff = encoded(&quad(64, 48), image::ImageFormat::Tiff);
    let ico = encoded(&quad(32, 32), image::ImageFormat::Ico);
    for (bytes, extent) in [
        (HEIC, (64, 48)),
        (AVIF, (64, 48)),
        (&tiff[..], (64, 48)),
        (&ico[..], (32, 32)),
    ] {
        let image = sent(bytes);
        assert_eq!(image.dimensions(), extent);
        assert_quadrants(&image, [0, 1, 2, 3]);
        let preview = placeholder(bytes).unwrap();
        assert_eq!(preview.width().max(preview.height()), 144);
        assert_quadrants(&preview, [0, 1, 2, 3]);
    }
    assert!(to_webp(b"%PDF-1.7 not an image").is_err());
}

#[test]
fn a_photo_is_sent_the_way_up_its_camera_recorded() {
    let upright = encoded(&quad(64, 48), image::ImageFormat::Jpeg);
    let image = sent(&with_orientation(&upright, 6));
    assert_eq!(image.dimensions(), (48, 64));
    assert_quadrants(&image, [2, 0, 3, 1]);
}
