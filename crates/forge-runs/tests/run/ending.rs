use super::*;

#[test]
fn a_stop_the_engine_never_confirms_still_shows_the_failure_that_came_first() {
    let disk_full = Ending::Finished(Outcome::failed("The disk is full."));
    let shown = disk_full.unconfirmed();
    let error = shown.error().unwrap();
    assert!(error.contains("The disk is full."), "{error}");
    assert!(error.contains("did not confirm it stopped"), "{error}");

    let persist = Ending::Stopped(Cleanup::Failed("persist engine event".to_owned()));
    let error = persist.unconfirmed().error().unwrap().to_owned();
    assert!(error.contains("persist engine event"), "{error}");
    assert!(error.contains("did not confirm it stopped"), "{error}");

    let overloaded = Ending::Finished(Outcome::retryable(
        "overloaded",
        forge_engines::Retry::Overloaded,
    ))
    .unconfirmed();
    assert!(
        matches!(&overloaded, Ending::Finished(outcome) if outcome.retry() == Some(forge_engines::Retry::Overloaded)),
        "the failure keeps its retry"
    );
}
