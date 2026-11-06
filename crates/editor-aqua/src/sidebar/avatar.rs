use std::fmt::Write as _;

const CELLS: u8 = 16;
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
const HUES: [(u32, u32); 17] = [
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

pub fn seed_hash(seed: &str) -> u32 {
    seed.encode_utf16().fold(0x811c_9dc5_u32, |hash, unit| {
        (hash ^ u32::from(unit)).wrapping_mul(0x0100_0193)
    })
}

pub fn lit(x: u8, y: u8) -> bool {
    let horizontal = (f64::from(x) + 0.5) / f64::from(CELLS);
    let vertical = 1.0 - (f64::from(y) + 0.5) / f64::from(CELLS);
    let density = f64::midpoint(horizontal, vertical).clamp(0.0, 1.0).min(0.9);
    let threshold = (f64::from(BAYER[usize::from(y & 3)][usize::from(x & 3)]) + 0.5) / 16.0;
    density > threshold
}

pub fn svg(seed: &str) -> String {
    let (base, light) = HUES[seed_hash(seed) as usize % HUES.len()];
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 16 16"><rect width="16" height="16" fill="#{base:06x}"/><g fill="#{light:06x}">"##
    );
    for y in 0..CELLS {
        for x in 0..CELLS {
            if lit(x, y) {
                write!(svg, r#"<rect x="{x}" y="{y}" width="1" height="1"/>"#).ok();
            }
        }
    }
    svg.push_str("</g></svg>");
    svg
}
