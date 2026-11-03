use super::*;
use artisan_api::ItemKind;

#[tokio::test]
async fn native_activity_claims_buffered_child_frames_without_cross_thread_leaks() {
    let fixture = Fixture::new();
    let ((outcome, events), (_, other)) = tokio::join!(
        run(fixture.turn("activity", None)),
        run(fixture.turn("hello", None)),
    );
    assert_eq!(outcome.status, artisan_api::RunStatus::Done);
    let root = session(&events);
    let child = format!("child-{root}");
    let agents: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let EngineEvent::Item {
                key,
                status,
                kind: ItemKind::Agent { .. },
                ..
            } = event
            {
                Some((key, status))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(agents.len(), 2);
    assert!(agents.iter().all(|(key, _)| **key == child));
    assert_eq!(*agents[0].1, Status::Running);
    assert_eq!(*agents[1].1, Status::Done);
    assert!(events.iter().any(|event| matches!(event, EngineEvent::Item { parent: Some(parent), kind: ItemKind::AssistantMessage { text, .. }, .. } if parent == &child && text.inline() == Some(format!("child of {root}").as_str()))));
    assert!(!other.iter().any(|event| matches!(
        event,
        EngineEvent::Item {
            kind: ItemKind::Agent { .. },
            ..
        }
    )));
}
