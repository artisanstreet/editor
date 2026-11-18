use test_support::transcript::{RecordError, Scrub, Source, Step, Transcript, record};

fn trace(records: &[serde_json::Value]) -> String {
    records
        .iter()
        .flat_map(|record| [record.to_string(), "\n".to_owned()])
        .collect()
}

#[test]
fn a_recorded_trace_replays_its_frames_and_where_input_closed() {
    let home = r"C:\Users\someone";
    let frame = serde_json::json!({ "type": "system", "cwd": format!(r"{home}\work") }).to_string();
    let steps = record(
        &trace(&[
            serde_json::json!({ "at": 1, "kind": "note", "text": "started (pid Some(7))" }),
            serde_json::json!({ "at": 2, "kind": "in", "text": r#"{"type":"user"}"# }),
            serde_json::json!({ "at": 3, "kind": "stderr", "text": "warming up" }),
            serde_json::json!({ "at": 4, "kind": "out", "text": frame }),
            serde_json::json!({ "at": 5, "kind": "note", "text": "closing stdin: result (cancelling: false; none)" }),
            serde_json::json!({ "at": 6, "kind": "note", "text": "exited: Ok(0)" }),
            serde_json::json!({ "at": 7, "kind": "end", "dropped": 0 }),
        ]),
        &[Scrub::new(home.to_owned(), "/home/user".to_owned())],
    )
    .expect("a whole trace records");
    assert_eq!(
        steps,
        [
            Step::In {
                text: r#"{"type":"user"}"#.to_owned()
            },
            Step::Out {
                text: r#"{"type":"system","cwd":"/home/user\\work"}"#.to_owned()
            },
            Step::Closed,
        ],
        "the fixture keeps the frames in order, marks where Forge closed input and holds no home path"
    );
    let transcript = Transcript::new(Source::Recorded, vec![steps.clone(), steps]);
    assert_eq!(
        Transcript::parse(&transcript.render()).expect("a rendered transcript parses"),
        transcript,
        "a written fixture reads back as the same launches"
    );
}

#[test]
fn a_trace_that_lost_frames_is_refused() {
    let truncated = record(
        &trace(&[
            serde_json::json!({ "at": 1, "kind": "out", "text": "{", "truncated_from": 300_000 }),
        ]),
        &[],
    );
    assert!(
        matches!(
            truncated,
            Err(RecordError::Truncated {
                line: 1,
                bytes: 300_000
            })
        ),
        "a cut frame cannot be replayed: {truncated:?}"
    );
    let dropped = record(
        &trace(&[
            serde_json::json!({ "at": 1, "kind": "out", "text": "{}" }),
            serde_json::json!({ "at": 2, "kind": "dropped", "records": 3 }),
        ]),
        &[],
    );
    assert!(
        matches!(
            dropped,
            Err(RecordError::Dropped {
                line: 2,
                records: 3
            })
        ),
        "a trace with missing frames cannot be replayed: {dropped:?}"
    );
}
