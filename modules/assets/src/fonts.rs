//! Bundled Spline Sans and Spline Sans Mono variable TrueType fonts.
//!
//! Files are embedded at compile time and registered once through
//! `artisan_ui::fonts::register_bundled_fonts`. Family names and weight ranges
//! match the fonts' internal name/fvar tables. See `fonts/FONTS.md` for
//! upstream sources, licenses and SHA-256 hashes. TrueType is required by
//! DirectWrite's in-memory loader; WOFF2 is not supported there.

use core::fmt;
use std::borrow::Cow;

/// One vendored variable typeface: its family identity plus embedded bytes.
#[derive(Clone, Copy, Debug)]
pub struct BundledFont {
    /// File name under `modules/assets/fonts/`.
    pub file_name: &'static str,
    /// Internal family name used by the native text system.
    pub family: &'static str,
    /// Inclusive variable-weight range verified from the font’s fvar table.
    pub weights: (u16, u16),
    /// Path of the license or provenance note under `modules/assets/`.
    pub license_path: &'static str,
    /// Embedded license or provenance note contents.
    pub license_text: &'static str,
    /// Unmodified upstream TrueType bytes (provenance in `fonts/FONTS.md`).
    pub bytes: &'static [u8],
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
    },
    BundledFont {
        file_name: "spline-sans-mono-variable.ttf",
        family: "Spline Sans Mono",
        weights: (300, 700),
        license_path: "licenses/spline-sans-mono-OFL.txt",
        license_text: include_str!("../licenses/spline-sans-mono-OFL.txt"),
        bytes: include_bytes!("../fonts/spline-sans-mono-variable.ttf"),
    },
];

/// Shapes the embedded bytes for `gpui::TextSystem::add_fonts`.
///
/// Borrowed by construction: `include_bytes!` already yields `&'static [u8]`,
/// so registration copies nothing.
#[must_use]
pub fn bundled_fonts() -> Vec<Cow<'static, [u8]>> {
    ALL.iter().map(|font| Cow::Borrowed(font.bytes)).collect()
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

#[cfg(test)]
mod tests {
    use super::{ALL, UnknownFont, bundled_fonts, lookup_family};

    /// Expected `(file name, byte length)` pins: any truncation or
    /// re-encode of a vendored binary fails here before it can reach a
    /// renderer and silently fall back to a system face.
    const EXPECTED_LENGTHS: [(&str, usize); 2] = [
        ("spline-sans-variable.ttf", 146_896),
        ("spline-sans-mono-variable.ttf", 118_744),
    ];

    #[test]
    fn catalog_carries_exactly_the_two_declared_faces() {
        assert_eq!(ALL.len(), 2, "two Spline families");
        let families: Vec<&str> = ALL.iter().map(|font| font.family).collect();
        assert_eq!(
            families,
            vec!["Spline Sans", "Spline Sans Mono"],
            "catalog order is by family name for binary search"
        );
        for (font, (file_name, length)) in ALL.iter().zip(EXPECTED_LENGTHS) {
            assert_eq!(font.file_name, file_name);
            assert_eq!(
                font.bytes.len(),
                length,
                "{file_name}: embedded length drift means the binary changed"
            );
        }
    }

    #[test]
    fn embedded_bytes_are_truetype_sfnt_containers() {
        // TrueType sfnt version `00 01 00 00`: catches text-encoding damage
        // (e.g. line-ending conversion) at compile-test time, and pins the
        // DirectWrite-loadable container — WOFF2 (`wOF2`) is rejected by the
        // in-memory loader on Windows (`DWRITE_E_FILEFORMAT`) and must never
        // be vendored here again.
        for font in ALL {
            assert!(
                font.bytes.len() > 48,
                "{}: implausibly small for a variable font",
                font.file_name
            );
            assert_eq!(
                &font.bytes[0..4],
                b"\x00\x01\x00\x00",
                "{}: missing TrueType sfnt magic; the binary is damaged or substituted",
                font.file_name
            );
        }
    }

    #[test]
    fn bundled_fonts_shapes_borrowed_slices_for_add_fonts() {
        let shaped = bundled_fonts();
        assert_eq!(shaped.len(), ALL.len());
        for (shaped, font) in shaped.iter().zip(ALL.iter()) {
            assert_eq!(
                shaped.as_ref(),
                font.bytes,
                "{}: shaped bytes must borrow the embedded bytes",
                font.file_name
            );
        }
    }

    #[test]
    fn family_lookup_resolves_each_face_and_rejects_unknowns() {
        for font in ALL {
            assert_eq!(
                lookup_family(font.family)
                    .expect("bundled family")
                    .file_name,
                font.file_name
            );
        }
        assert_eq!(
            lookup_family("Segoe UI").expect_err("system face is not bundled"),
            UnknownFont {
                id: String::from("Segoe UI"),
            }
        );
    }

    #[test]
    fn every_face_carries_a_nonempty_license_record() {
        for font in ALL {
            assert!(
                !font.license_text.is_empty(),
                "{}: license record must be embedded",
                font.file_name
            );
            assert!(
                font.license_path.starts_with("licenses/"),
                "{}: odd license path {}",
                font.file_name,
                font.license_path
            );
        }
    }
}
