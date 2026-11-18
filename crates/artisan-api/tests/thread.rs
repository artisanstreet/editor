use artisan_api::{Run, RunOutcome, RunState, RunStatus};
use serde_json::json;

fn stored(status: &str, error: Option<&str>, ended_at: Option<i64>) -> serde_json::Value {
    json!({
        "id": "r1",
        "thread_id": "t1",
        "engine": "claude",
        "model": null,
        "input_cutoff": 0,
        "status": status,
        "usage": { "input": 0, "output": 0, "reasoning": 0, "cache_read": 0, "cache_write": null, "context_used": null, "context_window": null, "cost_usd": null },
        "session_id": null,
        "error": error,
        "seq": 3,
        "started_at": 100,
        "ended_at": ended_at,
    })
}

#[test]
fn a_finished_run_keeps_its_outcome_error_and_ending_through_the_wire() {
    let run: Run = serde_json::from_value(stored("failed", Some("overloaded"), Some(250)))
        .expect("a failed run");
    assert_eq!(
        run.state,
        RunState::Finished {
            outcome: RunOutcome::Failed,
            error: Some("overloaded".to_owned()),
            ended_at: 250,
        }
    );
    assert_eq!(run.status(), RunStatus::Failed);
    let encoded = serde_json::to_value(&run).expect("encode");
    assert_eq!(encoded["status"], "failed");
    assert_eq!(encoded["ended_at"], 250);
    assert_eq!(serde_json::from_value::<Run>(encoded).expect("decode"), run);
}

#[test]
fn a_finished_run_stored_without_an_ending_ends_when_it_started() {
    let run: Run = serde_json::from_value(stored("done", None, None)).expect("a done run");
    assert_eq!(run.ended_at(), Some(100));
}

#[test]
fn a_running_run_cannot_carry_an_ending_or_an_error() {
    let run: Run = serde_json::from_value(stored("running", None, None)).expect("a live run");
    assert!(run.is_live());
    assert_eq!(
        serde_json::to_value(&run).expect("encode")["ended_at"],
        json!(null)
    );
    serde_json::from_value::<Run>(stored("running", None, Some(250)))
        .expect_err("a live run has no ending");
    serde_json::from_value::<Run>(stored("running", Some("boom"), None))
        .expect_err("a live run has no error");
}
