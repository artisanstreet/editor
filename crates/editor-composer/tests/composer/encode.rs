use super::to_webp;
use std::time::{Duration, Instant};

fn screenshot(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        let cell = (x / 7).wrapping_mul(2_654_435_761) ^ (y / 11).wrapping_mul(40_503);
        let inked = cell % 5 == 0 && (x % 7 + y % 11) % 3 != 0;
        let [red, green, blue, _] = cell.to_le_bytes();
        if inked {
            image::Rgba([red / 2, green / 2, blue / 2, 255])
        } else if y < 64 {
            image::Rgba([32, 34, 40, 255])
        } else {
            image::Rgba([250, 250, 248, 255])
        }
    });
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("the test screenshot encodes");
    png
}

fn fastest_of_three(mut run: impl FnMut()) -> Duration {
    (0..3)
        .map(|_| {
            let started = Instant::now();
            run();
            started.elapsed()
        })
        .min()
        .expect("three runs happened")
}

#[test]
fn a_pasted_high_resolution_screenshot_converts_in_a_few_decodes_time() {
    let png = screenshot(3066, 1918);
    let decode = fastest_of_three(|| {
        image::load_from_memory(&png).expect("the screenshot decodes");
    });
    let convert = fastest_of_three(|| {
        let webp = to_webp(&png).expect("the screenshot converts");
        let decoded = image::load_from_memory(&webp).expect("the WebP decodes");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (2576, 1611),
            "the screenshot is sent at the display size"
        );
    });
    assert!(
        convert < decode * 45,
        "converting a pasted screenshot took {convert:?}, {:.1} times decoding it ({decode:?}); the paste waits on it",
        convert.as_secs_f64() / decode.as_secs_f64()
    );
}
