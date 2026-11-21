use artisan_api::tls::Identity;
use artisan_api::{ContentId, ContentRange, ContentRef};
use editor_core::runtime::runtime;
use editor_core::{ContentWindow, ForgeClient, RangeState};
use std::time::Duration;
use test_support::editor::forge::{StubForge, empty_state};
use test_support::wait::wait_until;

fn source() -> ContentId {
    ContentId("ab".repeat(32))
}

fn page(start: u64, end: u64) -> ContentRange {
    ContentRange {
        id: source(),
        start,
        end,
        total: 1_000,
        text: "x".repeat(usize::try_from(end - start).expect("a small page")),
    }
}

#[test]
fn asking_for_the_end_of_a_source_does_not_show_an_earlier_page() {
    let forge = runtime().block_on(StubForge::serve(empty_state(), axum::Router::new()));
    let identity = Identity::generate("range-window-test").expect("identity");
    let client = ForgeClient::new(forge.address(), forge.fingerprint(), &identity).expect("client");
    let mut window = ContentWindow::new(
        client,
        ContentRef {
            id: source(),
            bytes: 1_000,
            lines: 10,
        },
    );
    forge
        .take("GET", &format!("/content/{}", source()))
        .respond(page(0, 100));
    wait_until("the first page arrives", Duration::from_secs(10), || {
        window.drain()
    });
    window.request(1_000);
    assert!(
        matches!(window.state(), RangeState::Loading),
        "the end of the source is fetched, not answered with bytes 0..100"
    );
}
