use super::scenario;
use artisan_api::RunStatus;
use std::time::Duration;

#[tokio::test]
async fn a_steer_is_answered_while_a_monitor_outlives_the_reply() {
    let dir = tempfile::tempdir().unwrap();
    let mut running = scenario::start_with(
        scenario::turn(dir.path(), "claude-monitor-steer", &[]),
        std::sync::Arc::new(scenario::Elapsed),
    );
    let replied = dir.path().join("replied");
    test_support::wait::eventually(
        "the first reply to be handled",
        test_support::wait::LIVENESS,
        || std::future::ready(replied.exists()),
    )
    .await;
    running.steer("change course").await;
    let outcome = running
        .outcome(Duration::from_secs(10))
        .await
        .expect("the run ends once the steer is answered and the monitor is gone");
    assert_eq!(outcome.status(), RunStatus::Done, "{:?}", outcome.error());
    assert!(
        !dir.path().join("input_closed_under_the_monitor").exists(),
        "input must stay open while the monitor runs"
    );
    assert!(
        std::fs::read_to_string(dir.path().join("steer"))
            .unwrap()
            .contains("change course"),
        "the steer reaches the running session"
    );
}
