use artisan_api::{Event, Thread, ThreadId, ThreadsPage};
use divan::Bencher;
use editor_core::ThreadPageDirection;
use std::cell::RefCell;
use std::time::{Duration, Instant};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

const THREADS: u64 = 1_000;
const BURST: usize = 50;
const ADMIT_WITHIN: Duration = Duration::from_secs(10);

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

struct Burst {
    stubbed: StubbedHost,
    seq: u64,
    last: Option<ThreadId>,
}

impl Burst {
    fn admit(&mut self) {
        assert_eq!(
            self.stubbed.core.pending_work(),
            0,
            "bench: the previous burst drained completely"
        );
        if let Some(id) = &self.last {
            let applied = self.stubbed.host().threads.get(id).map(|thread| thread.seq);
            assert_eq!(applied, Some(self.seq), "bench: the previous burst applied");
        }
        for _ in 0..BURST {
            self.seq += 1;
            let updated = thread((self.seq * 37) % THREADS, self.seq, self.seq);
            self.last = Some(updated.id.clone());
            self.stubbed
                .forge
                .publish(Event::Thread { thread: updated });
        }
        let deadline = Instant::now() + ADMIT_WITHIN;
        while self.stubbed.core.pending_work() < BURST {
            assert!(
                Instant::now() < deadline,
                "bench: the Editor never admitted the burst of {BURST} events"
            );
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }

    fn apply(&mut self) {
        while self.stubbed.core.drain_step(|_, _| {}) {}
    }
}

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 100, sample_size = 1)]
fn apply_a_burst_of_thread_updates(bencher: Bencher<'_, '_>) {
    let burst = RefCell::new(Burst {
        stubbed: loaded_host(),
        seq: THREADS,
        last: None,
    });
    bencher
        .with_inputs(|| burst.borrow_mut().admit())
        .bench_local_values(|()| burst.borrow_mut().apply());
}
