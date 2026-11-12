use crate::ui::chrome::card::hex;
use crate::ui::theme::Oklch;

pub const RIM: [(f32, [u8; 3]); 9] = [
    (0.00, [0xea, 0xf4, 0xff]),
    (0.06, [0xa9, 0xc8, 0xe6]),
    (0.16, [0x4c, 0x7b, 0xb6]),
    (0.30, [0x1d, 0x3a, 0x7a]),
    (0.50, [0x0b, 0x14, 0x40]),
    (0.70, [0x1d, 0x3a, 0x7a]),
    (0.84, [0x4c, 0x7b, 0xb6]),
    (0.94, [0xa9, 0xc8, 0xe6]),
    (1.00, [0xea, 0xf4, 0xff]),
];
pub const SEGMENTS: usize = 72;
const BODY: Oklch = Oklch::new(0.1757, 0.0, 0.0);
const INK: Oklch = Oklch::new(1.0, 0.0, 0.0);
const SHADE: Oklch = Oklch::new(0.0, 0.0, 0.0);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wedge {
    pub from_degrees: f32,
    pub to_degrees: f32,
    pub color: [u8; 3],
}

#[must_use]
pub fn sample(fraction: f32) -> [u8; 3] {
    let fraction = fraction.rem_euclid(1.0);
    let upper = RIM
        .iter()
        .position(|(at, _)| *at >= fraction)
        .unwrap_or(RIM.len() - 1)
        .max(1);
    let ((from_at, from), (to_at, to)) = (RIM[upper - 1], RIM[upper]);
    let span = (to_at - from_at).max(f32::EPSILON);
    let t = ((fraction - from_at) / span).clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    [
        lerp(from[0], to[0]),
        lerp(from[1], to[1]),
        lerp(from[2], to[2]),
    ]
}

#[must_use]
pub fn wedges(orbit_degrees: f32, segments: usize) -> Vec<Wedge> {
    let segments = segments.max(1);
    let step = 360.0 / segments as f32;
    (0..segments)
        .map(|index| {
            let from = index as f32 * step;
            Wedge {
                from_degrees: from + orbit_degrees,
                to_degrees: from + step + orbit_degrees,
                color: sample((from + step / 2.0) / 360.0),
            }
        })
        .collect()
}

fn mix(base: Oklch, ink: Oklch, amount: f32) -> String {
    hex(base.animate_to(ink, amount))
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> String {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    format!(
        "M{l} {y}H{rr}A{r} {r} 0 0 1 {right} {t}V{b}A{r} {r} 0 0 1 {rr} {bottom}H{l}A{r} {r} 0 0 1 {x} {b}V{t}A{r} {r} 0 0 1 {l} {y}Z",
        l = x + r,
        rr = x + w - r,
        right = x + w,
        t = y + r,
        b = y + h - r,
        bottom = y + h,
    )
}

#[must_use]
pub fn svg(width: f32, height: f32, radius: f32, orbit_degrees: f32) -> String {
    let (w, h) = (width.max(2.0), height.max(2.0));
    let (cx, cy) = (w / 2.0, h / 2.0);
    let reach = w.hypot(h);
    let point = |degrees: f32| {
        let radians = (degrees - 90.0).to_radians();
        (cx + reach * radians.cos(), cy + reach * radians.sin())
    };
    let fan = wedges(orbit_degrees, SEGMENTS)
        .into_iter()
        .map(|wedge| {
            let (x0, y0) = point(wedge.from_degrees);
            let (x1, y1) = point(wedge.to_degrees + 0.5);
            format!(
                r#"<path d="M{cx} {cy}L{x0} {y0}L{x1} {y1}Z" fill="rgb({},{},{})"/>"#,
                wedge.color[0], wedge.color[1], wedge.color[2]
            )
        })
        .collect::<String>();
    let outer = rounded(0.0, 0.0, w, h, radius);
    let inner = rounded(1.0, 1.0, w - 2.0, h - 2.0, (radius - 1.0).max(0.0));
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}"><defs><clipPath id="ring"><path clip-rule="evenodd" d="{outer}{inner}"/></clipPath><clipPath id="face"><path d="{inner}"/></clipPath><radialGradient id="body" gradientUnits="userSpaceOnUse" cx="{gx}" cy="{gy}" r="{gr}" gradientTransform="translate({gx} {gy}) scale(1 {ratio}) translate(-{gx} -{gy})"><stop offset="0" stop-color="{lit}"/><stop offset="0.7" stop-color="{base}"/></radialGradient></defs><g clip-path="url(#ring)">{fan}</g><path d="{inner}" fill="url(#body)"/><g clip-path="url(#face)"><rect x="0" y="1" width="{w}" height="1" fill="{top}"/><rect x="0" y="{low}" width="{w}" height="1" fill="{shade}"/></g></svg>"#,
        gx = w * 0.5,
        gy = h * 1.15,
        gr = w * 0.7,
        ratio = (h * 1.1) / (w * 0.7),
        lit = mix(BODY, INK, 0.07),
        base = hex(BODY),
        top = mix(BODY, INK, 0.12),
        shade = mix(BODY, SHADE, 0.25),
        low = h - 2.0,
    )
}
