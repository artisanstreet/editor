//! Links in a reply read as their page's title once Forge resolved it.

use super::{Forge, draw, editor, open, reply, wait};
use artisan_api::{ItemId, Status};
use editor_ui::markdown_renderer::RichLinkTitleSource as _;
use gpui::TestAppContext;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A web server with one titled page; counts how often it is fetched.
fn website(forge: &Forge) -> (SocketAddr, Arc<AtomicUsize>) {
    let fetched = Arc::new(AtomicUsize::new(0));
    let counter = fetched.clone();
    let site = axum::Router::new().route(
        "/post",
        axum::routing::get(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                axum::response::Html(
                    "<html><head><meta property=\"og:title\" content=\"Rust &amp; Friends\"></head></html>",
                )
            }
        }),
    );
    let listener = forge
        .runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let address = listener.local_addr().unwrap();
    forge
        .runtime
        .spawn(async move { axum::serve(listener, site).await });
    (address, fetched)
}

#[gpui::test]
fn a_bare_link_shows_its_page_title(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let (site, fetched) = website(&forge);
    let titled = format!("http://{site}/post");
    let missing = format!("http://{site}/missing");
    let thread = forge.thread();
    let text = format!("See {titled} and {missing} for more.");
    forge.put(&thread, &ItemId::new(), Status::Done, reply(&text));
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the reply", |cx| {
        cx.read(|cx| conversation.read(cx).items().count() == 1)
    });
    draw(cx);

    let title = |url: &str, cx: &mut gpui::VisualTestContext| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .shared
                .links
                .for_row(None)
                .resolved_title(url)
        })
    };
    wait(cx, "the title", |cx| title(&titled, cx).is_some());
    assert_eq!(title(&titled, cx).as_deref(), Some("Rust & Friends"));
    // A page without a title keeps the URL as the label.
    assert_eq!(title(&missing, cx), None);

    for _ in 0..3 {
        draw(cx);
        cx.run_until_parked();
    }
    assert_eq!(
        fetched.load(Ordering::SeqCst),
        1,
        "each link is asked for once"
    );
}
