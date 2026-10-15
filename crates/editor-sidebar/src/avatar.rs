//! The account avatar: a seeded, dithered gradient tile, one per host, so a
//! machine keeps its colour whoever is signed in. Ported from the previous
//! Editor's `profile_avatar` (itself `gradient-avatar.ts`).

use editor_ui::theme::{Oklch, RadiusStep, RadiusTokens};
use gpui::{
    Div, Hsla, Image, ImageFormat, ObjectFit, ParentElement as _, Styled as _, StyledImage as _,
    div, img, px,
};
use std::sync::Arc;

/// Cells across the tile: 2 px cells at the 32 px avatar.
const CELLS: u8 = 16;
/// Short of full density so the lit end keeps its grain.
const MAX_DENSITY: f64 = 0.9;
const BAYER_4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Tailwind's chromatic families, each ramping its 600 into its 400 toward
/// the top-right corner.
const PALETTE: [(Oklch, Oklch); 17] = [
    (
        Oklch::new(0.577, 0.245, 27.325),
        Oklch::new(0.704, 0.191, 22.216),
    ),
    (
        Oklch::new(0.646, 0.222, 41.116),
        Oklch::new(0.75, 0.183, 55.934),
    ),
    (
        Oklch::new(0.666, 0.179, 58.318),
        Oklch::new(0.828, 0.189, 84.429),
    ),
    (
        Oklch::new(0.681, 0.162, 75.834),
        Oklch::new(0.852, 0.199, 91.936),
    ),
    (
        Oklch::new(0.648, 0.2, 131.684),
        Oklch::new(0.841, 0.238, 128.85),
    ),
    (
        Oklch::new(0.627, 0.194, 149.214),
        Oklch::new(0.792, 0.209, 151.711),
    ),
    (
        Oklch::new(0.596, 0.145, 163.225),
        Oklch::new(0.765, 0.177, 163.223),
    ),
    (
        Oklch::new(0.6, 0.118, 184.704),
        Oklch::new(0.777, 0.152, 181.912),
    ),
    (
        Oklch::new(0.609, 0.126, 221.723),
        Oklch::new(0.789, 0.154, 211.53),
    ),
    (
        Oklch::new(0.588, 0.158, 241.966),
        Oklch::new(0.746, 0.16, 232.661),
    ),
    (
        Oklch::new(0.546, 0.245, 262.881),
        Oklch::new(0.707, 0.165, 254.624),
    ),
    (
        Oklch::new(0.511, 0.262, 276.966),
        Oklch::new(0.673, 0.182, 276.935),
    ),
    (
        Oklch::new(0.541, 0.281, 293.009),
        Oklch::new(0.702, 0.183, 293.541),
    ),
    (
        Oklch::new(0.558, 0.288, 302.321),
        Oklch::new(0.714, 0.203, 305.504),
    ),
    (
        Oklch::new(0.591, 0.293, 322.896),
        Oklch::new(0.74, 0.238, 322.16),
    ),
    (
        Oklch::new(0.592, 0.249, 0.584),
        Oklch::new(0.718, 0.202, 349.761),
    ),
    (
        Oklch::new(0.586, 0.253, 17.585),
        Oklch::new(0.712, 0.194, 13.428),
    ),
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

fn paints(seed: &str) -> (Hsla, Hsla) {
    let index = seed_hash(seed) as usize % PALETTE.len();
    let (base, lit) = PALETTE[index];
    (base.to_paint(), lit.to_paint())
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
pub(crate) fn avatar_image(seed: &str) -> Arc<Image> {
    let (base, lit_paint) = paints(seed);
    let color = |paint: Hsla| {
        let rgb = gpui::hsla_to_rgba(paint);
        format!(
            "rgb({}%, {}%, {}%)",
            rgb.red * 100.0,
            rgb.green * 100.0,
            rgb.blue * 100.0
        )
    };
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 16 16"><rect width="16" height="16" fill="{}"/><g fill="{}">"#,
        color(base),
        color(lit_paint)
    );
    for y in 0..CELLS {
        for x in 0..CELLS {
            if lit(x, y) {
                let _ = std::fmt::Write::write_fmt(
                    &mut svg,
                    format_args!(r#"<rect x="{x}" y="{y}" width="1" height="1"/>"#),
                );
            }
        }
    }
    svg.push_str("</g></svg>");
    Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()))
}

/// The 32 px avatar.
pub(crate) fn avatar(image: Arc<Image>) -> Div {
    div().size(px(32.0)).child(
        img(image)
            .size(px(32.0))
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .object_fit(ObjectFit::Cover),
    )
}
