use super::*;

#[test]
fn installing_a_remote_feed_keeps_it_for_later_updates() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join("release");
    let key = SigningKey::from_bytes(&[7; 32]);
    let (feed, server) = serve(directory.clone());
    publish(&directory, &key, "1.0.0");
    let mut command = ae();
    isolate(&mut command, home.path());
    let output = command
        .args([
            "install", "--forge", "none", "--editor", "none", "--yes", "--from", &feed,
        ])
        .env(
            "ARTISAN_RELEASE_PUBLIC_KEY_HEX",
            hex(&key.verifying_key().to_bytes()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let app = App::Ae.dir_under(&artisan_street(home.path()));
    assert_eq!(
        std::fs::read_to_string(app.join("release-feed"))
            .unwrap()
            .trim(),
        feed
    );
    let updated = publish(&directory, &key, "1.1.0");
    let mut command = ae();
    isolate(&mut command, home.path());
    let output = command
        .args(["update", "--yes"])
        .env(
            "ARTISAN_RELEASE_PUBLIC_KEY_HEX",
            hex(&key.verifying_key().to_bytes()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        std::fs::read(app.join("ae")).unwrap() == updated,
        "the update installed the newer release's ae"
    );
    assert_eq!(
        std::fs::read_to_string(app.join("version")).unwrap().trim(),
        env!("CARGO_PKG_VERSION"),
        "the recorded version is the installer's own"
    );
    drop(server);
}

#[expect(
    clippy::unwrap_used,
    reason = "a test fixture; a failure fails the test"
)]
fn run_ae(home: &Path, key: &SigningKey, args: &[&str]) -> Output {
    let mut command = ae();
    isolate(&mut command, home);
    command
        .args(args)
        .env(
            "ARTISAN_RELEASE_PUBLIC_KEY_HEX",
            hex(&key.verifying_key().to_bytes()),
        )
        .output()
        .unwrap()
}

#[expect(
    clippy::unwrap_used,
    reason = "a test fixture; a failure fails the test"
)]
fn plant_their_ae(home: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let theirs = home.join("home/.local/bin/ae");
    std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
    if theirs.symlink_metadata().is_ok() {
        std::fs::remove_file(&theirs).unwrap();
    }
    std::fs::write(&theirs, "#!/bin/sh\necho my own ae\n").unwrap();
    std::fs::set_permissions(&theirs, std::fs::Permissions::from_mode(0o755)).unwrap();
    theirs
}

#[test]
fn agreeing_to_replace_an_ae_that_isnt_artisans_carries_through_to_the_releases_installer() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join("release");
    let key = SigningKey::from_bytes(&[7; 32]);
    let (feed, server) = serve(directory.clone());
    let ours = App::Ae.dir_under(&artisan_street(home.path())).join("ae");
    publish(&directory, &key, "1.0.0");
    let theirs = plant_their_ae(home.path());

    let installed = run_ae(
        home.path(),
        &key,
        &[
            "install", "--forge", "none", "--editor", "none", "--yes", "--from", &feed,
        ],
    );

    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert_eq!(std::fs::read_link(&theirs).unwrap(), ours);

    plant_their_ae(home.path());
    publish(&directory, &key, "1.1.0");

    let updated = run_ae(home.path(), &key, &["update", "--yes"]);

    assert!(
        updated.status.success(),
        "{}",
        String::from_utf8_lossy(&updated.stderr)
    );
    assert_eq!(std::fs::read_link(&theirs).unwrap(), ours);
    drop(server);
}
