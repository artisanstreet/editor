use artisan_api::{Item, ItemId, ItemKind, ItemsPage, Phase, Status, ThreadId};
use divan::Bencher;
use editor_conversation::controller::Conversation;
use std::cell::RefCell;
use test_support::clock::ManualClock;
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

const ITEMS: usize = 200;
const THREADS: [&str; 2] = ["first", "second"];
const SWITCHES: usize = 100;
const SETTLED: std::time::Duration = std::time::Duration::from_secs(10);

fn main() {
    divan::main();
}

fn reply(thread: &str, position: u64) -> Item {
    Item {
        id: ItemId(format!("{thread}-{position}")),
        thread_id: ThreadId(thread.into()),
        position,
        seq: position,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::AssistantMessage {
            text: format!(
                "Reply {position} keeps the renderer steady while the sidebar updates. {}",
                "Every glyph stays in the atlas. ".repeat(32)
            )
            .into(),
            phase: Phase::Final,
        },
    }
}

struct Switching {
    clock: std::sync::Arc<ManualClock>,
    stubbed: StubbedHost,
    conversation: Conversation,
    next: usize,
}

impl Switching {
    fn loaded() -> Self {
        let clock = ManualClock::starting_at(0);
        let mut stubbed =
            StubbedHost::start_with_clock(empty_state(), axum::Router::new(), clock.clock());
        let mut conversation = Conversation::new(&stubbed.core);
        for thread in THREADS {
            let host = stubbed.host.clone();
            conversation.open_thread(&mut stubbed.core, host, ThreadId(thread.into()));
            stubbed
                .forge
                .take("GET", &format!("/threads/{thread}/transcript"))
                .respond(ItemsPage {
                    items: (1..=ITEMS as u64)
                        .map(|position| reply(thread, position))
                        .collect(),
                    runs: Vec::new(),
                    more: false,
                    work: Vec::new(),
                });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while conversation
                .view
                .as_ref()
                .map_or(0, |view| view.items.len())
                < ITEMS
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "bench: {thread} never loaded"
                );
                conversation.drain(&mut stubbed.core);
                std::thread::park_timeout(std::time::Duration::from_millis(1));
            }
            let heights: Vec<_> = conversation
                .rows
                .iter()
                .map(|(row, _)| (row.key(), 48.0))
                .collect();
            conversation.row_heights.extend(heights);
            clock.advance(SETTLED);
            conversation.expire_landing();
        }
        Self {
            clock,
            stubbed,
            conversation,
            next: 0,
        }
    }

    fn switch(&mut self) {
        let thread = THREADS[self.next % THREADS.len()];
        self.next += 1;
        let host = self.stubbed.host.clone();
        self.conversation
            .open_thread(&mut self.stubbed.core, host, ThreadId(thread.into()));
        assert!(
            self.conversation.rows.len() >= ITEMS && self.conversation.row_heights.len() >= ITEMS,
            "bench: {thread} comes back from the shelf with its rows and their heights"
        );
        self.clock.advance(SETTLED);
        self.conversation.expire_landing();
    }
}

#[divan::bench(sample_count = 20, sample_size = 1)]
fn switch_between_two_loaded_threads(bencher: Bencher<'_, '_>) {
    let switching = RefCell::new(Switching::loaded());
    bencher.bench_local(|| {
        let mut switching = switching.borrow_mut();
        for _ in 0..SWITCHES {
            switching.switch();
        }
    });
}
