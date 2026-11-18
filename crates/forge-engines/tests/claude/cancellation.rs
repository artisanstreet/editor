use super::scenario;
use crate::fake_engine;
use artisan_api::RunStatus;
use std::time::Duration;

#[tokio::test]
async fn cancellation_sends_the_interrupt_and_reaps_even_an_unresponsive_cli() {
    for ignore_interrupt in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let ignore_interrupt = ignore_interrupt.to_string();
        let mut running = scenario::start(scenario::turn(
            dir.path(),
            "claude-interrupt",
            &[("IGNORE_INTERRUPT", &ignore_interrupt)],
        ));
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dir.path().join("ready").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        running.cancel();
        let outcome = running.outcome(Duration::from_secs(10)).await.unwrap();
        assert_eq!(
            outcome.status(),
            RunStatus::Cancelled,
            "{:?}",
            outcome.error()
        );
        assert!(dir.path().join("interrupted").exists());
        assert!(
            fake_engine::ended(dir.path(), "cli").await,
            "cancelled CLI process must have ended"
        );
        #[cfg(unix)]
        assert!(
            fake_engine::reaped(dir.path(), "cli"),
            "cancelled CLI process must have been reaped"
        );
        running.drained().await;
    }
}

#[tokio::test]
async fn cancellation_still_ends_a_cli_that_closed_its_output_but_lives_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut running = scenario::start(scenario::turn(dir.path(), "claude-closes-output", &[]));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !dir.path().join("closed").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    running.cancel();
    let outcome = running
        .outcome(Duration::from_secs(2))
        .await
        .expect("Stop ends the run even after the CLI closed its output");
    assert_eq!(
        outcome.status(),
        RunStatus::Cancelled,
        "{:?}",
        outcome.error()
    );
    running.drained().await;
}
