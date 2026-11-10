#![cfg(test)]

use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, ThreadId};
use forge_engines::Engines;
use forge_mcp::{ArtisanMcp, server};
use forge_runs::Hub;
use forge_store::Store;
use serde_json::{Value, json};

struct Fixture {
    _directory: tempfile::TempDir,
    hub: Hub,
    mcp: ArtisanMcp,
    threads: Vec<ThreadId>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.hub.shutdown();
    }
}

impl Fixture {
    async fn new() -> Self {
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
        for title in ["Mermaid fixes", "Beautiful gradient", "Unrelated task"] {
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
        let hub = Hub::new(
            store,
            Engines::new(directory.path()),
            directory.path().to_path_buf(),
        );
        Self {
            mcp: ArtisanMcp::new(hub.clone()),
            hub,
            threads,
            _directory: directory,
        }
    }

    async fn call(&self, who: usize, name: &str, args: Value) -> Value {
        server::handle(&self.mcp,&self.threads[who],json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).await.unwrap()["result"].clone()
    }

    async fn claim(&self, who: usize, path: &str) -> Value {
        let result=self.call(who,"claim_resources",json!({"resources":[{"kind":"path","worktree":"/repo","path":path}],"purpose":"edit this module"})).await;
        assert_eq!(result["isError"], false, "{result}");
        result["structuredContent"]["record"].clone()
    }
}

#[tokio::test]
async fn relevant_workers_use_claims_instead_of_misleading_titles_and_paginate_after_ranking() {
    let fixture = Fixture::new().await;
    fixture.claim(0, "src/diagram.rs").await;
    fixture.claim(1, "src/diagram.rs").await;
    fixture.claim(2, "src/unrelated.rs").await;
    let first = fixture
        .call(0, "list_relevant_agents", json!({"limit":1}))
        .await;
    assert_eq!(first["isError"], false, "{first}");
    let page = &first["structuredContent"];
    assert_eq!(page["workers"][0]["thread"], json!(fixture.threads[1]));
    assert_eq!(
        page["workers"][0]["relevance"]["reasons"][0]["kind"],
        "overlapping_claim"
    );
    let second = fixture
        .call(
            0,
            "list_relevant_agents",
            json!({"limit":1,"cursor":page["next_cursor"]}),
        )
        .await;
    assert_eq!(
        second["structuredContent"]["workers"][0]["thread"],
        json!(fixture.threads[2])
    );
    assert!(second["structuredContent"]["next_cursor"].is_null());
    assert!(
        page["workers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["thread"] != json!(fixture.threads[0]))
    );
}

#[tokio::test]
async fn claims_warn_without_rejecting_overlap_and_stale_updates_return_recovery_information() {
    let fixture = Fixture::new().await;
    let first = fixture.claim(0, "src/diagram.rs").await;
    let second=fixture.call(1,"claim_resources",json!({"resources":[{"kind":"path","worktree":"/repo","path":"src/diagram.rs"}],"purpose":"overlapping work"})).await;
    assert_eq!(second["isError"], false);
    assert_eq!(
        second["structuredContent"]["overlaps"][0]["id"],
        first["id"]
    );
    let updated = fixture
        .call(
            0,
            "renew_resource_claim",
            json!({"id":first["id"],"revision":first["revision"],"lifetime_seconds":120}),
        )
        .await;
    assert_eq!(updated["isError"], false);
    let stale = fixture
        .call(
            0,
            "release_resources",
            json!({"id":first["id"],"revision":first["revision"]}),
        )
        .await;
    assert_eq!(stale["isError"], true);
    assert_eq!(
        stale["structuredContent"]["error"]["code"],
        "stale_revision"
    );
    assert_eq!(
        stale["structuredContent"]["error"]["current_revision"],
        updated["structuredContent"]["record"]["revision"]
    );
    let unauthorized = fixture
        .call(
            1,
            "release_resources",
            json!({"id":first["id"],"revision":updated["structuredContent"]["record"]["revision"]}),
        )
        .await;
    assert_eq!(unauthorized["isError"], true);
}

#[tokio::test]
async fn legacy_agent_scope_stays_parent_only_while_new_discovery_includes_peers() {
    let fixture = Fixture::new().await;
    let legacy = fixture.call(0, "agent_list", json!({})).await;
    assert!(
        legacy["structuredContent"]["workers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let peers = fixture.call(0, "list_agents", json!({})).await;
    assert_eq!(
        peers["structuredContent"]["workers"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let active = fixture.call(0, "list_active_agents", json!({})).await;
    assert!(
        active["structuredContent"]["workers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn malformed_resource_shapes_and_unknown_arguments_fail_before_persisting_claims() {
    let fixture = Fixture::new().await;
    for args in [
        json!({"resources":[{"kind":"path","path":"src/a.rs"}],"purpose":"missing worktree"}),
        json!({"resources":[{"kind":"named","namespace":"build","key":"windows"}],"purpose":"valid","unexpected":true}),
    ] {
        let response = fixture.call(0, "claim_resources", args).await;
        assert_eq!(response["isError"], true);
        assert_eq!(
            response["structuredContent"]["error"]["code"],
            "invalid_arguments"
        );
    }
    let claims = fixture.call(0, "list_resource_claims", json!({})).await;
    assert!(
        claims["structuredContent"]["records"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn long_item_continuation_preserves_every_utf8_character() {
    let fixture = Fixture::new().await;
    let text = "ø🦀終".repeat(15000);
    let item = fixture
        .hub
        .store()
        .upsert_item(forge_store::NewItem {
            id: artisan_api::ItemId::new(),
            thread_id: fixture.threads[0].clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: artisan_api::Status::Done,
            kind: artisan_api::ItemKind::AssistantMessage {
                text: text.clone().into(),
                phase: artisan_api::Phase::Final,
            },
        })
        .await
        .unwrap();
    let mut offset = 0;
    let mut rendered = String::new();
    loop {
        let page = fixture
            .call(
                0,
                "read_thread_item",
                json!({"item":item.id,"offset":offset,"limit":16001}),
            )
            .await;
        assert_eq!(page["isError"], false, "{page}");
        rendered.push_str(page["structuredContent"]["text"].as_str().unwrap());
        if let Some(next) = page["structuredContent"]["next_offset"].as_u64() {
            assert!(next > offset);
            offset = next;
        } else {
            assert_eq!(
                usize::try_from(page["structuredContent"]["total_bytes"].as_u64().unwrap()).unwrap(),
                rendered.len()
            );
            break;
        }
    }
    assert!(rendered.contains(&text));
    let character_offset = rendered.find('🦀').unwrap();
    let tiny = fixture
        .call(
            0,
            "read_thread_item",
            json!({"item":item.id,"offset":character_offset,"limit":1}),
        )
        .await;
    assert_eq!(tiny["structuredContent"]["text"], "🦀");
    let invalid = fixture
        .call(
            0,
            "read_thread_item",
            json!({"item":item.id,"offset":character_offset+1}),
        )
        .await;
    assert_eq!(invalid["isError"], true);
}
