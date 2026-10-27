//! Sealed catalog of first-party, vendored visual assets.
//!
//! Every static SVG source used by the editor is embedded in this crate.
//! Consumers resolve stable [`AssetId`]s to
//! embedded `&'static str` source bytes and metadata without npm packages,
//! Svelte components, browser runtimes, filesystem access, or any external
//! loader: the only content this crate exposes is checked-in catalog data.
//!
//! [`AssetId`] seals an internal catalog index next to the stable string key.
//! The single `catalog!` invocation below generates one associated constant
//! per asset, the sorted [`ALL`] table, and the parallel
//! [`AssetId::CONSTANTS`]; indices are validated against the id table during
//! constant evaluation, so code outside this crate cannot construct an
//! identifier that does not denote an entry. Dynamic strings reach an id only
//! through [`lookup`] or `FromStr`, which validate before returning an
//! indexed value.

use core::fmt;
use core::str::FromStr;

pub mod file_icons;
pub mod fonts;

/// Stable identifier for a vendored asset.
///
/// The string form is `<family>.<name>` (for example `tabler.check`). The internal index is assigned at
/// compile time from the catalog and can never be invalid.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AssetId {
    /// Position in [`ALL`], fixed by constant evaluation.
    index: usize,
    /// Stable catalog key.
    id: &'static str,
}

impl AssetId {
    /// Seals a catalog literal into an identifier, resolving its index during
    /// constant evaluation.
    ///
    /// # Compile-time-only invariant
    ///
    /// This constructor is private and `const`, and the sole caller is the
    /// `catalog!` expansion below, which passes string *literals*. An unknown
    /// key therefore aborts compilation inside [`locate`]; the panic in
    /// [`locate`] can never observe runtime or external input. Runtime strings
    /// reach an id exclusively through [`lookup`] / `FromStr`, which validate
    /// against the catalog first.
    const fn from_id(id: &'static str) -> Self {
        Self {
            index: locate(id),
            id,
        }
    }

    /// Returns the stable catalog key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.id
    }
}

/// Byte-wise `str` equality for const evaluation.
const fn str_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Whether `a` sorts strictly before `b`, bytewise like `str::cmp`.
const fn str_lt(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut i = 0;
    while i < a.len() && i < b.len() {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
        i += 1;
    }
    a.len() < b.len()
}

// `lookup` binary-searches the catalog: a row out of order would make its
// asset, and others, silently unfindable, so the order fails the build.
const _: () = {
    let mut i = 1;
    while i < CATALOG_IDS.len() {
        assert!(
            str_lt(CATALOG_IDS[i - 1], CATALOG_IDS[i]),
            "catalog! rows must be sorted by id"
        );
        i += 1;
    }
};

/// Resolves a catalog key to its position in [`CATALOG_IDS`].
///
/// # Compile-time-only invariant
///
/// Reachable only from `AssetId::from_id`, which is private, `const`, and
/// called only by the `catalog!` macro with `&'static str` literals. The
/// panic below therefore fires during constant evaluation of a bad literal —
/// a build failure — and can never be triggered by runtime or external input.
#[expect(
    clippy::panic,
    reason = "evaluated at compile time: an unknown id fails the build"
)]
const fn locate(needle: &str) -> usize {
    let mut i = 0;
    while i < CATALOG_IDS.len() {
        if str_eq(CATALOG_IDS[i], needle) {
            return i;
        }
        i += 1;
    }
    panic!("catalog id not found");
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id)
    }
}

impl FromStr for AssetId {
    type Err = UnknownAsset;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        lookup(s).map(|asset| asset.id)
    }
}

/// Source family of a vendored asset.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Family {
    /// First-party Artisan product artwork.
    Artisan,
    /// Brand marks served through legacy `<img>` wrappers.
    Brand,
    /// JetBrains-curated file icons (the svelte mark carries dual provenance).
    Jetbrains,
    /// Lobe Icons marks carried as inline Svelte components upstream.
    Lobe,
    /// Simple Icons path marks vendored by the legacy frontend.
    SimpleIcons,
    /// Logos distributed through the svgl project.
    Svgl,
    /// Tabler icon glyphs.
    Tabler,
}

impl Family {
    /// All families ordered by their id prefixes.
    pub const ALL: [Self; 7] = [
        Self::Artisan,
        Self::Brand,
        Self::Jetbrains,
        Self::Lobe,
        Self::SimpleIcons,
        Self::Svgl,
        Self::Tabler,
    ];

    /// Returns the id prefix used by every asset of this family.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Artisan => "artisan",
            Self::Brand => "brands",
            Self::Jetbrains => "jetbrains",
            Self::Lobe => "lobe",
            Self::SimpleIcons => "simple-icons",
            Self::Svgl => "svgl",
            Self::Tabler => "tabler",
        }
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.prefix())
    }
}

/// How a vendored asset is presented natively.
///
/// This is catalog-owned rendering policy, deliberately independent of
/// [`Asset::monochrome`], which stays the validator-derived artwork property
/// (docs/ui/ASSETS.md §10). Legacy call sites carried a second, different
/// predicate: `EngineMark.monochrome` / `RepositoryMark.monochrome` meant "a
/// single-color logo that must invert with the theme". Artwork-mono agrees
/// with that policy everywhere except the marks whose legacy call sites flag
/// non-inverting while their artwork is single-paint, because alpha-mask
/// tinting would flatten their authored brand colors. Exactly two such marks
/// exist in this catalog and they override the default:
///
/// - `svgl.claude-ai` — Claude clay `#D97757`, served for engine `claude`
///   and provider `anthropic`;
/// - `svgl.deepseek` — `DeepSeek` blue `#4D6BFE`, served for provider
///   `deepseek`.
///
/// Everything else follows the monochrome-derived default. Notably,
/// currentColor artwork (`svgl.qwen`, `lobe.*`, `simple-icons.*`) stays
/// [`Presentation::Tinted`]: painting with text color
/// is the native equivalent of its theme-adaptive or chip-whitened legacy
/// rendering, whereas full-color raster would pin the glyph to black.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Presentation {
    /// Alpha-mask rendering painted in GPUI text color; the asset tints and
    /// inverts with the theme.
    Tinted,
    /// Authored-color rendering over the embedded bytes; nothing recolored.
    FullColor,
}

/// Resolves a catalog row's presentation: the monochrome-derived default,
/// overridden by exactly the brand marks whose legacy call sites proved their
/// authored single-hue colors must survive (see [`Presentation`]).
///
/// Key comparison goes through [`str_eq`] because string-literal match
/// patterns are not yet allowed in const functions.
const fn presentation_policy(id: &str, monochrome: bool) -> Presentation {
    if str_eq(id, "svgl.claude-ai") || str_eq(id, "svgl.deepseek") {
        Presentation::FullColor
    } else if monochrome {
        Presentation::Tinted
    } else {
        Presentation::FullColor
    }
}

/// One vendored SVG asset: metadata plus embedded source text.
#[derive(Clone, Copy, Debug)]
pub struct Asset {
    /// Stable identifier, unique across [`ALL`].
    pub id: AssetId,
    /// Source family.
    pub family: Family,
    /// Root `viewBox` when the source declares one.
    pub view_box: Option<&'static str>,
    /// Whether the artwork's paint derives from a single color (see
    /// `docs/ui/ASSETS.md` §10 for the derivation rule).
    pub monochrome: bool,
    /// Catalog-owned presentation route; see [`Presentation`] for the policy
    /// and its evidenced exceptions to the monochrome default.
    pub presentation: Presentation,
    /// Checked-in location relative to `crates/editor-assets/`.
    pub source_path: &'static str,
    /// Embedded standalone SVG source, byte-identical to the checked-in file.
    pub source: &'static str,
}

/// Width and height parsed from an asset's `viewBox`.
type Dimensions = (f32, f32);

impl Asset {
    /// Intrinsic catalog dimensions, parsed once rather than during painting.
    pub fn intrinsic_dimensions(self) -> Option<Dimensions> {
        static DIMENSIONS: std::sync::OnceLock<Box<[Option<Dimensions>]>> =
            std::sync::OnceLock::new();
        DIMENSIONS.get_or_init(|| {
            ALL.iter()
                .map(|asset| {
                    let mut values = asset.view_box?.split_whitespace();
                    let _: f32 = values.next()?.parse().ok()?;
                    let _: f32 = values.next()?.parse().ok()?;
                    let width: f32 = values.next()?.parse().ok()?;
                    let height: f32 = values.next()?.parse().ok()?;
                    (values.next().is_none()
                        && width.is_finite()
                        && height.is_finite()
                        && width > 0.0
                        && height > 0.0)
                        .then_some((width, height))
                })
                .collect()
        })[self.id.index]
    }
}

macro_rules! catalog {
    ($(($const_name:ident, $id:literal, $family:expr, $view_box:expr, $monochrome:literal, $path:literal)),* $(,)?) => {
        /// Stable id keys in catalog order; backs compile-time index sealing.
        const CATALOG_IDS: &[&str] = &[$($id),*];

        impl AssetId {
            $(
                /// Catalog constant for this asset.
                pub const $const_name: AssetId = AssetId::from_id($id);
            )*

            /// The same ids as [`ALL`], spelled through the constants, in
            /// catalog order; proves total constant/catalog coverage.
            pub const CONSTANTS: &[AssetId] = &[$(AssetId::$const_name),*];
        }

        /// Every vendored asset, sorted by id so lookups can binary search.
        pub const ALL: &[Asset] = &[
            $(
                Asset {
                    id: AssetId::from_id($id),
                    family: $family,
                    view_box: $view_box,
                    monochrome: $monochrome,
                    presentation: presentation_policy($id, $monochrome),
                    source_path: $path,
                    source: include_str!(concat!("../", $path)),
                },
            )*
        ];
    };
}

catalog! {
    (ARTISAN_APP_ICON, "artisan.app-icon", Family::Artisan, Some("0 0 720 720"), false, "svg/artisan/app-icon.svg"),
    (ARTISAN_LOGO_GRADIENT, "artisan.logo-gradient", Family::Artisan, Some("0 0 720 720"), false, "svg/artisan/logo-gradient.svg"),
    (ARTISAN_RAIL_BAG_BODY, "artisan.rail-bag-body", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bag-body.svg"),
    (ARTISAN_RAIL_BAG_HANDLE, "artisan.rail-bag-handle", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bag-handle.svg"),
    (ARTISAN_RAIL_BAG_ITEM_DOT, "artisan.rail-bag-item-dot", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bag-item-dot.svg"),
    (ARTISAN_RAIL_BAG_ITEM_GEM, "artisan.rail-bag-item-gem", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bag-item-gem.svg"),
    (ARTISAN_RAIL_BAG_ITEM_SPARK, "artisan.rail-bag-item-spark", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bag-item-spark.svg"),
    (ARTISAN_RAIL_BARS_C1R1, "artisan.rail-bars-c1r1", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c1r1.svg"),
    (ARTISAN_RAIL_BARS_C1R2, "artisan.rail-bars-c1r2", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c1r2.svg"),
    (ARTISAN_RAIL_BARS_C1R3, "artisan.rail-bars-c1r3", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c1r3.svg"),
    (ARTISAN_RAIL_BARS_C1R4, "artisan.rail-bars-c1r4", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c1r4.svg"),
    (ARTISAN_RAIL_BARS_C1R5, "artisan.rail-bars-c1r5", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c1r5.svg"),
    (ARTISAN_RAIL_BARS_C2R1, "artisan.rail-bars-c2r1", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c2r1.svg"),
    (ARTISAN_RAIL_BARS_C2R2, "artisan.rail-bars-c2r2", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c2r2.svg"),
    (ARTISAN_RAIL_BARS_C2R3, "artisan.rail-bars-c2r3", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c2r3.svg"),
    (ARTISAN_RAIL_BARS_C2R4, "artisan.rail-bars-c2r4", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c2r4.svg"),
    (ARTISAN_RAIL_BARS_C2R5, "artisan.rail-bars-c2r5", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c2r5.svg"),
    (ARTISAN_RAIL_BARS_C3R1, "artisan.rail-bars-c3r1", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c3r1.svg"),
    (ARTISAN_RAIL_BARS_C3R2, "artisan.rail-bars-c3r2", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c3r2.svg"),
    (ARTISAN_RAIL_BARS_C3R3, "artisan.rail-bars-c3r3", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c3r3.svg"),
    (ARTISAN_RAIL_BARS_C3R4, "artisan.rail-bars-c3r4", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c3r4.svg"),
    (ARTISAN_RAIL_BARS_C3R5, "artisan.rail-bars-c3r5", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-bars-c3r5.svg"),
    (ARTISAN_RAIL_PEN_NIB, "artisan.rail-pen-nib", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-pen-nib.svg"),
    (ARTISAN_RAIL_PEN_PAGE, "artisan.rail-pen-page", Family::Artisan, Some("0 0 24 24"), true, "svg/artisan/rail-pen-page.svg"),
    (ARTISAN_STAR, "artisan.star", Family::Artisan, Some("0 0 100 100"), true, "svg/artisan/star.svg"),
    (ARTISAN_STARBURST, "artisan.starburst", Family::Artisan, Some("0 0 40 40"), true, "svg/artisan/starburst.svg"),
    (ARTISAN_SUCCESS_CHECK, "artisan.success-check", Family::Artisan, Some("0 0 16 16"), true, "svg/artisan/success-check.svg"),
    (ARTISAN_WORDMARK_STACKED, "artisan.wordmark-stacked", Family::Artisan, Some("0 0 136.675 225.15"), true, "svg/artisan/wordmark-stacked.svg"),
    (BRANDS_KIMI, "brands.kimi", Family::Brand, Some("0 0 24 25"), false, "svg/brands/kimi.svg"),
    (BRANDS_OPENCODE, "brands.opencode", Family::Brand, Some("128 96 256 320"), false, "svg/brands/opencode.svg"),
    (BRANDS_OPENCODE_LIGHT, "brands.opencode-light", Family::Brand, Some("128 96 256 320"), false, "svg/brands/opencode-light.svg"),
    (BRANDS_REFLECTION, "brands.reflection", Family::Brand, Some("64 64 272 272"), true, "svg/brands/reflection.svg"),
    (BRANDS_ZAI, "brands.zai", Family::Brand, Some("0 0 30 30"), false, "svg/brands/zai.svg"),
    (JETBRAINS_SVELTE, "jetbrains.svelte", Family::Jetbrains, Some("0 0 256 308"), false, "svg/jetbrains/svelte.svg"),
    (JETBRAINS_TEXT, "jetbrains.text", Family::Jetbrains, Some("0 0 16 16"), true, "svg/jetbrains/text.svg"),
    (JETBRAINS_2023_ACTIONSCRIPT, "jetbrains.theme2023-actionscript", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/actionScript.svg"),
    (JETBRAINS_2023_ACTIONSCRIPT_DARK, "jetbrains.theme2023-actionscript_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/actionScript_dark.svg"),
    (JETBRAINS_2023_ANGULARJS, "jetbrains.theme2023-angularjs", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/angularJS.svg"),
    (JETBRAINS_2023_ANYTYPE, "jetbrains.theme2023-anytype", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/anyType.svg"),
    (JETBRAINS_2023_ANYTYPE_DARK, "jetbrains.theme2023-anytype_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/anyType_dark.svg"),
    (JETBRAINS_2023_APPLICATION, "jetbrains.theme2023-application", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/application.svg"),
    (JETBRAINS_2023_APPLICATION_DARK, "jetbrains.theme2023-application_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/application_dark.svg"),
    (JETBRAINS_2023_ARCHIVE, "jetbrains.theme2023-archive", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/archive.svg"),
    (JETBRAINS_2023_ARCHIVE_DARK, "jetbrains.theme2023-archive_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/archive_dark.svg"),
    (JETBRAINS_2023_BEAM, "jetbrains.theme2023-beam", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/beam.svg"),
    (JETBRAINS_2023_BIOMEJS, "jetbrains.theme2023-biomejs", Family::Jetbrains, Some("0 0 64 64"), false, "svg/jetbrains-2023/biomejs.svg"),
    (JETBRAINS_2023_BUN, "jetbrains.theme2023-bun", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/bun.svg"),
    (JETBRAINS_2023_C, "jetbrains.theme2023-c", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/c.svg"),
    (JETBRAINS_2023_C3, "jetbrains.theme2023-c3", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/c3.svg"),
    (JETBRAINS_2023_C3_DARK, "jetbrains.theme2023-c3_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/c3_dark.svg"),
    (JETBRAINS_2023_C3INTERFACE, "jetbrains.theme2023-c3interface", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Interface.svg"),
    (JETBRAINS_2023_C3INTERFACE_DARK, "jetbrains.theme2023-c3interface_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Interface_dark.svg"),
    (JETBRAINS_2023_C3LIBRARY, "jetbrains.theme2023-c3library", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Library.svg"),
    (JETBRAINS_2023_C3LIBRARY_DARK, "jetbrains.theme2023-c3library_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Library_dark.svg"),
    (JETBRAINS_2023_C3TEST, "jetbrains.theme2023-c3test", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Test.svg"),
    (JETBRAINS_2023_C3TEST_DARK, "jetbrains.theme2023-c3test_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Test_dark.svg"),
    (JETBRAINS_2023_C_DARK, "jetbrains.theme2023-c_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/c_dark.svg"),
    (JETBRAINS_2023_CARGO, "jetbrains.theme2023-cargo", Family::Jetbrains, Some("0 0 17 17"), false, "svg/jetbrains-2023/cargo.svg"),
    (JETBRAINS_2023_CARGO_DARK, "jetbrains.theme2023-cargo_dark", Family::Jetbrains, Some("0 0 17 17"), false, "svg/jetbrains-2023/cargo_dark.svg"),
    (JETBRAINS_2023_CARGOLOCK, "jetbrains.theme2023-cargolock", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cargoLock.svg"),
    (JETBRAINS_2023_CARGOLOCK_DARK, "jetbrains.theme2023-cargolock_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cargoLock_dark.svg"),
    (JETBRAINS_2023_CLOJURE, "jetbrains.theme2023-clojure", Family::Jetbrains, Some("0 0 256 256"), false, "svg/jetbrains-2023/clojure.svg"),
    (JETBRAINS_2023_CMAKE, "jetbrains.theme2023-cmake", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/CMake.svg"),
    (JETBRAINS_2023_CMAKE_DARK, "jetbrains.theme2023-cmake_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/CMake_dark.svg"),
    (JETBRAINS_2023_CONFIG, "jetbrains.theme2023-config", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/config.svg"),
    (JETBRAINS_2023_CONFIG_DARK, "jetbrains.theme2023-config_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/config_dark.svg"),
    (JETBRAINS_2023_CPP, "jetbrains.theme2023-cpp", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cpp.svg"),
    (JETBRAINS_2023_CPP_DARK, "jetbrains.theme2023-cpp_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cpp_dark.svg"),
    (JETBRAINS_2023_CS, "jetbrains.theme2023-cs", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cs.svg"),
    (JETBRAINS_2023_CS_DARK, "jetbrains.theme2023-cs_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cs_dark.svg"),
    (JETBRAINS_2023_CSHTML, "jetbrains.theme2023-cshtml", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cshtml.svg"),
    (JETBRAINS_2023_CSHTML_DARK, "jetbrains.theme2023-cshtml_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cshtml_dark.svg"),
    (JETBRAINS_2023_CSPROJ, "jetbrains.theme2023-csproj", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/csproj.svg"),
    (JETBRAINS_2023_CSPROJ_DARK, "jetbrains.theme2023-csproj_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/csproj_dark.svg"),
    (JETBRAINS_2023_CSS, "jetbrains.theme2023-css", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/css.svg"),
    (JETBRAINS_2023_CSV, "jetbrains.theme2023-csv", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/csv.svg"),
    (JETBRAINS_2023_CSV_DARK, "jetbrains.theme2023-csv_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/csv_dark.svg"),
    (JETBRAINS_2023_CU, "jetbrains.theme2023-cu", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cu.svg"),
    (JETBRAINS_2023_CU_DARK, "jetbrains.theme2023-cu_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cu_dark.svg"),
    (JETBRAINS_2023_CUH, "jetbrains.theme2023-cuh", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cuh.svg"),
    (JETBRAINS_2023_CUH_DARK, "jetbrains.theme2023-cuh_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/cuh_dark.svg"),
    (JETBRAINS_2023_DART, "jetbrains.theme2023-dart", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/dart.svg"),
    (JETBRAINS_2023_DART_DARK, "jetbrains.theme2023-dart_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/dart_dark.svg"),
    (JETBRAINS_2023_DBMS, "jetbrains.theme2023-dbms", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/dbms.svg"),
    (JETBRAINS_2023_DBMS_DARK, "jetbrains.theme2023-dbms_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/dbms_dark.svg"),
    (JETBRAINS_2023_DOCKER, "jetbrains.theme2023-docker", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/docker.svg"),
    (JETBRAINS_2023_DOCKER_DARK, "jetbrains.theme2023-docker_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/docker_dark.svg"),
    (JETBRAINS_2023_DUNE, "jetbrains.theme2023-dune", Family::Jetbrains, Some("0 0 24 24"), false, "svg/jetbrains-2023/dune.svg"),
    (JETBRAINS_2023_EDITORCONFIG, "jetbrains.theme2023-editorconfig", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/editorConfig.svg"),
    (JETBRAINS_2023_EDITORCONFIG_DARK, "jetbrains.theme2023-editorconfig_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/editorConfig_dark.svg"),
    (JETBRAINS_2023_EEX, "jetbrains.theme2023-eex", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/eex.svg"),
    (JETBRAINS_2023_EEX_DARK, "jetbrains.theme2023-eex_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/eex_dark.svg"),
    (JETBRAINS_2023_ELIXIR, "jetbrains.theme2023-elixir", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/elixir.svg"),
    (JETBRAINS_2023_ELIXIR_DARK, "jetbrains.theme2023-elixir_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/elixir_dark.svg"),
    (JETBRAINS_2023_ERBFILE, "jetbrains.theme2023-erbfile", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/erbFile.svg"),
    (JETBRAINS_2023_ERBFILE_DARK, "jetbrains.theme2023-erbfile_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/erbFile_dark.svg"),
    (JETBRAINS_2023_ERLANG, "jetbrains.theme2023-erlang", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/erlang.svg"),
    (JETBRAINS_2023_ESLINT, "jetbrains.theme2023-eslint", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/eslint.svg"),
    (JETBRAINS_2023_FOLDER, "jetbrains.theme2023-folder", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folder.svg"),
    (JETBRAINS_2023_FOLDER_DARK, "jetbrains.theme2023-folder_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folder_dark.svg"),
    (JETBRAINS_2023_FOLDERCONTROLLERS, "jetbrains.theme2023-foldercontrollers", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderControllers.svg"),
    (JETBRAINS_2023_FOLDERCONTROLLERS_DARK, "jetbrains.theme2023-foldercontrollers_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderControllers_dark.svg"),
    (JETBRAINS_2023_FOLDERGITHUB, "jetbrains.theme2023-foldergithub", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderGithub.svg"),
    (JETBRAINS_2023_FOLDERGITHUB_DARK, "jetbrains.theme2023-foldergithub_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderGithub_dark.svg"),
    (JETBRAINS_2023_FOLDERHELPERS, "jetbrains.theme2023-folderhelpers", Family::Jetbrains, Some("0 0 17 16"), false, "svg/jetbrains-2023/folderHelpers.svg"),
    (JETBRAINS_2023_FOLDERHELPERS_DARK, "jetbrains.theme2023-folderhelpers_dark", Family::Jetbrains, Some("0 0 17 16"), false, "svg/jetbrains-2023/folderHelpers_dark.svg"),
    (JETBRAINS_2023_FOLDERMAILERS, "jetbrains.theme2023-foldermailers", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderMailers.svg"),
    (JETBRAINS_2023_FOLDERMAILERS_DARK, "jetbrains.theme2023-foldermailers_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderMailers_dark.svg"),
    (JETBRAINS_2023_FOLDERMIGRATIONS, "jetbrains.theme2023-foldermigrations", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderMigrations.svg"),
    (JETBRAINS_2023_FOLDERMIGRATIONS_DARK, "jetbrains.theme2023-foldermigrations_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderMigrations_dark.svg"),
    (JETBRAINS_2023_FOLDERTEST, "jetbrains.theme2023-foldertest", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderTest.svg"),
    (JETBRAINS_2023_FOLDERTEST_DARK, "jetbrains.theme2023-foldertest_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderTest_dark.svg"),
    (JETBRAINS_2023_FOLDERVENDOR, "jetbrains.theme2023-foldervendor", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderVendor.svg"),
    (JETBRAINS_2023_FOLDERVENDOR_DARK, "jetbrains.theme2023-foldervendor_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/folderVendor_dark.svg"),
    (JETBRAINS_2023_FONT, "jetbrains.theme2023-font", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/font.svg"),
    (JETBRAINS_2023_FONT_DARK, "jetbrains.theme2023-font_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/font_dark.svg"),
    (JETBRAINS_2023_GITIGNORE, "jetbrains.theme2023-gitignore", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/gitignore.svg"),
    (JETBRAINS_2023_GLEAM, "jetbrains.theme2023-gleam", Family::Jetbrains, Some("0 0 167 152"), false, "svg/jetbrains-2023/gleam.svg"),
    (JETBRAINS_2023_GLEAM_DARK, "jetbrains.theme2023-gleam_dark", Family::Jetbrains, Some("0 0 167 152"), false, "svg/jetbrains-2023/gleam_dark.svg"),
    (JETBRAINS_2023_GO, "jetbrains.theme2023-go", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/go.svg"),
    (JETBRAINS_2023_GO_DARK, "jetbrains.theme2023-go_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/go_dark.svg"),
    (JETBRAINS_2023_GOMOD, "jetbrains.theme2023-gomod", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/goMod.svg"),
    (JETBRAINS_2023_GOMOD_DARK, "jetbrains.theme2023-gomod_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/goMod_dark.svg"),
    (JETBRAINS_2023_GOSUM, "jetbrains.theme2023-gosum", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/goSum.svg"),
    (JETBRAINS_2023_GOSUM_DARK, "jetbrains.theme2023-gosum_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/goSum_dark.svg"),
    (JETBRAINS_2023_GOWORK, "jetbrains.theme2023-gowork", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/goWork.svg"),
    (JETBRAINS_2023_GOWORK_DARK, "jetbrains.theme2023-gowork_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/goWork_dark.svg"),
    (JETBRAINS_2023_GRAPHQL, "jetbrains.theme2023-graphql", Family::Jetbrains, Some("0 0 32 32"), false, "svg/jetbrains-2023/graphql.svg"),
    (JETBRAINS_2023_H, "jetbrains.theme2023-h", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/h.svg"),
    (JETBRAINS_2023_H_DARK, "jetbrains.theme2023-h_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/h_dark.svg"),
    (JETBRAINS_2023_HANDLEBARSJSON, "jetbrains.theme2023-handlebarsjson", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/handlebarsJson.svg"),
    (JETBRAINS_2023_HANDLEBARSJSON_DARK, "jetbrains.theme2023-handlebarsjson_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/handlebarsJson_dark.svg"),
    (JETBRAINS_2023_HASKELL, "jetbrains.theme2023-haskell", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/haskell.svg"),
    (JETBRAINS_2023_HCL_DARK, "jetbrains.theme2023-hcl_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/hcl_dark.svg"),
    (JETBRAINS_2023_HCL_LIGHT, "jetbrains.theme2023-hcl_light", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/hcl_light.svg"),
    (JETBRAINS_2023_HTML, "jetbrains.theme2023-html", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/html.svg"),
    (JETBRAINS_2023_HTML_DARK, "jetbrains.theme2023-html_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/html_dark.svg"),
    (JETBRAINS_2023_HTTP, "jetbrains.theme2023-http", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/http.svg"),
    (JETBRAINS_2023_HTTP_DARK, "jetbrains.theme2023-http_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/http_dark.svg"),
    (JETBRAINS_2023_IGNORED, "jetbrains.theme2023-ignored", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/ignored.svg"),
    (JETBRAINS_2023_IGNORED_DARK, "jetbrains.theme2023-ignored_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/ignored_dark.svg"),
    (JETBRAINS_2023_IMAGE, "jetbrains.theme2023-image", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/image.svg"),
    (JETBRAINS_2023_IMAGE_DARK, "jetbrains.theme2023-image_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/image_dark.svg"),
    (JETBRAINS_2023_INO, "jetbrains.theme2023-ino", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/ino.svg"),
    (JETBRAINS_2023_INO_DARK, "jetbrains.theme2023-ino_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/ino_dark.svg"),
    (JETBRAINS_2023_JAVA, "jetbrains.theme2023-java", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/java.svg"),
    (JETBRAINS_2023_JAVA_DARK, "jetbrains.theme2023-java_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/java_dark.svg"),
    (JETBRAINS_2023_JAVASCRIPT, "jetbrains.theme2023-javascript", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/javaScript.svg"),
    (JETBRAINS_2023_JAVASCRIPT_DARK, "jetbrains.theme2023-javascript_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/javaScript_dark.svg"),
    (JETBRAINS_2023_JSON, "jetbrains.theme2023-json", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/json.svg"),
    (JETBRAINS_2023_JSON_DARK, "jetbrains.theme2023-json_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/json_dark.svg"),
    (JETBRAINS_2023_JSTEST, "jetbrains.theme2023-jstest", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/jsTest.svg"),
    (JETBRAINS_2023_JSTEST_DARK, "jetbrains.theme2023-jstest_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/jsTest_dark.svg"),
    (JETBRAINS_2023_JSX, "jetbrains.theme2023-jsx", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/jsx.svg"),
    (JETBRAINS_2023_JSX_DARK, "jetbrains.theme2023-jsx_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/jsx_dark.svg"),
    (JETBRAINS_2023_JSXTEST, "jetbrains.theme2023-jsxtest", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/jsxTest.svg"),
    (JETBRAINS_2023_JSXTEST_DARK, "jetbrains.theme2023-jsxtest_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/jsxTest_dark.svg"),
    (JETBRAINS_2023_JUPYTER, "jetbrains.theme2023-jupyter", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/jupyter.svg"),
    (JETBRAINS_2023_KOTLIN, "jetbrains.theme2023-kotlin", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/kotlin.svg"),
    (JETBRAINS_2023_KOTLIN_DARK, "jetbrains.theme2023-kotlin_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/kotlin_dark.svg"),
    (JETBRAINS_2023_KOTLINSCRIPT, "jetbrains.theme2023-kotlinscript", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/kotlinScript.svg"),
    (JETBRAINS_2023_KOTLINSCRIPT_DARK, "jetbrains.theme2023-kotlinscript_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/kotlinScript_dark.svg"),
    (JETBRAINS_2023_LESS, "jetbrains.theme2023-less", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/less.svg"),
    (JETBRAINS_2023_LESS_DARK, "jetbrains.theme2023-less_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/less_dark.svg"),
    (JETBRAINS_2023_LOCK, "jetbrains.theme2023-lock", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/lock.svg"),
    (JETBRAINS_2023_LOCK_DARK, "jetbrains.theme2023-lock_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/lock_dark.svg"),
    (JETBRAINS_2023_LUA, "jetbrains.theme2023-lua", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/lua.svg"),
    (JETBRAINS_2023_MAKEFILE, "jetbrains.theme2023-makefile", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/makefile.svg"),
    (JETBRAINS_2023_MAKEFILE_DARK, "jetbrains.theme2023-makefile_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/makefile_dark.svg"),
    (JETBRAINS_2023_MARKDOWN, "jetbrains.theme2023-markdown", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/markdown.svg"),
    (JETBRAINS_2023_MARKDOWN_DARK, "jetbrains.theme2023-markdown_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/markdown_dark.svg"),
    (JETBRAINS_2023_MDX, "jetbrains.theme2023-mdx", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/mdx.svg"),
    (JETBRAINS_2023_ML, "jetbrains.theme2023-ml", Family::Jetbrains, Some("0 0 24 24"), false, "svg/jetbrains-2023/ml.svg"),
    (JETBRAINS_2023_MLI, "jetbrains.theme2023-mli", Family::Jetbrains, Some("0 0 24 24"), false, "svg/jetbrains-2023/mli.svg"),
    (JETBRAINS_2023_MODULE, "jetbrains.theme2023-module", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/module.svg"),
    (JETBRAINS_2023_MODULE_DARK, "jetbrains.theme2023-module_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/module_dark.svg"),
    (JETBRAINS_2023_NODEJS, "jetbrains.theme2023-nodejs", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/nodejs.svg"),
    (JETBRAINS_2023_NODEJS_DARK, "jetbrains.theme2023-nodejs_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/nodejs_dark.svg"),
    (JETBRAINS_2023_NPM, "jetbrains.theme2023-npm", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/npm.svg"),
    (JETBRAINS_2023_NPM_DARK, "jetbrains.theme2023-npm_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/npm_dark.svg"),
    (JETBRAINS_2023_OPAM, "jetbrains.theme2023-opam", Family::Jetbrains, Some("0 0 24 24"), false, "svg/jetbrains-2023/opam.svg"),
    (JETBRAINS_2023_OPENTOFU, "jetbrains.theme2023-opentofu", Family::Jetbrains, Some("0 0 64 64"), false, "svg/jetbrains-2023/opentofu.svg"),
    (JETBRAINS_2023_PHP, "jetbrains.theme2023-php", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/php.svg"),
    (JETBRAINS_2023_PHP_DARK, "jetbrains.theme2023-php_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/php_dark.svg"),
    (JETBRAINS_2023_PNPM, "jetbrains.theme2023-pnpm", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/pnpm.svg"),
    (JETBRAINS_2023_PNPM_DARK, "jetbrains.theme2023-pnpm_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/pnpm_dark.svg"),
    (JETBRAINS_2023_POSTCSS, "jetbrains.theme2023-postcss", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/postcss.svg"),
    (JETBRAINS_2023_PRETTIER, "jetbrains.theme2023-prettier", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/prettier.svg"),
    (JETBRAINS_2023_PRETTIER_DARK, "jetbrains.theme2023-prettier_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/prettier_dark.svg"),
    (JETBRAINS_2023_PROJECTPROPERTIES, "jetbrains.theme2023-projectproperties", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/projectProperties.svg"),
    (JETBRAINS_2023_PROJECTPROPERTIES_DARK, "jetbrains.theme2023-projectproperties_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/projectProperties_dark.svg"),
    (JETBRAINS_2023_PROPERTIES, "jetbrains.theme2023-properties", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/properties.svg"),
    (JETBRAINS_2023_PROPERTIES_DARK, "jetbrains.theme2023-properties_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/properties_dark.svg"),
    (JETBRAINS_2023_PROTOBUF, "jetbrains.theme2023-protobuf", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/protobuf.svg"),
    (JETBRAINS_2023_PROTOFILE, "jetbrains.theme2023-protofile", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/protoFile.svg"),
    (JETBRAINS_2023_PROTOFILE_DARK, "jetbrains.theme2023-protofile_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/protoFile_dark.svg"),
    (JETBRAINS_2023_PYTHON, "jetbrains.theme2023-python", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/python.svg"),
    (JETBRAINS_2023_RAKETASK, "jetbrains.theme2023-raketask", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/rakeTask.svg"),
    (JETBRAINS_2023_RAKETASK_DARK, "jetbrains.theme2023-raketask_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/rakeTask_dark.svg"),
    (JETBRAINS_2023_REGO, "jetbrains.theme2023-rego", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/rego.svg"),
    (JETBRAINS_2023_RUBY, "jetbrains.theme2023-ruby", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/ruby.svg"),
    (JETBRAINS_2023_RUBY_DARK, "jetbrains.theme2023-ruby_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/ruby_dark.svg"),
    (JETBRAINS_2023_RUBYGEMS, "jetbrains.theme2023-rubygems", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/rubyGems.svg"),
    (JETBRAINS_2023_RUBYGEMS_DARK, "jetbrains.theme2023-rubygems_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/rubyGems_dark.svg"),
    (JETBRAINS_2023_RUSTFILE, "jetbrains.theme2023-rustfile", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/rustFile.svg"),
    (JETBRAINS_2023_RUSTFILE_DARK, "jetbrains.theme2023-rustfile_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/rustFile_dark.svg"),
    (JETBRAINS_2023_SCALA, "jetbrains.theme2023-scala", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/scala.svg"),
    (JETBRAINS_2023_SCALA_DARK, "jetbrains.theme2023-scala_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/scala_dark.svg"),
    (JETBRAINS_2023_SCSS, "jetbrains.theme2023-scss", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/scss.svg"),
    (JETBRAINS_2023_SHELL, "jetbrains.theme2023-shell", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/shell.svg"),
    (JETBRAINS_2023_SHELL_DARK, "jetbrains.theme2023-shell_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/shell_dark.svg"),
    (JETBRAINS_2023_SLIM, "jetbrains.theme2023-slim", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/slim.svg"),
    (JETBRAINS_2023_SLIM_DARK, "jetbrains.theme2023-slim_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/slim_dark.svg"),
    (JETBRAINS_2023_SOLUTION, "jetbrains.theme2023-solution", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/solution.svg"),
    (JETBRAINS_2023_SOLUTION_DARK, "jetbrains.theme2023-solution_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/solution_dark.svg"),
    (JETBRAINS_2023_SQL, "jetbrains.theme2023-sql", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/sql.svg"),
    (JETBRAINS_2023_SQL_DARK, "jetbrains.theme2023-sql_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/sql_dark.svg"),
    (JETBRAINS_2023_SVELTE, "jetbrains.theme2023-svelte", Family::Jetbrains, Some("0 0 98.2 118"), false, "svg/jetbrains-2023/svelte.svg"),
    (JETBRAINS_2023_SWIFT, "jetbrains.theme2023-swift", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/swift.svg"),
    (JETBRAINS_2023_TAILWIND, "jetbrains.theme2023-tailwind", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/tailwind.svg"),
    (JETBRAINS_2023_TERRAFORM, "jetbrains.theme2023-terraform", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/terraform.svg"),
    (JETBRAINS_2023_TERRAFORM_DARK, "jetbrains.theme2023-terraform_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/terraform_dark.svg"),
    (JETBRAINS_2023_TEXT, "jetbrains.theme2023-text", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/text.svg"),
    (JETBRAINS_2023_TEXT_DARK, "jetbrains.theme2023-text_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/text_dark.svg"),
    (JETBRAINS_2023_TOML, "jetbrains.theme2023-toml", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/toml.svg"),
    (JETBRAINS_2023_TOML_DARK, "jetbrains.theme2023-toml_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/toml_dark.svg"),
    (JETBRAINS_2023_TSTEST, "jetbrains.theme2023-tstest", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/tsTest.svg"),
    (JETBRAINS_2023_TSTEST_DARK, "jetbrains.theme2023-tstest_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/tsTest_dark.svg"),
    (JETBRAINS_2023_TSX, "jetbrains.theme2023-tsx", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/tsx.svg"),
    (JETBRAINS_2023_TSX_DARK, "jetbrains.theme2023-tsx_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/tsx_dark.svg"),
    (JETBRAINS_2023_TSXTEST, "jetbrains.theme2023-tsxtest", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/tsxTest.svg"),
    (JETBRAINS_2023_TSXTEST_DARK, "jetbrains.theme2023-tsxtest_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/tsxTest_dark.svg"),
    (JETBRAINS_2023_TYPESCRIPT, "jetbrains.theme2023-typescript", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/typeScript.svg"),
    (JETBRAINS_2023_TYPESCRIPT_DARK, "jetbrains.theme2023-typescript_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/typeScript_dark.svg"),
    (JETBRAINS_2023_VERCEL, "jetbrains.theme2023-vercel", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/vercel.svg"),
    (JETBRAINS_2023_VITE, "jetbrains.theme2023-vite", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/vite.svg"),
    (JETBRAINS_2023_VITE_DARK, "jetbrains.theme2023-vite_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/vite_dark.svg"),
    (JETBRAINS_2023_VLANG, "jetbrains.theme2023-vlang", Family::Jetbrains, Some("0 0 32 32"), false, "svg/jetbrains-2023/vlang.svg"),
    (JETBRAINS_2023_VLANG_DARK, "jetbrains.theme2023-vlang_dark", Family::Jetbrains, Some("0 0 32 32"), false, "svg/jetbrains-2023/vlang_dark.svg"),
    (JETBRAINS_2023_VUEJS, "jetbrains.theme2023-vuejs", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/vueJs.svg"),
    (JETBRAINS_2023_XML, "jetbrains.theme2023-xml", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/xml.svg"),
    (JETBRAINS_2023_XML_DARK, "jetbrains.theme2023-xml_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/xml_dark.svg"),
    (JETBRAINS_2023_YAML, "jetbrains.theme2023-yaml", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/yaml.svg"),
    (JETBRAINS_2023_YAML_DARK, "jetbrains.theme2023-yaml_dark", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/yaml_dark.svg"),
    (JETBRAINS_2023_YARN, "jetbrains.theme2023-yarn", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/yarn.svg"),
    (JETBRAINS_2023_ZIG, "jetbrains.theme2023-zig", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains-2023/zig.svg"),
    (JETBRAINS_TS_TEST, "jetbrains.ts-test", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains/ts-test.svg"),
    (JETBRAINS_TYPESCRIPT, "jetbrains.typescript", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains/typescript.svg"),
    (LOBE_GOOGLE, "lobe.google", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/google.svg"),
    (LOBE_MINIMAX, "lobe.minimax", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/minimax.svg"),
    (LOBE_MISTRAL, "lobe.mistral", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/mistral.svg"),
    (LOBE_NVIDIA, "lobe.nvidia", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/nvidia.svg"),
    (LOBE_STEPFUN, "lobe.stepfun", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/stepfun.svg"),
    (LOBE_TENCENT, "lobe.tencent", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/tencent.svg"),
    (LOBE_XAI, "lobe.xai", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/xai.svg"),
    (LOBE_XIAOMI, "lobe.xiaomi", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/xiaomi.svg"),
    (SIMPLE_ICONS_BITBUCKET, "simple-icons.bitbucket", Family::SimpleIcons, Some("0 0 24 24"), true, "svg/simple-icons/bitbucket.svg"),
    (SIMPLE_ICONS_CODEBERG, "simple-icons.codeberg", Family::SimpleIcons, Some("0 0 24 24"), true, "svg/simple-icons/codeberg.svg"),
    (SIMPLE_ICONS_GITEA, "simple-icons.gitea", Family::SimpleIcons, Some("0 0 24 24"), true, "svg/simple-icons/gitea.svg"),
    (SIMPLE_ICONS_MEITUAN, "simple-icons.meituan", Family::SimpleIcons, Some("0 0 24 24"), true, "svg/simple-icons/meituan.svg"),
    (SIMPLE_ICONS_SOURCEHUT, "simple-icons.sourcehut", Family::SimpleIcons, Some("0 0 24 24"), true, "svg/simple-icons/sourcehut.svg"),
    (SIMPLE_ICONS_XIAOMI, "simple-icons.xiaomi", Family::SimpleIcons, Some("0 0 24 24"), true, "svg/simple-icons/xiaomi.svg"),
    (SVGL_CLAUDE_AI, "svgl.claude-ai", Family::Svgl, Some("0 0 256 257"), true, "svg/svgl/claude-ai.svg"),
    (SVGL_DEEPSEEK, "svgl.deepseek", Family::Svgl, Some("0 0 24 24"), true, "svg/svgl/deepseek.svg"),
    (SVGL_GEMINI, "svgl.gemini", Family::Svgl, Some("0 0 296 298"), false, "svg/svgl/gemini.svg"),
    (SVGL_GIT, "svgl.git", Family::Svgl, Some("0 0 256 256"), true, "svg/svgl/git.svg"),
    (SVGL_GITHUB, "svgl.github", Family::Svgl, Some("0 0 1024 1024"), true, "svg/svgl/github.svg"),
    (SVGL_GITLAB, "svgl.gitlab", Family::Svgl, Some("0 0 32 32"), false, "svg/svgl/gitlab.svg"),
    (SVGL_META, "svgl.meta", Family::Svgl, Some("0 0 256 171"), false, "svg/svgl/meta.svg"),
    (SVGL_MICROSOFT_AZURE, "svgl.microsoft-azure", Family::Svgl, Some("0 0 96 96"), false, "svg/svgl/microsoft-azure.svg"),
    (SVGL_OPENAI, "svgl.openai", Family::Svgl, Some("0 0 256 260"), true, "svg/svgl/openai.svg"),
    (SVGL_QWEN, "svgl.qwen", Family::Svgl, Some("0 0 24 24"), true, "svg/svgl/qwen.svg"),
    (TABLER_ALERT_TRIANGLE, "tabler.alert-triangle", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/alert-triangle.svg"),
    (TABLER_ARCHIVE, "tabler.archive", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/archive.svg"),
    (TABLER_ARROW_BAR_TO_DOWN, "tabler.arrow-bar-to-down", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrow-bar-to-down.svg"),
    (TABLER_ARROW_NARROW_LEFT, "tabler.arrow-narrow-left", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrow-narrow-left.svg"),
    (TABLER_ARROW_RIGHT, "tabler.arrow-right", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrow-right.svg"),
    (TABLER_ARROW_UP, "tabler.arrow-up", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrow-up.svg"),
    (TABLER_ARROWS_HORIZONTAL, "tabler.arrows-horizontal", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrows-horizontal.svg"),
    (TABLER_ARROWS_MINIMIZE, "tabler.arrows-minimize", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrows-minimize.svg"),
    (TABLER_BELL, "tabler.bell", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/bell.svg"),
    (TABLER_BELL_EXCLAMATION, "tabler.bell-exclamation", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/bell-exclamation.svg"),
    (TABLER_BOLT_FILLED, "tabler.bolt-filled", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/bolt-filled.svg"),
    (TABLER_BOT_ID, "tabler.bot-id", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/bot-id.svg"),
    (TABLER_BRAIN, "tabler.brain", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/brain.svg"),
    (TABLER_BRAND_VISUAL_STUDIO, "tabler.brand-visual-studio", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/brand-visual-studio.svg"),
    (TABLER_BUG, "tabler.bug", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/bug.svg"),
    (TABLER_CHART_COLUMN, "tabler.chart-column", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/chart-column.svg"),
    (TABLER_CHECK, "tabler.check", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/check.svg"),
    (TABLER_CHECKLIST, "tabler.checklist", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/checklist.svg"),
    (TABLER_CHEVRON_DOWN, "tabler.chevron-down", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/chevron-down.svg"),
    (TABLER_CHEVRON_LEFT, "tabler.chevron-left", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/chevron-left.svg"),
    (TABLER_CHEVRON_RIGHT, "tabler.chevron-right", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/chevron-right.svg"),
    (TABLER_CHEVRON_UP, "tabler.chevron-up", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/chevron-up.svg"),
    (TABLER_CIRCLE_CHECK, "tabler.circle-check", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/circle-check.svg"),
    (TABLER_CIRCLE_X, "tabler.circle-x", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/circle-x.svg"),
    (TABLER_CODE, "tabler.code", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/code.svg"),
    (TABLER_COPY, "tabler.copy", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/copy.svg"),
    (TABLER_CORNER_DOWN_LEFT, "tabler.corner-down-left", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/corner-down-left.svg"),
    (TABLER_DEVICE_LAPTOP, "tabler.device-laptop", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/device-laptop.svg"),
    (TABLER_DOWNLOAD, "tabler.download", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/download.svg"),
    (TABLER_EDIT, "tabler.edit", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/edit.svg"),
    (TABLER_EYE, "tabler.eye", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/eye.svg"),
    (TABLER_FILE_DIFF, "tabler.file-diff", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-diff.svg"),
    (TABLER_FILE_OFF, "tabler.file-off", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-off.svg"),
    (TABLER_FILE_PENCIL, "tabler.file-pencil", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-pencil.svg"),
    (TABLER_FILE_SEARCH, "tabler.file-search", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-search.svg"),
    (TABLER_FILE_SPARK, "tabler.file-spark", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-spark.svg"),
    (TABLER_FILE_TEXT, "tabler.file-text", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-text.svg"),
    (TABLER_FILE_X, "tabler.file-x", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-x.svg"),
    (TABLER_FOLDER, "tabler.folder", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/folder.svg"),
    (TABLER_FOLDER_CODE, "tabler.folder-code", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/folder-code.svg"),
    (TABLER_FOLDER_OPEN, "tabler.folder-open", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/folder-open.svg"),
    (TABLER_FOLDER_PLUS, "tabler.folder-plus", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/folder-plus.svg"),
    (TABLER_GIT_BRANCH, "tabler.git-branch", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/git-branch.svg"),
    (TABLER_LIST_DETAILS, "tabler.list-details", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/list-details.svg"),
    (TABLER_LOADER_2, "tabler.loader-2", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/loader-2.svg"),
    (TABLER_LOCK, "tabler.lock", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/lock.svg"),
    (TABLER_LOGIN, "tabler.login", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/login.svg"),
    (TABLER_MAXIMIZE, "tabler.maximize", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/maximize.svg"),
    (TABLER_MESSAGE_CIRCLE, "tabler.message-circle", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/message-circle.svg"),
    (TABLER_MESSAGE_PLUS, "tabler.message-plus", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/message-plus.svg"),
    (TABLER_MESSAGES, "tabler.messages", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/messages.svg"),
    (TABLER_MINUS, "tabler.minus", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/minus.svg"),
    (TABLER_MOON, "tabler.moon", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/moon.svg"),
    (TABLER_PALETTE, "tabler.palette", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/palette.svg"),
    (TABLER_PENCIL, "tabler.pencil", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/pencil.svg"),
    (TABLER_PLAYER_PAUSE_FILLED, "tabler.player-pause-filled", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/player-pause-filled.svg"),
    (TABLER_PLAYER_PLAY, "tabler.player-play", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/player-play.svg"),
    (TABLER_PLAYER_PLAY_FILLED, "tabler.player-play-filled", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/player-play-filled.svg"),
    (TABLER_PLAYER_STOP_FILLED, "tabler.player-stop-filled", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/player-stop-filled.svg"),
    (TABLER_PLAYER_TRACK_NEXT, "tabler.player-track-next", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/player-track-next.svg"),
    (TABLER_PLAYER_TRACK_PREV, "tabler.player-track-prev", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/player-track-prev.svg"),
    (TABLER_PLUG_CONNECTED, "tabler.plug-connected", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/plug-connected.svg"),
    (TABLER_QUESTION_MARK, "tabler.question-mark", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/question-mark.svg"),
    (TABLER_REFRESH, "tabler.refresh", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/refresh.svg"),
    (TABLER_ROTATE_CLOCKWISE, "tabler.rotate-clockwise", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/rotate-clockwise.svg"),
    (TABLER_RUN_SPRINT, "tabler.run-sprint", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/run-sprint.svg"),
    (TABLER_SEARCH, "tabler.search", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/search.svg"),
    (TABLER_SELECTOR, "tabler.selector", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/selector.svg"),
    (TABLER_SETTINGS, "tabler.settings", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/settings.svg"),
    (TABLER_SHIELD_LOCK, "tabler.shield-lock", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/shield-lock.svg"),
    (TABLER_SHOPPING_BAG, "tabler.shopping-bag", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/shopping-bag.svg"),
    (TABLER_SITEMAP, "tabler.sitemap", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/sitemap.svg"),
    (TABLER_SPARKLES, "tabler.sparkles", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/sparkles.svg"),
    (TABLER_STAR, "tabler.star", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/star.svg"),
    (TABLER_STAR_FILLED, "tabler.star-filled", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/star-filled.svg"),
    (TABLER_SUN, "tabler.sun", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/sun.svg"),
    (TABLER_TARGET_ARROW, "tabler.target-arrow", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/target-arrow.svg"),
    (TABLER_TERMINAL_2, "tabler.terminal-2", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/terminal-2.svg"),
    (TABLER_TEST_PIPE, "tabler.test-pipe", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/test-pipe.svg"),
    (TABLER_TOOL, "tabler.tool", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/tool.svg"),
    (TABLER_TRASH, "tabler.trash", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/trash.svg"),
    (TABLER_WORLD, "tabler.world", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/world.svg"),
    (TABLER_WORLD_SEARCH, "tabler.world-search", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/world-search.svg"),
    (TABLER_X, "tabler.x", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/x.svg"),
    (TABLER_ZOOM_IN, "tabler.zoom-in", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/zoom-in.svg"),
    (TABLER_ZOOM_OUT, "tabler.zoom-out", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/zoom-out.svg"),
}

/// An embedded license or attribution document.
#[derive(Clone, Copy, Debug)]
pub struct LicenseFile {
    /// Path relative to `crates/editor-assets/`.
    pub path: &'static str,
    /// Embedded document contents.
    pub contents: &'static str,
}

/// Failure returned when a string does not name a cataloged asset.
///
/// First-party by hand rather than through a derive crate so this catalog
/// stays dependency-free.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownAsset {
    /// The rejected identifier.
    pub id: String,
}

impl fmt::Display for UnknownAsset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown asset id `{}`", self.id)
    }
}

impl std::error::Error for UnknownAsset {}

/// Embedded license and attribution material, ordered by path.
pub const LICENSE_FILES: &[LicenseFile] = &[
    LicenseFile {
        path: "licenses/artisan-first-party-NOTES.md",
        contents: include_str!("../licenses/artisan-first-party-NOTES.md"),
    },
    LicenseFile {
        path: "licenses/jetbrains-2023-LICENSE.md",
        contents: include_str!("../licenses/jetbrains-2023-LICENSE.md"),
    },
    LicenseFile {
        path: "licenses/jetbrains-file-icons-README.md",
        contents: include_str!("../licenses/jetbrains-file-icons-README.md"),
    },
    LicenseFile {
        path: "licenses/lobe-icons-MIT.txt",
        contents: include_str!("../licenses/lobe-icons-MIT.txt"),
    },
    LicenseFile {
        path: "licenses/simple-icons-CC0.txt",
        contents: include_str!("../licenses/simple-icons-CC0.txt"),
    },
    LicenseFile {
        path: "licenses/simple-icons-NOTES.md",
        contents: include_str!("../licenses/simple-icons-NOTES.md"),
    },
    LicenseFile {
        path: "licenses/svgl-api-evidence.json",
        contents: include_str!("../licenses/svgl-api-evidence.json"),
    },
    LicenseFile {
        path: "licenses/svgl-brand-marks-NOTES.md",
        contents: include_str!("../licenses/svgl-brand-marks-NOTES.md"),
    },
    LicenseFile {
        path: "licenses/svgl-svelte-LICENSE.txt",
        contents: include_str!("../licenses/svgl-svelte-LICENSE.txt"),
    },
    LicenseFile {
        path: "licenses/tabler-MIT.txt",
        contents: include_str!("../licenses/tabler-MIT.txt"),
    },
];

/// Returns the cataloged asset for `id`.
///
/// Infallible by construction: the index sealed inside [`AssetId`] is fixed
/// during constant evaluation against this catalog, so this is a direct
/// lookup with no failure mode and no invariant panic.
#[must_use]
pub const fn get(id: AssetId) -> &'static Asset {
    &ALL[id.index]
}

/// Resolves a dynamic string to its cataloged asset.
///
/// Deterministic: [`ALL`] is sorted by key (checked at compile time), so this is a
/// plain binary search over the stable string keys.
///
/// # Errors
///
/// Returns [`UnknownAsset`] when `id` does not name a vendored asset.
pub fn lookup(id: &str) -> Result<&'static Asset, UnknownAsset> {
    ALL.binary_search_by(|probe| probe.id.as_str().cmp(id))
        .map_or_else(
            |_| {
                Err(UnknownAsset {
                    id: String::from(id),
                })
            },
            |index| Ok(&ALL[index]),
        )
}

/// Returns the embedded license/attribution document at `path`.
#[must_use]
pub fn license(path: &str) -> Option<&'static LicenseFile> {
    LICENSE_FILES.iter().find(|file| file.path == path)
}
