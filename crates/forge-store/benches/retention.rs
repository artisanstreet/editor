use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, ThreadId, now};
use divan::Bencher;
use forge_store::Store;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

const SINGLETONS: usize = 1_000;
const CHAIN: usize = 200;
const BATCH: usize = 8;

struct Fixture {
    _dir: tempfile::TempDir,
    database: PathBuf,
    runtime: tokio::runtime::Runtime,
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("bench: create copy");
    for entry in std::fs::read_dir(from).expect("bench: read fixture") {
        let entry = entry.expect("bench: fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("bench: entry type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("bench: copy fixture");
        }
    }
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| {
    let dir = tempfile::tempdir().expect("bench: temp dir");
    let database = dir.path().join("store");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("bench: runtime");
    runtime.block_on(async {
        let store = Store::open(&database).expect("bench: store");
        let project = store
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/work/app".into(),
                name: None,
            })
            .await
            .expect("bench: project")
            .id;
        let create = |parent: Option<ThreadId>| CreateThread {
            id: ThreadId::new(),
            project_id: project.clone(),
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: parent,
            spawned_by: None,
        };
        for _ in 0..SINGLETONS {
            store
                .create_thread(create(None))
                .await
                .expect("bench: thread");
        }
        let mut parent = None;
        for _ in 0..CHAIN {
            parent = Some(
                store
                    .create_thread(create(parent))
                    .await
                    .expect("bench: handoff")
                    .id,
            );
        }
        drop(store);
    });
    Fixture {
        _dir: dir,
        database,
        runtime,
    }
});

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 5)]
fn remove_every_inactive_thread(bencher: Bencher<'_, '_>) {
    let fixture = &*FIXTURE;
    bencher
        .with_inputs(|| {
            let copy = tempfile::tempdir().expect("bench: copy dir");
            copy_tree(&fixture.database, copy.path());
            let store = Store::open(copy.path()).expect("bench: open copy");
            (copy, store)
        })
        .bench_local_values(|(copy, store)| {
            let removed = fixture.runtime.block_on(async {
                let mut removed = 0;
                loop {
                    let batch = store
                        .remove_inactive_threads(now() + 60_000, HashSet::new(), BATCH)
                        .await
                        .expect("bench: remove");
                    if batch.is_empty() {
                        break removed;
                    }
                    removed += batch.len();
                }
            });
            assert_eq!(removed, SINGLETONS + CHAIN);
            (copy, store)
        });
}
