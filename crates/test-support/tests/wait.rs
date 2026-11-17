use std::time::Duration;
use test_support::wait::wait_until;

#[test]
#[should_panic(expected = "waiting for the flag that never rises")]
fn a_condition_that_never_holds_fails_naming_what_was_awaited() {
    wait_until(
        "the flag that never rises",
        Duration::from_millis(20),
        || false,
    );
}
