use artisan_api::{ProjectId, ThreadId};
use divan::Bencher;
use editor_sidebar::list::{AgeGroup, HistoryGroup, Lists, RowData, RowState};
use std::collections::HashMap;

const ROWS: usize = 5_000;
const PULSES: usize = 50;

fn row(index: usize, project: &ProjectId) -> RowData {
    RowData {
        id: ThreadId::new(),
        title: format!("Thread {index}"),
        subtitle: String::new(),
        project: project.clone(),
        state: if index.is_multiple_of(100) {
            RowState::Working
        } else {
            RowState::Idle
        },
        selected: false,
        settleable: false,
        matched: Vec::new(),
    }
}

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 50)]
fn retain_working_pulses(bencher: Bencher<'_, '_>) {
    let project = ProjectId::new();
    let rows: Vec<RowData> = (0..ROWS).map(|index| row(index, &project)).collect();
    let pulses: HashMap<ThreadId, u32> = rows
        .iter()
        .step_by(ROWS / PULSES)
        .map(|row| (row.id.clone(), 0))
        .collect();
    let lists = Lists {
        inbox: rows[..200].to_vec(),
        history: vec![HistoryGroup {
            age: AgeGroup::Older,
            rows: rows[200..].to_vec(),
        }],
        hits: Vec::new(),
        next_tick: None,
    };
    bencher.bench_local(|| {
        let mut pulses = pulses.clone();
        let working = lists.working();
        pulses.retain(|id, _| working.contains(id));
        pulses
    });
}
