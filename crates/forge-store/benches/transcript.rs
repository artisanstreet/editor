use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, ItemsQuery, Phase,
    ProjectId, Run, RunId, RunState, RunStatus, Status, ThreadId, ToolKind, Usage,
};
use divan::Bencher;
use forge_store::{NewItem, Store};
use std::sync::LazyLock;

const PROMPTS: usize = 600;
const HIDDEN_WORK: usize = 6_000;
const LIVE_SPANS: usize = 8;
const LIVE_SPAN_ITEMS: usize = 150;

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    store: Store,
    thread: ThreadId,
}

fn shell(index: usize) -> ItemKind {
    ItemKind::Tool {
        tool: ToolKind::Shell {
            command: format!("cargo test -p crate-{index}").into(),
            cwd: None,
            background: false,
            watch: false,
        },
        title: None,
        output: Some(artisan_api::Output::from_text(&format!(
            "test result: ok. {index} passed\n{}",
            "running tests in the renderer crate\n".repeat(160)
        ))),
        exit_code: Some(0),
        approval: None,
    }
}

fn user(index: usize) -> ItemKind {
    ItemKind::UserMessage {
        text: format!("Prompt {index}: make the sidebar keep its scroll position while threads update\nwith a second line of detail").into(),
        images: Vec::new(),
        steer: false,
        from_agent: None,
        sender: None,
    }
}

fn run(thread: &ThreadId, id: &RunId, status: RunStatus) -> Run {
    Run {
        config: None,
        id: id.clone(),
        thread_id: thread.clone(),
        engine: EngineId::Codex,
        model: None,
        input_cutoff: 0,
        state: artisan_api::RunOutcome::of(status).map_or(RunState::Running, |outcome| {
            RunState::Finished {
                outcome,
                error: None,
                ended_at: 0,
            }
        }),
        usage: Usage::default(),
        session_id: None,
        seq: 0,
        started_at: 0,
        continuation: None,
    }
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
                path: "/work".into(),
                name: None,
            })
            .await
            .expect("bench: project");
        let thread = store
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: project.id,
                config: EngineConfig::new(EngineId::Codex),
                title: None,
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .expect("bench: thread")
            .id;
        let put = |run: &RunId, kind: ItemKind| NewItem {
            id: ItemId::new(),
            thread_id: thread.clone(),
            run_id: Some(run.clone()),
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind,
        };
        for prompt in 0..PROMPTS {
            let id = RunId::from(format!("done-{prompt}").as_str());
            store
                .upsert_run(run(&thread, &id, RunStatus::Done))
                .await
                .expect("bench: run");
            store
                .upsert_item(put(&id, user(prompt)))
                .await
                .expect("bench: prompt");
            let work = if prompt == PROMPTS - 100 {
                HIDDEN_WORK
            } else {
                2
            };
            for call in 0..work {
                store
                    .upsert_item(put(&id, shell(call)))
                    .await
                    .expect("bench: work");
            }
            store
                .upsert_item(put(
                    &id,
                    ItemKind::AssistantMessage {
                        text: format!("Reply {prompt}").into(),
                        phase: Phase::Final,
                    },
                ))
                .await
                .expect("bench: reply");
        }
        let live = RunId::from("live");
        store
            .upsert_run(run(&thread, &live, RunStatus::Running))
            .await
            .expect("bench: live run");
        for span in 0..LIVE_SPANS {
            store
                .upsert_item(put(&live, user(PROMPTS + span)))
                .await
                .expect("bench: steer");
            for call in 0..LIVE_SPAN_ITEMS {
                store
                    .upsert_item(put(&live, shell(call)))
                    .await
                    .expect("bench: live work");
            }
        }
        thread
    });
    Fixture {
        _dir: dir,
        runtime,
        store,
        thread,
    }
});

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 20)]
fn latest_transcript_page(bencher: Bencher<'_, '_>) {
    let fixture = &*FIXTURE;
    bencher.bench_local(|| {
        fixture
            .runtime
            .block_on(fixture.store.transcript_rows(
                &fixture.thread,
                ItemsQuery {
                    before: None,
                    after: None,
                    limit: Some(100),
                },
            ))
            .expect("bench: page")
    });
}

#[divan::bench(sample_count = 20)]
fn page_across_hidden_work(bencher: Bencher<'_, '_>) {
    let fixture = &*FIXTURE;
    let page = fixture
        .runtime
        .block_on(fixture.store.transcript_rows(
            &fixture.thread,
            ItemsQuery {
                before: None,
                after: None,
                limit: Some(100),
            },
        ))
        .expect("bench: first page");
    let before = page.items.first().map(|item| item.position);
    bencher.bench_local(|| {
        fixture
            .runtime
            .block_on(fixture.store.transcript_rows(
                &fixture.thread,
                ItemsQuery {
                    before,
                    after: None,
                    limit: Some(400),
                },
            ))
            .expect("bench: page")
    });
}

#[divan::bench(sample_count = 20)]
fn inspector_refresh(bencher: Bencher<'_, '_>) {
    let fixture = &*FIXTURE;
    bencher.bench_local(|| {
        fixture
            .runtime
            .block_on(fixture.store.inspector(&fixture.thread))
            .expect("bench: inspector")
    });
}
