use crate::Launch;
use std::fs::{OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) fn binary() -> PathBuf {
    let test = std::env::current_exe().expect("the test binary has a path");
    let profile = test
        .parent()
        .and_then(Path::parent)
        .expect("test binaries live in <target>/<profile>/deps");
    let fake = profile
        .join("examples")
        .join(format!("fake_engine{}", std::env::consts::EXE_SUFFIX));
    assert!(
        fake.is_file(),
        "the fake engine {} is missing: build it with `cargo build -p forge-engines --example fake_engine` or run `cargo test -p forge-engines` without --lib",
        fake.display()
    );
    fake
}

pub(crate) fn launch(scenario: &str, env: &[(&str, &str)]) -> Launch {
    Launch {
        env: std::iter::once(("ARTISAN_FAKE", scenario))
            .chain(env.iter().copied())
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
        ..Launch::default()
    }
}

pub(crate) async fn ended(dir: &Path, name: &str) -> bool {
    let path = dir.join(format!("{name}.alive"));
    let marker = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap_or_else(|error| panic!("the fake engine marked {} alive: {error}", path.display()));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match marker.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(TryLockError::Error(error)) => {
                    panic!("lock the liveness marker {}: {error}", path.display())
                }
            }
        }
    })
    .await
    .is_ok()
}

#[cfg(unix)]
pub(crate) fn reaped(dir: &Path, name: &str) -> bool {
    let pid = std::fs::read_to_string(dir.join(format!("{name}.pid"))).unwrap();
    !std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}
