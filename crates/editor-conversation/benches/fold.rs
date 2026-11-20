use artisan_api::{
    Continuation, EngineId, Item, ItemId, ItemKind, Output, Phase, Run, RunId, RunOutcome,
    RunState, RunStatus, Status, ThreadId, ToolKind, Usage,
};
use editor_conversation::adapter::RowLayout;
use editor_conversation::disclosure::RowDisclosure;
use editor_conversation::fold::{expanded, fold};
use editor_conversation::thread::ThreadView;
use editor_conversation::view::factory::RowFingerprint;

const TURNS: u64 = 150;
const RUNS_PER_TURN: u64 = 5;
const TOOLS_PER_RUN: u64 = 3;

fn main() {
    divan::main();
}

fn thread() -> ThreadId {
    ThreadId("thread".into())
}

fn item(position: u64, run: &RunId, kind: ItemKind) -> Item {
    Item {
        id: ItemId(format!("item-{position}")),
        thread_id: thread(),
        position,
        seq: position,
        run_id: Some(run.clone()),
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind,
    }
}

fn run(id: &RunId, status: RunStatus, started_at: i64, carried: bool) -> Run {
    Run {
        config: None,
        id: id.clone(),
        thread_id: thread(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        state: RunOutcome::of(status).map_or(RunState::Running, |outcome| RunState::Finished {
            outcome,
            error: None,
            ended_at: started_at,
        }),
        usage: Usage::default(),
        session_id: None,
        seq: 1,
        started_at,
        continuation: carried.then_some(Continuation::Restart),
    }
}

fn shell(position: u64, run: &RunId) -> Item {
    item(
        position,
        run,
        ItemKind::Tool {
            tool: ToolKind::Shell {
                command: format!("cargo test -p crate-{position}").into(),
                cwd: None,
                background: false,
                watch: false,
            },
            title: None,
            output: Some(Output::from_text("test result: ok")),
            exit_code: Some(0),
            approval: None,
        },
    )
}

fn continued_turns() -> ThreadView {
    let (mut view, _) = ThreadView::open(thread());
    let mut position = 0;
    let mut started = 0;
    for turn in 0..TURNS {
        for part in 0..RUNS_PER_TURN {
            let id = RunId(format!("run-{turn}-{part}"));
            started += 1_000;
            let last = part + 1 == RUNS_PER_TURN;
            let status = if last {
                RunStatus::Done
            } else {
                RunStatus::Cancelled
            };
            view.runs
                .insert(id.clone(), run(&id, status, started, part > 0));
            if part == 0 {
                position += 1;
                view.apply_item(
                    item(
                        position,
                        &id,
                        ItemKind::UserMessage {
                            text: format!("Turn {turn}: fix the failing tests").into(),
                            steer: false,
                            from_agent: None,
                            images: Vec::new(),
                            sender: None,
                        },
                    ),
                    None,
                );
            }
            for _ in 0..TOOLS_PER_RUN {
                position += 1;
                view.apply_item(shell(position, &id), None);
            }
            if last {
                position += 1;
                view.apply_item(
                    item(
                        position,
                        &id,
                        ItemKind::AssistantMessage {
                            text: format!("Turn {turn} is done.").into(),
                            phase: Phase::Final,
                        },
                    ),
                    None,
                );
            }
        }
    }
    view
}

#[divan::bench(sample_count = 20)]
fn refold_continued_turns(bencher: divan::Bencher<'_, '_>) {
    let view = continued_turns();
    bencher.bench_local(|| {
        let folded = fold(&view, None, None);
        expanded(&view, folded.rows, &folded.runs)
    });
}

#[divan::bench(sample_count = 20)]
fn clone_the_row_snapshot_of_a_full_window(bencher: divan::Bencher<'_, '_>) {
    let view = continued_turns();
    let folded = fold(&view, None, None);
    let rows = expanded(&view, folded.rows, &folded.runs);
    bencher.bench_local(|| (view.clone(), rows.clone(), folded.runs.clone()));
}

#[divan::bench(sample_count = 20)]
fn space_every_row_of_continued_turns(bencher: divan::Bencher<'_, '_>) {
    let view = continued_turns();
    let folded = fold(&view, None, None);
    let rows = expanded(&view, folded.rows, &folded.runs);
    bencher.bench_local(|| {
        (0..rows.len())
            .map(|index| editor_conversation::fold::gap(&view, &folded.turns, &rows, index))
            .sum::<f32>()
    });
}

#[divan::bench(sample_count = 20)]
fn revise_every_row_of_continued_turns(bencher: divan::Bencher<'_, '_>) {
    let view = continued_turns();
    let folded = fold(&view, None, None);
    let rows = expanded(&view, folded.rows, &folded.runs);
    let layout = RowLayout {
        view: &view,
        turns: &folded.turns,
        rows: &rows,
        agent: false,
        composer_height: None,
    };
    let fingerprint = RowFingerprint {
        disclosure: RowDisclosure::default(),
        held: false,
        body_version: None,
        style_epoch: 0,
    };
    bencher.bench_local(|| {
        (0..rows.len())
            .filter_map(|index| fingerprint.value(&layout, index))
            .fold(0, u64::wrapping_add)
    });
}
