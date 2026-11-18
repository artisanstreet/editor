use divan::Bencher;
use editor_ui::markdown::presenter::MarkdownPresenter;
use editor_ui::markdown::{MarkdownEngine, ShapedSource};
use std::fmt::Write as _;

const FENCE_LINES: usize = 2_000;
const SMALL_FENCES: usize = 200;
const STREAMED_BYTES: usize = 200 * 1024;
const DELTA_BYTES: usize = 50;

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

fn streamed_reply() -> String {
    let mut source = String::new();
    let mut section = 0;
    while source.len() < STREAMED_BYTES {
        writeln!(
            source,
            "## Step {section}\n\nThe renderer keeps **row {section}** steady while `paint_{section}` runs, so the reader never sees text move.\n\n- first point for {section}\n- second point with [a link](https://example.com/{section})\n- third point\n\n```rust\nfn paint_{section}(scene: &mut Scene) {{\n    scene.push(Quad::new({section}));\n}}\n```\n\n| Field | Value |\n| --- | --- |\n| row | {section} |\n\nThat closes step {section}.\n"
        )
        .expect("bench: write to a String");
        section += 1;
    }
    source.truncate(STREAMED_BYTES);
    source
}

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 1, sample_size = 1)]
fn prepare_a_200_kib_reply_streamed_in_50_byte_deltas(bencher: Bencher<'_, '_>) {
    let engine = MarkdownEngine::new().expect("bench: engine");
    let reply = streamed_reply();
    bencher.bench_local(|| {
        let mut presenter = MarkdownPresenter::default();
        for end in (DELTA_BYTES..=reply.len()).step_by(DELTA_BYTES) {
            presenter
                .prepare(&engine, reply.get(..end).expect("bench: ASCII reply"))
                .expect("bench: prepare");
        }
    });
}

#[divan::bench(sample_count = 1, sample_size = 1)]
fn shape_a_200_kib_reply_streamed_in_50_byte_deltas(bencher: Bencher<'_, '_>) {
    let reply = streamed_reply();
    bencher.bench_local(|| {
        let mut shaped: Option<ShapedSource> = None;
        for end in (DELTA_BYTES..=reply.len()).step_by(DELTA_BYTES) {
            let source = reply.get(..end).expect("bench: ASCII reply");
            shaped = Some(
                match shaped.take() {
                    Some(previous) => previous.extend(source, None),
                    None => ShapedSource::new(source, None),
                }
                .expect("bench: shape"),
            );
        }
        shaped
    });
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
