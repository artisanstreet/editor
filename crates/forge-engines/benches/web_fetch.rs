use forge_engines::claudeai::html_to_text;

const QUERY_PAIRS: usize = 100_000;

fn main() {
    divan::main();
}

fn ampersand_heavy() -> String {
    let query = (0..QUERY_PAIRS)
        .map(|pair| format!("k{pair}=v{pair}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("<html><body><p>{query}</p><p>Tom &amp; Jerry</p></body></html>")
}

#[divan::bench(sample_count = 20)]
fn page_full_of_bare_ampersands(bencher: divan::Bencher<'_, '_>) {
    let page = ampersand_heavy();
    bencher.bench_local(|| html_to_text(&page));
}
