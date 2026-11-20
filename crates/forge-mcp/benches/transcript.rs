use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
    Status, ThreadId,
};
use divan::Bencher;
use forge_engines::Engines;
use forge_mcp::{ArtisanMcp, server};
use forge_runs::Hub;
use forge_store::{NewItem, Store};
use serde_json::{Value, json};
use std::sync::LazyLock;

const ITEM_BYTES: usize = 16 * 1024 * 1024;

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    mcp: ArtisanMcp,
    thread: ThreadId,
    item: ItemId,
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| {
    let dir = tempfile::tempdir().expect("bench: temp dir");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("bench: runtime");
    let (mcp, thread, item) = runtime.block_on(async {
        let store = Store::open(&dir.path().join("store")).expect("bench: store");
        let project = store
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/repo".into(),
                name: None,
            })
            .await
            .expect("bench: project");
        let thread = store
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: project.id,
                config: EngineConfig::new(EngineId::Codex),
                title: Some("Long log".into()),
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .expect("bench: thread")
            .id;
        let line = "2026-11-18T10:00:00Z renderer: repainted 48 damaged rows in 1.2 ms\n";
        let text = line.repeat(ITEM_BYTES / line.len());
        let item = store
            .upsert_item(NewItem {
                id: ItemId::new(),
                thread_id: thread.clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: text.into(),
                    phase: Phase::Final,
                },
            })
            .await
            .expect("bench: item")
            .id;
        let hub = Hub::new(store, Engines::new(dir.path()), dir.path().to_path_buf());
        (ArtisanMcp::new(hub), thread, item)
    });
    Fixture {
        _dir: dir,
        runtime,
        mcp,
        thread,
        item,
    }
});

fn main() {
    divan::main();
}

fn call(fixture: &Fixture, name: &str, arguments: &Value) -> Value {
    fixture
        .runtime
        .block_on(server::handle(
            &fixture.mcp,
            &forge_runs::GatewayCaller { thread: fixture.thread.clone(), ended: tokio_util::sync::CancellationToken::new(), run_stopped: None },
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        ))
        .expect("bench: call")
}

#[divan::bench(sample_count = 10)]
fn read_one_range_of_a_long_item(bencher: Bencher<'_, '_>) {
    let fixture = &*FIXTURE;
    bencher.bench_local(|| {
        call(
            fixture,
            "read_thread_item",
            &json!({"item":fixture.item,"offset":8_000_000,"limit":16000}),
        )
    });
}

#[divan::bench(sample_count = 10)]
fn read_thread_around_a_long_item(bencher: Bencher<'_, '_>) {
    let fixture = &*FIXTURE;
    bencher.bench_local(|| call(fixture, "read_thread", &json!({"item":fixture.item})));
}
