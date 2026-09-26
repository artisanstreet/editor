use std::{collections::HashMap, ffi::OsString, fs};

use sha2::{Digest, Sha256};

use super::*;
use crate::engine_core::{ManagedGeneration, ManagedToolchainState};

fn host(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let variables: HashMap<String, OsString> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
        .collect();
    move |name| variables.get(name).cloned()
}

fn lookup<'a>(environment: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsString> {
    environment
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

#[cfg(unix)]
#[test]
fn a_claude_on_path_is_never_resolved() {
    let root = tempfile::tempdir().unwrap();
    let shims = root.path().join("shims");
    fs::create_dir(&shims).unwrap();
    fs::write(shims.join("claude"), b"#!/bin/sh\necho fake\n").unwrap();
    let database = root.path().join("state").join("forge.db");
    fs::create_dir_all(database.parent().unwrap()).unwrap();
    let path = shims.to_string_lossy().into_owned();
    let result = resolve_launch_target_in(
        ManagedEngine::Claude,
        &database,
        &host(&[("PATH", &path), ("HOME", "/home/someone")]),
    );
    assert!(matches!(result, Err(ManagedEngineError::StateMissing)));
}

#[test]
fn the_override_must_be_an_absolute_regular_file_and_is_reported() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("forge.db");
    let executable = root.path().join("claude-dev");
    fs::write(&executable, b"dev").unwrap();
    let absolute = executable.to_string_lossy().into_owned();
    let target = resolve_launch_target_in(
        ManagedEngine::Claude,
        &database,
        &host(&[("ARTISAN_CLAUDE_EXECUTABLE", &absolute)]),
    )
    .unwrap();
    assert_eq!(target.source(), LaunchSource::Override);
    assert_eq!(target.executable(), executable);
    assert!(target.lease().unwrap().is_none());

    for relative in ["claude", "./claude"] {
        assert!(matches!(
            resolve_launch_target_in(
                ManagedEngine::Claude,
                &database,
                &host(&[("ARTISAN_CLAUDE_EXECUTABLE", relative)]),
            ),
            Err(ManagedEngineError::UnsafePath)
        ));
    }
}

#[test]
fn the_active_managed_generation_is_resolved_with_its_version() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("forge.db");
    let authority = ManagedEngineAuthority::new(ManagedEngine::Codex);
    let Ok(plan) = authority.plan() else {
        return;
    };
    let paths = authority.install_paths(&database).unwrap();
    paths.prepare().unwrap();
    let directory = "generation-0123456789abcdef0123456789abcdef";
    let executable = paths
        .versions_root()
        .join(directory)
        .join(plan.layout.entry());
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"codex").unwrap();
    let state = ManagedToolchainState::new(ManagedGeneration {
        binary: plan.layout.entry().to_owned(),
        directory: directory.to_owned(),
        sha256: super::super::pipeline::hex_lower(&Sha256::digest(b"codex")),
        size: Some(5),
        version: "0.157.1".to_owned(),
    });
    let _committed = authority
        .write_install_state(paths.engine_root(), &state)
        .unwrap();
    let target = resolve_launch_target_in(ManagedEngine::Codex, &database, &host(&[])).unwrap();
    assert_eq!(target.source(), LaunchSource::Managed);
    assert_eq!(target.executable(), executable);
    assert_eq!(target.version().unwrap().as_str(), "0.157.1");
    let lease = target.lease().unwrap();
    assert!(lease.is_some());

    let environment = target.environment().unwrap();
    let home = root.path().join("toolchain").join("codex").join("home");
    assert_eq!(
        lookup(&environment, "HOME"),
        Some(&home.clone().into_os_string())
    );
    assert_eq!(
        lookup(&environment, "CODEX_HOME"),
        Some(&home.join(".codex").into_os_string())
    );
    assert!(home.is_dir());

    fs::write(&executable, b"tampr").unwrap();
    assert!(matches!(
        resolve_launch_target_in(ManagedEngine::Codex, &database, &host(&[])),
        Err(ManagedEngineError::ExecutableHashMismatch)
    ));
}

#[test]
fn the_environment_is_explicit_and_never_inherits_unlisted_variables() {
    let home = std::env::temp_dir().join("artisan-engine-home");
    let tool = std::env::temp_dir().join("artisan-tools");
    let environment = build_environment(
        ManagedEngine::Claude,
        &home,
        std::slice::from_ref(&tool),
        &host(&[
            (
                "PATH",
                "/usr/bin:/mnt/c/Users/someone/AppData/Roaming/npm:/bin:relative",
            ),
            ("HOME", "/home/someone"),
            ("CLAUDE_CONFIG_DIR", "/mnt/c/Users/someone/.claude"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("ANTHROPIC_API_KEY", "operator-key"),
            ("OPENAI_API_KEY", "not-for-claude"),
            ("TERM", "xterm-256color"),
            ("LANG", "en_US.ISO-8859-1"),
        ]),
    );
    assert_eq!(
        lookup(&environment, "HOME"),
        Some(&home.clone().into_os_string())
    );
    assert_eq!(
        lookup(&environment, "CLAUDE_CONFIG_DIR"),
        Some(&home.join(".claude").into_os_string())
    );
    assert_eq!(
        lookup(&environment, "DISABLE_UPDATES"),
        Some(&OsString::from("1"))
    );
    assert_eq!(
        lookup(&environment, "ANTHROPIC_API_KEY"),
        Some(&OsString::from("operator-key"))
    );
    assert!(lookup(&environment, "OPENAI_API_KEY").is_none());
    assert!(lookup(&environment, "AWS_SECRET_ACCESS_KEY").is_none());
    assert_eq!(
        lookup(&environment, "LANG"),
        Some(&OsString::from("C.UTF-8"))
    );
    assert_eq!(
        lookup(&environment, "TERM"),
        Some(&OsString::from("xterm-256color"))
    );
    let path = lookup(&environment, "PATH").unwrap();
    let entries: Vec<PathBuf> = std::env::split_paths(path).collect();
    assert_eq!(entries.first(), Some(&tool));
    assert!(entries.iter().all(|entry| entry.is_absolute()));
    #[cfg(target_os = "linux")]
    {
        assert!(!entries.iter().any(|entry| entry.starts_with("/mnt")));
        assert_eq!(
            &entries[1..],
            [PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
        );
    }
    let names: Vec<&OsString> = environment.iter().map(|(name, _)| name).collect();
    let mut unique = names.clone();
    unique.dedup();
    assert_eq!(names.len(), unique.len());
}

#[cfg(unix)]
#[test]
fn an_empty_forge_path_falls_back_to_system_directories() {
    let environment = build_environment(
        ManagedEngine::Codex,
        Path::new("/state/toolchain/codex/home"),
        &[],
        &host(&[("PATH", "/mnt/c/Windows")]),
    );
    let path = lookup(&environment, "PATH").unwrap();
    #[cfg(target_os = "linux")]
    assert_eq!(path, &OsString::from("/usr/local/bin:/usr/bin:/bin"));
    #[cfg(not(target_os = "linux"))]
    assert!(!path.is_empty());
}
