const ROOT_MANIFEST: &str = include_str!("../../Cargo.toml");
const WORKSPACE_MANIFESTS: &[(&str, &str)] = &[
    (
        "packaging/Cargo.toml",
        include_str!("../../packaging/Cargo.toml"),
    ),
    (
        "scripts/screen_demo/Cargo.toml",
        include_str!("../../scripts/screen_demo/Cargo.toml"),
    ),
    (
        "modules/assets/Cargo.toml",
        include_str!("../../modules/assets/Cargo.toml"),
    ),
    (
        "modules/backend/Cargo.toml",
        include_str!("../../modules/backend/Cargo.toml"),
    ),
    (
        "modules/catalog/Cargo.toml",
        include_str!("../../modules/catalog/Cargo.toml"),
    ),
    (
        "modules/cli/Cargo.toml",
        include_str!("../../modules/cli/Cargo.toml"),
    ),
    (
        "modules/database/Cargo.toml",
        include_str!("../../modules/database/Cargo.toml"),
    ),
    (
        "modules/domain/Cargo.toml",
        include_str!("../../modules/domain/Cargo.toml"),
    ),
    (
        "modules/frontend/Cargo.toml",
        include_str!("../../modules/frontend/Cargo.toml"),
    ),
    (
        "modules/installer/Cargo.toml",
        include_str!("../../modules/installer/Cargo.toml"),
    ),
    (
        "modules/migrations/Cargo.toml",
        include_str!("../../modules/migrations/Cargo.toml"),
    ),
    (
        "modules/native_engine/Cargo.toml",
        include_str!("../../modules/native_engine/Cargo.toml"),
    ),
    (
        "modules/protocol/Cargo.toml",
        include_str!("../../modules/protocol/Cargo.toml"),
    ),
    (
        "modules/transport/Cargo.toml",
        include_str!("../../modules/transport/Cargo.toml"),
    ),
    (
        "modules/ui/Cargo.toml",
        include_str!("../../modules/ui/Cargo.toml"),
    ),
    (
        "scripts/capnp_codegen/Cargo.toml",
        include_str!("../../scripts/capnp_codegen/Cargo.toml"),
    ),
    (
        "scripts/native_dev/Cargo.toml",
        include_str!("../../scripts/native_dev/Cargo.toml"),
    ),
];

#[test]
fn workspace_manifests_do_not_declare_anyhow() {
    let forbidden_dependency = ["any", "how"].concat();

    for (path, manifest) in
        std::iter::once(("Cargo.toml", ROOT_MANIFEST)).chain(WORKSPACE_MANIFESTS.iter().copied())
    {
        assert!(
            !manifest
                .lines()
                .map(str::trim)
                .filter(|line| !line.starts_with('#'))
                .any(|line| line.contains(&forbidden_dependency)),
            "{path} declares the prohibited anyhow dependency"
        );
    }
}
