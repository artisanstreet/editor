//! Sealed catalog of first-party, vendored visual assets.
//!
//! Every static SVG source referenced by the legacy frontend is embedded in
//! this crate together with its provenance metadata from `manifest.toml`
//! (exposed as [`MANIFEST_TOML`]). Consumers resolve stable [`AssetId`]s to
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

pub mod fonts;

/// Stable identifier for a vendored asset.
///
/// The string form is `<family>.<name>` (for example `tabler.check`) and
/// matches an `id` row in the manifest. The internal index is assigned at
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

/// Resolves a catalog key to its position in [`CATALOG_IDS`].
///
/// # Compile-time-only invariant
///
/// Reachable only from `AssetId::from_id`, which is private, `const`, and
/// called only by the `catalog!` macro with `&'static str` literals. The
/// panic below therefore fires during constant evaluation of a bad literal —
/// a build failure — and can never be triggered by runtime or external input.
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
    pub const ALL: [Family; 7] = [
        Family::Artisan,
        Family::Brand,
        Family::Jetbrains,
        Family::Lobe,
        Family::SimpleIcons,
        Family::Svgl,
        Family::Tabler,
    ];

    /// Returns the id prefix used by every asset of this family.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Family::Artisan => "artisan",
            Family::Brand => "brands",
            Family::Jetbrains => "jetbrains",
            Family::Lobe => "lobe",
            Family::SimpleIcons => "simple-icons",
            Family::Svgl => "svgl",
            Family::Tabler => "tabler",
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
/// currentColor artwork (`svgl.qwen`, `lobe.*`, `simple-icons.*`,
/// `brands.hermes`) stays [`Presentation::Tinted`]: painting with text color
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

/// One vendored SVG asset: manifest metadata plus embedded source text.
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
    /// Checked-in location relative to `modules/assets/`.
    pub source_path: &'static str,
    /// Embedded standalone SVG source, byte-identical to the checked-in file.
    pub source: &'static str,
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
    (ARTISAN_STAR, "artisan.star", Family::Artisan, Some("0 0 100 100"), true, "svg/artisan/star.svg"),
    (ARTISAN_SUCCESS_CHECK, "artisan.success-check", Family::Artisan, Some("0 0 16 16"), true, "svg/artisan/success-check.svg"),
    (BRANDS_HERMES, "brands.hermes", Family::Brand, Some("0 0 24 24"), true, "svg/brands/hermes.svg"),
    (BRANDS_KIMI, "brands.kimi", Family::Brand, Some("0 0 24 25"), false, "svg/brands/kimi.svg"),
    (BRANDS_OPENCODE, "brands.opencode", Family::Brand, Some("0 0 240 300"), false, "svg/brands/opencode.svg"),
    (BRANDS_ZAI, "brands.zai", Family::Brand, Some("0 0 30 30"), false, "svg/brands/zai.svg"),
    (JETBRAINS_SVELTE, "jetbrains.svelte", Family::Jetbrains, Some("0 0 256 308"), false, "svg/jetbrains/svelte.svg"),
    (JETBRAINS_TEXT, "jetbrains.text", Family::Jetbrains, Some("0 0 16 16"), true, "svg/jetbrains/text.svg"),
    (JETBRAINS_TS_TEST, "jetbrains.ts-test", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains/ts-test.svg"),
    (JETBRAINS_TYPESCRIPT, "jetbrains.typescript", Family::Jetbrains, Some("0 0 16 16"), false, "svg/jetbrains/typescript.svg"),
    (LOBE_MINIMAX, "lobe.minimax", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/minimax.svg"),
    (LOBE_NVIDIA, "lobe.nvidia", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/nvidia.svg"),
    (LOBE_TENCENT, "lobe.tencent", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/tencent.svg"),
    (LOBE_XIAOMI, "lobe.xiaomi", Family::Lobe, Some("0 0 24 24"), true, "svg/lobe/xiaomi.svg"),
    (SIMPLE_ICONS_BITBUCKET, "simple-icons.bitbucket", Family::SimpleIcons, Some("0 0 24 24"), true, "svg/simple-icons/bitbucket.svg"),
    (SIMPLE_ICONS_CODEBERG, "simple-icons.codeberg", Family::SimpleIcons, Some("0 0 24 24"), true, "svg/simple-icons/codeberg.svg"),
    (SIMPLE_ICONS_GITEA, "simple-icons.gitea", Family::SimpleIcons, Some("0 0 24 24"), true, "svg/simple-icons/gitea.svg"),
    (SIMPLE_ICONS_SOURCEHUT, "simple-icons.sourcehut", Family::SimpleIcons, Some("0 0 24 24"), true, "svg/simple-icons/sourcehut.svg"),
    (SVGL_CLAUDE_AI, "svgl.claude-ai", Family::Svgl, Some("0 0 256 257"), true, "svg/svgl/claude-ai.svg"),
    (SVGL_CURSOR, "svgl.cursor", Family::Svgl, Some("0 0 466.73 532.09"), true, "svg/svgl/cursor.svg"),
    (SVGL_DEEPSEEK, "svgl.deepseek", Family::Svgl, Some("0 0 24 24"), true, "svg/svgl/deepseek.svg"),
    (SVGL_GEMINI, "svgl.gemini", Family::Svgl, Some("0 0 296 298"), false, "svg/svgl/gemini.svg"),
    (SVGL_GIT, "svgl.git", Family::Svgl, Some("0 0 256 256"), true, "svg/svgl/git.svg"),
    (SVGL_GITHUB, "svgl.github", Family::Svgl, Some("0 0 1024 1024"), true, "svg/svgl/github.svg"),
    (SVGL_GITLAB, "svgl.gitlab", Family::Svgl, Some("0 0 32 32"), false, "svg/svgl/gitlab.svg"),
    (SVGL_GROK, "svgl.grok", Family::Svgl, Some("0 0 1024 1024"), true, "svg/svgl/grok.svg"),
    (SVGL_META, "svgl.meta", Family::Svgl, Some("0 0 256 171"), false, "svg/svgl/meta.svg"),
    (SVGL_MICROSOFT_AZURE, "svgl.microsoft-azure", Family::Svgl, Some("0 0 96 96"), false, "svg/svgl/microsoft-azure.svg"),
    (SVGL_OPENAI, "svgl.openai", Family::Svgl, Some("0 0 256 260"), true, "svg/svgl/openai.svg"),
    (SVGL_QWEN, "svgl.qwen", Family::Svgl, Some("0 0 24 24"), true, "svg/svgl/qwen.svg"),
    (TABLER_ALERT_TRIANGLE, "tabler.alert-triangle", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/alert-triangle.svg"),
    (TABLER_ARROW_BAR_TO_DOWN, "tabler.arrow-bar-to-down", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrow-bar-to-down.svg"),
    (TABLER_ARROW_RIGHT, "tabler.arrow-right", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrow-right.svg"),
    (TABLER_ARROW_UP, "tabler.arrow-up", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrow-up.svg"),
    (TABLER_ARROWS_HORIZONTAL, "tabler.arrows-horizontal", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrows-horizontal.svg"),
    (TABLER_ARROWS_MINIMIZE, "tabler.arrows-minimize", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/arrows-minimize.svg"),
    (TABLER_BELL, "tabler.bell", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/bell.svg"),
    (TABLER_BOLT_FILLED, "tabler.bolt-filled", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/bolt-filled.svg"),
    (TABLER_BOT_ID, "tabler.bot-id", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/bot-id.svg"),
    (TABLER_BRAIN, "tabler.brain", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/brain.svg"),
    (TABLER_BRAND_VISUAL_STUDIO, "tabler.brand-visual-studio", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/brand-visual-studio.svg"),
    (TABLER_BUG, "tabler.bug", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/bug.svg"),
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
    (TABLER_FILE_DIFF, "tabler.file-diff", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-diff.svg"),
    (TABLER_FILE_OFF, "tabler.file-off", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-off.svg"),
    (TABLER_FILE_PENCIL, "tabler.file-pencil", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-pencil.svg"),
    (TABLER_FILE_SEARCH, "tabler.file-search", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/file-search.svg"),
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
    (TABLER_PALETTE, "tabler.palette", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/palette.svg"),
    (TABLER_PENCIL, "tabler.pencil", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/pencil.svg"),
    (TABLER_PLAYER_PLAY, "tabler.player-play", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/player-play.svg"),
    (TABLER_PLAYER_STOP_FILLED, "tabler.player-stop-filled", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/player-stop-filled.svg"),
    (TABLER_PLAYER_TRACK_NEXT, "tabler.player-track-next", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/player-track-next.svg"),
    (TABLER_PLAYER_TRACK_PREV, "tabler.player-track-prev", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/player-track-prev.svg"),
    (TABLER_QUESTION_MARK, "tabler.question-mark", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/question-mark.svg"),
    (TABLER_REFRESH, "tabler.refresh", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/refresh.svg"),
    (TABLER_ROTATE_CLOCKWISE, "tabler.rotate-clockwise", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/rotate-clockwise.svg"),
    (TABLER_SEARCH, "tabler.search", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/search.svg"),
    (TABLER_SELECTOR, "tabler.selector", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/selector.svg"),
    (TABLER_SETTINGS, "tabler.settings", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/settings.svg"),
    (TABLER_SHIELD_LOCK, "tabler.shield-lock", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/shield-lock.svg"),
    (TABLER_SHOPPING_BAG, "tabler.shopping-bag", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/shopping-bag.svg"),
    (TABLER_SPARKLES, "tabler.sparkles", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/sparkles.svg"),
    (TABLER_STAR, "tabler.star", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/star.svg"),
    (TABLER_STAR_FILLED, "tabler.star-filled", Family::Tabler, Some("0 0 24 24"), true, "svg/tabler/star-filled.svg"),
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
    /// Path relative to `modules/assets/`.
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

/// Full text of `modules/assets/manifest.toml`, including per-use records.
pub const MANIFEST_TOML: &str = include_str!("../manifest.toml");

/// Embedded license and attribution material, ordered by path.
pub const LICENSE_FILES: &[LicenseFile] = &[
    LicenseFile {
        path: "licenses/artisan-first-party-NOTES.md",
        contents: include_str!("../licenses/artisan-first-party-NOTES.md"),
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
/// Deterministic: [`ALL`] is sorted by key (proven externally), so this is a
/// plain binary search over the stable string keys.
///
/// # Errors
///
/// Returns [`UnknownAsset`] when `id` does not name a vendored asset.
pub fn lookup(id: &str) -> Result<&'static Asset, UnknownAsset> {
    match ALL.binary_search_by(|probe| probe.id.as_str().cmp(id)) {
        Ok(index) => Ok(&ALL[index]),
        Err(_) => Err(UnknownAsset {
            id: String::from(id),
        }),
    }
}

/// Returns the embedded license/attribution document at `path`.
#[must_use]
pub fn license(path: &str) -> Option<&'static LicenseFile> {
    LICENSE_FILES.iter().find(|file| file.path == path)
}
