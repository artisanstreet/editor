//! Transcript pages: a thread pages in the rows a reader sees, the work
//! between them summarised, so a turn of any length opens in one page.

use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, Item, ItemId, ItemKind, ItemsQuery, Phase,
    ProjectId, Run, RunId, RunStatus, Status, ThreadId, ToolKind, Usage,
};
use forge_store::{NewItem, Store};

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn thread(store: &Store) -> ThreadId {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work".into(),
            name: None,
        })
        .await
        .unwrap();
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
        .unwrap()
        .id
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn put(
    store: &Store,
    thread: &ThreadId,
    run: &RunId,
    status: Status,
    kind: ItemKind,
) -> Item {
    store
        .upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread.clone(),
            run_id: Some(run.clone()),
            parent_id: None,
            native_id: None,
            status,
            kind,
        })
        .await
        .unwrap()
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn run(store: &Store, thread: &ThreadId, id: &RunId, status: RunStatus) {
    store
        .upsert_run(Run {
            config: None,
            id: id.clone(),
            thread_id: thread.clone(),
            engine: EngineId::Codex,
            model: None,
            input_cutoff: 0,
            status,
            usage: Usage::default(),
            session_id: None,
            error: None,
            seq: 0,
            started_at: 0,
            ended_at: None,
            continuation: None,
        })
        .await
        .unwrap();
}

fn user(text: &str, steer: bool) -> ItemKind {
    ItemKind::UserMessage {
        text: text.into(),
        images: Vec::new(),
        steer,
        from_agent: None,
        sender: None,
    }
}

fn shell(command: &str) -> ItemKind {
    ItemKind::Tool {
        tool: ToolKind::Shell {
            command: command.into(),
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

fn text(text: &str, phase: Phase) -> ItemKind {
    ItemKind::AssistantMessage {
        text: text.into(),
        phase,
    }
}

/// A finished turn of hundreds of calls, a steer and a question in the
/// middle, then a live turn still working.
async fn long_turns(store: &Store, thread: &ThreadId) -> (RunId, RunId, Vec<Item>) {
    let (done, live) = (RunId::from("done"), RunId::from("live"));
    run(store, thread, &done, RunStatus::Done).await;
    let mut stored = vec![
        put(
            store,
            thread,
            &done,
            Status::Done,
            user("Animate it", false),
        )
        .await,
    ];
    for step in 0..450 {
        let kind = match step {
            200 => user("Also the crawl", true),
            300 => ItemKind::Question {
                blocking: true,
                questions: artisan_api::Collection::Inline(Vec::new()),
                answers: None,
                answer_text: None,
            },
            step if step % 50 == 0 => text("Checking the rig.", Phase::Commentary),
            step => shell(&format!("cargo test -p anim{step}")),
        };
        let status = if step == 300 {
            Status::AwaitingUser
        } else {
            Status::Done
        };
        stored.push(put(store, thread, &done, status, kind).await);
    }
    stored.push(
        put(
            store,
            thread,
            &done,
            Status::Done,
            text("Animated.", Phase::Final),
        )
        .await,
    );
    run(store, thread, &live, RunStatus::Running).await;
    stored.push(
        put(
            store,
            thread,
            &live,
            Status::Done,
            user("Now the crawl", false),
        )
        .await,
    );
    for step in 0..150 {
        stored.push(
            put(
                store,
                thread,
                &live,
                Status::Done,
                shell(&format!("build {step}")),
            )
            .await,
        );
    }
    (done, live, stored)
}

#[tokio::test]
async fn a_long_turn_opens_as_its_rows_and_its_work_in_spans() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store).await;
    let (done, live, stored) = long_turns(&store, &thread).await;

    let tail = store
        .transcript_rows(
            &thread,
            ItemsQuery {
                before: None,
                after: None,
                limit: Some(40),
            },
        )
        .await
        .unwrap();
    assert!(!tail.more, "five rows fit one page");
    let rows: Vec<_> = tail
        .items
        .iter()
        .filter(|item| !item.folds_into_work())
        .map(|item| item.position)
        .collect();
    let expected: Vec<_> = stored
        .iter()
        .filter(|item| !item.folds_into_work())
        .map(|item| item.position)
        .collect();
    assert_eq!(rows, expected, "the messages, the question and the reply");
    // The finished turn's work is summarised, split by the question that
    // stands on its own; the steer folds into it.
    let finished: Vec<_> = tail
        .work
        .iter()
        .filter(|span| span.run_id.as_ref() == Some(&done))
        .map(|span| (span.count, span.worked))
        .collect();
    assert_eq!(finished, [(300, true), (149, true)]);
    // The live turn's newest work ships: it is open, and its phase reads
    // from it. The rest of it is a span ending just before.
    let live_items: Vec<_> = tail
        .items
        .iter()
        .filter(|item| item.run_id.as_ref() == Some(&live) && item.folds_into_work())
        .collect();
    assert_eq!(live_items.len(), 100);
    let rest = tail
        .work
        .iter()
        .find(|span| span.run_id.as_ref() == Some(&live))
        .expect("the live turn's older work");
    assert_eq!(rest.count, 50);
    assert!(rest.last < live_items[0].position);
    assert!(
        serde_json::to_vec(&tail).unwrap().len() < 64 * 1024,
        "opening reads a small page, not the turn"
    );

    // Opening the finished section reads its work newest first, a page at
    // a time, ending where its span ends.
    let span = tail.work.iter().find(|span| span.count == 149).unwrap();
    let page = store
        .work_page(
            &thread,
            ItemsQuery {
                before: Some(span.last + 1),
                after: Some(span.first - 1),
                limit: Some(100),
            },
        )
        .await
        .unwrap();
    assert!(page.more);
    assert_eq!(page.items.len(), 100);
    assert_eq!(page.items.last().unwrap().position, span.last);
    assert!(page.items.iter().all(Item::folds_into_work));
}

/// Paging back a row at a time, every root item arrives exactly once: as a
/// row or counted in one span.
#[tokio::test]
async fn row_pages_cover_the_thread_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store).await;
    let (.., stored) = long_turns(&store, &thread).await;
    let mut before = None;
    let (mut rows, mut counted, mut spans) = (0usize, 0u32, Vec::new());
    loop {
        let page = store
            .transcript_rows(
                &thread,
                ItemsQuery {
                    before,
                    after: None,
                    limit: Some(1),
                },
            )
            .await
            .unwrap();
        rows += page.items.len();
        counted += page.work.iter().map(|span| span.count).sum::<u32>();
        spans.extend(page.work.iter().map(|span| (span.first, span.last)));
        let oldest = page.items.first().map(|item| item.position);
        if !page.more {
            break;
        }
        before = oldest;
    }
    spans.sort_unstable();
    assert!(
        spans.windows(2).all(|pair| pair[0].1 < pair[1].0),
        "spans never overlap: {spans:?}"
    );
    assert_eq!(rows + counted as usize, stored.len());
}
