#![cfg(unix)]

use artisan_api::EngineId;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use test_support::replay::ReplayingEngine;
use test_support::transcript::{Source, Transcript};

const STALE_ENGINE: &str = "#!/bin/sh\necho 'unknown variant `exit`' >&2\nexit 3\n";

struct Example(PathBuf);

impl Example {
    fn stale(name: &str) -> Self {
        let test = std::env::current_exe().expect("the test binary has a path");
        let examples = test
            .parent()
            .and_then(std::path::Path::parent)
            .expect("test binaries live in <target>/<profile>/deps")
            .join("examples");
        std::fs::create_dir_all(&examples).expect("create the examples folder");
        let path = examples.join(name);
        std::fs::write(&path, STALE_ENGINE).expect("write the stale engine");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make the stale engine runnable");
        Self(path)
    }
}

impl Drop for Example {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).expect("remove the stale engine");
    }
}

#[test]
#[should_panic(expected = "rebuild it with `cargo build --example stale_replay_engine")]
fn a_replaying_engine_older_than_the_transcript_is_refused_before_the_test_runs() {
    let name = format!("stale_replay_engine_{}", std::process::id());
    let _example = Example::stale(&name);
    let data = tempfile::tempdir().expect("create Forge's data folder");

    drop(ReplayingEngine::install_transcript(
        data.path(),
        &name,
        EngineId::Claude,
        &Transcript::new(Source::Synthetic, Vec::new()),
    ));
}
