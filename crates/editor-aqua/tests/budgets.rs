#![cfg(all(feature = "qualification", target_os = "linux"))]
#![expect(
    unsafe_code,
    clippy::undocumented_unsafe_blocks,
    reason = "the budget binary counts heap bytes through its own global allocator"
)]

#[path = "budgets/allocator.rs"]
mod allocator;
#[path = "budgets/editor.rs"]
mod editor;
#[path = "budgets/ledger.rs"]
mod ledger;

use allocator::{Counting, Heap};
use aqua_core::Point;
use artisan_api::{
    ContentRef, Event, INLINE_TEXT_BYTES, ItemId, ItemKind, LiveCoverage, LiveTarget, Phase,
    Status, TextContent, ThreadId,
};
use editor::{Editor, seed};
use editor_aqua::core::runtime::runtime;
use editor_aqua::qualification::fixture::Fixture;
use editor_aqua::qualification::observation::Observation;
use ledger::Ledger;
use std::fmt::Write as _;

#[global_allocator]
static HEAP: Counting = Counting;

const TURNS: usize = 200;
const STREAM_CHUNKS: usize = 24;
const PERSIST_EVERY: usize = 6;
const IDLE_FRAMES: usize = 240;
const SCROLL_NOTCHES: usize = 20;
const WIDTH: f32 = 1200.0;
const HEIGHT: f32 = 800.0;

fn reply(turn: usize) -> String {
    let mut text = format!(
        "## Step {turn}\n\nThe renderer keeps **{turn}** glyph runs cached and evicts the oldest when the atlas fills.\n\n- shaping stays on the UI thread\n- rasterizing moves to the worker\n- uploads batch per frame\n"
    );
    if turn.is_multiple_of(3) {
        write!(
            text,
            "\n```rust\nfn evict(cache: &mut Cache, turn: usize) {{\n    cache.retain(|run| run.turn + {turn} > turn);\n}}\n```\n"
        )
        .expect("formatting into a String cannot fail");
    }
    text
}

fn long_thread() -> Fixture {
    let items: Vec<_> = (0..TURNS)
        .flat_map(|turn| {
            [
                serde_json::json!({
                    "id": format!("ask-{turn:04}"),
                    "thread_id": "long",
                    "run_id": null,
                    "parent_id": null,
                    "native_id": null,
                    "status": "done",
                    "kind": {"type": "user_message", "text": format!("Explain step {turn} of the glyph cache.")},
                }),
                serde_json::json!({
                    "id": format!("reply-{turn:04}"),
                    "thread_id": "long",
                    "run_id": null,
                    "parent_id": null,
                    "native_id": null,
                    "status": "done",
                    "kind": {"type": "assistant_message", "text": reply(turn), "phase": "final"},
                }),
            ]
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "projects": [{"id": "project", "path": "/budgets/project", "name": "project"}],
        "threads": [{
            "id": "long",
            "project_id": "project",
            "config": {"engine": "claude", "model": null, "effort": null},
            "title": "A long thread",
        }],
        "runs": [],
        "items": items,
    }))
    .expect("the long-thread fixture decodes")
}

fn landed(observed: &Observation) -> bool {
    observed.page_loaded && !observed.page_in_flight && !observed.landing && observed.row_count > 0
}

fn stored(text: &str) -> impl Fn(&Editor, &Observation) -> bool + '_ {
    let stored = ContentRef::from_text(text);
    move |_, observed| {
        landed(observed)
            && observed.items.iter().any(|item| {
                item.id.as_str() == "streamed"
                    && matches!(&item.kind, ItemKind::AssistantMessage { text: TextContent::Stored(shown), .. } if shown.id == stored.id)
            })
    }
}

fn streamed(bytes: usize) -> impl Fn(&Editor, &Observation) -> bool {
    move |editor, observed| {
        landed(observed)
            && editor.live_bytes(&ThreadId::from("long"), &streamed_target()) == Some(bytes)
    }
}

fn streamed_target() -> LiveTarget {
    LiveTarget::Text {
        item_id: ItemId::from("streamed"),
    }
}

fn preamble() -> String {
    let mut text = String::new();
    let mut rule = 0;
    while text.len() + STREAM_CHUNKS * 80 / 2 < INLINE_TEXT_BYTES {
        write!(
            text,
            "Rule {rule} of the glyph cache keeps one atlas page per font size and evicts by age.\n\n"
        )
        .expect("formatting into a String cannot fail");
        rule += 1;
    }
    text
}

fn upsert(editor: &Editor, text: &str, status: Status) {
    runtime()
        .block_on(editor.store().upsert_item(forge_store::NewItem {
            id: ItemId::from("streamed"),
            thread_id: ThreadId::from("long"),
            run_id: None,
            parent_id: None,
            native_id: None,
            status,
            kind: ItemKind::AssistantMessage {
                text: text.to_owned().into(),
                phase: Phase::Final,
            },
        }))
        .expect("the streamed reply is stored");
}

fn live(editor: &Editor, delta: &str, append: bool, total: usize) {
    editor.store().publish_live(Event::Live {
        thread_id: ThreadId::from("long"),
        target: streamed_target(),
        text: delta.to_owned(),
        append,
        coverage: LiveCoverage {
            start: 0,
            total: total as u64,
        },
    });
}

fn stream(editor: &mut Editor, ledger: &mut Ledger) {
    let mut text = preamble();
    upsert(editor, "", Status::Running);
    live(editor, &text, false, text.len());
    editor.settle("the start of a long reply", streamed(text.len()));
    let mut span = editor::Span::default();
    let mut persisted = 0;
    for chunk in 0..STREAM_CHUNKS {
        let delta =
            format!("Paragraph {chunk} of the streamed answer explains one more cache rule.\n\n");
        text.push_str(&delta);
        live(editor, &delta, true, text.len());
        if chunk % PERSIST_EVERY == PERSIST_EVERY - 1 {
            upsert(editor, &text, Status::Running);
            persisted += u64::from(text.len() > INLINE_TEXT_BYTES);
        }
        span.extend(editor.settle("a streamed chunk", streamed(text.len())));
    }
    assert!(
        persisted >= 2,
        "the reply is persisted as stored content while it streams, measured {persisted} times"
    );
    ledger.measure("stream_window_rebuilds", span.rebuilds(), &span.causes());
    ledger.measure(
        "stream_section_refreshes_per_frame",
        span.most_refreshes(),
        &span.causes(),
    );
    ledger.measure(
        "stream_access_nodes_published_per_frame",
        span.most_published(),
        "",
    );
    upsert(editor, &text, Status::Done);
    let finished = editor.settle("the finished reply", stored(&text));
    ledger.measure(
        "finish_window_rebuilds",
        finished.rebuilds(),
        &finished.causes(),
    );
    ledger.measure(
        "finish_section_refreshes_per_frame",
        finished.most_refreshes(),
        &finished.causes(),
    );
    ledger.measure(
        "finish_access_nodes_published_per_frame",
        finished.most_published(),
        "",
    );
}

fn scroll(editor: &mut Editor, ledger: &mut Ledger) {
    let before = editor.observation().expect("the thread is observable");
    let span = editor.scroll(Point::new(WIDTH * 0.6, HEIGHT * 0.4), SCROLL_NOTCHES, -3.0);
    let after = editor.observation().expect("the thread is observable");
    assert_ne!(
        (before.visible_row_start, before.scroll_y_bits),
        (after.visible_row_start, after.scroll_y_bits),
        "the wheel moved the transcript"
    );
    ledger.measure("scroll_window_rebuilds", span.rebuilds(), &span.causes());
    ledger.measure(
        "scroll_section_refreshes_per_frame",
        span.most_refreshes(),
        &span.causes(),
    );
    ledger.measure(
        "scroll_access_nodes_examined_per_frame",
        span.most_examined(),
        "",
    );
    ledger.measure(
        "scroll_access_nodes_published_per_frame",
        span.most_published(),
        "",
    );
}

#[test]
fn a_long_thread_stays_within_its_performance_budgets() {
    let mut ledger = Ledger::load(include_str!("budgets/editor.json"));
    let seeded = seed(&long_thread());
    let heap = Heap::now();
    let mut editor = Editor::open(
        seeded,
        &ThreadId::from("long"),
        f64::from(WIDTH),
        f64::from(HEIGHT),
    );
    let opened = editor.settle("opening the thread", |_, observed| landed(observed));
    ledger.measure("open_window_rebuilds", opened.rebuilds(), &opened.causes());
    ledger.measure(
        "open_section_refreshes",
        opened.refreshes(),
        &opened.causes(),
    );
    ledger.measure(
        "open_retained_kib",
        Heap::now().retained_since(heap) / 1024,
        "",
    );
    let idle = editor.idle(IDLE_FRAMES);
    ledger.measure(
        "idle_window_rebuilds",
        idle.rebuilds() + idle.refreshes(),
        &idle.causes(),
    );
    ledger.measure(
        "idle_frames_requested",
        idle.requested() + idle.animating(),
        "",
    );
    ledger.measure("idle_access_nodes_published", idle.published(), "");
    ledger.measure("rest_access_nodes", idle.most_examined(), "");
    stream(&mut editor, &mut ledger);
    scroll(&mut editor, &mut ledger);
    ledger.enforce();
}
