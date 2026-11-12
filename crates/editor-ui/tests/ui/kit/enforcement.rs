use std::fs;
use std::path::{Path, PathBuf};

const ENFORCED: &[&str] = &["editor-sources"];

const LITERAL: &[&str] = &[
    "px(",
    "Length::Px(",
    ".gap(",
    ".padding(",
    ".radius(",
    ".font_size(",
];

const BANNED: &[&str] = &[
    "TextStyle {",
    "TextStyle::default()",
    "ProseTypography::",
    "Oklch::new(",
    "RadiusStep::",
    "ShadowLayer {",
    "GpuiShadowLayer {",
];

fn violations(source: &str) -> Vec<String> {
    let banned = BANNED
        .iter()
        .flat_map(|pattern| source.match_indices(pattern).map(move |(at, _)| (at, *pattern)));
    let literal = LITERAL.iter().flat_map(|pattern| {
        source
            .match_indices(pattern)
            .filter(move |(at, _)| {
                source[at + pattern.len()..]
                    .trim_start()
                    .starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.')
            })
            .map(move |(at, _)| (at, *pattern))
    });
    let mut found = banned
        .chain(literal)
        .map(|(at, pattern)| {
            let line = source[..at].lines().count().max(1);
            format!("line {line}: {pattern}")
        })
        .collect::<Vec<_>>();
    found.sort();
    found
}

fn crates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn sources(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .flat_map(|path| match path.is_dir() {
            true => sources(&path),
            false if path.extension().is_some_and(|ext| ext == "rs") => vec![path],
            false => Vec::new(),
        })
        .collect()
}

#[test]
fn enforced_crates_carry_the_canonical_clippy_bans() {
    let canonical = fs::read_to_string(crates().join("editor-ui/feature-clippy.toml"));
    assert!(canonical.is_ok(), "the canonical ban list is missing");
    let canonical = canonical.unwrap_or_default();
    for name in ENFORCED {
        let local = fs::read_to_string(crates().join(name).join("clippy.toml")).unwrap_or_default();
        assert_eq!(local, canonical, "{name}/clippy.toml drifted from editor-ui/feature-clippy.toml");
    }
}

#[test]
fn enforced_crates_contain_no_hand_rolled_styles() {
    let offenders = ENFORCED
        .iter()
        .flat_map(|name| sources(&crates().join(name).join("src")))
        .flat_map(|path| {
            let source = fs::read_to_string(&path).unwrap_or_default();
            violations(&source)
                .into_iter()
                .map(move |found| format!("{}: {found}", path.display()))
        })
        .collect::<Vec<_>>();
    assert!(offenders.is_empty(), "use the editor_ui::kit instead:\n{}", offenders.join("\n"));
}

#[test]
fn the_scanner_catches_what_clippy_cannot_see() {
    let source = "let a = px(8.0);\nlet b = Style::new().gap(Space::S8.px());\nlet c = TextStyle {\n};\nlet d = x.radius( 4.4);";
    assert_eq!(
        violations(source),
        vec![
            "line 1: px(".to_owned(),
            "line 3: TextStyle {".to_owned(),
            "line 5: .radius(".to_owned(),
        ],
        "literal numbers and struct literals are flagged, token calls are not"
    );
}

#[test]
fn the_scanner_ignores_token_driven_geometry() {
    let source = "Surface::Menu.build(&theme, rows).style(Style::new().width(Length::Px(width)))";
    assert!(violations(source).is_empty(), "variables and tokens are allowed");
}
