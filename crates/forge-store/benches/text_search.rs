use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
    Status, TextSearchQuery, ThreadId, UpdateThread,
};
use divan::Bencher;
use forge_store::{NewItem, Store};
use std::sync::LazyLock;

const THREADS: usize = 300;
const MESSAGES: usize = 40;
const ARCHIVED: usize = 60;
const LONG_LINE_THREADS: usize = 30;
const LONG_LINE_BYTES: usize = 60 * 1024;

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    store: Store,
}

fn said(thread: &ThreadId, text: String) -> NewItem {
    NewItem {
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
    }
}

fn message(thread: usize, index: usize) -> String {
    let paragraph = "The renderer keeps every glyph in an atlas and repaints only the damaged rows, so scrolling stays smooth while the sidebar updates. ";
    format!("Thread {thread} reply {index}. {}", paragraph.repeat(16))
}

fn empty_store() -> (tempfile::TempDir, tokio::runtime::Runtime, Store, ProjectId) {
    let dir = tempfile::tempdir().expect("bench: temp dir");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("bench: runtime");
    let store = Store::open(dir.path()).expect("bench: store");
    let project = runtime
        .block_on(store.create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        }))
        .expect("bench: project")
        .id;
    (dir, runtime, store, project)
}

async fn new_thread(store: &Store, project: &ProjectId, thread: usize) -> ThreadId {
    store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.clone(),
            config: EngineConfig::new(EngineId::Claude),
            title: Some(format!("Thread {thread}")),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .expect("bench: thread")
        .id
}

static LONG_LINES: LazyLock<Fixture> = LazyLock::new(|| {
    let (dir, runtime, store, project) = empty_store();
    let line = "minified(bundle);".repeat(LONG_LINE_BYTES / 17);
    runtime.block_on(async {
        for thread in 0..LONG_LINE_THREADS {
            let id = new_thread(&store, &project, thread).await;
            store
                .upsert_item(said(&id, format!("{line} heron")))
                .await
                .expect("bench: item");
        }
    });
    Fixture {
        _dir: dir,
        runtime,
        store,
    }
});

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| {
    let (dir, runtime, store, project) = empty_store();
    runtime.block_on(async {
        for thread in 0..THREADS {
            let id = new_thread(&store, &project, thread).await;
            for index in 0..MESSAGES {
                let text = if thread < ARCHIVED && index == 0 {
                    format!("{} archived pelican", message(thread, index))
                } else {
                    message(thread, index)
                };
                store
                    .upsert_item(said(&id, text))
                    .await
                    .expect("bench: item");
            }
            if thread < ARCHIVED {
                store
                    .update_thread(
                        &id,
                        UpdateThread {
                            title: None,
                            archived: Some(true),
                            config: None,
                            read_seq: None,
                        },
                    )
                    .await
                    .expect("bench: archive");
            }
        }
    });
    Fixture {
        _dir: dir,
        runtime,
        store,
    }
});

fn main() {
    divan::main();
}

fn search(bencher: Bencher<'_, '_>, query: &str) {
    search_in(bencher, &FIXTURE, query);
}

fn search_in(bencher: Bencher<'_, '_>, fixture: &Fixture, query: &str) {
    bencher.bench_local(|| {
        fixture
            .runtime
            .block_on(fixture.store.text_search(TextSearchQuery {
                query: query.into(),
                limit: Some(30),
            }))
            .expect("bench: search")
    });
}

#[divan::bench(sample_count = 10)]
fn missing_word(bencher: Bencher<'_, '_>) {
    search(bencher, "flamingo");
}

#[divan::bench(sample_count = 10)]
fn word_only_in_archived_threads(bencher: Bencher<'_, '_>) {
    search(bencher, "pelican");
}

#[divan::bench(sample_count = 10)]
fn common_phrase(bencher: Bencher<'_, '_>) {
    search(bencher, "damaged rows");
}

#[divan::bench(sample_count = 10)]
fn uppercase_phrase(bencher: Bencher<'_, '_>) {
    search(bencher, "DAMAGED ROWS");
}

#[divan::bench(sample_count = 10)]
fn missing_accented_word(bencher: Bencher<'_, '_>) {
    search(bencher, "RÉNDERER");
}

#[divan::bench(sample_count = 10)]
fn missing_two_letters(bencher: Bencher<'_, '_>) {
    search(bencher, "qz");
}

#[divan::bench(sample_count = 10)]
fn match_at_the_end_of_a_long_line(bencher: Bencher<'_, '_>) {
    search_in(bencher, &LONG_LINES, "heron");
}
