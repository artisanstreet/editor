use super::*;

#[test]
fn names_compound_extensions_and_modes_follow_the_theme() {
    assert_eq!(
        file(Path::new("Cargo.toml"), false),
        AssetId::JETBRAINS_2023_CARGO
    );
    assert_eq!(
        file(Path::new("Cargo.toml"), true),
        AssetId::JETBRAINS_2023_CARGO_DARK
    );
    assert_eq!(
        file(Path::new("src/main.rs"), false),
        AssetId::JETBRAINS_2023_RUSTFILE
    );
    assert_eq!(
        file(Path::new("component.test.ts"), true),
        AssetId::JETBRAINS_2023_TSTEST_DARK
    );
    assert_eq!(
        file(Path::new("component.ts"), true),
        AssetId::JETBRAINS_2023_TYPESCRIPT_DARK
    );
    assert_eq!(file(Path::new("unknown.zzz"), false), LIGHT_FILE);
}

#[test]
fn every_bundled_icon_and_mapping_is_in_the_asset_catalog() {
    let assets: Vec<_> = crate::ALL
        .iter()
        .filter(|asset| asset.id.as_str().starts_with("jetbrains.theme2023-"))
        .collect();
    assert_eq!(assets.len(), 210);
    assert!(assets.iter().all(|asset| asset.source.contains("<svg")));
    for table in [LIGHT_NAMES, LIGHT_EXTENSIONS, DARK_NAMES, DARK_EXTENSIONS] {
        assert!(table.iter().all(|(_, id)| crate::get(*id).id == *id));
    }
}
