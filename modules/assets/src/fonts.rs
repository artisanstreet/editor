//! Bundled Artisan Neo, Cal Sans, Spline Sans and Spline Sans Mono
//! TrueType fonts.
//!
//! Files are embedded at compile time and registered once through
//! `artisan_ui::fonts::register_bundled_fonts`. Family names and weight ranges
//! match the source fonts' internal name/fvar tables. Static instances of
//! weights 300–700 plus the conversation-prose weights 410 and 630 are
//! registered because the WGPU text backend matches faces by metadata and
//! does not apply variable weight axes. See `fonts/FONTS.md` for upstream
//! sources, licenses and SHA-256 hashes. TrueType is required by
//! DirectWrite's in-memory loader; WOFF2 is not supported there.

use core::fmt;
use std::borrow::Cow;

/// One vendored typeface: its family identity plus embedded bytes.
#[derive(Clone, Copy, Debug)]
pub struct BundledFont {
    /// File name under `modules/assets/fonts/`.
    pub file_name: &'static str,
    /// Internal family name used by the native text system.
    pub family: &'static str,
    /// Inclusive weight range verified from the font's fvar or OS/2 table.
    pub weights: (u16, u16),
    /// Path of the license or provenance note under `modules/assets/`.
    pub license_path: &'static str,
    /// Embedded license or provenance note contents.
    pub license_text: &'static str,
    /// Unmodified upstream TrueType bytes (provenance in `fonts/FONTS.md`).
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
        file_name: "artisan-neo-600.ttf",
        family: "Artisan Neo",
        weights: (600, 600),
        license_path: "licenses/artisan-neo-OFL.txt",
        license_text: include_str!("../licenses/artisan-neo-OFL.txt"),
        bytes: include_bytes!("../fonts/artisan-neo-600.ttf"),
        static_faces: &[include_bytes!("../fonts/artisan-neo-600.ttf")],
    },
    BundledFont {
        file_name: "cal-sans-700.ttf",
        family: "Cal Sans",
        weights: (700, 700),
        license_path: "licenses/cal-sans-OFL.txt",
        license_text: include_str!("../licenses/cal-sans-OFL.txt"),
        bytes: include_bytes!("../fonts/cal-sans-700.ttf"),
        static_faces: &[include_bytes!("../fonts/cal-sans-700.ttf")],
    },
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
        file_name: "twemoji-mozilla.ttf",
        family: "Twemoji Mozilla",
        weights: (400, 400),
        license_path: "fonts/TWEMOJI.md",
        license_text: include_str!("../fonts/TWEMOJI.md"),
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

#[cfg(test)]
mod tests {
    use super::{ALL, UnknownFont, bundled_fonts, lookup_family};

    /// Expected `(file name, byte length)` pins: any truncation or
    /// re-encode of a vendored binary fails here before it can reach a
    /// renderer and silently fall back to a system face.
    const EXPECTED_LENGTHS: [(&str, usize); 5] = [
        ("artisan-neo-600.ttf", 343_516),
        ("cal-sans-700.ttf", 219_644),
        ("spline-sans-variable.ttf", 146_896),
        ("spline-sans-mono-variable.ttf", 118_744),
        ("twemoji-mozilla.ttf", 1_474_284),
    ];

    #[test]
    fn catalog_carries_declared_text_and_emoji_faces() {
        assert_eq!(ALL.len(), 5, "four text families plus the emoji fallback");
        let families: Vec<&str> = ALL.iter().map(|font| font.family).collect();
        assert_eq!(
            families,
            vec!["Artisan Neo", "Cal Sans", "Spline Sans", "Spline Sans Mono", "Twemoji Mozilla"],
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
        assert_eq!(shaped.len(), 15);
        let expected = ALL.iter().flat_map(|font| font.static_faces.iter());
        for (shaped, bytes) in shaped.iter().zip(expected) {
            assert_eq!(shaped.as_ref(), *bytes);
        }
    }

    #[test]
    fn prose_statics_carry_exact_weight_metadata_and_no_axes() {
        // The conversation-prose faces (body 410, headings 630) are true
        // `instantiateVariableFont` instances from `spline-sans-variable.ttf`,
        // not relabeled 400/600 binaries: outlines differ from their
        // neighbors, OS/2 `usWeightClass` carries the exact prose weight the
        // WGPU matcher selects on, and no `fvar` axis remains. Naming mirrors
        // the neighboring statics (Regular-style linking: nameID 2 stays
        // `Regular`) so style matching keeps working.
        let spline = lookup_family("Spline Sans").expect("bundled Spline Sans");
        assert_eq!(
            spline.static_faces.len(),
            7,
            "300, 400, 410, 500, 600, 630, 700 in ascending order"
        );
        let faces: [(&[u8], usize, u16); 2] = [
            (spline.static_faces[2], 56_792, 410),
            (spline.static_faces[5], 57_008, 630),
        ];
        for (bytes, length, weight) in faces {
            assert_eq!(
                &bytes[0..4],
                b"\x00\x01\x00\x00",
                "weight {weight}: missing TrueType sfnt magic"
            );
            assert_eq!(
                bytes.len(),
                length,
                "weight {weight}: embedded length drift means the binary changed"
            );
            assert_eq!(
                sfnt_weight_class(bytes).expect("OS/2 table with a weight class"),
                weight,
                "weight {weight}: OS/2 metadata must carry the exact prose weight"
            );
            assert!(
                !sfnt_has_table(bytes, b"fvar"),
                "weight {weight}: a remaining fvar axis would defeat static matching"
            );
        }
        // Distinct outlines, not copies: each prose face's `glyf` table
        // differs from both neighbors it sits between. (Full-binary
        // inequality would prove nothing about outlines — name and head
        // metadata alone already differ — so the comparison addresses the
        // outline table directly.)
        for (face, left, right) in [
            (spline.static_faces[2], spline.static_faces[1], spline.static_faces[3]),
            (spline.static_faces[5], spline.static_faces[4], spline.static_faces[6]),
        ] {
            let outline = sfnt_table_bytes(face, b"glyf").expect("glyf table");
            let left_outline = sfnt_table_bytes(left, b"glyf").expect("left glyf");
            let right_outline = sfnt_table_bytes(right, b"glyf").expect("right glyf");
            assert_ne!(outline, left_outline, "outlines must differ from the left neighbor");
            assert_ne!(outline, right_outline, "outlines must differ from the right neighbor");
        }
    }

    /// Reads OS/2 `usWeightClass` (offset 4) without a font parser.
    fn sfnt_weight_class(bytes: &[u8]) -> Option<u16> {
        let table = sfnt_table_bytes(bytes, b"OS/2")?;
        Some(u16::from_be_bytes(table.get(4..6)?.try_into().ok()?))
    }

    /// Slices one table's bytes from an sfnt directory.
    fn sfnt_table_bytes<'bytes>(bytes: &'bytes [u8], tag: &[u8; 4]) -> Option<&'bytes [u8]> {
        let count_bytes = bytes.get(4..6)?;
        let count_bytes: [u8; 2] = count_bytes.try_into().ok()?;
        let count = u16::from_be_bytes(count_bytes) as usize;
        (0..count).find_map(|index| {
            let record = bytes.get(12 + index * 16..12 + index * 16 + 16)?;
            if &record[0..4] != tag {
                return None;
            }
            let offset = u32::from_be_bytes(record[8..12].try_into().ok()?) as usize;
            let length = u32::from_be_bytes(record[12..16].try_into().ok()?) as usize;
            bytes.get(offset..offset + length)
        })
    }
    /// Reports whether the sfnt directory names `tag`.
    fn sfnt_has_table(bytes: &[u8], tag: &[u8; 4]) -> bool {
        sfnt_table_bytes(bytes, tag).is_some()
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
    fn artisan_neo_registers_its_verified_semibold_static_face() {
        // Internal name-table identity `Artisan Neo` / `SemiBold` with OS/2
        // `usWeightClass` 600, instantiated from the variable source with
        // wght pinned at 600 and opsz at its default 14 so no fvar axis
        // remains. The native wordmark requests exactly this family at
        // weight 600 so the platform matcher cannot silently fall back to
        // a system face.
        let font = lookup_family("Artisan Neo").expect("bundled Artisan Neo face");
        assert_eq!(font.file_name, "artisan-neo-600.ttf");
        assert_eq!(font.weights, (600, 600));
        assert_eq!(font.bytes.len(), 343_516);
        assert_eq!(
            &font.bytes[0..4],
            b"\x00\x01\x00\x00",
            "artisan-neo-600.ttf: missing TrueType sfnt magic"
        );
        assert_eq!(
            font.static_faces.len(),
            1,
            "Artisan Neo ships one static SemiBold face"
        );
        assert_eq!(font.static_faces[0], font.bytes);
    }

    #[test]
    fn cal_sans_registers_its_verified_bold_static_face() {
        // Internal name-table identity `Cal Sans` / `Bold` with OS/2
        // `usWeightClass` 700, verified from the vendored bytes (no fvar
        // axis: a single static Bold face). The native wordmark requests
        // exactly this family at weight 700 so the platform matcher cannot
        // silently fall back to a system face.
        let font = lookup_family("Cal Sans").expect("bundled Cal Sans face");
        assert_eq!(font.file_name, "cal-sans-700.ttf");
        assert_eq!(font.weights, (700, 700));
        assert_eq!(font.bytes.len(), 219_644);
        assert_eq!(
            &font.bytes[0..4],
            b"\x00\x01\x00\x00",
            "cal-sans-700.ttf: missing TrueType sfnt magic"
        );
        assert_eq!(
            font.static_faces.len(),
            1,
            "Cal Sans ships one static Bold face"
        );
        assert_eq!(font.static_faces[0], font.bytes);
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
