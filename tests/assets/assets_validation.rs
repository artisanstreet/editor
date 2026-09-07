//! Hermetic validation of the sealed vendored-asset foundation.
//!
//! Proves, with the standard library plus the pinned `sha2`, `toml`, and
//! `roxmltree` crates and only Bazel-wired `data` (no network, `node_modules`,
//! npm, Python, browser, or ambient filesystem access beyond declared
//! runfiles):
//!
//! - portable runfiles resolution through pure helpers exercised on synthetic
//!   layouts: directory trees under `RUNFILES_DIR` or an executable-sibling
//!   `.runfiles` probed across `TEST_WORKSPACE`, `artisan_editor`, and `_main`
//!   prefixes, `RUNFILES_MANIFEST_FILE` accepted only when it actually maps
//!   the probe artifact, and a bounded local-review source-root fallback
//!   (`ARTISAN_ASSETS_SOURCE_ROOT`);
//! - four-way bijection among physical SVG runfiles, the explicit BUILD
//!   `ASSET_SOURCES` list, manifest `source_path` rows, and `Asset.source_path`
//!   — catching missing *and* extra files;
//! - physical bytes exactly equal to embedded `Asset::source`, to
//!   `MANIFEST_TOML.as_bytes()`, and to recorded raw-byte sha256 digests
//!   (`.gitattributes` pins LF);
//! - manifest schema via the pinned `toml` crate: duplicate keys rejected by
//!   the parser itself, exact field sets per row kind, id/family grammar,
//!   normalized no-escape paths (origins included), use-site closure;
//! - catalog presentation policy independent of the validator-derived
//!   `monochrome` property: monochrome-derived default with exactly the two
//!   evidenced authored-color brand exceptions, deterministic across lookups
//!   for every one of the 104 ids;
//! - standalone-SVG validity and policy via `roxmltree` document parsing:
//!   DOCTYPE/ENTITY declarations, script/foreignObject elements, every `on*`
//!   event attribute, and non-allowlisted href/src values are rejected, while
//!   XML declarations, comments, internal fragment links, and nested base64
//!   data-image payloads pass.

use core::fmt::Write as _;

use sha2::{Digest, Sha256};

use artisan_assets::{
    AssetId, LICENSE_FILES, MANIFEST_TOML, Presentation, get as catalog_get, license, lookup,
};

// ------------------------------------------------------------ runfiles

const SOURCE_ROOT_ENV: &str = "ARTISAN_ASSETS_SOURCE_ROOT";
const PROBE_REL: &str = "modules/assets/manifest.toml";

/// Workspace prefixes accepted in runfile keys, priority order preserved:
/// nonempty `TEST_WORKSPACE`, then this module's name, then Bazel's
/// canonical `_main`. Duplicates are dropped on insertion; the order of
/// first appearance is never reordered.
fn build_prefixes(test_workspace: Option<&str>) -> Vec<String> {
    let mut prefixes = Vec::new();
    let mut push_unique = |candidate: String| {
        if !prefixes.contains(&candidate) {
            prefixes.push(candidate);
        }
    };
    if let Some(ws) = test_workspace
        && !ws.is_empty()
    {
        push_unique(String::from(ws));
    }
    push_unique(String::from("artisan_editor"));
    push_unique(String::from("_main"));
    prefixes
}

/// Returns the first prefix under which `root` actually serves [`PROBE_REL`].
/// A directory that merely exists without serving the artifact yields `None`.
fn probe_tree_prefix(root: &std::path::Path, prefixes: &[String]) -> Option<String> {
    prefixes.iter().find_map(|prefix| {
        let candidate = root.join(prefix).join(PROBE_REL);
        candidate.is_file().then(|| prefix.clone())
    })
}

/// True when `map` resolves [`PROBE_REL`] under one of the workspace
/// prefixes (or unprefixed). A merely non-empty unrelated manifest fails
/// this gate and must not win layout selection.
fn manifest_serves_probe(
    map: &std::collections::BTreeMap<String, String>,
    prefixes: &[String],
) -> bool {
    prefixes
        .iter()
        .any(|prefix| map.contains_key(&format!("{prefix}/{PROBE_REL}")))
        || map.contains_key(PROBE_REL)
}

/// Resolves repository-relative paths (`modules/assets/...`) through Bazel
/// runfiles, falling back to a bounded source root for local review runs.
#[derive(Clone, Debug)]
enum Resolver {
    /// Directory-backed runfiles; `prefix` is the verified workspace dir.
    Tree {
        root: std::path::PathBuf,
        prefix: String,
    },
    /// `RUNFILES_MANIFEST_FILE` mappings from runfile path to real path.
    Manifest {
        map: std::collections::BTreeMap<String, String>,
    },
    /// Local review checkout root (env `ARTISAN_ASSETS_SOURCE_ROOT`).
    SourceRoot { root: std::path::PathBuf },
}

impl Resolver {
    /// Picks a layout from explicit options; pure apart from filesystem
    /// probing, so synthetic tests can drive it without touching process env.
    ///
    /// Precedence: a manifest that serves the probe artifact wins over any
    /// directory layout; a directory layout must serve the artifact under
    /// some workspace prefix; the source root is last.
    fn pick(
        manifest: Option<std::collections::BTreeMap<String, String>>,
        runfiles_dir: Option<&std::path::Path>,
        prefixes: &[String],
    ) -> Option<Resolver> {
        if let Some(map) = manifest.filter(|map| manifest_serves_probe(map, prefixes)) {
            return Some(Resolver::Manifest { map });
        }
        if let Some(dir) = runfiles_dir
            && let Some(prefix) = probe_tree_prefix(dir, prefixes)
        {
            return Some(Resolver::Tree {
                root: dir.to_path_buf(),
                prefix,
            });
        }
        None
    }

    /// Detects the layout from the process environment.
    fn detect() -> Resolver {
        let prefixes = workspace_prefixes_for_tests();
        let manifest = std::env::var("RUNFILES_MANIFEST_FILE")
            .ok()
            .and_then(|path| {
                std::fs::read_to_string(path)
                    .ok()
                    .map(|text| parse_runfiles_manifest(&text))
            });
        let runfiles_dir = std::env::var("RUNFILES_DIR")
            .ok()
            .map(std::path::PathBuf::from);
        if let Some(resolver) = Self::pick(manifest, runfiles_dir.as_deref(), &prefixes) {
            return resolver;
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let root = dir.join(format!(
                "{}.runfiles",
                exe.file_stem().and_then(|s| s.to_str()).unwrap_or_default()
            ));
            if let Some(prefix) = probe_tree_prefix(&root, &prefixes) {
                return Resolver::Tree { root, prefix };
            }
        }
        // Bounded local-review fallback: an explicit source checkout root,
        // used only when no runfiles layout exists.
        if let Ok(root) = std::env::var(SOURCE_ROOT_ENV)
            && std::path::Path::new(&root).join(PROBE_REL).is_file()
        {
            return Resolver::SourceRoot {
                root: std::path::PathBuf::from(root),
            };
        }
        panic!(
            "no usable runfiles layout found and ${SOURCE_ROOT_ENV} is unset or \
             does not point at this checkout root"
        );
    }

    fn resolve(&self, repo_rel: &str) -> std::path::PathBuf {
        match self {
            Resolver::Tree { root, prefix } => root.join(prefix).join(repo_rel),
            Resolver::SourceRoot { root } => root.join(repo_rel),
            Resolver::Manifest { map } => {
                for prefix in workspace_prefixes_for_tests() {
                    if let Some(path) = map.get(&format!("{prefix}/{repo_rel}")) {
                        return std::path::PathBuf::from(path);
                    }
                }
                if let Some(path) = map.get(repo_rel) {
                    return std::path::PathBuf::from(path);
                }
                panic!("runfiles manifest lacks entry for {repo_rel}");
            }
        }
    }

    /// True when the resolver sees the whole checkout, so legacy-tree
    /// existence can be asserted.
    fn is_local_review(&self) -> bool {
        matches!(self, Resolver::SourceRoot { .. })
    }

    /// Physical SVG files as package-relative `svg/<family>/<file>` paths,
    /// deduplicated. Tree/source modes walk the directory; manifest mode
    /// enumerates the manifest's own keys under every supported workspace
    /// prefix, so extra files remain detectable there too.
    fn physical_svgs(&self) -> Vec<String> {
        let mut files = match self {
            Resolver::Manifest { map } => {
                let mut files = Vec::new();
                for key in map.keys() {
                    for rel in strip_workspace_prefixes(key) {
                        push_if_svg(&mut files, &rel);
                    }
                }
                files
            }
            Resolver::Tree { root, prefix } => {
                let mut files = Vec::new();
                let base = root.join(prefix).join("modules/assets/svg");
                collect_svg_files(&base, &base, &mut files);
                files
            }
            Resolver::SourceRoot { root } => {
                let mut files = Vec::new();
                let base = root.join("modules/assets/svg");
                collect_svg_files(&base, &base, &mut files);
                files
            }
        };
        files.sort();
        files.dedup();
        files
    }
}

/// Expands one runfile key into the logical repo-relative suffixes implied by
/// each matching workspace prefix (at most one per prefix, plus bare).
fn strip_workspace_prefixes(key: &str) -> Vec<String> {
    let mut out = Vec::new();
    for prefix in workspace_prefixes_for_tests() {
        let headed = format!("{prefix}/");
        if let Some(rest) = key.strip_prefix(&headed)
            && let Some(rel) = rest.strip_prefix("modules/assets/")
        {
            out.push(String::from(rel));
        }
    }
    if let Some(rel) = key.strip_prefix("modules/assets/") {
        out.push(String::from(rel));
    }
    out
}

fn workspace_prefixes_for_tests() -> Vec<String> {
    build_prefixes(std::env::var("TEST_WORKSPACE").ok().as_deref())
}

fn push_if_svg(files: &mut Vec<String>, package_rel: &str) {
    // Case-sensitive on purpose: the normalized corpus uses lowercase .svg.
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let is_svg = package_rel.ends_with(".svg");
    if is_svg && package_rel.starts_with("svg/") && !files.contains(&String::from(package_rel)) {
        files.push(String::from(package_rel));
    }
}

fn parse_runfiles_manifest(text: &str) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        map.insert(String::from(key), String::from(value));
    }
    map
}

/// Recursively collects `.svg` files under `root`, returning
/// package-relative paths (`svg/<family>/<file>.svg`).
fn collect_svg_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    let mut names: Vec<_> = entries.filter_map(Result::ok).collect();
    names.sort_by_key(std::fs::DirEntry::path);
    for entry in names {
        let path = entry.path();
        if path.is_dir() {
            collect_svg_files(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext == "svg") {
            let rel = path
                .strip_prefix(root)
                .expect("walk root is a prefix")
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push(format!("svg/{rel}"));
        }
    }
}

// ------------------------------------------------------------------ digest

/// Lowercase hex of the SHA-256 of `bytes`, compared against the manifest's
/// raw-byte digests (`.gitattributes` pins LF).
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_of(hasher.finalize().as_slice())
}

fn hex_of(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.write_fmt(format_args!("{b:02x}"))
            .expect("string writes never fail");
    }
    out
}

// ------------------------------------------------------- manifest via toml

fn field<'a>(row: &'a toml::Value, key: &str) -> &'a str {
    row.get(key)
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("field `{key}` missing or not a string"))
}

fn bool_field(row: &toml::Value, key: &str) -> bool {
    row.get(key)
        .and_then(toml::Value::as_bool)
        .unwrap_or_else(|| panic!("field `{key}` missing or not a bool"))
}

fn array_field(row: &toml::Value, key: &str) -> Vec<String> {
    row.get(key)
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("field `{key}` missing or not an array"))
        .iter()
        .map(|v| v.as_str().expect("array element string").to_owned())
        .collect()
}

fn has_field(row: &toml::Value, key: &str) -> bool {
    row.get(key).is_some()
}

fn row_keys(row: &toml::Value) -> Vec<String> {
    let mut keys: Vec<String> = row
        .as_table()
        .expect("manifest row is a table")
        .keys()
        .map(String::from)
        .collect();
    keys.sort();
    keys
}

/// Parses the manifest with the pinned `toml` crate. Duplicate keys inside
/// any table are rejected by TOML itself (see the negative test below).
fn parse_manifest(text: &str) -> (Vec<toml::Value>, Vec<toml::Value>) {
    let doc: toml::Table = text
        .parse()
        .expect("manifest.toml must be valid TOML under the pinned spec");
    assert_eq!(
        doc.get("schema_version")
            .and_then(toml::Value::as_integer)
            .unwrap_or_default(),
        1,
        "schema_version must be 1"
    );
    let assets = doc
        .get("asset")
        .and_then(|v| v.as_array())
        .expect("[[asset]] rows")
        .clone();
    let uses = doc
        .get("use")
        .and_then(|v| v.as_array())
        .expect("[[use]] rows")
        .clone();
    assert!(!assets.is_empty(), "no [[asset]] rows parsed");
    (assets, uses)
}

// --------------------------------------------------------- BUILD list parser

/// Extracts an explicit `NAME = [ "a", "b", ... ]` list from BUILD.bazel text.
fn parse_build_list(build_text: &str, name: &str) -> Vec<String> {
    let marker = format!("{name} = [");
    let start = build_text
        .find(&marker)
        .unwrap_or_else(|| panic!("{marker} missing from BUILD.bazel"));
    let end = build_text[start..]
        .find(']')
        .unwrap_or_else(|| panic!("{name} list unterminated"));
    let body = &build_text[start + marker.len()..start + end];
    body.split(',')
        .map(|item| item.trim().trim_matches('"').to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

// ------------------------------------------------- roxmltree SVG validation

/// Parses and policy-checks one vendored SVG document.
///
/// Well-formedness comes from `roxmltree` itself, parsed with
/// `ParsingOptions::allow_dtd = false` as the authoritative DTD gate; the
/// explicit pre-parse declaration scan below additionally states the corpus
/// policy against DTD/ENTITY constructs regardless of parser tolerance.
fn assert_safe_svg(source: &str, label: &str) {
    // Template/Svelte artifacts cannot appear in standalone assets.
    assert!(!source.contains("${"), "{label}: template artifact");
    assert!(!source.contains("{..."), "{label}: svelte spread artifact");
    assert!(!source.contains("{@html"), "{label}: svelte html artifact");

    // Declarations are rejected before parsing so the policy, not merely the
    // parser's tolerance, is what excludes them. Case variants are caught by
    // lowering; malformed whitespace variants fall through to the parser,
    // which rejects them as invalid XML under the no-DTD option.
    let lowered = source.to_ascii_lowercase();
    for forbidden in ["<!doctype", "<!entity"] {
        assert!(
            !lowered.contains(forbidden),
            "{label}: {forbidden} construct forbidden"
        );
    }

    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        ..roxmltree::ParsingOptions::default()
    };
    let doc = roxmltree::Document::parse_with_options(source, options)
        .unwrap_or_else(|e| panic!("{label}: invalid XML: {e}"));

    let root = doc.root_element();
    assert_eq!(
        root.tag_name().name(),
        "svg",
        "{label}: root element must be svg"
    );

    for node in doc.descendants() {
        if !node.is_element() {
            continue;
        }
        let element = node.tag_name().name().to_ascii_lowercase();
        assert!(element != "script", "{label}: script elements forbidden");
        assert!(
            element != "foreignobject",
            "{label}: foreignObject elements forbidden"
        );
        for attr in node.attributes() {
            let attr_local = attr.name().to_ascii_lowercase();
            assert!(
                !(attr_local.len() > 2 && attr_local.starts_with("on")),
                "{label}: event-handler attribute `{}` forbidden",
                attr.name(),
            );
            if matches!(attr_local.as_str(), "href" | "src") {
                check_reference(attr.value(), &attr_local, label);
            }
        }
    }
}

/// href/src allowlist: internal fragments and exactly the nested base64
/// SVG payload prefix the legacy app icons require
/// (`data:image/svg+xml;base64,`). Every other data form, scheme, external
/// URL, protocol-relative target, or relative path is rejected.
fn check_reference(value: &str, attr: &str, label: &str) {
    const ALLOWED_DATA_PREFIX: &str = "data:image/svg+xml;base64,";

    // The data allowlist is literal case-sensitive bytes: uppercase or
    // mixed-case spellings of the prefix are rejected like any other value.
    let trimmed = value.trim();
    if trimmed.starts_with(ALLOWED_DATA_PREFIX) {
        return;
    }

    // Only a real internal fragment qualifies: a hash followed by a
    // nonempty identifier. Bare `#` and empty/whitespace-only values are
    // rejected outright.
    if let Some(fragment) = trimmed.strip_prefix('#') {
        assert!(
            !fragment.is_empty(),
            "{label}: bare `{attr}=\"#\"` fragment forbidden"
        );
        return;
    }

    let lowered = trimmed.to_ascii_lowercase();
    for scheme in ["javascript:", "file:", "http:", "https:", "data:"] {
        assert!(
            !lowered.starts_with(scheme),
            "{label}: {attr} scheme `{scheme}` forbidden"
        );
    }
    assert!(
        !trimmed.starts_with("//"),
        "{label}: protocol-relative {attr} `{trimmed}` forbidden"
    );
    panic!("{label}: {attr} value `{trimmed}` is neither a fragment nor a base64 svg data image");
}

/// Returns the root element's `viewBox`, when declared (exact attribute name,
/// per SVG case sensitivity).
fn root_view_box<'a>(doc: &'a roxmltree::Document) -> Option<&'a str> {
    doc.root_element().attribute("viewBox")
}

fn view_box_is_well_formed(view_box: &str) -> bool {
    let numbers: Vec<&str> = view_box.split_whitespace().collect();
    numbers.len() == 4
        && numbers
            .iter()
            .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '.'))
        && view_box == view_box.trim()
        && !view_box.contains("..")
}

// ------------------------------------------------- monochrome re-derivation

/// Re-runs the documented artwork-paint derivation (docs/ui/ASSETS.md §10)
/// over the parsed tree: distinct literal paints among fill/stroke/
/// stop-color attributes and style-block declarations, ignoring subtrees of
/// mask/clipPath; monochrome also requires absence of image and gradient
/// elements.
fn derive_monochrome(doc: &roxmltree::Document) -> bool {
    fn is_ignored_paint(value: &str) -> bool {
        // Values are lowercased before this check, so the ignored token is
        // `currentcolor`; a source-file `currentColor` must never count as a
        // literal paint.
        matches!(
            value,
            "" | "none" | "currentcolor" | "inherit" | "transparent"
        ) || value.starts_with("url(")
    }

    fn push_paint(paints: &mut Vec<String>, value: &str) {
        let normalized = value.trim().to_ascii_lowercase();
        if !is_ignored_paint(&normalized) && !paints.contains(&normalized) {
            paints.push(normalized);
        }
    }

    let mut paints: Vec<String> = Vec::new();
    let mut has_image = false;
    let mut has_gradient = false;

    for node in doc.descendants() {
        if !node.is_element() {
            continue;
        }
        let name = node.tag_name().name().to_ascii_lowercase();
        match name.as_str() {
            "image" => has_image = true,
            "lineargradient" | "radialgradient" => has_gradient = true,
            _ => {}
        }

        let masked = node.ancestors().skip(1).any(|ancestor| {
            matches!(
                ancestor.tag_name().name().to_ascii_lowercase().as_str(),
                "mask" | "clippath"
            )
        });
        if masked {
            continue;
        }
        for attr in node.attributes() {
            let key = attr.name().to_ascii_lowercase();
            if matches!(key.as_str(), "fill" | "stroke" | "stop-color") {
                push_paint(&mut paints, attr.value());
            }
        }
        if name == "style"
            && let Some(body) = node.text()
        {
            for prop in ["fill", "stroke"] {
                let needle = format!("{prop}:");
                let mut search_from = 0;
                while let Some(rel) = body[search_from..].find(&needle) {
                    let at = search_from + rel + needle.len();
                    let tail = &body[at..];
                    let end = tail.find([';', '}']).unwrap_or(tail.len());
                    push_paint(&mut paints, &tail[..end]);
                    search_from = at + end.max(1);
                }
            }
        }
    }

    paints.len() <= 1 && !has_image && !has_gradient
}

// ------------------------------------------------------------------ helpers

const ASSET_BASE_FIELDS: &[&str] = &[
    "id",
    "family",
    "name",
    "origin_kind",
    "origin_path",
    "license_spdx",
    "license_file",
    "view_box",
    "monochrome",
    "source_path",
    "sha256",
];
const ASSET_OPTIONAL_FIELDS: &[&str] = &["notes"];
const ORIGIN_PACKAGE_FIELDS: &[&str] = &["origin_package", "origin_version"];
const USE_FIELDS: &[&str] = &[
    "assets",
    "channel",
    "classification",
    "id",
    "reachability",
    "sites",
];

fn normalized_repo_rel(path: &str, label: &str) {
    assert!(!path.is_empty(), "{label}: empty path");
    assert!(!path.contains('\\'), "{label}: backslash in {path}");
    assert!(!path.contains("//"), "{label}: doubled separator in {path}");
    assert!(!path.contains("./"), "{label}: dot segment in {path}");
    assert!(!path.contains(".."), "{label}: traversal in {path}");
    assert!(
        !path.starts_with('/') && !path.contains(':'),
        "{label}: absolute path {path}"
    );
    assert_eq!(path, path.trim(), "{label}: untrimmed path {path}");
}

fn frontend_site(path: &str, label: &str) {
    normalized_repo_rel(path, label);
    assert!(
        path.starts_with("modules/frontend/src/"),
        "{label}: site outside legacy frontend: {path}"
    );
    assert!(
        !path.starts_with("modules/frontend/src/modules/frontend/src/"),
        "{label}: doubled frontend prefix: {path}"
    );
}

// --------------------------------------------------- resolver layout tests

/// Creates `<root>/<prefix>/modules/assets/manifest.toml` with placeholder
/// bytes so tree probing has a real artifact to find. Uses disposable temp
/// directories only — never junctions into the repository.
fn make_tree_layout(root: &std::path::Path, prefix: &str) {
    let probe = root.join(prefix).join(PROBE_REL);
    std::fs::create_dir_all(probe.parent().expect("probe parent")).expect("create synthetic tree");
    std::fs::write(&probe, b"schema_version = 1\n").expect("write probe");
}

fn synthetic_prefixes(custom: Option<&str>) -> Vec<String> {
    build_prefixes(custom)
}

#[test]
fn tree_layout_resolves_under_artisan_editor() {
    let prefixes = synthetic_prefixes(None);
    let root = std::env::temp_dir().join("artisan_assets_tree_ws_default");
    let _ = std::fs::remove_dir_all(&root);
    make_tree_layout(&root, "artisan_editor");

    assert_eq!(
        probe_tree_prefix(&root, &prefixes).as_deref(),
        Some("artisan_editor")
    );
    std::fs::remove_dir_all(&root).expect("cleanup synthetic tree");
}

#[test]
fn tree_layout_resolves_under_main() {
    let prefixes = synthetic_prefixes(None);
    let root = std::env::temp_dir().join("artisan_assets_tree_ws_main");
    let _ = std::fs::remove_dir_all(&root);
    // Only _main exists; the more specific name is absent.
    make_tree_layout(&root, "_main");

    assert_eq!(
        probe_tree_prefix(&root, &prefixes).as_deref(),
        Some("_main")
    );
    std::fs::remove_dir_all(&root).expect("cleanup synthetic tree");
}

#[test]
fn test_workspace_prefix_wins_over_defaults() {
    let prefixes = synthetic_prefixes(Some("custom_test_ws"));
    assert_eq!(prefixes.first().map(String::as_str), Some("custom_test_ws"));
    assert!(
        prefixes.windows(2).all(|pair| pair[0] != pair[1]),
        "prefixes must be unique"
    );

    let root = std::env::temp_dir().join("artisan_assets_tree_ws_custom");
    let _ = std::fs::remove_dir_all(&root);
    // Both custom and default layouts exist; specificity must win.
    make_tree_layout(&root, "_main");
    make_tree_layout(&root, "custom_test_ws");

    assert_eq!(
        probe_tree_prefix(&root, &prefixes).as_deref(),
        Some("custom_test_ws")
    );
    std::fs::remove_dir_all(&root).expect("cleanup synthetic tree");
}

#[test]
fn partial_runfiles_dir_never_serves_a_probe() {
    let prefixes = synthetic_prefixes(None);
    let empty = std::env::temp_dir().join("artisan_assets_tree_empty");
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(&empty).expect("create empty dir");

    assert_eq!(probe_tree_prefix(&empty, &prefixes), None);
    std::fs::remove_dir_all(&empty).expect("cleanup empty dir");
}

#[test]
fn prefixes_preserve_exact_priority_order() {
    // Nonempty TEST_WORKSPACE first, then the module name, then _main.
    assert_eq!(
        synthetic_prefixes(Some("custom_test_ws")),
        vec![
            String::from("custom_test_ws"),
            String::from("artisan_editor"),
            String::from("_main")
        ]
    );
    // Without TEST_WORKSPACE the defaults follow in the same relative order.
    assert_eq!(
        synthetic_prefixes(None),
        vec![String::from("artisan_editor"), String::from("_main")]
    );
}

#[test]
fn duplicate_workspaces_collapse_without_reordering() {
    // TEST_WORKSPACE equal to a default collapses onto that default; later
    // defaults that are not yet present are still appended after it.
    assert_eq!(
        synthetic_prefixes(Some("artisan_editor")),
        vec![String::from("artisan_editor"), String::from("_main")]
    );
    assert_eq!(
        synthetic_prefixes(Some("_main")),
        vec![String::from("_main"), String::from("artisan_editor")]
    );
    // Empty TEST_WORKSPACE is treated as absent.
    assert_eq!(
        build_prefixes(Some("")),
        vec![String::from("artisan_editor"), String::from("_main")]
    );
}

#[test]
fn currentcolor_is_ignored_not_counted_as_a_paint() {
    let monochrome_for = |body: &str| {
        let xml = format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{body}</svg>");
        let doc = roxmltree::Document::parse(&xml).expect("fixture parses");
        derive_monochrome(&doc)
    };

    // A currentColor-only mark stays monochrome under any case spelling.
    for spelling in ["currentColor", "CURRENTCOLOR", "currentcolor"] {
        assert!(
            monochrome_for(&format!(
                "<rect fill=\"{spelling}\" stroke=\"{spelling}\"/>"
            )),
            "{spelling}: must not make the artwork polychrome"
        );
    }

    // Discriminating case: currentColor beside one literal color must yield
    // exactly that literal; counting currentColor would report two paints
    // and flip this to polychrome.
    assert!(
        monochrome_for("<rect fill=\"currentColor\"/><circle fill=\"#ffffff\"/>"),
        "currentColor must be ignored next to one literal paint"
    );

    // Two distinct literals remain polychrome (control).
    assert!(
        !monochrome_for("<rect fill=\"#000000\"/><circle fill=\"#ffffff\"/>"),
        "control: two distinct literals are polychrome"
    );
}
#[test]
fn manifest_mode_maps_prefixed_keys_for_every_workspace() {
    for prefix in ["artisan_editor", "_main", "custom_test_ws"] {
        let map = std::collections::BTreeMap::from([(
            format!("{prefix}/{PROBE_REL}"),
            String::from("ignored-for-gating"),
        )]);
        assert!(
            manifest_serves_probe(&map, &synthetic_prefixes(Some(prefix))),
            "{prefix}: manifest gate failed"
        );
        // An unprefixed key is equally acceptable.
        let bare = std::collections::BTreeMap::from([(String::from(PROBE_REL), String::from("x"))]);
        assert!(manifest_serves_probe(
            &bare,
            &synthetic_prefixes(Some(prefix))
        ));
    }
}

#[test]
fn unrelated_manifest_does_not_gate_the_layout() {
    let unrelated = std::collections::BTreeMap::from([(
        String::from("some_other_workspace/other.txt"),
        String::from("x"),
    )]);
    assert!(
        !manifest_serves_probe(&unrelated, &synthetic_prefixes(None)),
        "an unrelated non-empty manifest must not count as serving the probe"
    );
}

#[test]
fn valid_manifest_beats_partial_runfiles_dir() {
    let prefixes = synthetic_prefixes(None);

    // Partial dir: exists but serves nothing.
    let partial = std::env::temp_dir().join("artisan_assets_tree_partial");
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).expect("create partial dir");

    // Valid manifest maps the probe under a supported workspace prefix.
    let manifest = Some(std::collections::BTreeMap::from([(
        format!("artisan_editor/{PROBE_REL}"),
        String::from("nowhere"),
    )]));

    match Resolver::pick(manifest.clone(), Some(&partial), &prefixes) {
        Some(Resolver::Manifest { .. }) => {}
        other => panic!("manifest must beat a partial dir, got {other:?}"),
    }

    // Without the manifest the same partial dir yields no layout at all.
    assert!(Resolver::pick(None, Some(&partial), &prefixes).is_none());

    // A directory that does serve the probe wins when no manifest exists.
    make_tree_layout(&partial, "artisan_editor");
    match Resolver::pick(None, Some(&partial), &prefixes) {
        Some(Resolver::Tree { prefix, .. }) => assert_eq!(prefix, "artisan_editor"),
        other => panic!("expected tree layout, got {other:?}"),
    }
    std::fs::remove_dir_all(&partial).expect("cleanup partial dir");
}

// ------------------------------------------------------------------- tests

#[test]
fn sha256_vectors_hold_for_the_pinned_crate() {
    use sha2::Digest as _;
    assert_eq!(
        hex_of(Sha256::digest(b"").as_slice()),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex_of(Sha256::digest(b"abc").as_slice()),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex_of(
            Sha256::digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").as_slice()
        ),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn catalog_is_sorted_unique_and_totally_covered_by_constants() {
    let all = artisan_assets::ALL;
    assert_eq!(all.len(), 104, "catalog size drifted");
    for pair in all.windows(2) {
        assert!(
            pair[0].id.as_str() < pair[1].id.as_str(),
            "catalog out of order: {} then {}",
            pair[0].id,
            pair[1].id,
        );
    }
    assert_eq!(AssetId::CONSTANTS.len(), all.len());
    for (constant, asset) in AssetId::CONSTANTS.iter().zip(all) {
        assert_eq!(constant.as_str(), asset.id.as_str());
        assert_eq!(catalog_get(*constant).source_path, asset.source_path);
    }
}

#[test]
fn lookups_are_deterministic_binary_searches_and_typed_for_unknowns() {
    let first = lookup("svgl.github").expect("svgl.github");
    for _ in 0..3 {
        assert_eq!(first.source, lookup("svgl.github").expect("ok").source);
        assert_eq!(first.source_path, catalog_get(first.id).source_path);
    }
    let error = lookup("not.an-asset").expect_err("must fail");
    assert_eq!(error.id, "not.an-asset");
    assert_eq!(error.to_string(), "unknown asset id `not.an-asset`");
    assert!("tabler.check".parse::<AssetId>().is_ok());
    assert!("tabler.missing".parse::<AssetId>().is_err());
}

#[test]
fn runtime_strings_can_only_become_ids_through_validation() {
    // The index inside AssetId is sealed: `from_id`/`locate` are private and
    // const, invoked exclusively by the catalog macro with string literals,
    // so an unknown id aborts compilation instead of existing at runtime.
    // Externally this invariant is observable exactly here: every dynamic
    // string either validates to a real catalog entry or fails typed, and no
    // public constructor accepts raw input.
    for candidate in [
        "",
        "tabler",
        ".check",
        "tabler.",
        "TABLER.CHECK",
        "tabler.check ",
        "tabler.does-not-exist",
        "../tabler.check",
    ] {
        assert!(lookup(candidate).is_err(), "`{candidate}` must not resolve");
        assert!(
            candidate.parse::<AssetId>().is_err(),
            "`{candidate}` must not parse"
        );
    }
    // Every constant still round-trips through the validated path.
    for constant in AssetId::CONSTANTS {
        let parsed = constant.as_str().parse::<AssetId>().expect("constant id");
        assert_eq!(parsed.as_str(), constant.as_str());
        assert_eq!(
            catalog_get(parsed).source_path,
            catalog_get(*constant).source_path
        );
    }
}

#[test]
fn families_match_id_prefixes_and_source_paths_everywhere() {
    for asset in artisan_assets::ALL {
        let (prefix, name) = asset.id.as_str().split_once('.').expect("family.name");
        assert_eq!(
            prefix,
            asset.family.prefix(),
            "{} family mismatch",
            asset.id
        );
        assert_eq!(
            asset.source_path,
            format!("svg/{prefix}/{name}.svg"),
            "{} source_path grammar",
            asset.id,
        );
    }
}

#[test]
fn manifest_rows_have_exact_fields_and_unique_ids_and_paths() {
    let resolver = Resolver::detect();
    let raw =
        std::fs::read(resolver.resolve("modules/assets/manifest.toml")).expect("manifest runfile");
    assert_eq!(
        raw,
        MANIFEST_TOML.as_bytes(),
        "physical manifest bytes differ from embedded MANIFEST_TOML"
    );
    let text = std::str::from_utf8(&raw).expect("manifest utf-8");
    let (assets, uses) = parse_manifest(text);
    assert_eq!(assets.len(), 104);
    assert_eq!(uses.len(), 91);

    let mut asset_ids: Vec<&str> = Vec::new();
    let mut source_paths: Vec<String> = Vec::new();
    for row in &assets {
        let id = field(row, "id");
        let origin_kind = field(row, "origin_kind");
        let mut keys = row_keys(row);
        keys.sort();

        // Allowed set: required base fields, optional notes, and the
        // package-origin pair only when origin_kind is "package". Unknown
        // extras are rejected; notes are allowed but never required.
        let mut allowed: Vec<&str> = ASSET_BASE_FIELDS.to_vec();
        allowed.extend_from_slice(ASSET_OPTIONAL_FIELDS);
        if origin_kind == "package" {
            allowed.extend_from_slice(ORIGIN_PACKAGE_FIELDS);
        }
        allowed.sort_unstable();
        allowed.dedup();
        for key in &keys {
            assert!(
                allowed.contains(&key.as_str()),
                "{id}: unexpected field `{key}`"
            );
        }
        let missing: Vec<&str> = ASSET_BASE_FIELDS
            .iter()
            .filter(|required| !keys.contains(&String::from(**required)))
            .copied()
            .collect();
        assert!(missing.is_empty(), "{id}: missing fields {missing:?}");

        match origin_kind {
            "package" => {
                assert!(field(row, "origin_package").starts_with('@'));
                assert!(!field(row, "origin_version").is_empty());
            }
            "local" => {
                assert!(!has_field(row, "origin_package"));
                assert!(!has_field(row, "origin_version"));
            }
            other => panic!("{id}: origin_kind `{other}` invalid"),
        }

        assert_eq!(id.matches('.').count(), 1, "id grammar: {id}");
        assert!(
            id.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-'),
            "id charset: {id}"
        );
        let prefix = id.split_once('.').expect("dot").0;
        assert_eq!(format!("{prefix}.{}", field(row, "name")), id);

        let digest = field(row, "sha256");
        assert_eq!(digest.len(), 64, "{id}: digest length");
        assert!(
            digest.chars().all(|c| c.is_ascii_hexdigit()) && digest == digest.to_ascii_lowercase(),
            "{id}: digest not lowercase hex"
        );

        asset_ids.push(id);
        source_paths.push(String::from(field(row, "source_path")));
    }

    // Duplicate detection: explicit before/dedup/after comparison.
    asset_ids.sort_unstable();
    let before_ids = asset_ids.len();
    asset_ids.dedup();
    let after_ids = asset_ids.len();
    assert_eq!(before_ids, after_ids, "duplicate asset ids");

    source_paths.sort();
    let before_paths = source_paths.len();
    source_paths.dedup();
    let after_paths = source_paths.len();
    assert_eq!(before_paths, after_paths, "duplicate source_path values");

    let mut use_ids: Vec<&str> = Vec::new();
    for row in &uses {
        let id = field(row, "id");
        let mut keys = row_keys(row);
        keys.sort();
        let mut expected: Vec<&str> = USE_FIELDS.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected, "{id}: exact [[use]] field set");
        use_ids.push(id);
    }
    use_ids.sort_unstable();
    let before_use_ids = use_ids.len();
    use_ids.dedup();
    assert_eq!(before_use_ids, use_ids.len(), "duplicate use ids");
}

#[test]
fn all_recorded_paths_are_normalized_without_escape() {
    let resolver = Resolver::detect();
    let text = std::fs::read_to_string(resolver.resolve("modules/assets/manifest.toml"))
        .expect("manifest readable");
    let (assets, uses) = parse_manifest(&text);

    for row in &assets {
        let id = field(row, "id");
        let source_path = field(row, "source_path");
        normalized_repo_rel(source_path, id);
        assert!(source_path.starts_with("svg/"), "{id}: {source_path}");
        assert!(
            resolver
                .resolve(&format!("modules/assets/{source_path}"))
                .exists(),
            "{id}: source_path does not resolve: {source_path}"
        );

        let license_file = field(row, "license_file");
        normalized_repo_rel(license_file, id);
        assert!(
            license_file.starts_with("licenses/"),
            "{id}: odd license path"
        );

        // Origins are normalized for every row regardless of kind; only
        // existence stays gated to the local-review checkout.
        let origin_path = field(row, "origin_path");
        normalized_repo_rel(origin_path, id);
        if field(row, "origin_kind") == "local" {
            assert!(
                origin_path.starts_with("modules/frontend/src/"),
                "{id}: local origin outside legacy tree: {origin_path}"
            );
            if resolver.is_local_review() {
                assert!(
                    resolver.resolve(origin_path).exists(),
                    "{id}: origin path does not exist: {origin_path}"
                );
            }
        }
    }

    for row in &uses {
        let id = field(row, "id");
        for site in array_field(row, "sites") {
            frontend_site(&site, id);
            if resolver.is_local_review() {
                assert!(
                    resolver.resolve(&site).exists(),
                    "{id}: use-site path does not exist: {site}"
                );
            }
        }
    }
}

#[test]
fn physical_files_biject_with_build_manifest_and_api() {
    let resolver = Resolver::detect();

    let physical = resolver.physical_svgs();

    let build_text = std::fs::read_to_string(resolver.resolve("modules/assets/BUILD.bazel"))
        .expect("BUILD.bazel runfile readable");
    let mut build_sources = parse_build_list(&build_text, "ASSET_SOURCES");
    build_sources.sort();

    let text = std::fs::read_to_string(resolver.resolve("modules/assets/manifest.toml"))
        .expect("manifest readable");
    let (assets, _) = parse_manifest(&text);
    let mut manifest_paths: Vec<String> = assets
        .iter()
        .map(|row| String::from(field(row, "source_path")))
        .collect();
    manifest_paths.sort();

    let mut api_paths: Vec<&str> = artisan_assets::ALL
        .iter()
        .map(|asset| asset.source_path)
        .collect();
    api_paths.sort_unstable();

    assert_eq!(physical.len(), 104, "physical svg count");
    assert_eq!(build_sources.len(), 104, "BUILD ASSET_SOURCES count");
    assert_eq!(manifest_paths.len(), 104, "manifest source_path count");
    assert_eq!(api_paths.len(), 104, "API source_path count");

    assert_eq!(physical, build_sources, "physical vs BUILD ASSET_SOURCES");
    assert_eq!(
        physical, manifest_paths,
        "physical vs manifest source_paths"
    );
    assert_eq!(physical, api_paths, "physical vs API Asset.source_path");
}

#[test]
fn physical_bytes_equal_embedded_source_and_raw_digest() {
    let resolver = Resolver::detect();
    let text = std::fs::read_to_string(resolver.resolve("modules/assets/manifest.toml"))
        .expect("manifest readable");
    let (assets, _) = parse_manifest(&text);
    assert_eq!(assets.len(), artisan_assets::ALL.len());

    for row in &assets {
        let id = field(row, "id");
        let source_path = field(row, "source_path");
        let asset = lookup(id).unwrap_or_else(|e| panic!("manifest row missing from API: {e}"));

        // Manifest row must agree with the API's own recorded path.
        assert_eq!(source_path, asset.source_path, "{id}: source_path drift");

        let bytes = std::fs::read(resolver.resolve(&format!("modules/assets/{source_path}")))
            .unwrap_or_else(|e| panic!("{id}: read {source_path}: {e}"));
        let physical = std::str::from_utf8(&bytes)
            .unwrap_or_else(|e| panic!("{id}: source is not utf-8: {e}"));

        assert_eq!(
            physical, asset.source,
            "{id}: physical bytes != embedded source"
        );
        assert_eq!(
            field(row, "sha256"),
            sha256_hex(&bytes),
            "{id}: raw-byte digest drift"
        );

        let doc = roxmltree::Document::parse(physical)
            .unwrap_or_else(|e| panic!("{id}: invalid XML: {e}"));
        let recorded = field(row, "view_box");
        let found = root_view_box(&doc);
        if let Some(actual) = found {
            assert_eq!(recorded, actual, "{id}: manifest vs file viewBox");
            assert_eq!(asset.view_box, Some(actual), "{id}: API vs file viewBox");
            assert!(view_box_is_well_formed(actual), "{id}: viewBox `{actual}`");
        } else {
            assert_eq!(recorded, "", "{id}: unexpected empty viewBox");
            assert_eq!(asset.view_box, None);
        }

        assert_eq!(
            bool_field(row, "monochrome"),
            derive_monochrome(&doc),
            "{id}: monochrome derivation mismatch"
        );

        assert_safe_svg(physical, id);
    }
}

#[test]
fn presentation_policy_is_independent_of_monochrome_with_exactly_two_exceptions() {
    // docs/ui/ASSETS.md §10: `monochrome` is the validator-derived artwork
    // property, while legacy EngineMark/RepositoryMark flags were rendering
    // policy ("single-color logo that must invert with the theme"). The
    // catalog records native presentation separately from artwork structure:
    // the default derives from `monochrome`, overridden by exactly the brand
    // marks whose legacy call sites proved their authored single-hue colors
    // must survive — Claude clay #D97757 (engine `claude`, provider
    // `anthropic`) and DeepSeek blue #4D6BFE (provider `deepseek`).
    const AUTHORED_COLOR_EXCEPTIONS: [&str; 2] = ["svgl.claude-ai", "svgl.deepseek"];

    let mut tinted = 0usize;
    let mut full_color = 0usize;
    let mut monochrome_tinted = 0usize;
    let mut monochrome_full_color: Vec<&str> = Vec::new();

    for asset in artisan_assets::ALL {
        let presentation = catalog_get(asset.id).presentation;
        match presentation {
            Presentation::Tinted => tinted += 1,
            Presentation::FullColor => full_color += 1,
        }
        if asset.monochrome {
            match presentation {
                Presentation::Tinted => monochrome_tinted += 1,
                Presentation::FullColor => monochrome_full_color.push(asset.id.as_str()),
            }
        } else {
            assert_eq!(
                presentation,
                Presentation::FullColor,
                "{}: polychrome artwork must render full-color",
                asset.id
            );
        }

        // Determinism: both catalog paths agree for every id.
        let via_lookup = lookup(asset.id.as_str())
            .expect("catalog id resolves")
            .presentation;
        assert_eq!(
            presentation, via_lookup,
            "{}: presentation is deterministic across lookups",
            asset.id
        );
    }

    // The divergence set is exactly the two evidenced exceptions: no other
    // monochrome asset may bypass theme tinting.
    monochrome_full_color.sort_unstable();
    assert_eq!(
        monochrome_full_color, AUTHORED_COLOR_EXCEPTIONS,
        "authored-color overrides beyond the evidenced brand marks"
    );

    // Exhaustive counts over all 104 ids: 12 polychrome artworks plus the two
    // authored-color exceptions render full-color; every other asset tints.
    assert_eq!(artisan_assets::ALL.len(), 104);
    assert_eq!(full_color, 14);
    assert_eq!(tinted, 90);
    assert_eq!(monochrome_tinted, 90);

    // The exceptions really carry their authored single-hue colors in the
    // embedded bytes while their structural monochrome stays true.
    for id in AUTHORED_COLOR_EXCEPTIONS {
        let asset = lookup(id).expect("exception id resolves");
        assert!(asset.monochrome, "{id}: artwork monochrome stays true");
        assert_eq!(asset.presentation, Presentation::FullColor);
    }
    let claude = lookup("svgl.claude-ai").expect("claude-ai");
    assert!(claude.source.contains("#D97757"), "authored clay missing");
    let deepseek = lookup("svgl.deepseek").expect("deepseek");
    assert!(deepseek.source.contains("#4D6BFE"), "authored blue missing");

    // Ordinary Tabler/currentColor controls stay tinted. currentColor brand
    // artwork (`svgl.qwen`) adapts through text color exactly like its legacy
    // rendering and must not be flipped to full-color raster, which would pin
    // it to black.
    let check = lookup("tabler.check").expect("control glyph");
    assert!(check.monochrome);
    assert_eq!(check.presentation, Presentation::Tinted);
    let qwen = lookup("svgl.qwen").expect("qwen");
    assert!(qwen.monochrome && qwen.source.contains("currentColor"));
    assert_eq!(qwen.presentation, Presentation::Tinted);
}

#[test]
fn use_sites_reference_known_assets_and_link_the_whole_catalog() {
    const CHANNELS: &[&str] = &[
        "tabler-direct",
        "tabler-barrel",
        "tabler-direct+barrel",
        "svgl",
        "checked-in-svg",
        "brand-component",
        "inline-component",
        "generated",
        "css-mask",
        "dormant",
    ];
    const REACHABILITY: &[&str] = &["shipped", "dev-only", "dormant"];
    const CLASSIFICATIONS: &[&str] = &[
        "static-vendored",
        "data-driven-native",
        "renderer-deferred",
        "shader-deferred",
        "dormant",
    ];

    let (assets, uses) = parse_manifest(MANIFEST_TOML);
    assert_eq!(assets.len(), 104);
    let mut linked: Vec<String> = Vec::new();
    let mut shader_deferred = 0usize;

    for row in &uses {
        let id = field(row, "id");
        let channel = field(row, "channel");
        let reachability = field(row, "reachability");
        let classification = field(row, "classification");
        assert!(CHANNELS.contains(&channel), "{id}: channel {channel}");
        assert!(
            REACHABILITY.contains(&reachability),
            "{id}: reachability {reachability}"
        );
        assert!(
            CLASSIFICATIONS.contains(&classification),
            "{id}: classification {classification}"
        );
        if classification == "shader-deferred" {
            shader_deferred += 1;
        }

        let sites = array_field(row, "sites");
        assert!(!sites.is_empty(), "{id}: no sites");
        let linked_assets = array_field(row, "assets");
        if classification == "static-vendored" {
            assert!(
                !linked_assets.is_empty(),
                "{id}: static-vendored links no assets"
            );
        } else {
            assert!(
                linked_assets.is_empty(),
                "{id}: non-static use links assets"
            );
        }
        for asset_id in linked_assets {
            assert!(lookup(&asset_id).is_ok(), "{id}: unknown asset {asset_id}");
            linked.push(asset_id);
        }
    }
    assert_eq!(
        shader_deferred, 0,
        "shader deferrals require ASSETS.md evidence"
    );

    linked.sort();
    linked.dedup();
    let mut catalog: Vec<&str> = artisan_assets::ALL.iter().map(|a| a.id.as_str()).collect();
    catalog.sort_unstable();
    assert_eq!(linked.len(), catalog.len(), "linkage != catalog size");
    for (used, expected) in linked.iter().zip(catalog.iter()) {
        assert_eq!(used, expected, "linked set differs from catalog");
    }
}

#[test]
fn license_documents_are_exact_resolvable_and_nonempty() {
    let resolver = Resolver::detect();
    let build_text = std::fs::read_to_string(resolver.resolve("modules/assets/BUILD.bazel"))
        .expect("read BUILD");
    let mut build_docs = parse_build_list(&build_text, "LICENSE_DOCS");
    build_docs.sort();

    let mut api_docs: Vec<&str> = LICENSE_FILES.iter().map(|d| d.path).collect();
    api_docs.sort_unstable();
    assert_eq!(build_docs.len(), 9, "BUILD LICENSE_DOCS count");
    assert_eq!(build_docs, api_docs, "BUILD vs API license doc sets");

    for doc in LICENSE_FILES {
        assert!(!doc.contents.is_empty(), "{}: empty", doc.path);
        assert!(
            resolver
                .resolve(&format!("modules/assets/{}", doc.path))
                .exists(),
            "{}: not present among validation runfiles",
            doc.path
        );
    }

    let (assets, _) = parse_manifest(MANIFEST_TOML);
    for row in &assets {
        let id = field(row, "id");
        let spdx = field(row, "license_spdx");
        assert!(
            matches!(
                spdx,
                "MIT"
                    | "Apache-2.0"
                    | "CC0-1.0"
                    | "LicenseRef-Brand-Mark"
                    | "LicenseRef-First-Party"
            ),
            "{id}: unknown SPDX {spdx}"
        );
        let file = field(row, "license_file");
        normalized_repo_rel(file, id);
        assert!(
            file.starts_with("licenses/"),
            "{id}: odd license path {file}"
        );
        assert!(
            build_docs.contains(&String::from(file)),
            "{id}: license file {file} absent from BUILD LICENSE_DOCS"
        );
        assert!(license(file).is_some(), "{id}: license not embedded");
    }
}

#[test]
fn representative_metadata_fixtures_hold() {
    let check = lookup("tabler.check").expect("tabler.check");
    assert_eq!(check.view_box, Some("0 0 24 24"));
    assert!(check.monochrome);
    assert_eq!(check.source_path, "svg/tabler/check.svg");
    assert!(
        check
            .source
            .contains("stroke=\"currentColor\" stroke-width=\"2\"")
    );

    let star_filled = lookup("tabler.star-filled").expect("star-filled");
    assert!(
        star_filled
            .source
            .contains("fill=\"currentColor\" stroke=\"none\"")
    );

    assert!(lookup("tabler.minus").is_ok());
    assert!(lookup("tabler.chevron-up").is_ok());
    assert_eq!(AssetId::TABLER_CHECK.as_str(), check.id.as_str());
    assert_eq!(
        catalog_get(AssetId::SVGL_GITHUB).source_path,
        "svg/svgl/github.svg"
    );

    let github = lookup("svgl.github").expect("github");
    assert_eq!(github.view_box, Some("0 0 1024 1024"));
    assert!(github.monochrome);
    assert!(github.source.contains("#1b1f23"));

    let gradient = lookup("artisan.logo-gradient").expect("logo-gradient");
    assert!(!gradient.monochrome);
    assert!(gradient.source.contains("<linearGradient"));

    let app_icon = lookup("artisan.app-icon").expect("app-icon");
    assert!(!app_icon.monochrome);
    assert!(app_icon.source.contains("data:image/svg+xml;base64,"));

    let manifest_doc: toml::Table = MANIFEST_TOML.parse().expect("manifest parses");
    let svelte = manifest_doc
        .get("asset")
        .and_then(|v| v.as_array())
        .expect("asset rows")
        .iter()
        .find(|row| field(row, "id") == "jetbrains.svelte")
        .expect("jetbrains.svelte row");
    assert!(field(svelte, "notes").contains("Dual provenance"));

    let opencode = lookup("brands.opencode").expect("opencode");
    assert!(!opencode.monochrome);
    assert!(opencode.source.contains("#4B4646"));
}

// ------------------------------------------- adversarial safety validation

/// Asserts `assert_safe_svg` rejects each document (via panic unwind).
fn expect_forbidden(label: &str, document: &str) {
    let owned = String::from(document);
    let result = std::panic::catch_unwind(move || {
        assert_safe_svg(&owned, label);
    });
    assert!(result.is_err(), "{label}: forbidden construct was accepted");
}

#[test]
fn forbidden_variants_panic_including_whitespace_and_namespaced_forms() {
    // Plain root: no namespace prefixes declared, so any prefixed name here
    // would fail parsing rather than exercising the policy traversal.
    let open = "<svg xmlns=\"http://www.w3.org/2000/svg\">";
    // Namespace-valid root: xlink and svg prefixes declared so namespaced
    // negatives reach the policy traversal instead of the parser.
    let ns_open = "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:svg=\"http://www.w3.org/2000/svg\">";
    let close = "</svg>";

    // Declarations.
    expect_forbidden(
        "doctype",
        "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x.dtd\"><svg/>",
    );
    expect_forbidden("entity", "<!ENTITY grab ALL><svg><p>&grab;</p></svg>");
    expect_forbidden("doctype-inside", &format!("<svg/><!DOCTYPE html>{close}"));

    // Forbidden elements, including mixed case and a namespace-valid
    // namespaced form that the traversal itself must reject.
    expect_forbidden(
        "script-element",
        &format!("{open}<script>alert(1)</script>{close}"),
    );
    expect_forbidden(
        "script-mixed-case",
        &format!("{open}<ScRiPt>alert(1)</ScRiPt>{close}"),
    );
    expect_forbidden(
        "foreign-object",
        &format!("{open}<foreignObject><p>html</p></foreignObject>{close}"),
    );
    expect_forbidden(
        "foreign-object-namespaced",
        &format!("{ns_open}<svg:foreignObject/>{close}"),
    );

    // Event handlers: plain, mixed case, whitespace around '=', and
    // namespace-valid namespaced forms.
    for handler in [
        format!("{open}<rect onclick=\"boom()\"/>{close}"),
        format!("{open}<rect ONCLICK=\"boom()\"/>{close}"),
        format!("{open}<rect onclick = \"boom()\"/>{close}"),
        format!("{ns_open}<a xlink:onClick=\"boom()\"/>{close}"),
        format!("{open}<rect onclick= \"boom()\"/>{close}"),
    ] {
        expect_forbidden("event-handler", &handler);
    }

    // Reference rejections. Every bad value is exercised twice: once as a
    // plain href (no prefixes involved) and once as a namespace-valid
    // xlink:href so the attribute policy, not the parser, rejects it.
    for bad in [
        "javascript:alert(1)",
        "JavaScript:alert(1)",
        "file:///etc/passwd",
        "http://cdn.example/x.svg",
        "https://cdn.example/x.svg",
        "//protocol-relative.example/x.svg",
        "relative/path.svg",
        // Data forms outside the exact allowlisted prefix.
        "data:image/png;base64,aGVsbG8=",
        "data:text/html;base64,PHN2Zy8+",
        "data:image/svg+xml,PHN2Zy8+",
        // Case variants of the allowlisted prefix are rejected: the data
        // gate matches literal case-sensitive bytes only.
        "DATA:image/svg+xml;base64,PHN2Zy8+",
        "data:IMAGE/SVG+XML;base64,PHN2Zy8+",
        // Empty, whitespace-only, and bare-hash references.
        "",
        "   ",
        "#",
    ] {
        expect_forbidden(
            "href-plain",
            &format!("{open}<a href=\"{bad}\">x</a>{close}"),
        );
        expect_forbidden(
            "href-namespaced",
            &format!("{ns_open}<a xlink:href=\"{bad}\">x</a>{close}"),
        );
        expect_forbidden(
            "href-padded",
            &format!("{ns_open}<use xlink:href = \"{bad}\"/>{close}"),
        );
        expect_forbidden(
            "src-scheme",
            &format!("{open}<image src=\"{bad}\"/>{close}"),
        );
    }

    // Declaration policy: case variants are caught by the pre-parse scan;
    // malformed whitespace variants fall through to the no-DTD parser gate.
    for declaration in [
        "<!DOCTYPE svg SYSTEM \"x.dtd\">",
        "<!doctype svg SYSTEM \"x.dtd\">",
        "<!Entity grab ALL>",
        "<! ENTITY grab ALL>",
        "<!  DOCTYPE svg>",
    ] {
        expect_forbidden("declaration-policy", &format!("{declaration}{open}{close}"));
    }
}

#[test]
fn allowed_constructs_pass_the_safety_parser() {
    // XML declaration plus leading and internal comments.
    assert_safe_svg(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- provenance note -->\n<svg xmlns=\"http://www.w3.org/2000/svg\"><!-- internal --><path d=\"M0 0\"/></svg>",
        "decl-and-comments",
    );

    // Internal fragment links, including namespace-valid namespaced forms.
    assert_safe_svg(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"><a xlink:href=\"#top\"><circle r=\"4\"/></a></svg>",
        "fragment-link",
    );
    assert_safe_svg(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><use href=\"#glyph\"/></svg>",
        "bare-fragment",
    );

    // Harmless namespaced reference control: prefixes declared on the root,
    // internal-fragment value, no forbidden constructs — must pass.
    assert_safe_svg(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:svg=\"http://www.w3.org/2000/svg\"><svg:a xlink:href=\"#glyph\"><svg:rect width=\"2\" height=\"2\"/></svg:a></svg>",
        "namespaced-control",
    );

    // The exact nested base64 SVG payload prefix (the app-icon pattern).
    assert_safe_svg(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><image href=\"data:image/svg+xml;base64,PHN2Zy8+\"/></svg>",
        "data-image",
    );

    // Structural validity still enforced alongside policy.
    assert!(
        std::panic::catch_unwind(|| assert_safe_svg("<svg><rect></svg>", "unbalanced")).is_err(),
        "unclosed element must fail"
    );
}
