use super::*;

#[tokio::test]
async fn native_metadata_does_not_become_a_description_or_a_parent_worker() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let mut turn = run(&thread, RunStatus::Running);
    turn.session_id = Some("root-session".into());
    store.upsert_run(turn).await.unwrap();
    for (id, path, description, address) in [
        (
            "legacy-worker",
            "/root/vegetation",
            "/root/vegetation",
            "worker-session",
        ),
        (
            "real-description",
            "/root/ocean",
            "Check ocean geometry",
            "ocean-session",
        ),
        ("parent-message", "/root", "/root", "root-session"),
    ] {
        let mut item = message(&thread, id, "");
        item.status = Status::Running;
        item.kind = ItemKind::Agent {
            engine: Some(EngineId::Codex),
            thread: None,
            name: Some(path.into()),
            description: description.into(),
            prompt: None,
            agent_id: Some(address.into()),
            context_used: None,
            model: None,
            background: false,
            result: None,
            label: None,
        };
        store.upsert_item(item).await.unwrap();
    }
    let summary = store.inspector(&thread).await.unwrap();
    assert_eq!(summary.agents.len(), 2);
    let legacy = summary
        .agents
        .iter()
        .find(|agent| agent.id.as_str() == "legacy-worker")
        .unwrap();
    assert_eq!(legacy.description.byte_len(), 0);
    let real = summary
        .agents
        .iter()
        .find(|agent| agent.id.as_str() == "real-description")
        .unwrap();
    assert_eq!(real.description.inline(), Some("Check ocean geometry"));
    let stored = store
        .item(&ItemId::from("legacy-worker"))
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(stored.kind, ItemKind::Agent { description, .. } if description.inline() == Some("/root/vegetation"))
    );
}
