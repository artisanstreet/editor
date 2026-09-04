//! Bundled legacy typefaces, embedded as compile-time bytes.
//!
//! The four `@font-face` families the legacy editor declares
//! (`modules/frontend/src/lib/styles/fonts.css:7–37`); provenance, digests,
//! and the exclusion rationale for the remaining legacy font files live in
//! `fonts/FONTS.md`. Consumers never touch the files on disk: [`ALL`]
//! carries the embedded bytes, and [`bundled_fonts`] shapes them for
//! `gpui::TextSystem::add_fonts` (DirectWrite in-memory references on
//! Windows). Registration itself belongs at app startup — see
//! `artisan_ui::fonts::register_bundled_fonts` — because this crate is
//! dependency-free by design and must not name GPUI.
//!
//! Family names and weight ranges here must stay identical to the
//! `artisan_ui::theme::TypographyTokens` roles; the `typography_gradient`
//! suite pins both sides.

use core::fmt;
use std::borrow::Cow;

/// One vendored variable typeface: its legacy identity plus embedded bytes.
#[derive(Clone, Copy, Debug)]
pub struct BundledFont {
    /// File name under `modules/assets/fonts/`.
    pub file_name: &'static str,
    /// Family name exactly as declared in the legacy `@font-face` block and
    /// therefore as GPUI must resolve it (e.g. `"Artisan Neo"`).
    pub family: &'static str,
    /// Inclusive variable-weight range from the `@font-face` declaration.
    pub weights: (u16, u16),
    /// Path of the license or provenance note under `modules/assets/`.
    pub license_path: &'static str,
    /// Embedded license or provenance note contents.
    pub license_text: &'static str,
    /// Embedded font bytes, bit-identical to the legacy `@font-face` source.
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
        file_name: "artisan-neo-variable.woff2",
        family: "Artisan Neo",
        weights: (100, 900),
        license_path: "licenses/artisan-neo-OFL.txt",
        license_text: include_str!("../licenses/artisan-neo-OFL.txt"),
        bytes: include_bytes!("../fonts/artisan-neo-variable.woff2"),
    },
    BundledFont {
        file_name: "cal-sans-variable.woff2",
        family: "Cal Sans",
        weights: (100, 1000),
        license_path: "licenses/cal-sans-OFL.txt",
        license_text: include_str!("../licenses/cal-sans-OFL.txt"),
        bytes: include_bytes!("../fonts/cal-sans-variable.woff2"),
    },
    BundledFont {
        file_name: "jetbrains-mono-variable.woff2",
        family: "JetBrains Mono",
        weights: (100, 800),
        license_path: "licenses/jetbrains-mono-OFL.txt",
        license_text: include_str!("../licenses/jetbrains-mono-OFL.txt"),
        bytes: include_bytes!("../fonts/jetbrains-mono-variable.woff2"),
    },
    BundledFont {
        file_name: "sigurd-artisan.woff2",
        family: "Sigurd Variable",
        weights: (300, 900),
        license_path: "licenses/sigurd-artisan-NOTES.md",
        license_text: include_str!("../licenses/sigurd-artisan-NOTES.md"),
        bytes: include_bytes!("../fonts/sigurd-artisan.woff2"),
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

/// Resolves a family name (exactly as declared, e.g. `"Artisan Neo"`) to its
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
    const EXPECTED_LENGTHS: [(&str, usize); 4] = [
        ("artisan-neo-variable.woff2", 354_924),
        ("cal-sans-variable.woff2", 210_520),
        ("jetbrains-mono-variable.woff2", 113_672),
        ("sigurd-artisan.woff2", 18_812),
    ];

    #[test]
    fn catalog_carries_exactly_the_four_declared_faces() {
        assert_eq!(ALL.len(), 4, "four legacy @font-face families");
        let families: Vec<&str> = ALL.iter().map(|font| font.family).collect();
        assert_eq!(
            families,
            vec![
                "Artisan Neo",
                "Cal Sans",
                "JetBrains Mono",
                "Sigurd Variable"
            ],
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
    fn embedded_bytes_are_woff2_containers() {
        // WOFF2 magic `wOF2` (RFC 9839 §3): catches text-encoding damage
        // (e.g. line-ending conversion) at compile-test time.
        for font in ALL {
            assert!(
                font.bytes.len() > 48,
                "{}: implausibly small for a variable font",
                font.file_name
            );
            assert_eq!(
                &font.bytes[0..4],
                b"wOF2",
                "{}: missing WOFF2 magic; the binary is damaged or substituted",
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
