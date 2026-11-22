use artisan_api::{Event, Thread, ThreadId, ThreadsPage};
use divan::Bencher;
use editor_core::ThreadPageDirection;
use std::cell::RefCell;
use std::time::{Duration, Instant};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

const THREADS: u64 = 1_000;
const BURST: usize = 50;
const PAGE: u64 = 200;
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

fn wait_admitted(stubbed: &StubbedHost, before: usize, what: &str) {
    let deadline = Instant::now() + ADMIT_WITHIN;
    while stubbed.core.pending_work() <= before {
        assert!(
            Instant::now() < deadline,
            "bench: the Editor never admitted {what}"
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

fn drain(stubbed: &mut StubbedHost) {
    while stubbed.core.drain_step(|_, _| {}) {}
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
        next: Some(artisan_api::ThreadCursor {
            activity_seq: 0,
            id: ThreadId("thread-0".into()),
        }),
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

struct Pages {
    stubbed: StubbedHost,
    direction: ThreadPageDirection,
    activity: u64,
}

impl Pages {
    fn admit(&mut self) {
        drain(&mut self.stubbed);
        assert!(
            !self.stubbed.host().thread_page(false).is_loading(),
            "bench: the previous page applied"
        );
        let direction = self.direction;
        self.stubbed.host_mut().load_threads(false, direction);
        let threads: Vec<Thread> = (0..PAGE)
            .map(|step| match direction {
                ThreadPageDirection::Older => {
                    self.activity -= 1;
                    thread(self.activity + THREADS, self.activity, self.activity)
                }
                ThreadPageDirection::Latest | ThreadPageDirection::Newer => {
                    let index = (step * 5 + self.activity) % THREADS;
                    thread(index, self.activity + step, (index * 7) % THREADS)
                }
            })
            .collect();
        let next = threads.last().map(|last| artisan_api::ThreadCursor {
            activity_seq: last.activity_seq,
            id: last.id.clone(),
        });
        let before = self.stubbed.core.pending_work();
        self.stubbed
            .forge
            .take("GET", "/threads")
            .respond(ThreadsPage {
                threads,
                runs: Vec::new(),
                next,
                newer: false,
            });
        wait_admitted(&self.stubbed, before, "the page");
    }
}

#[divan::bench(sample_count = 50, sample_size = 1)]
fn page_back_past_a_full_window(bencher: Bencher<'_, '_>) {
    let pages = RefCell::new(Pages {
        stubbed: loaded_host(),
        direction: ThreadPageDirection::Older,
        activity: 1_000_000,
    });
    bencher
        .with_inputs(|| pages.borrow_mut().admit())
        .bench_local_values(|()| drain(&mut pages.borrow_mut().stubbed));
}

#[divan::bench(sample_count = 50, sample_size = 1)]
fn merge_a_refreshed_page_into_a_full_window(bencher: Bencher<'_, '_>) {
    let pages = RefCell::new(Pages {
        stubbed: loaded_host(),
        direction: ThreadPageDirection::Latest,
        activity: THREADS,
    });
    bencher
        .with_inputs(|| {
            let mut pages = pages.borrow_mut();
            pages.activity += PAGE;
            pages.admit();
        })
        .bench_local_values(|()| drain(&mut pages.borrow_mut().stubbed));
}

struct Deletions {
    stubbed: StubbedHost,
    next: u64,
}

impl Deletions {
    fn admit(&mut self) {
        let first = self.next;
        for index in first..first + BURST as u64 {
            self.stubbed.forge.publish(Event::Thread {
                thread: thread(THREADS + index, THREADS + index, THREADS + index),
            });
        }
        self.stubbed.apply_published();
        drain(&mut self.stubbed);
        let before = self.stubbed.core.pending_work();
        for index in first..first + BURST as u64 {
            self.stubbed.forge.publish(Event::ThreadRemoved {
                thread_id: ThreadId(format!("thread-{}", THREADS + index)),
            });
        }
        self.next += BURST as u64;
        wait_admitted(&self.stubbed, before + BURST - 1, "the burst of deletions");
    }
}

#[divan::bench(sample_count = 50, sample_size = 1)]
fn delete_a_burst_of_threads(bencher: Bencher<'_, '_>) {
    let deletions = RefCell::new(Deletions {
        stubbed: loaded_host(),
        next: 0,
    });
    bencher
        .with_inputs(|| deletions.borrow_mut().admit())
        .bench_local_values(|()| drain(&mut deletions.borrow_mut().stubbed));
}
