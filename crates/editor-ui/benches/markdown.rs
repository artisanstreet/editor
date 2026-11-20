use divan::Bencher;
use editor_ui::markdown::presenter::MarkdownPresenter;
use editor_ui::markdown::{MarkdownEngine, ShapedSource};
use std::fmt::Write as _;

const FENCE_LINES: usize = 2_000;
const SMALL_FENCES: usize = 200;
const STREAMED_BYTES: usize = 200 * 1024;
const DELTA_BYTES: usize = 50;
const SHAPED_BYTES: usize = 64 * 1024;

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

fn list_reply() -> String {
    let mut source = String::from("Here is every step:\n\n");
    let mut step = 0;
    while source.len() < SHAPED_BYTES {
        writeln!(
            source,
            "{}. Step {step} keeps **row {step}** steady while `paint_{step}` runs\n   - it reads the cache\n   - it writes the atlas",
            step + 1
        )
        .expect("bench: write to a String");
        step += 1;
    }
    source.truncate(SHAPED_BYTES);
    source
}

fn index_signature_reply() -> String {
    let mut source = String::from("```ts\ntype Rows = { [key: string]: number };\n```\n\n");
    let mut section = 0;
    while source.len() < SHAPED_BYTES {
        writeln!(
            source,
            "Section {section} keeps the renderer steady while `paint_{section}` runs.\n"
        )
        .expect("bench: write to a String");
        section += 1;
    }
    source.truncate(SHAPED_BYTES);
    source
}

fn prepare_streamed(reply: &str) {
    let engine = MarkdownEngine::new().expect("bench: engine");
    let mut presenter = MarkdownPresenter::default();
    for end in (DELTA_BYTES..=reply.len()).step_by(DELTA_BYTES) {
        presenter
            .prepare(&engine, reply.get(..end).expect("bench: ASCII reply"))
            .expect("bench: prepare");
    }
}

fn shape_streamed(reply: &str) -> Option<ShapedSource> {
    let mut shaped: Option<ShapedSource> = None;
    let mut start = 0;
    for end in (DELTA_BYTES..=reply.len()).step_by(DELTA_BYTES) {
        let delta = reply.get(start..end).expect("bench: ASCII reply");
        shaped = Some(
            match shaped.take() {
                Some(previous) => previous.append(delta),
                None => ShapedSource::new(delta, None),
            }
            .expect("bench: shape"),
        );
        start = end;
    }
    shaped
}

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 1, sample_size = 1)]
fn prepare_a_64_kib_list_streamed_in_50_byte_deltas(bencher: Bencher<'_, '_>) {
    let reply = list_reply();
    bencher.bench_local(|| prepare_streamed(&reply));
}

#[divan::bench(sample_count = 1, sample_size = 1)]
fn shape_a_64_kib_list_streamed_in_50_byte_deltas(bencher: Bencher<'_, '_>) {
    let reply = list_reply();
    bencher.bench_local(|| shape_streamed(&reply));
}

#[divan::bench(sample_count = 1, sample_size = 1)]
fn prepare_a_64_kib_reply_after_an_index_signature_streamed_in_50_byte_deltas(
    bencher: Bencher<'_, '_>,
) {
    let reply = index_signature_reply();
    bencher.bench_local(|| prepare_streamed(&reply));
}

#[divan::bench(sample_count = 1, sample_size = 1)]
fn shape_a_64_kib_reply_after_an_index_signature_streamed_in_50_byte_deltas(
    bencher: Bencher<'_, '_>,
) {
    let reply = index_signature_reply();
    bencher.bench_local(|| shape_streamed(&reply));
}

#[divan::bench(sample_count = 1, sample_size = 1)]
fn prepare_a_200_kib_reply_streamed_in_50_byte_deltas(bencher: Bencher<'_, '_>) {
    let reply = streamed_reply();
    bencher.bench_local(|| prepare_streamed(&reply));
}

#[divan::bench(sample_count = 1, sample_size = 1)]
fn shape_a_200_kib_reply_streamed_in_50_byte_deltas(bencher: Bencher<'_, '_>) {
    let reply = streamed_reply();
    bencher.bench_local(|| shape_streamed(&reply));
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
