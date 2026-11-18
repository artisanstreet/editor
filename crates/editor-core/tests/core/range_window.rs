use artisan_api::{ContentId, ContentRange, ContentRef};
use editor_core::runtime::runtime;
use editor_core::{ContentWindow, ForgeClient, RangeState};
use std::time::Duration;
use test_support::wait::wait_until;

fn page(start: u64, end: u64) -> ContentRange {
    ContentRange {
        id: ContentId("source".into()),
        start,
        end,
        total: 1_000,
        text: "x".repeat(usize::try_from(end - start).expect("a small page")),
    }
}

#[test]
fn asking_for_the_end_of_a_source_does_not_show_an_earlier_page() {
    let mut window = ContentWindow::new(
        ForgeClient::plain_for_tests("http://127.0.0.1:9"),
        ContentRef {
            id: ContentId("source".into()),
            bytes: 1_000,
            lines: 10,
        },
    );
    let first = window.completion_for_tests();
    assert!(runtime().block_on(first.complete(Ok(page(0, 100)))));
    wait_until("the first page arrives", Duration::from_secs(10), || {
        window.drain()
    });
    window.request(1_000);
    assert!(
        matches!(window.state(), RangeState::Loading),
        "the end of the source is fetched, not answered with bytes 0..100"
    );
}
