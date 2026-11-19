use artisan_api::{Event, Thread, ThreadId, ThreadsPage};
use divan::Bencher;
use editor_core::ThreadPageDirection;
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

const THREADS: u64 = 1_000;
const UPDATES: u64 = 100;
const CHUNK: u64 = 25;

fn thread(index: u64, seq: u64, activity: u64) -> Thread {
    serde_json::from_value(serde_json::json!({
        "id": format!("thread-{index}"),
        "project_id": "project",
        "title": format!("Thread {index}"),
        "config": {"engine": "claude"},
        "activity_seq": activity,
        "read_seq": activity,
        "seq": seq,
        "created_at": 0,
        "updated_at": 0,
        "archived": false,
    }))
    .expect("bench: thread")
}

fn loaded_host() -> StubbedHost {
    let mut stubbed = StubbedHost::start(empty_state(), axum::Router::new());
    stubbed
        .host_mut()
        .load_threads(false, ThreadPageDirection::Latest);
    stubbed.forge.take("GET", "/threads").respond(ThreadsPage {
        threads: (0..THREADS)
            .map(|index| thread(index, index, index))
            .collect(),
        runs: Vec::new(),
        next: None,
        newer: false,
    });
    stubbed.settle("bench: the first page applies", |core, host| {
        core.host(host)
            .is_some_and(|data| !data.thread_page(false).is_loading())
    });
    stubbed
}

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 20)]
fn apply_a_burst_of_thread_updates(bencher: Bencher<'_, '_>) {
    let mut stubbed = loaded_host();
    let mut seq = THREADS;
    bencher.bench_local(|| {
        for chunk in 0..UPDATES / CHUNK {
            let mut last = None;
            for update in chunk * CHUNK..(chunk + 1) * CHUNK {
                seq += 1;
                let updated = thread((update * 37) % THREADS, seq, seq);
                last = Some((updated.id.clone(), seq));
                stubbed.forge.publish(Event::Thread { thread: updated });
            }
            let (id, seq): (ThreadId, u64) = last.expect("bench: a chunk has updates");
            stubbed.settle("bench: the burst applies", |core, host| {
                core.host(host)
                    .and_then(|data| data.threads.get(&id))
                    .is_some_and(|thread| thread.seq == seq)
            });
        }
    });
}
