use super::*;
use std::time::Duration;

const STAND_IN: &str = "FORGE_ENGINES_STAND_IN_FOR_FORGE";
const TEST: &str = "process::containment_tests::an_engine_tree_dies_with_a_killed_forge";
const ENGINE_STARTED: &str = "engine started";

#[tokio::test]
async fn an_engine_tree_dies_with_a_killed_forge() {
    if std::env::var_os(STAND_IN).is_some() {
        stand_in_for_forge().await;
    }
    let mut forge = Command::new(std::env::current_exe().unwrap())
        .args([TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(STAND_IN, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut engine_stderr = forge.stderr.take().unwrap();
    let mut announcements = BufReader::new(forge.stdout.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(line) = announcements.next_line().await.unwrap() {
            if line.ends_with(ENGINE_STARTED) {
                return;
            }
        }
        panic!("the stand-in Forge ended before starting its engine");
    })
    .await
    .expect("the stand-in Forge starts its engine");
    forge.start_kill().unwrap();
    forge.wait().await.unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        engine_stderr.read_to_end(&mut rest),
    )
    .await
    .expect("the engine and its tools must die with Forge, closing the stderr they inherited")
    .unwrap();
}

#[expect(
    clippy::print_stdout,
    reason = "the stand-in Forge tells the test its engine started"
)]
async fn stand_in_for_forge() -> ! {
    let mut engine = Command::new("sh");
    engine
        .args(["-c", "sleep 600 & sleep 600"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let _engine = owned_command(engine, crate::sandbox::Sandbox::Open).unwrap();
    println!("{ENGINE_STARTED}");
    std::future::pending().await
}
