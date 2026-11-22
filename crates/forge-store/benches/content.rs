use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
    Status, ThreadId,
};
use divan::Bencher;
use forge_store::{NewItem, Store};
use std::sync::LazyLock;

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    store: Store,
    thread: ThreadId,
    text: String,
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| {
    let dir = tempfile::tempdir().expect("bench: temp dir");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("bench: runtime");
    let store = Store::open(dir.path()).expect("bench: store");
    let thread = runtime.block_on(async {
        let project = store
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/work/app".into(),
                name: None,
            })
            .await
            .expect("bench: project");
        store
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: project.id,
                config: EngineConfig::new(EngineId::Claude),
                title: None,
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .expect("bench: thread")
            .id
    });
    let text = "- a list line of prose that a reply repeats\n+ another line\n".repeat(60_000);
    Fixture {
        _dir: dir,
        runtime,
        store,
        thread,
        text,
    }
});

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 20)]
fn store_a_repeated_large_reply(bencher: Bencher<'_, '_>) {
    let fixture = &*FIXTURE;
    bencher.bench_local(|| {
        fixture
            .runtime
            .block_on(fixture.store.upsert_item(NewItem {
                id: ItemId::new(),
                thread_id: fixture.thread.clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: fixture.text.as_str().into(),
                    phase: Phase::Final,
                },
            }))
            .expect("bench: store reply")
    });
}
