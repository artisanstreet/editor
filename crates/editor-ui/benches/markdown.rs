use divan::Bencher;
use editor_ui::markdown::MarkdownEngine;
use editor_ui::markdown::presenter::MarkdownPresenter;
use std::fmt::Write as _;

const FENCE_LINES: usize = 2_000;
const SMALL_FENCES: usize = 200;

fn long_fence() -> String {
    let mut source = String::from("Here is the renderer:\n\n```rust\n");
    for line in 0..FENCE_LINES {
        writeln!(
            source,
            "    let value_{line} = renderer.paint(\"row {line}\", 0x{line:06x}, Some(&scene)); // #ff8800"
        )
        .expect("bench: write to a String");
    }
    source.push_str("```\n\nAnd it keeps streaming");
    source
}

fn many_fences() -> String {
    let mut source = String::new();
    for fence in 0..SMALL_FENCES {
        writeln!(
            source,
            "Step {fence}:\n\n```sh\ncargo test -p crate-{fence}\n```\n"
        )
        .expect("bench: write to a String");
    }
    source.push_str("Streaming tail");
    source
}

fn main() {
    divan::main();
}

fn prepare_growing(bencher: Bencher<'_, '_>, base: &str) {
    let engine = MarkdownEngine::new().expect("bench: engine");
    let mut presenter = MarkdownPresenter::default();
    let mut source = base.to_owned();
    presenter.prepare(&engine, &source).expect("bench: prepare");
    bencher.bench_local(|| {
        source.push_str(" more");
        presenter.prepare(&engine, &source).expect("bench: prepare")
    });
}

#[divan::bench(sample_count = 10)]
fn prepare_after_a_long_closed_fence(bencher: Bencher<'_, '_>) {
    prepare_growing(bencher, &long_fence());
}

#[divan::bench(sample_count = 10)]
fn prepare_after_many_fences(bencher: Bencher<'_, '_>) {
    prepare_growing(bencher, &many_fences());
}
