const CARGO_BAZEL_LOCK: &str = include_str!("../../Cargo.Bazel.lock");
const CARGO_LOCK: &str = include_str!("../../Cargo.lock");
const ROOT_MANIFEST: &str = include_str!("../../Cargo.toml");
const WORKSPACE_MANIFESTS: &[(&str, &str)] = &[
    (
        "modules/assets/Cargo.toml",
        include_str!("../../modules/assets/Cargo.toml"),
    ),
    (
        "modules/backend/Cargo.toml",
        include_str!("../../modules/backend/Cargo.toml"),
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
        "modules/migrations/Cargo.toml",
        include_str!("../../modules/migrations/Cargo.toml"),
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
];

#[test]
fn resolved_product_graph_contains_no_anyhow_package() {
    let forbidden_package = ["name", "=", "\"anyhow\""];
    let forbidden_package = forbidden_package.join(" ");

    assert!(
        CARGO_LOCK
            .lines()
            .all(|line| line.trim() != forbidden_package),
        "Cargo.lock contains the prohibited anyhow package"
    );
}

#[test]
fn crate_universe_graph_contains_no_anyhow_package() {
    let forbidden_name = ["\"name\"", ":", "\"anyhow\""].join(" ");

    assert!(
        !CARGO_BAZEL_LOCK
            .lines()
            .any(|line| line.trim() == forbidden_name),
        "Cargo.Bazel.lock contains the prohibited anyhow package"
    );
}

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
