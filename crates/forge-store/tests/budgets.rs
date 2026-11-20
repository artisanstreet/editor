use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, ItemsQuery, Phase,
    ProjectId, RunId, Status, ThreadId, ToolKind,
};
use forge_store::{NewItem, Store};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    limit: u64,
    reason: String,
}

fn budget(name: &str) -> Budget {
    let mut budgets: BTreeMap<String, Budget> =
        serde_json::from_str(include_str!("budgets/store.json"))
            .expect("the budget file is valid JSON");
    budgets
        .remove(name)
        .unwrap_or_else(|| panic!("budgets/store.json has no {name}"))
}

struct Counted {
    _directory: tempfile::TempDir,
    _lock: forge_store::DataLock,
    store: Store,
    steps: Arc<AtomicU64>,
}

impl Counted {
    fn open() -> Self {
        let directory = tempfile::tempdir().expect("a Forge data folder");
        let steps = Arc::new(AtomicU64::new(0));
        let counting = Arc::clone(&steps);
        let connections = forge_store::Connections::new(move |path| {
            let mut conn = rusqlite::Connection::open(path)?;
            let counting = Arc::clone(&counting);
            conn.progress_handler(
                1,
                Some(move || {
                    counting.fetch_add(1, Ordering::Relaxed);
                    false
                }),
            )?;
            Ok(conn)
        });
        let lock = forge_store::DataLock::acquire(directory.path()).expect("hold the data folder");
        let store = Store::open_restoring_in(
            &lock,
            forge_store::Environment {
                connections,
                ..forge_store::Environment::system()
            },
        )
        .expect("open the counted store");
        Self {
            _directory: directory,
            _lock: lock,
            store,
            steps,
        }
    }

    async fn steps_of<T>(&self, work: impl Future<Output = T>) -> (u64, T) {
        let before = self.steps.load(Ordering::Relaxed);
        let output = work.await;
        (self.steps.load(Ordering::Relaxed) - before, output)
    }
}

fn report(name: &str, measured: u64, budget: &Budget) {
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write as _;
        let mut summary = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .expect("the job summary opens for appending");
        writeln!(
            summary,
            "| Budget | Measured | Limit |\n| --- | ---: | ---: |\n| `{name}` | {measured} | {} |",
            budget.limit
        )
        .expect("the job summary accepts the store budget");
    }
    assert!(
        measured <= budget.limit,
        "{name} measured {measured} against a budget of {}: {}",
        budget.limit,
        budget.reason
    );
}

async fn thread(store: &Store) -> ThreadId {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/budget".into(),
            name: None,
        })
        .await
        .expect("create the project");
    store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Codex),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .expect("create the thread")
        .id
}

fn item(thread: &ThreadId, run: &RunId, kind: ItemKind) -> NewItem {
    NewItem {
        id: ItemId::new(),
        thread_id: thread.clone(),
        run_id: Some(run.clone()),
        parent_id: None,
        native_id: None,
        status: Status::Done,
        kind,
    }
}

fn prompt(text: &str) -> ItemKind {
    ItemKind::UserMessage {
        text: text.into(),
        images: Vec::new(),
        steer: false,
        from_agent: None,
        sender: None,
    }
}

fn shell() -> ItemKind {
    ItemKind::Tool {
        tool: ToolKind::Shell {
            command: "cargo test".into(),
            cwd: None,
            background: false,
            watch: false,
        },
        title: None,
        output: None,
        exit_code: Some(0),
        approval: None,
    }
}

fn reply(text: &str) -> ItemKind {
    ItemKind::AssistantMessage {
        text: text.into(),
        phase: Phase::Final,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collapsed_work_costs_the_same_however_much_of_it_is_hidden() {
    const HIDDEN: usize = 2_000;
    let budget = budget("transcript_rows_steps_across_hidden_work");
    let counted = Counted::open();
    let store = &counted.store;
    let thread = thread(store).await;
    let run = RunId::from("finished");
    store
        .upsert_item(item(&thread, &run, prompt("Fix the build")))
        .await
        .expect("store the prompt");
    for _ in 0..HIDDEN {
        store
            .upsert_item(item(&thread, &run, shell()))
            .await
            .expect("store hidden work");
    }
    store
        .upsert_item(item(&thread, &run, reply("Fixed")))
        .await
        .expect("store the reply");
    let query = ItemsQuery {
        before: Some(u64::MAX >> 1),
        after: None,
        limit: Some(100),
    };
    store
        .transcript_rows(&thread, query.clone())
        .await
        .expect("open a read connection");
    let (steps, page) = counted
        .steps_of(store.transcript_rows(&thread, query))
        .await;
    let page = page.expect("read the transcript");
    assert_eq!(page.items.len(), 2);
    assert_eq!(
        page.work.iter().map(|span| span.count).collect::<Vec<_>>(),
        [u32::try_from(HIDDEN).expect("the hidden work fits a count")]
    );
    report("transcript_rows_steps_across_hidden_work", steps, &budget);
}
