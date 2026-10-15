//! Bundled Spline Sans and Spline Sans Mono native OpenType fonts.
//!
//! Files are embedded at compile time and registered once through
//! `editor_ui::fonts::register_bundled_fonts`. Family names and weight ranges
//! match the source fonts' internal name/fvar tables. Static instances of
//! weights 300–700 plus the conversation-prose weights 410 and 630 are
//! registered because the WGPU text backend matches faces by metadata and
//! does not apply variable weight axes. See `fonts/FONTS.md` for upstream
//! sources, licenses and SHA-256 hashes. Native sfnt containers are required
//! by DirectWrite's in-memory loader; WOFF2 is not supported there.

use core::fmt;
use std::borrow::Cow;

/// One vendored typeface: its family identity plus embedded bytes.
#[derive(Clone, Copy, Debug)]
pub struct BundledFont {
    /// File name under `crates/editor-assets/fonts/`.
    pub file_name: &'static str,
    /// Internal family name used by the native text system.
    pub family: &'static str,
    /// Inclusive weight range verified from the font's fvar or OS/2 table.
    pub weights: (u16, u16),
    /// Path of the license or provenance note under `crates/editor-assets/`.
    pub license_path: &'static str,
    /// Embedded license or provenance note contents.
    pub license_text: &'static str,
    /// Unmodified upstream font bytes (provenance in `fonts/FONTS.md`).
    pub bytes: &'static [u8],
    /// Static weight instances registered by renderers without variable-axis support.
    pub static_faces: &'static [&'static [u8]],
}

/// Failure returned when a string does not name a bundled typeface.
///
/// First-party by hand so this catalog stays dependency-free, mirroring
/// [`crate::UnknownAsset`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownFont {
    /// The rejected identifier.
    pub id: String,
}

impl fmt::Display for UnknownFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown bundled font `{}`", self.id)
    }
}

impl std::error::Error for UnknownFont {}

/// Every bundled typeface, ordered by family name so lookups binary search.
pub const ALL: &[BundledFont] = &[
    BundledFont {
        file_name: "spline-sans-variable.ttf",
        family: "Spline Sans",
        weights: (300, 700),
        license_path: "licenses/spline-sans-OFL.txt",
        license_text: include_str!("../licenses/spline-sans-OFL.txt"),
        bytes: include_bytes!("../fonts/spline-sans-variable.ttf"),
        static_faces: &[
            include_bytes!("../fonts/spline-sans-300.ttf"),
            include_bytes!("../fonts/spline-sans-400.ttf"),
            include_bytes!("../fonts/spline-sans-410.ttf"),
            include_bytes!("../fonts/spline-sans-500.ttf"),
            include_bytes!("../fonts/spline-sans-600.ttf"),
            include_bytes!("../fonts/spline-sans-630.ttf"),
            include_bytes!("../fonts/spline-sans-700.ttf"),
        ],
    },
    BundledFont {
        file_name: "spline-sans-mono-variable.ttf",
        family: "Spline Sans Mono",
        weights: (300, 700),
        license_path: "licenses/spline-sans-mono-OFL.txt",
        license_text: include_str!("../licenses/spline-sans-mono-OFL.txt"),
        bytes: include_bytes!("../fonts/spline-sans-mono-variable.ttf"),
        static_faces: &[
            include_bytes!("../fonts/spline-sans-mono-300.ttf"),
            include_bytes!("../fonts/spline-sans-mono-400.ttf"),
            include_bytes!("../fonts/spline-sans-mono-500.ttf"),
            include_bytes!("../fonts/spline-sans-mono-600.ttf"),
            include_bytes!("../fonts/spline-sans-mono-700.ttf"),
        ],
    },
    BundledFont {
        file_name: "test-founders-grotesk-bold.otf",
        family: "Test Founders Grotesk",
        weights: (600, 700),
        license_path: "licenses/founders-grotesk-test.txt",
        license_text: include_str!("../licenses/founders-grotesk-test.txt"),
        bytes: include_bytes!("../fonts/test-founders-grotesk-bold.otf"),
        static_faces: &[
            include_bytes!("../fonts/test-founders-grotesk-semibold.otf"),
            include_bytes!("../fonts/test-founders-grotesk-bold.otf"),
        ],
    },
    BundledFont {
        file_name: "twemoji-mozilla.ttf",
        family: "Twemoji Mozilla",
        weights: (400, 400),
        license_path: "licenses/twemoji-LICENSE.md",
        license_text: include_str!("../licenses/twemoji-LICENSE.md"),
        bytes: include_bytes!("../fonts/twemoji-mozilla.ttf"),
        static_faces: &[include_bytes!("../fonts/twemoji-mozilla.ttf")],
    },
];

/// Shapes the embedded bytes for `gpui::TextSystem::add_fonts`.
///
/// Borrowed by construction: `include_bytes!` already yields `&'static [u8]`,
/// so registration copies nothing.
#[must_use]
pub fn bundled_fonts() -> Vec<Cow<'static, [u8]>> {
    ALL.iter()
        .flat_map(|font| font.static_faces.iter().map(|bytes| Cow::Borrowed(*bytes)))
        .collect()
}

/// Resolves a family name (exactly as declared, e.g. `"Spline Sans"`) to its
/// bundled typeface.
///
/// # Errors
///
/// Returns [`UnknownFont`] when `family` names no vendored face.
pub fn lookup_family(family: &str) -> Result<&'static BundledFont, UnknownFont> {
    match ALL.binary_search_by(|probe| probe.family.cmp(family)) {
        Ok(index) => Ok(&ALL[index]),
        Err(_) => Err(UnknownFont {
            id: String::from(family),
        }),
    }
}
