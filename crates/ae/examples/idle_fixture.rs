//! An isolated Forge with a completed conversation for Editor measurements.
//! Usage: cargo run -p ae --example idle_fixture -- /absolute/new/directory
//! The directory must not exist. This process never manages a Forge service.

use anyhow::{Context as _, Result};
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId, Run,
    RunId, RunStatus, Status, ThreadId, Usage,
    tls::{Identity, fingerprint_of},
};
use forge_store::NewItem;
use serde_json::json;
use std::path::PathBuf;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .context("provide a new fixture directory")?,
    );
    anyhow::ensure!(root.is_absolute(), "fixture directory must be absolute");
    std::fs::create_dir(&root).context("fixture directory must not already exist")?;
    let editor = root.join("editor");
    let project = root.join("project");
    std::fs::create_dir(&editor)?;
    std::fs::create_dir(&project)?;
    let server = forge_http::start(forge_http::ServeConfig {
        data_dir: root.join("forge"),
        listen: "127.0.0.1:0".parse()?,
    })
    .await?;
    let store = server.store();
    let address = server.local_addr()?.to_string();
    let identity = Identity::generate("artisan-idle-fixture")?;
    store
        .add_device("Idle fixture".into(), identity.certificate.clone())
        .await?;
    let fingerprint = fingerprint_of(&forge_http::identity(store).await?.certificate)?;
    std::fs::write(editor.join("identity.json"), serde_json::to_vec(&identity)?)?;
    std::fs::write(
        editor.join("hosts.json"),
        serde_json::to_vec_pretty(&json!({
            "hosts": [{
                "id": "idle-fixture", "name": "Performance fixture", "address": address,
                "fingerprint": fingerprint, "kind": {"type": "remote"}
            }],
            "last": "idle-fixture"
        }))?,
    )?;
    let project_id = ProjectId::from("idle-project");
    let thread_id = ThreadId::from("idle-conversation");
    store
        .create_project(CreateProject {
            id: project_id.clone(),
            path: project.display().to_string(),
            name: Some("Performance fixture".into()),
        })
        .await?;
    store
        .create_thread(CreateThread {
            id: thread_id.clone(),
            project_id,
            config: EngineConfig::new(EngineId::Claude),
            title: Some("Completed conversation".into()),
            parent_thread_id: None,
        })
        .await?;
    for turn in 0..20 {
        let run_id = RunId::from(format!("idle-run-{turn}"));
        store
            .upsert_run(Run {
                id: run_id.clone(),
                thread_id: thread_id.clone(),
                engine: EngineId::Claude,
                model: None,
                input_cutoff: 0,
                status: RunStatus::Done,
                usage: Usage::default(),
                session_id: None,
                error: None,
                seq: 0,
                started_at: 1_700_000_000_000 + turn * 60_000,
                ended_at: Some(1_700_000_030_000 + turn * 60_000),
            })
            .await?;
        let items = [
            ItemKind::UserMessage {
                text: format!("Review iteration {} of this small Rust function.", turn + 1).into(),
                images: Vec::new(),
                steer: false,
            },
            ItemKind::AssistantMessage {
                phase: Phase::Final,
                text: format!(
                    "Iteration {} is complete. The function handles empty input and returns the sum.\n\n\
                     ```rust\nfn total(values: &[u32]) -> u32 {{\n    values.iter().sum()\n}}\n```\n\n\
                     - Empty input returns zero.\n- The input slice stays unchanged.\n\n\
                     | Case | Result |\n| --- | --- |\n| Empty | 0 |\n| Three values | 6 |\n\n\
                     The checks passed. This conversation has no running tools or animations.",
                    turn + 1
                ).into(),
            },
        ];
        for (index, kind) in items.into_iter().enumerate() {
            store
                .upsert_item(NewItem {
                    id: ItemId::from(format!("idle-item-{turn}-{index}")),
                    thread_id: thread_id.clone(),
                    run_id: Some(run_id.clone()),
                    parent_id: None,
                    native_id: None,
                    status: Status::Done,
                    kind,
                })
                .await?;
        }
    }
    let metadata = json!({
        "schema_version": 1,
        "scenario": "completed-conversation-v1",
        "turns": 20,
        "items": 40,
        "editor_data": editor,
        "link": "artisan://thread/idle-fixture/idle-conversation",
        "address": address,
        "fixture_pid": std::process::id()
    });
    std::fs::write(
        root.join("fixture.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )?;
    println!("{metadata}");
    server.run().await
}
