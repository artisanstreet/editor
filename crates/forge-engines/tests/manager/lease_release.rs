use super::*;

#[test]
fn dropping_binary_lease_releases_lock_with_an_inherited_descriptor_open() {
    let directory = tempfile::tempdir().unwrap();
    let engine = EngineId::Codex;
    let layout = Layout::new(directory.path());
    let binary = layout
        .version_dir(engine, "0.150.0")
        .join(host_spec(engine).artifact.unwrap().entry);
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(&binary, "lease fixture").unwrap();
    install::switch(&layout, engine, "0.150.0").unwrap();
    let engines = Engines::new(directory.path());
    let lease = engines.lease(engine).unwrap();
    let inherited = lease.usage.try_clone().unwrap();
    let competing = install::usage_lock(&layout.home(engine)).unwrap();
    assert!(fs2::FileExt::try_lock_exclusive(&competing).is_err());
    drop(lease);
    fs2::FileExt::try_lock_exclusive(&competing)
        .expect("the ended owner releases its lease even before the inherited handle closes");
    drop(inherited);
}
