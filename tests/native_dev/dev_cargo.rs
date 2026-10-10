//! The Debug stage's Cargo builds: their arguments, target directories,
//! versions, and the payloads staged from their binaries.

use std::path::{Path, PathBuf};

use artisan_build_info::{BuildInfo, Channel, FORMAT_VERSION};
use native_dev::{
    cargo::{
        BINARIES, Build, FEATURES, PROFILE, cargo_arguments, dev_version, executable,
        output_directory, shell_attribute, stage_payload, target_directory, target_triple,
    },
    nix::{Checkout, Target},
    payload_identity,
};

fn checkout() -> Checkout {
    Checkout {
        root: "/home/ada/editor".into(),
    }
}

#[test]
fn each_build_keeps_its_own_target_directory() {
    let checkout = checkout();
    assert_eq!(
        target_directory(&checkout, Build::Payload(Target::Windows)),
        Path::new("/home/ada/editor/target/dev/windows")
    );
    assert_eq!(
        target_directory(&checkout, Build::Runner(Target::Windows)),
        Path::new("/home/ada/editor/target/dev/windows-runner")
    );
    assert_eq!(
        output_directory(&checkout, Build::Payload(Target::Windows)),
        Path::new("/home/ada/editor/target/dev/windows/x86_64-pc-windows-gnu/production-debug")
    );
    assert_eq!(shell_attribute(Target::Linux), "cargo-linux");
    assert_eq!(executable(Target::Windows, "dev"), "dev.exe");
    assert_eq!(executable(Target::Linux, "dev"), "dev");
}

#[test]
fn payload_builds_select_the_four_binaries_with_debug_features() {
    let arguments = cargo_arguments(&checkout(), Build::Payload(Target::Windows));
    let joined = arguments.join(" ");
    assert!(
        joined.starts_with(&format!(
            "build --locked --profile {PROFILE} --target x86_64-pc-windows-gnu --target-dir /home/ada/editor/target/dev/windows "
        )),
        "{joined}"
    );
    for (package, binary) in BINARIES {
        assert!(
            joined.contains(&format!("-p {package} --bin {binary}")),
            "{joined}"
        );
    }
    assert!(
        joined.ends_with(&format!("--features {}", FEATURES.join(","))),
        "{joined}"
    );
}

#[test]
fn runner_builds_select_only_the_runner() {
    let arguments = cargo_arguments(&checkout(), Build::Runner(Target::Linux));
    let joined = arguments.join(" ");
    assert!(
        joined.contains(&format!("--target {}", target_triple(Target::Linux))),
        "{joined}"
    );
    assert!(
        joined.ends_with("-p artisan-native-dev --bin dev"),
        "{joined}"
    );
    assert!(!joined.contains("--features"), "{joined}");
}

#[test]
fn every_build_is_its_own_version() {
    assert_eq!(
        dev_version("0.4.0", 1284, Some("1a2b3c4d5e6f7a8b"), true, 1_760_000_000),
        "0.4.0-dev.1284+g1a2b3c4d5e.dirty.c1760000000"
    );
    assert_eq!(
        dev_version("0.4.0", 0, None, false, 7),
        "0.4.0-dev.0+c7"
    );
    assert_ne!(
        dev_version("0.4.0", 1, Some("abc"), false, 1),
        dev_version("0.4.0", 1, Some("abc"), false, 2)
    );
}

fn scratch(case: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "artisan-native-dev-cargo-{case}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("scratch");
    directory
}

fn identity(version: &str) -> BuildInfo {
    BuildInfo {
        format_version: FORMAT_VERSION,
        version: version.to_owned(),
        channel: Channel::Dev,
        commit: None,
        dirty: false,
        profile: PROFILE.to_owned(),
        target: target_triple(Target::Windows),
        built_at: None,
    }
}

#[test]
fn staging_replaces_the_payload_with_the_binaries_and_identity() {
    let directory = scratch("stage");
    let binaries = directory.join("out");
    std::fs::create_dir_all(&binaries).expect("out");
    for (_, name) in BINARIES {
        std::fs::write(binaries.join(executable(Target::Windows, name)), name).expect("binary");
    }
    let payload = directory.join("payload");
    std::fs::create_dir_all(payload.join("bin")).expect("stale payload");
    std::fs::write(payload.join("bin/stale.exe"), "stale").expect("stale binary");

    stage_payload(&binaries, &payload, Target::Windows, &identity("0.0.0-dev.1+c1"))
        .expect("staged");

    assert!(!payload.join("bin/stale.exe").exists());
    for (_, name) in BINARIES {
        let staged = payload.join("bin").join(executable(Target::Windows, name));
        assert_eq!(std::fs::read_to_string(staged).expect("staged binary"), name);
    }
    assert_eq!(
        payload_identity(&payload).expect("identity"),
        identity("0.0.0-dev.1+c1")
    );
    std::fs::remove_dir_all(&directory).expect("cleanup");
}

#[test]
fn staging_refuses_missing_binaries() {
    let directory = scratch("missing");
    let binaries = directory.join("out");
    std::fs::create_dir_all(&binaries).expect("out");
    let refusal = stage_payload(
        &binaries,
        &directory.join("payload"),
        Target::Linux,
        &identity("0.0.0-dev.1+c1"),
    )
    .expect_err("refused")
    .to_string();
    assert!(refusal.contains("cannot stage"), "{refusal}");
    std::fs::remove_dir_all(&directory).expect("cleanup");
}
