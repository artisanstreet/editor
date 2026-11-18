use artisan_api::{
    Collection, CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Output,
    Phase, PlanStep, ProjectId, Status, StepStatus, ThreadId, ToolKind,
};
use divan::Bencher;
use forge_store::{GrepQuery, NewItem, RecallScope, Store};
use std::sync::LazyLock;

const ITEMS: usize = 4_000;
const PLAN_STEPS: usize = 20_000;
const LONG_MATCHES: usize = 2_000;

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    store: Store,
    plan: NewItem,
}

fn item(thread: &ThreadId, kind: ItemKind) -> NewItem {
    NewItem {
        id: ItemId::new(),
        thread_id: thread.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status: Status::Done,
        kind,
    }
}

fn shell(thread: &ThreadId, output: &str) -> NewItem {
    item(
        thread,
        ItemKind::Tool {
            tool: ToolKind::Shell {
                command: "cargo build".into(),
                cwd: None,
                background: false,
                watch: false,
            },
            title: None,
            output: Some(Output::from_text(output)),
            exit_code: Some(0),
            approval: None,
        },
    )
}

fn plan(thread: &ThreadId, proposal: &str) -> NewItem {
    item(
        thread,
        ItemKind::Plan {
            steps: Collection::Inline(
                (0..PLAN_STEPS)
                    .map(|step| PlanStep {
                        text: format!("Step {step}: move the widget renderer behind the scene cache and measure it").into(),
                        status: StepStatus::Pending,
                    })
                    .collect(),
            ),
            proposal: Some(proposal.into()),
            approval: None,
        },
    )
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| {
    let dir = tempfile::tempdir().expect("bench: temp dir");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("bench: runtime");
    let store = Store::open(dir.path()).expect("bench: store");
    let plan = runtime.block_on(async {
        let project = store
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/work/app".into(),
                name: None,
            })
            .await
            .expect("bench: project");
        let thread = store
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: project.id,
                config: EngineConfig::new(EngineId::Claude),
                title: Some("Long build thread".into()),
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .expect("bench: thread")
            .id;
        let long = (0..LONG_MATCHES)
            .map(|line| format!("warning: unused variable `frame_{line}`"))
            .collect::<Vec<_>>()
            .join("\n");
        store
            .upsert_item(shell(&thread, &long))
            .await
            .expect("bench: long output");
        for index in 0..ITEMS {
            let new = if index % 2 == 0 {
                shell(
                    &thread,
                    &format!("Compiling crate {index}\nFinished build {index}: linker ok"),
                )
            } else {
                item(
                    &thread,
                    ItemKind::AssistantMessage {
                        text: format!("Reply {index}: the scene bound holds at 512 items while the sidebar refreshes.").into(),
                        phase: Phase::Final,
                    },
                )
            };
            store.upsert_item(new).await.expect("bench: item");
        }
        let stored = store
            .upsert_item(plan(&thread, "first proposal"))
            .await
            .expect("bench: plan");
        let mut plan = plan(&thread, "first proposal");
        plan.id = stored.id;
        plan.kind = stored.kind;
        plan
    });
    Fixture {
        _dir: dir,
        runtime,
        store,
        plan,
    }
});

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 20)]
fn reindex_stored_plan(bencher: Bencher<'_, '_>) {
    let fixture = &*FIXTURE;
    let mut flip = false;
    bencher.bench_local(|| {
        flip = !flip;
        let mut plan = fixture.plan.clone();
        if let ItemKind::Plan { proposal, .. } = &mut plan.kind {
            *proposal = Some(
                if flip {
                    "second proposal"
                } else {
                    "first proposal"
                }
                .into(),
            );
        }
        fixture
            .runtime
            .block_on(fixture.store.upsert_item(plan))
            .expect("bench: reindex")
    });
}

fn grep(pattern: &str, cursor: Option<forge_store::GrepCursor>) -> GrepQuery {
    GrepQuery {
        pattern: pattern.into(),
        ignore_case: false,
        output: true,
        context: 1,
        max_matches: Some(100),
        cursor,
    }
}

#[divan::bench(sample_count = 20)]
fn grep_pages_through_one_long_output(bencher: Bencher<'_, '_>) {
    let fixture = &*FIXTURE;
    bencher.bench_local(|| {
        let mut cursor = None;
        let mut found = 0;
        loop {
            let page = fixture
                .runtime
                .block_on(
                    fixture
                        .store
                        .recall_grep(RecallScope::default(), grep("unused variable", cursor)),
                )
                .expect("bench: grep");
            found += page.matches.len();
            match page.next {
                Some(next) => cursor = Some(next),
                None => break found,
            }
        }
    });
}

#[divan::bench(sample_count = 20)]
fn search_output_second_page(bencher: Bencher<'_, '_>) {
    let fixture = &*FIXTURE;
    bencher.bench_local(|| {
        let first = fixture
            .runtime
            .block_on(fixture.store.recall_search(
                RecallScope::default(),
                "linker ok".into(),
                true,
                50,
                None,
            ))
            .expect("bench: search");
        fixture
            .runtime
            .block_on(fixture.store.recall_search(
                RecallScope::default(),
                "linker ok".into(),
                true,
                50,
                first.next,
            ))
            .expect("bench: search")
    });
}
