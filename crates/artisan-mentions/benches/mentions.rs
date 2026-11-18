use artisan_mentions::ThreadTitles;
use divan::Bencher;

const TITLES: usize = 5_000;
const CANDIDATES: usize = 50;

fn titles() -> Vec<String> {
    (0..TITLES)
        .map(|index| format!("Fix the sidebar scroll position after thread {index} updates"))
        .collect()
}

fn draft() -> String {
    let mut draft = String::new();
    for index in 0..CANDIDATES {
        draft.push_str("As in @Fix the sidebar scroll position after thread ");
        draft.push_str(&(index * 97).to_string());
        draft.push_str(" updates, keep going @someone. ");
    }
    draft
}

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 50)]
fn tokenize_a_mention_heavy_draft(bencher: Bencher<'_, '_>) {
    let titles = ThreadTitles::new(&titles());
    let draft = draft();
    bencher.bench_local(|| titles.mentions(&draft));
}

#[divan::bench(sample_count = 20)]
fn compile_the_title_catalog(bencher: Bencher<'_, '_>) {
    let titles = titles();
    bencher.bench_local(|| ThreadTitles::new(&titles));
}
