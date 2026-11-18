//! An isolated Forge with a completed conversation for Editor measurements.
//! Usage: cargo run -p ae --example idle_fixture -- /absolute/new/directory
//! The directory must not exist. This process never manages a Forge service.
#![expect(
    clippy::print_stdout,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]

use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId, Run,
    RunId, RunOutcome, RunState, Status, ThreadId, Usage,
    tls::{Identity, fingerprint_of},
};
use forge_store::NewItem;
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
enum FixtureError {
    #[error("provide a new fixture directory")]
    NoDirectory,
    #[error("fixture directory {} must be absolute", path.display())]
    Relative { path: PathBuf },
    #[error("create {}; the fixture directory must not already exist", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("start the fixture Forge")]
    Start(#[from] forge_http::StartError),
    #[error("read the fixture Forge's address")]
    Address(#[source] std::io::Error),
    #[error(transparent)]
    Identity(#[from] artisan_api::tls::GenerateIdentityError),
    #[error(transparent)]
    Certificate(#[from] forge_store::CertificateError),
    #[error(transparent)]
    AddDevice(#[from] forge_store::AddDeviceError),
    #[error(transparent)]
    ForgeIdentity(#[from] forge_http::IdentityError),
    #[error(transparent)]
    Fingerprint(#[from] artisan_api::tls::DecodeCertificateError),
    #[error(transparent)]
    Project(#[from] forge_store::CreateProjectError),
    #[error(transparent)]
    Thread(#[from] forge_store::CreateThreadError),
    #[error(transparent)]
    Run(#[from] forge_store::UpsertRunError),
    #[error(transparent)]
    Item(#[from] forge_store::UpsertItemError),
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), FixtureError> {
    std::fs::write(path, bytes).map_err(|source| FixtureError::Write {
        path: path.to_owned(),
        source,
    })
}

fn create(path: &Path) -> Result<(), FixtureError> {
    std::fs::create_dir(path).map_err(|source| FixtureError::Create {
        path: path.to_owned(),
        source,
    })
}

fn to_json(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("invariant: fixture JSON always serializes")
}

#[tokio::main(flavor = "current_thread")]
#[expect(
    clippy::too_many_lines,
    reason = "the fixture is built step by step in one readable script"
)]
async fn main() -> Result<(), FixtureError> {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or(FixtureError::NoDirectory)?,
    );
    if !root.is_absolute() {
        return Err(FixtureError::Relative { path: root });
    }
    create(&root)?;
    let editor = root.join("editor");
    let project = root.join("project");
    create(&editor)?;
    create(&project)?;
    let server = forge_http::start(forge_http::ServeConfig {
        data_dir: root.join("forge"),
        listen: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
    })
    .await?;
    let store = server.store();
    let address = server
        .local_addr()
        .map_err(FixtureError::Address)?
        .to_string();
    let identity = Identity::generate("artisan-idle-fixture")?;
    store
        .add_device(
            "Idle fixture".into(),
            forge_store::DeviceCertificate::parse(&identity.certificate)?,
        )
        .await?;
    let fingerprint = fingerprint_of(&forge_http::identity(store).await?.certificate)?;
    write(&editor.join("identity.json"), &to_json(&identity))?;
    write(
        &editor.join("hosts.json"),
        &to_json(&json!({
            "hosts": [{
                "id": "idle-fixture", "name": "Performance fixture", "address": address,
                "fingerprint": fingerprint, "kind": {"type": "remote"}
            }],
            "last": "idle-fixture"
        })),
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
            spawned_by: None,
        })
        .await?;
    for turn in 0..20 {
        let run_id = RunId::from(format!("idle-run-{turn}"));
        store
            .upsert_run(Run {
                config: None,
                id: run_id.clone(),
                thread_id: thread_id.clone(),
                engine: EngineId::Claude,
                model: None,
                input_cutoff: 0,
                state: RunState::Finished {
                    outcome: RunOutcome::Done,
                    error: None,
                    ended_at: 1_700_000_030_000 + turn * 60_000,
                },
                usage: Usage::default(),
                session_id: None,
                seq: 0,
                started_at: 1_700_000_000_000 + turn * 60_000,
                continuation: None,
            })
            .await?;
        let items = [
            ItemKind::UserMessage {
                text: format!("Review iteration {} of this small Rust function.", turn + 1).into(),
                images: Vec::new(),
                steer: false,
                from_agent: None,
                sender: None,
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
    write(&root.join("fixture.json"), &to_json(&metadata))?;
    println!("{metadata}");
    server.run().await;
    Ok(())
}
