use super::*;

#[expect(
    clippy::unwrap_used,
    reason = "a test fixture; a failure fails the test"
)]
fn stage_build(stage: &Path, version: &str) -> Vec<u8> {
    let os = stage.join(std::env::consts::OS);
    std::fs::create_dir_all(&os).unwrap();
    let mut ae = std::fs::read(env!("CARGO_BIN_EXE_ae")).unwrap();
    ae.extend_from_slice(format!("build {version}").as_bytes());
    std::fs::write(os.join(App::Ae.binary_name()), &ae).unwrap();
    std::fs::write(os.join("VERSION"), format!("{version}\n")).unwrap();
    ae
}

#[expect(
    clippy::unwrap_used,
    reason = "a test fixture; a failure fails the test"
)]
fn through_the_editor(home: &Path, stage: &Path, key: &SigningKey, args: &[&str]) -> Output {
    let mut command = ae();
    isolate(&mut command, home);
    command
        .args(args)
        .env(
            "ARTISAN_RELEASE_PUBLIC_KEY_HEX",
            hex(&key.verifying_key().to_bytes()),
        )
        .env("ARTISAN_TEST_CARRIED_AE", env!("CARGO_BIN_EXE_ae"))
        .env("ARTISAN_TEST_CARRIED_STAGE", stage)
        .output()
        .unwrap()
}

#[test]
#[ignore = "writes the Windows user's registry (PATH, uninstall entry); CI runs it on a throwaway runner"]
fn a_remote_feed_installs_through_the_editor_and_later_updates_from_it() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join("release");
    let stage = home.path().join("stage");
    let key = SigningKey::from_bytes(&[7; 32]);
    let (feed, server) = serve(directory.clone());
    let app = App::Ae.dir_under(&artisan_street(home.path()));
    let installed = || std::fs::read(app.join(App::Ae.binary_name())).unwrap();
    let first = stage_build(&stage, "1.0.0");
    publish(&directory, &key, "1.0.0");

    let output = through_the_editor(
        home.path(),
        &stage,
        &key,
        &[
            "install", "--forge", "none", "--editor", "none", "--yes", "--from", &feed,
        ],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(installed() == first, "the Editor's carried ae is installed");
    assert_eq!(
        std::fs::read_to_string(app.join("release-feed"))
            .unwrap()
            .trim(),
        feed,
        "the feed is remembered for updates"
    );

    let newer = stage_build(&stage, "1.1.0");
    publish(&directory, &key, "1.1.0");
    let output = through_the_editor(home.path(), &stage, &key, &["update", "--yes"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(installed() == newer, "the update installed the newer ae");
    assert_eq!(
        std::fs::read_to_string(app.join("version")).unwrap().trim(),
        "1.1.0"
    );
    drop(server);
}
