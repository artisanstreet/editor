use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, Item, ItemId, ItemKind, ItemsQuery, Phase,
    ProjectId, RunId, Status, ThreadId, ToolKind, WorkSpan,
};
use forge_store::{NewItem, Store};
use std::collections::BTreeMap;

struct Dice(u64);

impl Dice {
    fn roll(&mut self, sides: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % sides as u64).expect("a roll is below the side count")
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Tool,
    Reply,
    Agent,
    Prompt,
}

fn kind(shape: Shape, dice: &mut Dice) -> ItemKind {
    match shape {
        Shape::Tool => ItemKind::Tool {
            tool: ToolKind::Shell {
                command: "cargo test".into(),
                cwd: None,
                background: false,
                watch: false,
            },
            title: None,
            output: None,
            exit_code: None,
            approval: None,
        },
        Shape::Reply => ItemKind::AssistantMessage {
            text: "working on it".into(),
            phase: [Phase::Commentary, Phase::Final][dice.roll(2)],
        },
        Shape::Agent => ItemKind::Agent {
            name: None,
            description: "review".into(),
            prompt: None,
            agent_id: None,
            context_used: None,
            model: None,
            background: false,
            result: None,
            label: Some("reviewer".into()),
            engine: Some(EngineId::Codex),
            thread: None,
        },
        Shape::Prompt => ItemKind::UserMessage {
            text: "next".into(),
            images: Vec::new(),
            steer: dice.roll(2) == 0,
            from_agent: None,
            sender: None,
        },
    }
}

fn reference(items: &BTreeMap<ItemId, Item>) -> Vec<WorkSpan> {
    let mut roots: Vec<&Item> = items
        .values()
        .filter(|item| item.parent_id.is_none())
        .collect();
    roots.sort_by_key(|item| item.position);
    let mut spans: Vec<WorkSpan> = Vec::new();
    let mut open = false;
    for item in roots {
        if !item.folds_into_work() {
            open = false;
            continue;
        }
        let worked = matches!(
            item.kind,
            ItemKind::Tool { .. } | ItemKind::Agent { .. } | ItemKind::Plan { .. }
        );
        match spans.last_mut() {
            Some(span) if open && span.run_id == item.run_id => {
                span.last = item.position;
                span.count += 1;
                span.worked |= worked;
            }
            _ => spans.push(WorkSpan {
                run_id: item.run_id.clone(),
                first: item.position,
                last: item.position,
                count: 1,
                worked,
            }),
        }
        open = true;
    }
    spans
}

fn reference_after(items: &BTreeMap<ItemId, Item>, after: u64) -> Vec<WorkSpan> {
    reference(items)
        .into_iter()
        .filter_map(|span| {
            let inside: Vec<&Item> = items
                .values()
                .filter(|item| {
                    item.parent_id.is_none()
                        && item.folds_into_work()
                        && item.position >= span.first
                        && item.position <= span.last
                        && item.position > after
                })
                .collect();
            let first = inside.iter().map(|item| item.position).min()?;
            Some(WorkSpan {
                first,
                last: inside.iter().map(|item| item.position).max()?,
                count: u32::try_from(inside.len()).expect("a span's rows fit a count"),
                worked: inside.iter().any(|item| {
                    matches!(
                        item.kind,
                        ItemKind::Tool { .. } | ItemKind::Agent { .. } | ItemKind::Plan { .. }
                    )
                }),
                ..span
            })
        })
        .collect()
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn thread(store: &Store) -> ThreadId {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/spans".into(),
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
async fn shown(store: &Store, thread: &ThreadId, query: ItemsQuery) -> Vec<WorkSpan> {
    store.transcript_rows(thread, query).await.unwrap().work
}

#[tokio::test]
async fn collapsed_work_matches_the_transcript_through_every_kind_of_edit() {
    for seed in [0x9e37_79b9_7f4a_7c15, 0x2545_f491_4f6c_dd1d, 7] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let thread = thread(&store).await;
        let runs = [
            None,
            Some(RunId::from("first")),
            Some(RunId::from("second")),
        ];
        let mut dice = Dice(seed);
        let mut items: BTreeMap<ItemId, Item> = BTreeMap::new();
        let mut shapes: BTreeMap<ItemId, Shape> = BTreeMap::new();
        for step in 0..400 {
            let existing: Vec<ItemId> = items.keys().cloned().collect();
            let editing = !existing.is_empty() && dice.roll(2) == 0;
            let (id, shape) = if editing {
                let id = existing[dice.roll(existing.len())].clone();
                let shape = shapes[&id];
                (id, shape)
            } else {
                let shape = [
                    Shape::Tool,
                    Shape::Tool,
                    Shape::Reply,
                    Shape::Agent,
                    Shape::Prompt,
                ][dice.roll(5)];
                (ItemId::new(), shape)
            };
            let parent_id = (dice.roll(8) == 0 && !existing.is_empty())
                .then(|| existing[dice.roll(existing.len())].clone())
                .filter(|parent| *parent != id);
            let item = store
                .upsert_item(NewItem {
                    id: id.clone(),
                    thread_id: thread.clone(),
                    run_id: runs[dice.roll(runs.len())].clone(),
                    parent_id,
                    native_id: None,
                    status: [Status::Done, Status::Running, Status::AwaitingUser][dice.roll(3)],
                    kind: kind(shape, &mut dice),
                })
                .await
                .unwrap();
            shapes.insert(id.clone(), shape);
            items.insert(id, item);
            assert_eq!(
                shown(
                    &store,
                    &thread,
                    ItemsQuery {
                        before: None,
                        after: None,
                        limit: Some(500),
                    },
                )
                .await,
                reference(&items),
                "seed {seed:#x}, step {step}"
            );
        }
        let positions: Vec<u64> = items.values().map(|item| item.position).collect();
        let middle = positions[positions.len() / 2];
        let clipped = reference_after(&items, middle);
        assert_eq!(
            shown(
                &store,
                &thread,
                ItemsQuery {
                    before: None,
                    after: Some(middle),
                    limit: Some(500),
                },
            )
            .await,
            clipped,
            "seed {seed:#x}: a page starting inside a span shows only its part after the bound"
        );
    }
}
