//! Deterministic, dithered gradient avatars.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/identity/gradient-avatar.ts`. A stable seed
//! selects one of the chromatic Tailwind OKLCH pairs, while a fixed Bayer
//! matrix turns the diagonal ramp into merged horizontal SVG runs. The
//! output is deliberately standalone markup so it can be serialized or
//! handed to a renderer without any theme, DOM, or application state.

#![allow(clippy::module_name_repetitions)]

use std::fmt::Write as _;

/// One chromatic Tailwind palette entry used by a gradient avatar.
///
/// `from` is the darker 600 shade used for the tile background and `to` is the
/// lighter 400 shade used for lit dither cells. The strings are kept as
/// complete OKLCH CSS values so the standalone SVG has no dependency on CSS
/// variables or a surrounding theme.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GradientAvatarColor {
    /// Stable family name, such as `"red"` or `"blue"`.
    pub name: &'static str,
    /// Tailwind 600 OKLCH value used as the unlit shade.
    pub from: &'static str,
    /// Tailwind 400 OKLCH value used as the lit shade.
    pub to: &'static str,
}

impl GradientAvatarColor {
    /// All 17 chromatic palette entries in their canonical TypeScript order.
    pub const ALL: [Self; 17] = [
        Self {
            name: "red",
            from: "oklch(57.7% 0.245 27.325)",
            to: "oklch(70.4% 0.191 22.216)",
        },
        Self {
            name: "orange",
            from: "oklch(64.6% 0.222 41.116)",
            to: "oklch(75% 0.183 55.934)",
        },
        Self {
            name: "amber",
            from: "oklch(66.6% 0.179 58.318)",
            to: "oklch(82.8% 0.189 84.429)",
        },
        Self {
            name: "yellow",
            from: "oklch(68.1% 0.162 75.834)",
            to: "oklch(85.2% 0.199 91.936)",
        },
        Self {
            name: "lime",
            from: "oklch(64.8% 0.2 131.684)",
            to: "oklch(84.1% 0.238 128.85)",
        },
        Self {
            name: "green",
            from: "oklch(62.7% 0.194 149.214)",
            to: "oklch(79.2% 0.209 151.711)",
        },
        Self {
            name: "emerald",
            from: "oklch(59.6% 0.145 163.225)",
            to: "oklch(76.5% 0.177 163.223)",
        },
        Self {
            name: "teal",
            from: "oklch(60% 0.118 184.704)",
            to: "oklch(77.7% 0.152 181.912)",
        },
        Self {
            name: "cyan",
            from: "oklch(60.9% 0.126 221.723)",
            to: "oklch(78.9% 0.154 211.53)",
        },
        Self {
            name: "sky",
            from: "oklch(58.8% 0.158 241.966)",
            to: "oklch(74.6% 0.16 232.661)",
        },
        Self {
            name: "blue",
            from: "oklch(54.6% 0.245 262.881)",
            to: "oklch(70.7% 0.165 254.624)",
        },
        Self {
            name: "indigo",
            from: "oklch(51.1% 0.262 276.966)",
            to: "oklch(67.3% 0.182 276.935)",
        },
        Self {
            name: "violet",
            from: "oklch(54.1% 0.281 293.009)",
            to: "oklch(70.2% 0.183 293.541)",
        },
        Self {
            name: "purple",
            from: "oklch(55.8% 0.288 302.321)",
            to: "oklch(71.4% 0.203 305.504)",
        },
        Self {
            name: "fuchsia",
            from: "oklch(59.1% 0.293 322.896)",
            to: "oklch(74% 0.238 322.16)",
        },
        Self {
            name: "pink",
            from: "oklch(59.2% 0.249 0.584)",
            to: "oklch(71.8% 0.202 349.761)",
        },
        Self {
            name: "rose",
            from: "oklch(58.6% 0.253 17.585)",
            to: "oklch(71.2% 0.194 13.428)",
        },
    ];
}

/// The canonical avatar palette, exposed for consumers that need the selected
/// color's complete typed entry as well as [`gradient_avatar_color_for`].
pub const AVATAR_PALETTE: [GradientAvatarColor; 17] = GradientAvatarColor::ALL;

/// Bayer 4×4 thresholds, normalized exactly as in the TypeScript source.
pub const BAYER4: [[f64; 4]; 4] = [
    [0.03125, 0.53125, 0.15625, 0.65625],
    [0.78125, 0.28125, 0.90625, 0.40625],
    [0.21875, 0.71875, 0.09375, 0.59375],
    [0.96875, 0.46875, 0.84375, 0.34375],
];

/// Number of cells along each edge of the square avatar.
pub const AVATAR_CELLS: usize = 16;

/// Maximum dither density, leaving a few darker cells in the lit corner.
pub const MAXIMUM_DENSITY: f64 = 0.9;

/// A horizontally merged run of lit cells in one avatar row.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GradientAvatarRun {
    /// First cell column, from the left.
    pub x: usize,
    /// Cell row, from the top.
    pub y: usize,
    /// Number of contiguous lit cells in the run.
    pub width: usize,
}

/// Computes the JavaScript-compatible FNV-1a hash for a seed.
///
/// JavaScript's `seed.length` and `seed.charCodeAt(index)` iterate UTF-16 code
/// units. [`str::encode_utf16`] supplies those same units for Rust strings,
/// including both surrogate units of a supplementary Unicode scalar. Each
/// multiplication wraps at 32 bits, matching `Math.imul(..., 0x01000193)`
/// followed by `>>> 0`.
#[must_use]
pub fn gradient_avatar_seed_hash(seed: &str) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for code_unit in seed.encode_utf16() {
        hash ^= u32::from(code_unit);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Selects the canonical typed palette entry for a stable seed.
#[must_use]
pub fn gradient_avatar_color_for(seed: &str) -> GradientAvatarColor {
    let index = (gradient_avatar_seed_hash(seed) as usize) % AVATAR_PALETTE.len();
    AVATAR_PALETTE[index]
}

#[allow(clippy::manual_clamp)]
fn clamp(value: f64) -> f64 {
    value.min(1.0).max(0.0)
}

fn lit_runs() -> Vec<GradientAvatarRun> {
    let mut runs = Vec::new();

    #[allow(clippy::cast_precision_loss)]
    for y in 0..AVATAR_CELLS {
        let mut run_start = None;

        for x in 0..=AVATAR_CELLS {
            let lit = if x < AVATAR_CELLS {
                let horizontal_progress = (x as f64 + 0.5) / AVATAR_CELLS as f64;
                let vertical_progress = 1.0 - (y as f64 + 0.5) / AVATAR_CELLS as f64;
                let density = MAXIMUM_DENSITY
                    .min(clamp(f64::midpoint(horizontal_progress, vertical_progress)));
                density > BAYER4[y & 3][x & 3]
            } else {
                false
            };

            if lit && run_start.is_none() {
                run_start = Some(x);
            }
            if !lit && let Some(start) = run_start.take() {
                runs.push(GradientAvatarRun {
                    x: start,
                    y,
                    width: x - start,
                });
            }
        }
    }

    runs
}

/// Returns the deterministic merged lit-cell geometry used by the SVG.
#[must_use]
pub fn gradient_avatar_runs() -> Vec<GradientAvatarRun> {
    lit_runs()
}

fn escape_attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            '<' => escaped.push_str("&lt;"),
            character => escaped.push(character),
        }
    }
    escaped
}

/// Renders the exact standalone SVG counterpart of `GradientAvatarSvg`.
///
/// `None` emits `aria-hidden="true"`; `Some(title)` emits `role="img"` and
/// an escaped `aria-label`. Only `&`, `"`, and `<` are escaped, matching the
/// source's three ordered `replaceAll` calls. No title element, CSS variable,
/// or surrounding component is added.
#[must_use]
pub fn gradient_avatar_svg(seed: &str, title: Option<&str>) -> String {
    let color = gradient_avatar_color_for(seed);
    let label = match title {
        None => " aria-hidden=\"true\"".to_owned(),
        Some(title) => format!(" role=\"img\" aria-label=\"{}\"", escape_attribute(title)),
    };
    let mut lit = String::new();
    for run in gradient_avatar_runs() {
        write!(
            &mut lit,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"1\"/>",
            run.x, run.y, run.width
        )
        .expect("writing SVG run markup to a String cannot fail");
    }

    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {AVATAR_CELLS} {AVATAR_CELLS}\" width=\"100%\" height=\"100%\" preserveAspectRatio=\"xMidYMid slice\" shape-rendering=\"crispEdges\"{label}><rect width=\"{AVATAR_CELLS}\" height=\"{AVATAR_CELLS}\" fill=\"{}\"/><g fill=\"{}\">{lit}</g></svg>",
        color.from, color.to
    )
}
