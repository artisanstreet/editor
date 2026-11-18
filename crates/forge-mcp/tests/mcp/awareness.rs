use crate::ArtisanMcp;
use crate::server::handle;
use crate::validation::validate;
use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, ThreadId};
use forge_engines::Engines;
use forge_runs::Hub;
use forge_store::Store;
use serde_json::{Value, json};
use std::collections::BTreeMap;

async fn call(mcp: &ArtisanMcp, caller: &ThreadId, name: &str, arguments: Value) -> Value {
    let reply = handle(
        mcp,
        caller,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
    )
    .await
    .unwrap();
    assert_eq!(reply["result"]["isError"], false, "{name}: {reply}");
    reply["result"]["structuredContent"].clone()
}

struct Fixture {
    _directory: tempfile::TempDir,
    hub: Hub,
    mcp: ArtisanMcp,
    threads: Vec<ThreadId>,
    item: artisan_api::ItemId,
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("store")).unwrap();
    let project = ProjectId::new();
    store
        .create_project(CreateProject {
            id: project.clone(),
            path: "/repo".into(),
            name: Some("editor".into()),
        })
        .await
        .unwrap();
    let mut threads = Vec::new();
    for title in ["Diagrams", "Gradients"] {
        let id = ThreadId::new();
        store
            .create_thread(CreateThread {
                id: id.clone(),
                project_id: project.clone(),
                config: EngineConfig::new(EngineId::Codex),
                title: Some(title.into()),
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .unwrap();
        threads.push(id);
    }
    let item = store
        .upsert_item(forge_store::NewItem {
            id: artisan_api::ItemId::new(),
            thread_id: threads[0].clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: artisan_api::Status::Done,
            kind: artisan_api::ItemKind::AssistantMessage {
                text: "done".into(),
                phase: artisan_api::Phase::Final,
            },
        })
        .await
        .unwrap();
    let hub = Hub::new(
        store,
        Engines::new(directory.path()),
        directory.path().to_path_buf(),
    );
    let mcp = ArtisanMcp::new(hub.clone());
    Fixture {
        _directory: directory,
        hub,
        mcp,
        threads,
        item: item.id,
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one call per advertised discovery and coordination tool"
)]
async fn discovery_and_coordination_results_keep_the_contract_they_advertise() {
    let Fixture {
        _directory,
        hub,
        mcp,
        threads,
        item,
    } = fixture().await;
    let listed = handle(
        &mcp,
        &threads[0],
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    )
    .await
    .unwrap();
    let schemas: BTreeMap<String, Value> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool.get("outputSchema").is_some())
        .map(|tool| {
            (
                tool["name"].as_str().unwrap().to_owned(),
                tool["outputSchema"].clone(),
            )
        })
        .collect();
    let [me, peer] = [&threads[0], &threads[1]];
    let claim = json!({"resources":[{"kind":"path","worktree":"/repo","path":"src/diagram.rs"}],"purpose":"edit diagrams"});
    let calls = [
        (
            peer,
            "publish_work",
            json!({"objective":"gradients","worktree":"/repo","state":"completed"}),
        ),
        (
            me,
            "publish_work",
            json!({"objective":"diagrams","worktree":"/repo","state":"running","dependencies":[peer]}),
        ),
        (me, "claim_resources", claim.clone()),
        (peer, "claim_resources", claim),
        (me, "get_work", json!({})),
        (me, "list_resource_claims", json!({})),
        (
            me,
            "list_resource_claims",
            json!({"resources":[{"kind":"path","worktree":"/repo","path":"src/diagram.rs"}]}),
        ),
        (me, "list_coordination_messages", json!({})),
        (
            me,
            "get_coordination_record",
            json!({"id":format!("work:{me}")}),
        ),
        (
            me,
            "wait_for_work",
            json!({"thread":peer,"timeout_seconds":0}),
        ),
        (me, "list_coordination_events", json!({"after":0})),
        (me, "list_relevant_agents", json!({})),
        (me, "list_relevant_threads", json!({})),
        (me, "list_agents", json!({})),
        (me, "list_threads", json!({})),
        (me, "list_active_agents", json!({})),
        (me, "list_active_threads", json!({})),
        (me, "get_agent", json!({"agent":peer})),
        (me, "get_thread", json!({"thread":peer})),
        (
            me,
            "find_threads_touching_files",
            json!({"paths":["/repo/src/diagram.rs"]}),
        ),
        (me, "list_harnesses", json!({})),
        (me, "list_models", json!({"harness":"codex"})),
        (me, "read_thread_item", json!({"item":item})),
    ];
    let mut results = BTreeMap::new();
    for (caller, name, arguments) in calls {
        let result = call(&mcp, caller, name, arguments).await;
        validate(&schemas[name], &result, name, 0)
            .unwrap_or_else(|error| panic!("{name} broke its schema: {error}\n{result}"));
        results.insert(name, result);
    }
    hub.shutdown();

    let mut nameless = results["list_agents"].clone();
    nameless["workers"][0]
        .as_object_mut()
        .unwrap()
        .remove("thread");
    assert!(validate(&schemas["list_agents"], &nameless, "list_agents", 0).is_err());

    let mut unranked = results["list_relevant_agents"].clone();
    unranked["workers"][0]
        .as_object_mut()
        .unwrap()
        .remove("relevance");
    assert!(
        validate(
            &schemas["list_relevant_agents"],
            &unranked,
            "list_relevant_agents",
            0
        )
        .is_err()
    );

    let mut wordy = results["claim_resources"].clone();
    wordy["record"]["revision"] = json!("1");
    assert!(validate(&schemas["claim_resources"], &wordy, "claim_resources", 0).is_err());

    let mut unscoped = results["list_threads"].clone();
    unscoped.as_object_mut().unwrap().remove("scope");
    assert!(validate(&schemas["list_threads"], &unscoped, "list_threads", 0).is_err());
}
