//! The account avatar: a seeded, dithered gradient tile, one per host, so a
//! machine keeps its colour whoever is signed in. Ported from the previous
//! Editor's `profile_avatar` (itself `gradient-avatar.ts`).

use editor_ui::theme::{RadiusStep, RadiusTokens};
use gpui::{
    Div, Image, ImageFormat, ObjectFit, ParentElement as _, Styled as _, StyledImage as _, div,
    img, px,
};
use std::sync::Arc;

/// Cells across the tile: 2 px cells at the 32 px avatar.
const CELLS: u8 = 16;
/// Short of full density so the lit end keeps its grain.
const MAX_DENSITY: f64 = 0.9;
const BAYER_4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Identity art, content not UI: each avatar's single-hue ramp, Tailwind's
/// chromatic families from their 600 (the tile) into their 400 (the cells
/// that light toward the top-right corner), as sRGB `0xRRGGBB`. An avatar
/// is an image; it paints the same in light and dark mode.
const AVATAR_HUES: [(u32, u32); 17] = [
    (0x00e7_000b, 0x00ff_6467),
    (0x00f5_4900, 0x00ff_8904),
    (0x00e1_7100, 0x00ff_b900),
    (0x00d0_8700, 0x00fd_c700),
    (0x005e_a500, 0x009a_e600),
    (0x0000_a63e, 0x0005_df72),
    (0x0000_9966, 0x0000_d492),
    (0x0000_9689, 0x0000_d5be),
    (0x0000_92b8, 0x0000_d3f2),
    (0x0000_84d1, 0x0000_bcff),
    (0x0015_5dfc, 0x0051_a2ff),
    (0x004f_39f6, 0x007c_86ff),
    (0x007f_22fe, 0x00a6_84ff),
    (0x0098_10fa, 0x00c2_7aff),
    (0x00c8_00de, 0x00ed_6aff),
    (0x00e6_0076, 0x00fb_64b6),
    (0x00ec_003f, 0x00ff_637e),
];

/// FNV-1a over UTF-16 code units, so a seed's colour matches the web
/// Editor's.
fn seed_hash(seed: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    let mut units = [0_u16; 2];
    for ch in seed.chars() {
        for unit in ch.encode_utf16(&mut units) {
            hash ^= u32::from(*unit);
            hash = hash.wrapping_mul(0x0100_0193);
        }
    }
    hash
}

/// The seed's ramp: the tile's base and its lit cells.
fn paints(seed: &str) -> (u32, u32) {
    AVATAR_HUES[seed_hash(seed) as usize % AVATAR_HUES.len()]
}

/// Whether a cell paints lit: progress along the bottom-left → top-right
/// diagonal against the Bayer threshold.
fn lit(x: u8, y: u8) -> bool {
    let horizontal = (f64::from(x) + 0.5) / f64::from(CELLS);
    let vertical = 1.0 - (f64::from(y) + 0.5) / f64::from(CELLS);
    let density = f64::midpoint(horizontal, vertical)
        .clamp(0.0, 1.0)
        .min(MAX_DENSITY);
    let threshold = (f64::from(BAYER_4[usize::from(y & 3)][usize::from(x & 3)]) + 0.5) / 16.0;
    density > threshold
}

/// The tile for `seed`, drawn as one image so the rounded clip applies to
/// every cell.
pub fn avatar_image(seed: &str) -> Arc<Image> {
    let (base, lit_paint) = paints(seed);
    let color = |rgb: u32| format!("#{rgb:06x}");
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 16 16"><rect width="16" height="16" fill="{}"/><g fill="{}">"#,
        color(base),
        color(lit_paint)
    );
    for y in 0..CELLS {
        for x in 0..CELLS {
            if lit(x, y) {
                // Writing into a String cannot fail.
                std::fmt::Write::write_fmt(
                    &mut svg,
                    format_args!(r#"<rect x="{x}" y="{y}" width="1" height="1"/>"#),
                )
                .ok();
            }
        }
    }
    svg.push_str("</g></svg>");
    Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()))
}

/// The 32 px avatar.
pub fn avatar(image: Arc<Image>) -> Div {
    div().size(px(32.0)).child(
        img(image)
            .size(px(32.0))
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .object_fit(ObjectFit::Cover),
    )
}
