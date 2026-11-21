use core::fmt;
use core::str::FromStr;

pub mod art;
pub mod file_icons;
pub mod fonts;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AssetId {
    index: usize,
    id: &'static str,
}

impl AssetId {
    const fn from_id(id: &'static str) -> Self {
        Self {
            index: locate(id),
            id,
        }
    }

    #[must_use]
    const fn as_str(self) -> &'static str {
        self.id
    }
}

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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Presentation {
    Tinted,
    FullColor,
}

const fn presentation_policy(id: &str, monochrome: bool) -> Presentation {
    if str_eq(id, "svgl.claude-ai") || str_eq(id, "svgl.deepseek") {
        Presentation::FullColor
    } else if monochrome {
        Presentation::Tinted
    } else {
        Presentation::FullColor
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Asset {
    pub id: AssetId,
    pub view_box: Option<&'static str>,
    pub presentation: Presentation,
    pub source: &'static str,
}

type Dimensions = (f32, f32);

impl Asset {
    pub fn intrinsic_dimensions(self) -> Option<Dimensions> {
        static DIMENSIONS: std::sync::OnceLock<Box<[Option<Dimensions>]>> =
            std::sync::OnceLock::new();
        DIMENSIONS.get_or_init(|| {
            ALL.iter()
                .map(|asset| {
                    asset
                        .view_box
                        .map(|view_box| parse_view_box(asset.id, view_box))
                })
                .collect()
        })[self.id.index]
    }
}

fn parse_view_box(id: AssetId, view_box: &str) -> Dimensions {
    let values = view_box
        .split_whitespace()
        .map(str::parse::<f32>)
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| {
            panic!(
                "invariant: asset {id} has view box {view_box:?} with a non-numeric value: {error}"
            )
        });
    match values.as_slice() {
        &[_, _, width, height]
            if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 =>
        {
            (width, height)
        }
        _ => panic!(
            "invariant: asset {id} has view box {view_box:?}, not four numbers with a positive size"
        ),
    }
}

macro_rules! catalog {
    ($(($const_name:ident, $id:literal, $view_box:expr, $monochrome:literal, $path:literal)),* $(,)?) => {
        const CATALOG_IDS: &[&str] = &[$($id),*];

        impl AssetId {
            $(
                pub const $const_name: AssetId = AssetId::from_id($id);
            )*
        }

        pub const ALL: &[Asset] = &[
            $(
                Asset {
                    id: AssetId::from_id($id),
                    view_box: $view_box,
                    presentation: presentation_policy($id, $monochrome),
                    source: include_str!(concat!("../", $path)),
                },
            )*
        ];
    };
}

catalog! {
    (ARTISAN_RAIL_BAG_BODY, "artisan.rail-bag-body", Some("0 0 24 24"), true, "svg/artisan/rail-bag-body.svg"),
    (ARTISAN_RAIL_BAG_HANDLE, "artisan.rail-bag-handle", Some("0 0 24 24"), true, "svg/artisan/rail-bag-handle.svg"),
    (ARTISAN_RAIL_BAG_ITEM_DOT, "artisan.rail-bag-item-dot", Some("0 0 24 24"), true, "svg/artisan/rail-bag-item-dot.svg"),
    (ARTISAN_RAIL_BAG_ITEM_GEM, "artisan.rail-bag-item-gem", Some("0 0 24 24"), true, "svg/artisan/rail-bag-item-gem.svg"),
    (ARTISAN_RAIL_BAG_ITEM_SPARK, "artisan.rail-bag-item-spark", Some("0 0 24 24"), true, "svg/artisan/rail-bag-item-spark.svg"),
    (ARTISAN_RAIL_BARS_C1R1, "artisan.rail-bars-c1r1", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c1r1.svg"),
    (ARTISAN_RAIL_BARS_C1R2, "artisan.rail-bars-c1r2", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c1r2.svg"),
    (ARTISAN_RAIL_BARS_C1R3, "artisan.rail-bars-c1r3", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c1r3.svg"),
    (ARTISAN_RAIL_BARS_C1R4, "artisan.rail-bars-c1r4", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c1r4.svg"),
    (ARTISAN_RAIL_BARS_C1R5, "artisan.rail-bars-c1r5", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c1r5.svg"),
    (ARTISAN_RAIL_BARS_C2R1, "artisan.rail-bars-c2r1", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c2r1.svg"),
    (ARTISAN_RAIL_BARS_C2R2, "artisan.rail-bars-c2r2", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c2r2.svg"),
    (ARTISAN_RAIL_BARS_C2R3, "artisan.rail-bars-c2r3", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c2r3.svg"),
    (ARTISAN_RAIL_BARS_C2R4, "artisan.rail-bars-c2r4", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c2r4.svg"),
    (ARTISAN_RAIL_BARS_C2R5, "artisan.rail-bars-c2r5", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c2r5.svg"),
    (ARTISAN_RAIL_BARS_C3R1, "artisan.rail-bars-c3r1", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c3r1.svg"),
    (ARTISAN_RAIL_BARS_C3R2, "artisan.rail-bars-c3r2", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c3r2.svg"),
    (ARTISAN_RAIL_BARS_C3R3, "artisan.rail-bars-c3r3", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c3r3.svg"),
    (ARTISAN_RAIL_BARS_C3R4, "artisan.rail-bars-c3r4", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c3r4.svg"),
    (ARTISAN_RAIL_BARS_C3R5, "artisan.rail-bars-c3r5", Some("0 0 24 24"), true, "svg/artisan/rail-bars-c3r5.svg"),
    (ARTISAN_RAIL_PEN_NIB, "artisan.rail-pen-nib", Some("0 0 24 24"), true, "svg/artisan/rail-pen-nib.svg"),
    (ARTISAN_RAIL_PEN_PAGE, "artisan.rail-pen-page", Some("0 0 24 24"), true, "svg/artisan/rail-pen-page.svg"),
    (ARTISAN_STARBURST, "artisan.starburst", Some("0 0 40 40"), true, "svg/artisan/starburst.svg"),
    (ARTISAN_WORDMARK_STACKED, "artisan.wordmark-stacked", Some("0 0 136.675 225.15"), true, "svg/artisan/wordmark-stacked.svg"),
    (BRANDS_KIMI, "brands.kimi", Some("0 0 24 25"), false, "svg/brands/kimi.svg"),
    (BRANDS_OPENCODE, "brands.opencode", Some("128 96 256 320"), false, "svg/brands/opencode.svg"),
    (BRANDS_OPENCODE_LIGHT, "brands.opencode-light", Some("128 96 256 320"), false, "svg/brands/opencode-light.svg"),
    (BRANDS_REFLECTION, "brands.reflection", Some("64 64 272 272"), true, "svg/brands/reflection.svg"),
    (BRANDS_ZAI, "brands.zai", Some("0 0 30 30"), false, "svg/brands/zai.svg"),
    (JETBRAINS_2023_ACTIONSCRIPT, "jetbrains.theme2023-actionscript", Some("0 0 16 16"), false, "svg/jetbrains-2023/actionScript.svg"),
    (JETBRAINS_2023_ACTIONSCRIPT_DARK, "jetbrains.theme2023-actionscript_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/actionScript_dark.svg"),
    (JETBRAINS_2023_ANGULARJS, "jetbrains.theme2023-angularjs", Some("0 0 16 16"), false, "svg/jetbrains-2023/angularJS.svg"),
    (JETBRAINS_2023_ANYTYPE, "jetbrains.theme2023-anytype", Some("0 0 16 16"), false, "svg/jetbrains-2023/anyType.svg"),
    (JETBRAINS_2023_ANYTYPE_DARK, "jetbrains.theme2023-anytype_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/anyType_dark.svg"),
    (JETBRAINS_2023_APPLICATION, "jetbrains.theme2023-application", Some("0 0 16 16"), false, "svg/jetbrains-2023/application.svg"),
    (JETBRAINS_2023_APPLICATION_DARK, "jetbrains.theme2023-application_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/application_dark.svg"),
    (JETBRAINS_2023_ARCHIVE, "jetbrains.theme2023-archive", Some("0 0 16 16"), false, "svg/jetbrains-2023/archive.svg"),
    (JETBRAINS_2023_ARCHIVE_DARK, "jetbrains.theme2023-archive_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/archive_dark.svg"),
    (JETBRAINS_2023_BEAM, "jetbrains.theme2023-beam", Some("0 0 16 16"), false, "svg/jetbrains-2023/beam.svg"),
    (JETBRAINS_2023_BIOMEJS, "jetbrains.theme2023-biomejs", Some("0 0 64 64"), false, "svg/jetbrains-2023/biomejs.svg"),
    (JETBRAINS_2023_BUN, "jetbrains.theme2023-bun", Some("0 0 16 16"), false, "svg/jetbrains-2023/bun.svg"),
    (JETBRAINS_2023_C, "jetbrains.theme2023-c", Some("0 0 16 16"), false, "svg/jetbrains-2023/c.svg"),
    (JETBRAINS_2023_C3, "jetbrains.theme2023-c3", Some("0 0 16 16"), false, "svg/jetbrains-2023/c3.svg"),
    (JETBRAINS_2023_C3_DARK, "jetbrains.theme2023-c3_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/c3_dark.svg"),
    (JETBRAINS_2023_C3INTERFACE, "jetbrains.theme2023-c3interface", Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Interface.svg"),
    (JETBRAINS_2023_C3INTERFACE_DARK, "jetbrains.theme2023-c3interface_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Interface_dark.svg"),
    (JETBRAINS_2023_C3LIBRARY, "jetbrains.theme2023-c3library", Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Library.svg"),
    (JETBRAINS_2023_C3LIBRARY_DARK, "jetbrains.theme2023-c3library_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Library_dark.svg"),
    (JETBRAINS_2023_C3TEST, "jetbrains.theme2023-c3test", Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Test.svg"),
    (JETBRAINS_2023_C3TEST_DARK, "jetbrains.theme2023-c3test_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/c3Test_dark.svg"),
    (JETBRAINS_2023_C_DARK, "jetbrains.theme2023-c_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/c_dark.svg"),
    (JETBRAINS_2023_CARGO, "jetbrains.theme2023-cargo", Some("0 0 17 17"), false, "svg/jetbrains-2023/cargo.svg"),
    (JETBRAINS_2023_CARGO_DARK, "jetbrains.theme2023-cargo_dark", Some("0 0 17 17"), false, "svg/jetbrains-2023/cargo_dark.svg"),
    (JETBRAINS_2023_CARGOLOCK, "jetbrains.theme2023-cargolock", Some("0 0 16 16"), false, "svg/jetbrains-2023/cargoLock.svg"),
    (JETBRAINS_2023_CARGOLOCK_DARK, "jetbrains.theme2023-cargolock_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/cargoLock_dark.svg"),
    (JETBRAINS_2023_CLOJURE, "jetbrains.theme2023-clojure", Some("0 0 256 256"), false, "svg/jetbrains-2023/clojure.svg"),
    (JETBRAINS_2023_CMAKE, "jetbrains.theme2023-cmake", Some("0 0 16 16"), false, "svg/jetbrains-2023/CMake.svg"),
    (JETBRAINS_2023_CMAKE_DARK, "jetbrains.theme2023-cmake_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/CMake_dark.svg"),
    (JETBRAINS_2023_CONFIG, "jetbrains.theme2023-config", Some("0 0 16 16"), false, "svg/jetbrains-2023/config.svg"),
    (JETBRAINS_2023_CONFIG_DARK, "jetbrains.theme2023-config_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/config_dark.svg"),
    (JETBRAINS_2023_CPP, "jetbrains.theme2023-cpp", Some("0 0 16 16"), false, "svg/jetbrains-2023/cpp.svg"),
    (JETBRAINS_2023_CPP_DARK, "jetbrains.theme2023-cpp_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/cpp_dark.svg"),
    (JETBRAINS_2023_CS, "jetbrains.theme2023-cs", Some("0 0 16 16"), false, "svg/jetbrains-2023/cs.svg"),
    (JETBRAINS_2023_CS_DARK, "jetbrains.theme2023-cs_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/cs_dark.svg"),
    (JETBRAINS_2023_CSHTML, "jetbrains.theme2023-cshtml", Some("0 0 16 16"), false, "svg/jetbrains-2023/cshtml.svg"),
    (JETBRAINS_2023_CSHTML_DARK, "jetbrains.theme2023-cshtml_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/cshtml_dark.svg"),
    (JETBRAINS_2023_CSPROJ, "jetbrains.theme2023-csproj", Some("0 0 16 16"), false, "svg/jetbrains-2023/csproj.svg"),
    (JETBRAINS_2023_CSPROJ_DARK, "jetbrains.theme2023-csproj_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/csproj_dark.svg"),
    (JETBRAINS_2023_CSS, "jetbrains.theme2023-css", Some("0 0 16 16"), false, "svg/jetbrains-2023/css.svg"),
    (JETBRAINS_2023_CSV, "jetbrains.theme2023-csv", Some("0 0 16 16"), false, "svg/jetbrains-2023/csv.svg"),
    (JETBRAINS_2023_CSV_DARK, "jetbrains.theme2023-csv_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/csv_dark.svg"),
    (JETBRAINS_2023_CU, "jetbrains.theme2023-cu", Some("0 0 16 16"), false, "svg/jetbrains-2023/cu.svg"),
    (JETBRAINS_2023_CU_DARK, "jetbrains.theme2023-cu_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/cu_dark.svg"),
    (JETBRAINS_2023_CUH, "jetbrains.theme2023-cuh", Some("0 0 16 16"), false, "svg/jetbrains-2023/cuh.svg"),
    (JETBRAINS_2023_CUH_DARK, "jetbrains.theme2023-cuh_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/cuh_dark.svg"),
    (JETBRAINS_2023_DART, "jetbrains.theme2023-dart", Some("0 0 16 16"), false, "svg/jetbrains-2023/dart.svg"),
    (JETBRAINS_2023_DART_DARK, "jetbrains.theme2023-dart_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/dart_dark.svg"),
    (JETBRAINS_2023_DBMS, "jetbrains.theme2023-dbms", Some("0 0 16 16"), false, "svg/jetbrains-2023/dbms.svg"),
    (JETBRAINS_2023_DBMS_DARK, "jetbrains.theme2023-dbms_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/dbms_dark.svg"),
    (JETBRAINS_2023_DOCKER, "jetbrains.theme2023-docker", Some("0 0 16 16"), false, "svg/jetbrains-2023/docker.svg"),
    (JETBRAINS_2023_DOCKER_DARK, "jetbrains.theme2023-docker_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/docker_dark.svg"),
    (JETBRAINS_2023_DUNE, "jetbrains.theme2023-dune", Some("0 0 24 24"), false, "svg/jetbrains-2023/dune.svg"),
    (JETBRAINS_2023_EDITORCONFIG, "jetbrains.theme2023-editorconfig", Some("0 0 16 16"), false, "svg/jetbrains-2023/editorConfig.svg"),
    (JETBRAINS_2023_EDITORCONFIG_DARK, "jetbrains.theme2023-editorconfig_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/editorConfig_dark.svg"),
    (JETBRAINS_2023_EEX, "jetbrains.theme2023-eex", Some("0 0 16 16"), false, "svg/jetbrains-2023/eex.svg"),
    (JETBRAINS_2023_EEX_DARK, "jetbrains.theme2023-eex_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/eex_dark.svg"),
    (JETBRAINS_2023_ELIXIR, "jetbrains.theme2023-elixir", Some("0 0 16 16"), false, "svg/jetbrains-2023/elixir.svg"),
    (JETBRAINS_2023_ELIXIR_DARK, "jetbrains.theme2023-elixir_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/elixir_dark.svg"),
    (JETBRAINS_2023_ERBFILE, "jetbrains.theme2023-erbfile", Some("0 0 16 16"), false, "svg/jetbrains-2023/erbFile.svg"),
    (JETBRAINS_2023_ERBFILE_DARK, "jetbrains.theme2023-erbfile_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/erbFile_dark.svg"),
    (JETBRAINS_2023_ERLANG, "jetbrains.theme2023-erlang", Some("0 0 16 16"), false, "svg/jetbrains-2023/erlang.svg"),
    (JETBRAINS_2023_ESLINT, "jetbrains.theme2023-eslint", Some("0 0 16 16"), false, "svg/jetbrains-2023/eslint.svg"),
    (JETBRAINS_2023_FOLDER, "jetbrains.theme2023-folder", Some("0 0 16 16"), false, "svg/jetbrains-2023/folder.svg"),
    (JETBRAINS_2023_FOLDER_DARK, "jetbrains.theme2023-folder_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/folder_dark.svg"),
    (JETBRAINS_2023_FOLDERCONTROLLERS, "jetbrains.theme2023-foldercontrollers", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderControllers.svg"),
    (JETBRAINS_2023_FOLDERCONTROLLERS_DARK, "jetbrains.theme2023-foldercontrollers_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderControllers_dark.svg"),
    (JETBRAINS_2023_FOLDERGITHUB, "jetbrains.theme2023-foldergithub", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderGithub.svg"),
    (JETBRAINS_2023_FOLDERGITHUB_DARK, "jetbrains.theme2023-foldergithub_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderGithub_dark.svg"),
    (JETBRAINS_2023_FOLDERHELPERS, "jetbrains.theme2023-folderhelpers", Some("0 0 17 16"), false, "svg/jetbrains-2023/folderHelpers.svg"),
    (JETBRAINS_2023_FOLDERHELPERS_DARK, "jetbrains.theme2023-folderhelpers_dark", Some("0 0 17 16"), false, "svg/jetbrains-2023/folderHelpers_dark.svg"),
    (JETBRAINS_2023_FOLDERMAILERS, "jetbrains.theme2023-foldermailers", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderMailers.svg"),
    (JETBRAINS_2023_FOLDERMAILERS_DARK, "jetbrains.theme2023-foldermailers_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderMailers_dark.svg"),
    (JETBRAINS_2023_FOLDERMIGRATIONS, "jetbrains.theme2023-foldermigrations", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderMigrations.svg"),
    (JETBRAINS_2023_FOLDERMIGRATIONS_DARK, "jetbrains.theme2023-foldermigrations_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderMigrations_dark.svg"),
    (JETBRAINS_2023_FOLDERTEST, "jetbrains.theme2023-foldertest", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderTest.svg"),
    (JETBRAINS_2023_FOLDERTEST_DARK, "jetbrains.theme2023-foldertest_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderTest_dark.svg"),
    (JETBRAINS_2023_FOLDERVENDOR, "jetbrains.theme2023-foldervendor", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderVendor.svg"),
    (JETBRAINS_2023_FOLDERVENDOR_DARK, "jetbrains.theme2023-foldervendor_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/folderVendor_dark.svg"),
    (JETBRAINS_2023_FONT, "jetbrains.theme2023-font", Some("0 0 16 16"), false, "svg/jetbrains-2023/font.svg"),
    (JETBRAINS_2023_FONT_DARK, "jetbrains.theme2023-font_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/font_dark.svg"),
    (JETBRAINS_2023_GITIGNORE, "jetbrains.theme2023-gitignore", Some("0 0 16 16"), false, "svg/jetbrains-2023/gitignore.svg"),
    (JETBRAINS_2023_GLEAM, "jetbrains.theme2023-gleam", Some("0 0 167 152"), false, "svg/jetbrains-2023/gleam.svg"),
    (JETBRAINS_2023_GLEAM_DARK, "jetbrains.theme2023-gleam_dark", Some("0 0 167 152"), false, "svg/jetbrains-2023/gleam_dark.svg"),
    (JETBRAINS_2023_GO, "jetbrains.theme2023-go", Some("0 0 16 16"), false, "svg/jetbrains-2023/go.svg"),
    (JETBRAINS_2023_GO_DARK, "jetbrains.theme2023-go_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/go_dark.svg"),
    (JETBRAINS_2023_GOMOD, "jetbrains.theme2023-gomod", Some("0 0 16 16"), false, "svg/jetbrains-2023/goMod.svg"),
    (JETBRAINS_2023_GOMOD_DARK, "jetbrains.theme2023-gomod_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/goMod_dark.svg"),
    (JETBRAINS_2023_GOSUM, "jetbrains.theme2023-gosum", Some("0 0 16 16"), false, "svg/jetbrains-2023/goSum.svg"),
    (JETBRAINS_2023_GOSUM_DARK, "jetbrains.theme2023-gosum_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/goSum_dark.svg"),
    (JETBRAINS_2023_GOWORK, "jetbrains.theme2023-gowork", Some("0 0 16 16"), false, "svg/jetbrains-2023/goWork.svg"),
    (JETBRAINS_2023_GOWORK_DARK, "jetbrains.theme2023-gowork_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/goWork_dark.svg"),
    (JETBRAINS_2023_GRAPHQL, "jetbrains.theme2023-graphql", Some("0 0 32 32"), false, "svg/jetbrains-2023/graphql.svg"),
    (JETBRAINS_2023_H, "jetbrains.theme2023-h", Some("0 0 16 16"), false, "svg/jetbrains-2023/h.svg"),
    (JETBRAINS_2023_H_DARK, "jetbrains.theme2023-h_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/h_dark.svg"),
    (JETBRAINS_2023_HANDLEBARSJSON, "jetbrains.theme2023-handlebarsjson", Some("0 0 16 16"), false, "svg/jetbrains-2023/handlebarsJson.svg"),
    (JETBRAINS_2023_HANDLEBARSJSON_DARK, "jetbrains.theme2023-handlebarsjson_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/handlebarsJson_dark.svg"),
    (JETBRAINS_2023_HASKELL, "jetbrains.theme2023-haskell", Some("0 0 16 16"), false, "svg/jetbrains-2023/haskell.svg"),
    (JETBRAINS_2023_HCL_DARK, "jetbrains.theme2023-hcl_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/hcl_dark.svg"),
    (JETBRAINS_2023_HCL_LIGHT, "jetbrains.theme2023-hcl_light", Some("0 0 16 16"), false, "svg/jetbrains-2023/hcl_light.svg"),
    (JETBRAINS_2023_HTML, "jetbrains.theme2023-html", Some("0 0 16 16"), false, "svg/jetbrains-2023/html.svg"),
    (JETBRAINS_2023_HTML_DARK, "jetbrains.theme2023-html_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/html_dark.svg"),
    (JETBRAINS_2023_HTTP, "jetbrains.theme2023-http", Some("0 0 16 16"), false, "svg/jetbrains-2023/http.svg"),
    (JETBRAINS_2023_HTTP_DARK, "jetbrains.theme2023-http_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/http_dark.svg"),
    (JETBRAINS_2023_IGNORED, "jetbrains.theme2023-ignored", Some("0 0 16 16"), false, "svg/jetbrains-2023/ignored.svg"),
    (JETBRAINS_2023_IGNORED_DARK, "jetbrains.theme2023-ignored_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/ignored_dark.svg"),
    (JETBRAINS_2023_IMAGE, "jetbrains.theme2023-image", Some("0 0 16 16"), false, "svg/jetbrains-2023/image.svg"),
    (JETBRAINS_2023_IMAGE_DARK, "jetbrains.theme2023-image_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/image_dark.svg"),
    (JETBRAINS_2023_INO, "jetbrains.theme2023-ino", Some("0 0 16 16"), false, "svg/jetbrains-2023/ino.svg"),
    (JETBRAINS_2023_INO_DARK, "jetbrains.theme2023-ino_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/ino_dark.svg"),
    (JETBRAINS_2023_JAVA, "jetbrains.theme2023-java", Some("0 0 16 16"), false, "svg/jetbrains-2023/java.svg"),
    (JETBRAINS_2023_JAVA_DARK, "jetbrains.theme2023-java_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/java_dark.svg"),
    (JETBRAINS_2023_JAVASCRIPT, "jetbrains.theme2023-javascript", Some("0 0 16 16"), false, "svg/jetbrains-2023/javaScript.svg"),
    (JETBRAINS_2023_JAVASCRIPT_DARK, "jetbrains.theme2023-javascript_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/javaScript_dark.svg"),
    (JETBRAINS_2023_JSON, "jetbrains.theme2023-json", Some("0 0 16 16"), false, "svg/jetbrains-2023/json.svg"),
    (JETBRAINS_2023_JSON_DARK, "jetbrains.theme2023-json_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/json_dark.svg"),
    (JETBRAINS_2023_JSTEST, "jetbrains.theme2023-jstest", Some("0 0 16 16"), false, "svg/jetbrains-2023/jsTest.svg"),
    (JETBRAINS_2023_JSTEST_DARK, "jetbrains.theme2023-jstest_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/jsTest_dark.svg"),
    (JETBRAINS_2023_JSX, "jetbrains.theme2023-jsx", Some("0 0 16 16"), false, "svg/jetbrains-2023/jsx.svg"),
    (JETBRAINS_2023_JSX_DARK, "jetbrains.theme2023-jsx_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/jsx_dark.svg"),
    (JETBRAINS_2023_JSXTEST, "jetbrains.theme2023-jsxtest", Some("0 0 16 16"), false, "svg/jetbrains-2023/jsxTest.svg"),
    (JETBRAINS_2023_JSXTEST_DARK, "jetbrains.theme2023-jsxtest_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/jsxTest_dark.svg"),
    (JETBRAINS_2023_JUPYTER, "jetbrains.theme2023-jupyter", Some("0 0 16 16"), false, "svg/jetbrains-2023/jupyter.svg"),
    (JETBRAINS_2023_KOTLIN, "jetbrains.theme2023-kotlin", Some("0 0 16 16"), false, "svg/jetbrains-2023/kotlin.svg"),
    (JETBRAINS_2023_KOTLIN_DARK, "jetbrains.theme2023-kotlin_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/kotlin_dark.svg"),
    (JETBRAINS_2023_KOTLINSCRIPT, "jetbrains.theme2023-kotlinscript", Some("0 0 16 16"), false, "svg/jetbrains-2023/kotlinScript.svg"),
    (JETBRAINS_2023_KOTLINSCRIPT_DARK, "jetbrains.theme2023-kotlinscript_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/kotlinScript_dark.svg"),
    (JETBRAINS_2023_LESS, "jetbrains.theme2023-less", Some("0 0 16 16"), false, "svg/jetbrains-2023/less.svg"),
    (JETBRAINS_2023_LESS_DARK, "jetbrains.theme2023-less_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/less_dark.svg"),
    (JETBRAINS_2023_LOCK, "jetbrains.theme2023-lock", Some("0 0 16 16"), false, "svg/jetbrains-2023/lock.svg"),
    (JETBRAINS_2023_LOCK_DARK, "jetbrains.theme2023-lock_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/lock_dark.svg"),
    (JETBRAINS_2023_LUA, "jetbrains.theme2023-lua", Some("0 0 16 16"), false, "svg/jetbrains-2023/lua.svg"),
    (JETBRAINS_2023_MAKEFILE, "jetbrains.theme2023-makefile", Some("0 0 16 16"), false, "svg/jetbrains-2023/makefile.svg"),
    (JETBRAINS_2023_MAKEFILE_DARK, "jetbrains.theme2023-makefile_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/makefile_dark.svg"),
    (JETBRAINS_2023_MARKDOWN, "jetbrains.theme2023-markdown", Some("0 0 16 16"), false, "svg/jetbrains-2023/markdown.svg"),
    (JETBRAINS_2023_MARKDOWN_DARK, "jetbrains.theme2023-markdown_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/markdown_dark.svg"),
    (JETBRAINS_2023_MDX, "jetbrains.theme2023-mdx", Some("0 0 16 16"), false, "svg/jetbrains-2023/mdx.svg"),
    (JETBRAINS_2023_ML, "jetbrains.theme2023-ml", Some("0 0 24 24"), false, "svg/jetbrains-2023/ml.svg"),
    (JETBRAINS_2023_MLI, "jetbrains.theme2023-mli", Some("0 0 24 24"), false, "svg/jetbrains-2023/mli.svg"),
    (JETBRAINS_2023_MODULE, "jetbrains.theme2023-module", Some("0 0 16 16"), false, "svg/jetbrains-2023/module.svg"),
    (JETBRAINS_2023_MODULE_DARK, "jetbrains.theme2023-module_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/module_dark.svg"),
    (JETBRAINS_2023_NODEJS, "jetbrains.theme2023-nodejs", Some("0 0 16 16"), false, "svg/jetbrains-2023/nodejs.svg"),
    (JETBRAINS_2023_NODEJS_DARK, "jetbrains.theme2023-nodejs_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/nodejs_dark.svg"),
    (JETBRAINS_2023_NPM, "jetbrains.theme2023-npm", Some("0 0 16 16"), false, "svg/jetbrains-2023/npm.svg"),
    (JETBRAINS_2023_NPM_DARK, "jetbrains.theme2023-npm_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/npm_dark.svg"),
    (JETBRAINS_2023_OPAM, "jetbrains.theme2023-opam", Some("0 0 24 24"), false, "svg/jetbrains-2023/opam.svg"),
    (JETBRAINS_2023_OPENTOFU, "jetbrains.theme2023-opentofu", Some("0 0 64 64"), false, "svg/jetbrains-2023/opentofu.svg"),
    (JETBRAINS_2023_PHP, "jetbrains.theme2023-php", Some("0 0 16 16"), false, "svg/jetbrains-2023/php.svg"),
    (JETBRAINS_2023_PHP_DARK, "jetbrains.theme2023-php_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/php_dark.svg"),
    (JETBRAINS_2023_PNPM, "jetbrains.theme2023-pnpm", Some("0 0 16 16"), false, "svg/jetbrains-2023/pnpm.svg"),
    (JETBRAINS_2023_PNPM_DARK, "jetbrains.theme2023-pnpm_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/pnpm_dark.svg"),
    (JETBRAINS_2023_POSTCSS, "jetbrains.theme2023-postcss", Some("0 0 16 16"), false, "svg/jetbrains-2023/postcss.svg"),
    (JETBRAINS_2023_PRETTIER, "jetbrains.theme2023-prettier", Some("0 0 16 16"), false, "svg/jetbrains-2023/prettier.svg"),
    (JETBRAINS_2023_PRETTIER_DARK, "jetbrains.theme2023-prettier_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/prettier_dark.svg"),
    (JETBRAINS_2023_PROJECTPROPERTIES, "jetbrains.theme2023-projectproperties", Some("0 0 16 16"), false, "svg/jetbrains-2023/projectProperties.svg"),
    (JETBRAINS_2023_PROJECTPROPERTIES_DARK, "jetbrains.theme2023-projectproperties_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/projectProperties_dark.svg"),
    (JETBRAINS_2023_PROPERTIES, "jetbrains.theme2023-properties", Some("0 0 16 16"), false, "svg/jetbrains-2023/properties.svg"),
    (JETBRAINS_2023_PROPERTIES_DARK, "jetbrains.theme2023-properties_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/properties_dark.svg"),
    (JETBRAINS_2023_PROTOFILE, "jetbrains.theme2023-protofile", Some("0 0 16 16"), false, "svg/jetbrains-2023/protoFile.svg"),
    (JETBRAINS_2023_PROTOFILE_DARK, "jetbrains.theme2023-protofile_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/protoFile_dark.svg"),
    (JETBRAINS_2023_PYTHON, "jetbrains.theme2023-python", Some("0 0 16 16"), false, "svg/jetbrains-2023/python.svg"),
    (JETBRAINS_2023_RAKETASK, "jetbrains.theme2023-raketask", Some("0 0 16 16"), false, "svg/jetbrains-2023/rakeTask.svg"),
    (JETBRAINS_2023_RAKETASK_DARK, "jetbrains.theme2023-raketask_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/rakeTask_dark.svg"),
    (JETBRAINS_2023_REGO, "jetbrains.theme2023-rego", Some("0 0 16 16"), false, "svg/jetbrains-2023/rego.svg"),
    (JETBRAINS_2023_RUBY, "jetbrains.theme2023-ruby", Some("0 0 16 16"), false, "svg/jetbrains-2023/ruby.svg"),
    (JETBRAINS_2023_RUBY_DARK, "jetbrains.theme2023-ruby_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/ruby_dark.svg"),
    (JETBRAINS_2023_RUBYGEMS, "jetbrains.theme2023-rubygems", Some("0 0 16 16"), false, "svg/jetbrains-2023/rubyGems.svg"),
    (JETBRAINS_2023_RUBYGEMS_DARK, "jetbrains.theme2023-rubygems_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/rubyGems_dark.svg"),
    (JETBRAINS_2023_RUSTFILE, "jetbrains.theme2023-rustfile", Some("0 0 16 16"), false, "svg/jetbrains-2023/rustFile.svg"),
    (JETBRAINS_2023_RUSTFILE_DARK, "jetbrains.theme2023-rustfile_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/rustFile_dark.svg"),
    (JETBRAINS_2023_SCALA, "jetbrains.theme2023-scala", Some("0 0 16 16"), false, "svg/jetbrains-2023/scala.svg"),
    (JETBRAINS_2023_SCALA_DARK, "jetbrains.theme2023-scala_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/scala_dark.svg"),
    (JETBRAINS_2023_SCSS, "jetbrains.theme2023-scss", Some("0 0 16 16"), false, "svg/jetbrains-2023/scss.svg"),
    (JETBRAINS_2023_SHELL, "jetbrains.theme2023-shell", Some("0 0 16 16"), false, "svg/jetbrains-2023/shell.svg"),
    (JETBRAINS_2023_SHELL_DARK, "jetbrains.theme2023-shell_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/shell_dark.svg"),
    (JETBRAINS_2023_SLIM, "jetbrains.theme2023-slim", Some("0 0 16 16"), false, "svg/jetbrains-2023/slim.svg"),
    (JETBRAINS_2023_SLIM_DARK, "jetbrains.theme2023-slim_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/slim_dark.svg"),
    (JETBRAINS_2023_SOLUTION, "jetbrains.theme2023-solution", Some("0 0 16 16"), false, "svg/jetbrains-2023/solution.svg"),
    (JETBRAINS_2023_SOLUTION_DARK, "jetbrains.theme2023-solution_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/solution_dark.svg"),
    (JETBRAINS_2023_SQL, "jetbrains.theme2023-sql", Some("0 0 16 16"), false, "svg/jetbrains-2023/sql.svg"),
    (JETBRAINS_2023_SQL_DARK, "jetbrains.theme2023-sql_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/sql_dark.svg"),
    (JETBRAINS_2023_SVELTE, "jetbrains.theme2023-svelte", Some("0 0 98.2 118"), false, "svg/jetbrains-2023/svelte.svg"),
    (JETBRAINS_2023_SWIFT, "jetbrains.theme2023-swift", Some("0 0 16 16"), false, "svg/jetbrains-2023/swift.svg"),
    (JETBRAINS_2023_TAILWIND, "jetbrains.theme2023-tailwind", Some("0 0 16 16"), false, "svg/jetbrains-2023/tailwind.svg"),
    (JETBRAINS_2023_TERRAFORM, "jetbrains.theme2023-terraform", Some("0 0 16 16"), false, "svg/jetbrains-2023/terraform.svg"),
    (JETBRAINS_2023_TERRAFORM_DARK, "jetbrains.theme2023-terraform_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/terraform_dark.svg"),
    (JETBRAINS_2023_TEXT, "jetbrains.theme2023-text", Some("0 0 16 16"), false, "svg/jetbrains-2023/text.svg"),
    (JETBRAINS_2023_TEXT_DARK, "jetbrains.theme2023-text_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/text_dark.svg"),
    (JETBRAINS_2023_TOML, "jetbrains.theme2023-toml", Some("0 0 16 16"), false, "svg/jetbrains-2023/toml.svg"),
    (JETBRAINS_2023_TOML_DARK, "jetbrains.theme2023-toml_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/toml_dark.svg"),
    (JETBRAINS_2023_TSTEST, "jetbrains.theme2023-tstest", Some("0 0 16 16"), false, "svg/jetbrains-2023/tsTest.svg"),
    (JETBRAINS_2023_TSTEST_DARK, "jetbrains.theme2023-tstest_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/tsTest_dark.svg"),
    (JETBRAINS_2023_TSX, "jetbrains.theme2023-tsx", Some("0 0 16 16"), false, "svg/jetbrains-2023/tsx.svg"),
    (JETBRAINS_2023_TSX_DARK, "jetbrains.theme2023-tsx_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/tsx_dark.svg"),
    (JETBRAINS_2023_TSXTEST, "jetbrains.theme2023-tsxtest", Some("0 0 16 16"), false, "svg/jetbrains-2023/tsxTest.svg"),
    (JETBRAINS_2023_TSXTEST_DARK, "jetbrains.theme2023-tsxtest_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/tsxTest_dark.svg"),
    (JETBRAINS_2023_TYPESCRIPT, "jetbrains.theme2023-typescript", Some("0 0 16 16"), false, "svg/jetbrains-2023/typeScript.svg"),
    (JETBRAINS_2023_TYPESCRIPT_DARK, "jetbrains.theme2023-typescript_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/typeScript_dark.svg"),
    (JETBRAINS_2023_VERCEL, "jetbrains.theme2023-vercel", Some("0 0 16 16"), false, "svg/jetbrains-2023/vercel.svg"),
    (JETBRAINS_2023_VITE, "jetbrains.theme2023-vite", Some("0 0 16 16"), false, "svg/jetbrains-2023/vite.svg"),
    (JETBRAINS_2023_VITE_DARK, "jetbrains.theme2023-vite_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/vite_dark.svg"),
    (JETBRAINS_2023_VLANG, "jetbrains.theme2023-vlang", Some("0 0 32 32"), false, "svg/jetbrains-2023/vlang.svg"),
    (JETBRAINS_2023_VLANG_DARK, "jetbrains.theme2023-vlang_dark", Some("0 0 32 32"), false, "svg/jetbrains-2023/vlang_dark.svg"),
    (JETBRAINS_2023_VUEJS, "jetbrains.theme2023-vuejs", Some("0 0 16 16"), false, "svg/jetbrains-2023/vueJs.svg"),
    (JETBRAINS_2023_XML, "jetbrains.theme2023-xml", Some("0 0 16 16"), false, "svg/jetbrains-2023/xml.svg"),
    (JETBRAINS_2023_XML_DARK, "jetbrains.theme2023-xml_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/xml_dark.svg"),
    (JETBRAINS_2023_YAML, "jetbrains.theme2023-yaml", Some("0 0 16 16"), false, "svg/jetbrains-2023/yaml.svg"),
    (JETBRAINS_2023_YAML_DARK, "jetbrains.theme2023-yaml_dark", Some("0 0 16 16"), false, "svg/jetbrains-2023/yaml_dark.svg"),
    (JETBRAINS_2023_YARN, "jetbrains.theme2023-yarn", Some("0 0 16 16"), false, "svg/jetbrains-2023/yarn.svg"),
    (JETBRAINS_2023_ZIG, "jetbrains.theme2023-zig", Some("0 0 16 16"), false, "svg/jetbrains-2023/zig.svg"),
    (LOBE_GOOGLE, "lobe.google", Some("0 0 24 24"), true, "svg/lobe/google.svg"),
    (LOBE_MINIMAX, "lobe.minimax", Some("0 0 24 24"), true, "svg/lobe/minimax.svg"),
    (LOBE_MISTRAL, "lobe.mistral", Some("0 0 24 24"), true, "svg/lobe/mistral.svg"),
    (LOBE_NVIDIA, "lobe.nvidia", Some("0 0 24 24"), true, "svg/lobe/nvidia.svg"),
    (LOBE_STEPFUN, "lobe.stepfun", Some("0 0 24 24"), true, "svg/lobe/stepfun.svg"),
    (LOBE_TENCENT, "lobe.tencent", Some("0 0 24 24"), true, "svg/lobe/tencent.svg"),
    (LOBE_XAI, "lobe.xai", Some("0 0 24 24"), true, "svg/lobe/xai.svg"),
    (SIMPLE_ICONS_BITBUCKET, "simple-icons.bitbucket", Some("0 0 24 24"), true, "svg/simple-icons/bitbucket.svg"),
    (SIMPLE_ICONS_CODEBERG, "simple-icons.codeberg", Some("0 0 24 24"), true, "svg/simple-icons/codeberg.svg"),
    (SIMPLE_ICONS_GITEA, "simple-icons.gitea", Some("0 0 24 24"), true, "svg/simple-icons/gitea.svg"),
    (SIMPLE_ICONS_MEITUAN, "simple-icons.meituan", Some("0 0 24 24"), true, "svg/simple-icons/meituan.svg"),
    (SIMPLE_ICONS_SOURCEHUT, "simple-icons.sourcehut", Some("0 0 24 24"), true, "svg/simple-icons/sourcehut.svg"),
    (SIMPLE_ICONS_XIAOMI, "simple-icons.xiaomi", Some("0 0 24 24"), true, "svg/simple-icons/xiaomi.svg"),
    (SVGL_CLAUDE_AI, "svgl.claude-ai", Some("0 0 256 257"), true, "svg/svgl/claude-ai.svg"),
    (SVGL_DEEPSEEK, "svgl.deepseek", Some("0 0 24 24"), true, "svg/svgl/deepseek.svg"),
    (SVGL_GIT, "svgl.git", Some("0 0 256 256"), true, "svg/svgl/git.svg"),
    (SVGL_GITHUB, "svgl.github", Some("0 0 1024 1024"), true, "svg/svgl/github.svg"),
    (SVGL_GITLAB, "svgl.gitlab", Some("0 0 32 32"), false, "svg/svgl/gitlab.svg"),
    (SVGL_META, "svgl.meta", Some("0 0 256 171"), false, "svg/svgl/meta.svg"),
    (SVGL_MICROSOFT_AZURE, "svgl.microsoft-azure", Some("0 0 96 96"), false, "svg/svgl/microsoft-azure.svg"),
    (SVGL_OPENAI, "svgl.openai", Some("0 0 256 260"), true, "svg/svgl/openai.svg"),
    (SVGL_QWEN, "svgl.qwen", Some("0 0 24 24"), true, "svg/svgl/qwen.svg"),
    (TABLER_ALERT_TRIANGLE, "tabler.alert-triangle", Some("0 0 24 24"), true, "svg/tabler/alert-triangle.svg"),
    (TABLER_ARCHIVE, "tabler.archive", Some("0 0 24 24"), true, "svg/tabler/archive.svg"),
    (TABLER_ARROW_BAR_TO_DOWN, "tabler.arrow-bar-to-down", Some("0 0 24 24"), true, "svg/tabler/arrow-bar-to-down.svg"),
    (TABLER_ARROW_RIGHT, "tabler.arrow-right", Some("0 0 24 24"), true, "svg/tabler/arrow-right.svg"),
    (TABLER_ARROW_UP, "tabler.arrow-up", Some("0 0 24 24"), true, "svg/tabler/arrow-up.svg"),
    (TABLER_ARROWS_HORIZONTAL, "tabler.arrows-horizontal", Some("0 0 24 24"), true, "svg/tabler/arrows-horizontal.svg"),
    (TABLER_BELL, "tabler.bell", Some("0 0 24 24"), true, "svg/tabler/bell.svg"),
    (TABLER_BELL_EXCLAMATION, "tabler.bell-exclamation", Some("0 0 24 24"), true, "svg/tabler/bell-exclamation.svg"),
    (TABLER_BOLT_FILLED, "tabler.bolt-filled", Some("0 0 24 24"), true, "svg/tabler/bolt-filled.svg"),
    (TABLER_BOT_ID, "tabler.bot-id", Some("0 0 24 24"), true, "svg/tabler/bot-id.svg"),
    (TABLER_BRAIN, "tabler.brain", Some("0 0 24 24"), true, "svg/tabler/brain.svg"),
    (TABLER_CHECK, "tabler.check", Some("0 0 24 24"), true, "svg/tabler/check.svg"),
    (TABLER_CHEVRON_DOWN, "tabler.chevron-down", Some("0 0 24 24"), true, "svg/tabler/chevron-down.svg"),
    (TABLER_CHEVRON_LEFT, "tabler.chevron-left", Some("0 0 24 24"), true, "svg/tabler/chevron-left.svg"),
    (TABLER_CHEVRON_RIGHT, "tabler.chevron-right", Some("0 0 24 24"), true, "svg/tabler/chevron-right.svg"),
    (TABLER_CHEVRON_UP, "tabler.chevron-up", Some("0 0 24 24"), true, "svg/tabler/chevron-up.svg"),
    (TABLER_CIRCLE_ARROW_UP, "tabler.circle-arrow-up", Some("0 0 24 24"), true, "svg/tabler/circle-arrow-up.svg"),
    (TABLER_CIRCLE_CHECK, "tabler.circle-check", Some("0 0 24 24"), true, "svg/tabler/circle-check.svg"),
    (TABLER_COPY, "tabler.copy", Some("0 0 24 24"), true, "svg/tabler/copy.svg"),
    (TABLER_DEVICE_LAPTOP, "tabler.device-laptop", Some("0 0 24 24"), true, "svg/tabler/device-laptop.svg"),
    (TABLER_DOWNLOAD, "tabler.download", Some("0 0 24 24"), true, "svg/tabler/download.svg"),
    (TABLER_EYE, "tabler.eye", Some("0 0 24 24"), true, "svg/tabler/eye.svg"),
    (TABLER_FILE_DIFF, "tabler.file-diff", Some("0 0 24 24"), true, "svg/tabler/file-diff.svg"),
    (TABLER_FILE_OFF, "tabler.file-off", Some("0 0 24 24"), true, "svg/tabler/file-off.svg"),
    (TABLER_FILE_PENCIL, "tabler.file-pencil", Some("0 0 24 24"), true, "svg/tabler/file-pencil.svg"),
    (TABLER_FILE_SEARCH, "tabler.file-search", Some("0 0 24 24"), true, "svg/tabler/file-search.svg"),
    (TABLER_FILE_SPARK, "tabler.file-spark", Some("0 0 24 24"), true, "svg/tabler/file-spark.svg"),
    (TABLER_FILE_TEXT, "tabler.file-text", Some("0 0 24 24"), true, "svg/tabler/file-text.svg"),
    (TABLER_FILE_X, "tabler.file-x", Some("0 0 24 24"), true, "svg/tabler/file-x.svg"),
    (TABLER_FOLDER, "tabler.folder", Some("0 0 24 24"), true, "svg/tabler/folder.svg"),
    (TABLER_FOLDER_CODE, "tabler.folder-code", Some("0 0 24 24"), true, "svg/tabler/folder-code.svg"),
    (TABLER_FOLDER_OPEN, "tabler.folder-open", Some("0 0 24 24"), true, "svg/tabler/folder-open.svg"),
    (TABLER_FOLDER_PLUS, "tabler.folder-plus", Some("0 0 24 24"), true, "svg/tabler/folder-plus.svg"),
    (TABLER_GIT_BRANCH, "tabler.git-branch", Some("0 0 24 24"), true, "svg/tabler/git-branch.svg"),
    (TABLER_LIST_DETAILS, "tabler.list-details", Some("0 0 24 24"), true, "svg/tabler/list-details.svg"),
    (TABLER_LOCK, "tabler.lock", Some("0 0 24 24"), true, "svg/tabler/lock.svg"),
    (TABLER_LOGOUT, "tabler.logout", Some("0 0 24 24"), true, "svg/tabler/logout.svg"),
    (TABLER_MESSAGE_CIRCLE, "tabler.message-circle", Some("0 0 24 24"), true, "svg/tabler/message-circle.svg"),
    (TABLER_MESSAGE_CIRCLE_QUESTION, "tabler.message-circle-question", Some("0 0 24 24"), true, "svg/tabler/message-circle-question.svg"),
    (TABLER_MESSAGES, "tabler.messages", Some("0 0 24 24"), true, "svg/tabler/messages.svg"),
    (TABLER_MINUS, "tabler.minus", Some("0 0 24 24"), true, "svg/tabler/minus.svg"),
    (TABLER_MOON, "tabler.moon", Some("0 0 24 24"), true, "svg/tabler/moon.svg"),
    (TABLER_PENCIL, "tabler.pencil", Some("0 0 24 24"), true, "svg/tabler/pencil.svg"),
    (TABLER_PENCIL_PLUS, "tabler.pencil-plus", Some("0 0 24 24"), true, "svg/tabler/pencil-plus.svg"),
    (TABLER_PLAYER_PAUSE_FILLED, "tabler.player-pause-filled", Some("0 0 24 24"), true, "svg/tabler/player-pause-filled.svg"),
    (TABLER_PLAYER_PLAY_FILLED, "tabler.player-play-filled", Some("0 0 24 24"), true, "svg/tabler/player-play-filled.svg"),
    (TABLER_PLAYER_STOP_FILLED, "tabler.player-stop-filled", Some("0 0 24 24"), true, "svg/tabler/player-stop-filled.svg"),
    (TABLER_PLUG_CONNECTED, "tabler.plug-connected", Some("0 0 24 24"), true, "svg/tabler/plug-connected.svg"),
    (TABLER_QUESTION_MARK, "tabler.question-mark", Some("0 0 24 24"), true, "svg/tabler/question-mark.svg"),
    (TABLER_REFRESH, "tabler.refresh", Some("0 0 24 24"), true, "svg/tabler/refresh.svg"),
    (TABLER_RUN_SPRINT, "tabler.run-sprint", Some("0 0 24 24"), true, "svg/tabler/run-sprint.svg"),
    (TABLER_SEARCH, "tabler.search", Some("0 0 24 24"), true, "svg/tabler/search.svg"),
    (TABLER_SELECTOR, "tabler.selector", Some("0 0 24 24"), true, "svg/tabler/selector.svg"),
    (TABLER_SETTINGS, "tabler.settings", Some("0 0 24 24"), true, "svg/tabler/settings.svg"),
    (TABLER_SHIELD_LOCK, "tabler.shield-lock", Some("0 0 24 24"), true, "svg/tabler/shield-lock.svg"),
    (TABLER_SITEMAP, "tabler.sitemap", Some("0 0 24 24"), true, "svg/tabler/sitemap.svg"),
    (TABLER_SPARKLES, "tabler.sparkles", Some("0 0 24 24"), true, "svg/tabler/sparkles.svg"),
    (TABLER_SUN, "tabler.sun", Some("0 0 24 24"), true, "svg/tabler/sun.svg"),
    (TABLER_TARGET_ARROW, "tabler.target-arrow", Some("0 0 24 24"), true, "svg/tabler/target-arrow.svg"),
    (TABLER_TERMINAL_2, "tabler.terminal-2", Some("0 0 24 24"), true, "svg/tabler/terminal-2.svg"),
    (TABLER_TOOL, "tabler.tool", Some("0 0 24 24"), true, "svg/tabler/tool.svg"),
    (TABLER_TRASH, "tabler.trash", Some("0 0 24 24"), true, "svg/tabler/trash.svg"),
    (TABLER_USER, "tabler.user", Some("0 0 24 24"), true, "svg/tabler/user.svg"),
    (TABLER_WORLD, "tabler.world", Some("0 0 24 24"), true, "svg/tabler/world.svg"),
    (TABLER_WORLD_SEARCH, "tabler.world-search", Some("0 0 24 24"), true, "svg/tabler/world-search.svg"),
    (TABLER_X, "tabler.x", Some("0 0 24 24"), true, "svg/tabler/x.svg"),
    (TABLER_ZOOM_IN, "tabler.zoom-in", Some("0 0 24 24"), true, "svg/tabler/zoom-in.svg"),
    (TABLER_ZOOM_OUT, "tabler.zoom-out", Some("0 0 24 24"), true, "svg/tabler/zoom-out.svg"),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownAsset {
    id: String,
}

impl fmt::Display for UnknownAsset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown asset id `{}`", self.id)
    }
}

impl std::error::Error for UnknownAsset {}

#[must_use]
pub const fn get(id: AssetId) -> &'static Asset {
    &ALL[id.index]
}

fn lookup(id: &str) -> Result<&'static Asset, UnknownAsset> {
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
