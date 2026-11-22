use editor_aqua::frame::sign_in::dither::{Mask, Paint, Params, Source, dither, paint};
use image::{Rgba, RgbaImage};

const PARAMS: Params = Params {
    pixel_size: 1.0,
    contrast: 1.0,
    brightness: 1.0,
    threshold_bias: 0.0,
    max_resolution: 1024.0,
    fade_right: 0.0,
};

fn solid(width: u32, height: u32, value: u8) -> RgbaImage {
    RgbaImage::from_pixel(width, height, Rgba([value, value, value, 255]))
}

fn lit(mask: &Mask, x: u32, y: u32) -> bool {
    mask.lit(x, y)
}

#[test]
fn a_wide_image_sits_on_the_bottom_edge() {
    let rgba = solid(40, 10, 255);
    let mask = dither(Source::new(&rgba), 40.0, 40.0, PARAMS);
    assert!((0..mask.width()).all(|x| !lit(&mask, x, 0)));
    assert!((0..mask.width()).all(|x| lit(&mask, x, mask.height() - 1)));
}

#[test]
fn black_stays_dark_and_fading_right_clears_the_right_edge() {
    let dark = solid(16, 16, 0);
    let mask = dither(Source::new(&dark), 16.0, 16.0, PARAMS);
    assert!((0..16).all(|y| (0..16).all(|x| !lit(&mask, x, y))));
    let bright = solid(16, 16, 255);
    let faded = dither(
        Source::new(&bright),
        16.0,
        16.0,
        Params {
            fade_right: 0.5,
            ..PARAMS
        },
    );
    let column = |x: u32| (0..faded.height()).filter(|y| lit(&faded, x, *y)).count();
    assert_eq!(column(0), 16);
    assert!(column(15) < column(8));
}

#[test]
fn painting_fades_in_from_the_left_and_leaves_gaps_clear() {
    let mask = Mask::from_fn(4, 1, |x, _| x != 1);
    let pixels = paint(
        &mask,
        Paint {
            rgb: [10, 20, 30],
            opacity: 1.0,
            fade_in_left: 0.5,
        },
    );
    let alpha: Vec<u8> = pixels.pixels().map(|pixel| pixel[3]).collect();
    assert_eq!(alpha[1], 0);
    assert!(alpha[0] < alpha[2]);
    assert_eq!(alpha[3], 255);
    assert_eq!(&pixels.get_pixel(0, 0).0[0..3], &[10, 20, 30]);
}
