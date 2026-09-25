//! Build identity: stored in the payload, read back from the executable's
//! location, and reported honestly when absent.

use std::{fs, path::Path};

use artisan_build_info::{
    BuildIdentity, BuildInfo, BuildInfoError, Channel, FORMAT_VERSION, RESOURCE_PATH,
};

fn info(channel: Channel, commit: Option<&str>, dirty: bool) -> BuildInfo {
    BuildInfo {
        format_version: FORMAT_VERSION,
        version: "0.4.0-dev.12+g1a2b3c4d5e".to_owned(),
        channel,
        commit: commit.map(str::to_owned),
        dirty,
        profile: "dev".to_owned(),
        target: "x86_64-unknown-linux-gnu".to_owned(),
        built_at: None,
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn write_payload(version_root: &Path, info: &BuildInfo) {
    fs::create_dir_all(version_root.join("bin")).unwrap();
    fs::create_dir_all(version_root.join("resources")).unwrap();
    fs::write(version_root.join(RESOURCE_PATH), info.to_json()).unwrap();
}

#[test]
fn documents_round_trip_through_the_payload() {
    let directory = tempfile::tempdir().unwrap();
    let expected = info(Channel::Dev, Some("1a2b3c4d5e6f7a8b9c0d"), true);
    write_payload(directory.path(), &expected);
    assert_eq!(BuildInfo::read(directory.path()).unwrap(), expected);
}

#[test]
fn unknown_fields_and_formats_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("resources")).unwrap();
    let path = directory.path().join(RESOURCE_PATH);

    let mut value = serde_json::to_value(info(Channel::Stable, None, false)).unwrap();
    value["unexpected"] = serde_json::json!(true);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(matches!(
        BuildInfo::read(directory.path()),
        Err(BuildInfoError::Invalid(_))
    ));

    let mut value = serde_json::to_value(info(Channel::Stable, None, false)).unwrap();
    value["format_version"] = serde_json::json!(2);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(matches!(
        BuildInfo::read(directory.path()),
        Err(BuildInfoError::UnsupportedFormat(2))
    ));
}

#[test]
fn a_versioned_binary_reads_its_own_version_root() {
    let directory = tempfile::tempdir().unwrap();
    let version_root = directory.path().join("versions").join("0.4.0");
    let expected = info(Channel::Nightly, Some("abcdef0123456789"), false);
    write_payload(&version_root, &expected);

    let identity = BuildIdentity::for_executable(&version_root.join("bin").join(exe("editor")));
    assert_eq!(identity, BuildIdentity::Installed(expected));
}

#[test]
fn the_permanent_launcher_reads_the_active_version() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let active = info(Channel::Dev, Some("feedface00112233"), false);
    write_payload(&root.join("versions").join("active"), &active);
    write_payload(
        &root.join("versions").join("older"),
        &info(Channel::Dev, Some("0000000000000000"), false),
    );
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(
        root.join("installation.json"),
        serde_json::to_vec(&serde_json::json!({ "active_version": "active" })).unwrap(),
    )
    .unwrap();

    let identity = BuildIdentity::for_executable(&root.join("bin").join(exe("ae")));
    assert_eq!(identity, BuildIdentity::Installed(active));
}

#[test]
fn an_unsafe_active_version_is_never_followed() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    write_payload(
        &directory.path().join("escaped"),
        &info(Channel::Stable, None, false),
    );
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(
        root.join("installation.json"),
        serde_json::to_vec(&serde_json::json!({ "active_version": "../../escaped" })).unwrap(),
    )
    .unwrap();

    let identity = BuildIdentity::for_executable(&root.join("bin").join(exe("ae")));
    assert!(matches!(identity, BuildIdentity::Unstaged(_)));
}

#[test]
fn binaries_outside_a_payload_report_themselves_as_unstaged() {
    let directory = tempfile::tempdir().unwrap();
    let identity =
        BuildIdentity::for_executable(&directory.path().join("target/debug").join(exe("editor")));
    let BuildIdentity::Unstaged(build) = &identity else {
        panic!("a raw cargo binary must not claim an installed identity");
    };
    assert_eq!(build.package_version, env!("CARGO_PKG_VERSION"));
    let line = identity.to_string();
    assert!(line.contains("unstaged"), "{line}");
    assert!(
        identity
            .title_marker()
            .is_some_and(|marker| marker.starts_with("Unstaged"))
    );
}

#[test]
fn only_stable_builds_omit_the_title_marker() {
    let stable = BuildIdentity::Installed(info(Channel::Stable, Some("1a2b3c4d5e6f"), false));
    assert_eq!(stable.title_marker(), None);

    let dev = BuildIdentity::Installed(info(Channel::Dev, Some("1a2b3c4d5e6f"), true));
    assert_eq!(dev.title_marker().as_deref(), Some("Dev 1a2b3c4d5e+"));
    assert_eq!(dev.badge().as_deref(), Some("Dev 1a2b3c4+"));

    let nightly = BuildIdentity::Installed(info(Channel::Nightly, None, false));
    assert_eq!(nightly.title_marker().as_deref(), Some("Nightly"));
    assert_eq!(nightly.badge().as_deref(), Some("Nightly"));
    assert_eq!(stable.badge(), None);
    assert_eq!(
        BuildIdentity::Unstaged(artisan_build_info::UnstagedBuild::this_binary()).badge(),
        None
    );
}

#[test]
fn version_lines_name_version_channel_commit_and_profile() {
    let identity = BuildIdentity::Installed(info(Channel::Dev, Some("1a2b3c4d5e6f"), true));
    assert_eq!(
        identity.to_string(),
        "0.4.0-dev.12+g1a2b3c4d5e (dev channel, commit 1a2b3c4d5e with uncommitted changes, \
         dev profile, x86_64-unknown-linux-gnu)"
    );
    assert_eq!(identity.version(), "0.4.0-dev.12+g1a2b3c4d5e");
}
