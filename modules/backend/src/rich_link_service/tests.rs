use std::sync::{
    Arc,
    atomic::{AtomicI64, AtomicUsize, Ordering},
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{html::ParsedRichLinkHtml, *};

struct ManualClock(AtomicI64);

impl ManualClock {
    fn new(now_ms: i64) -> Self {
        Self(AtomicI64::new(now_ms))
    }

    fn advance(&self, delta_ms: i64) {
        self.0.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl RichLinkClock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct ScriptedFetcher {
    calls: AtomicUsize,
    delay: Duration,
    page: Result<RichLinkPage, RichLinkError>,
}

impl ScriptedFetcher {
    fn new(page: Result<RichLinkPage, RichLinkError>, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            delay,
            page,
        })
    }

    fn page(html: &str) -> Arc<Self> {
        Self::new(
            Ok(RichLinkPage {
                final_url: "https://example.com/final".to_owned(),
                content_type: "text/html".to_owned(),
                body: html.as_bytes().to_vec(),
            }),
            Duration::ZERO,
        )
    }

    fn failure(error: RichLinkError) -> Arc<Self> {
        Self::new(Err(error), Duration::ZERO)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl RichLinkPageFetcher for ScriptedFetcher {
    fn fetch(&self, _url: Url) -> RichLinkFetchFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let page = self.page.clone();
        let delay = self.delay;
        Box::pin(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            page
        })
    }
}

fn resolver(fetcher: Arc<ScriptedFetcher>) -> RichLinkResolver {
    RichLinkResolver::new(fetcher, RichLinkResolverOptions::default())
        .expect("default options are valid")
}

#[test]
fn parser_prefers_open_graph_then_twitter_then_document_title() {
    let html = r#"<!doctype html>
        <html><head>
        <meta content="Twitter &amp; Co" name="twitter:title">
        <TITLE>  Document
            Title  </TITLE>
        <meta property="og:title" content="Open Graph Title">
        </head></html>"#;
    let parsed = parse_rich_link_html(html);
    assert_eq!(parsed.title.as_deref(), Some("Document Title"));
    assert_eq!(parsed.twitter.as_deref(), Some("Twitter & Co"));
    assert_eq!(parsed.open_graph.as_deref(), Some("Open Graph Title"));
    assert_eq!(parsed.page_name(), Some("Open Graph Title"));

    let title_only = parse_rich_link_html("<html><head><title>Only Title</title></head></html>");
    assert_eq!(title_only.page_name(), Some("Only Title"));

    let twitter_only = parse_rich_link_html(
        "<meta property=\"og:title\"><meta name=\"twitter:title\" content=\"Twitter First\">",
    );
    assert_eq!(twitter_only.page_name(), Some("Twitter First"));

    let name_og = parse_rich_link_html("<meta name=\"og:title\" content=\"Named OG\">");
    assert_eq!(name_og.page_name(), Some("Named OG"));

    let first_wins = parse_rich_link_html(
        "<meta property=\"og:title\" content=\"First\"><meta property=\"og:title\" content=\"Second\">",
    );
    assert_eq!(first_wins.page_name(), Some("First"));
}

#[test]
fn parser_ignores_degenerate_bodies_and_comments() {
    assert_eq!(parse_rich_link_html(""), ParsedRichLinkHtml::default());
    assert_eq!(
        parse_rich_link_html("just plain text, no markup"),
        ParsedRichLinkHtml::default()
    );
    assert_eq!(
        parse_rich_link_html("<html><body>no head tags</body></html>"),
        ParsedRichLinkHtml::default()
    );
    assert_eq!(
        parse_rich_link_html("<title>   </title>"),
        ParsedRichLinkHtml::default()
    );
    assert_eq!(
        parse_rich_link_html("<!-- <title>Commented</title> -->"),
        ParsedRichLinkHtml::default()
    );
    assert_eq!(
        parse_rich_link_html("<meta property=\"og:title\" content=\"\">"),
        ParsedRichLinkHtml::default()
    );
    assert_eq!(
        parse_rich_link_html("<meta property=\"og:title\" content=\"Unquoted Title\">").page_name(),
        Some("Unquoted Title")
    );
    assert_eq!(
        parse_rich_link_html("<title>Unclosed Title").page_name(),
        Some("Unclosed Title")
    );
}

#[test]
fn parser_bounds_and_entity_decodes_metadata_text() {
    let long_title = "x".repeat(RICH_LINK_PAGE_NAME_MAX_CHARS + 100);
    let parsed = parse_rich_link_html(&format!("<title>{long_title}</title>"));
    let page_name = parsed.page_name().expect("bounded title survives");
    assert_eq!(page_name.chars().count(), RICH_LINK_PAGE_NAME_MAX_CHARS);

    let decoded = parse_rich_link_html(
        "<title>&#72;&#x65;llo &amp; &quot;world&quot; &hellip; &unknown;</title>",
    );
    assert_eq!(decoded.page_name(), Some("Hello & \"world\" … &unknown;"));
}

#[tokio::test]
async fn resolve_uses_host_fallback_for_titleless_html() {
    let fetcher = ScriptedFetcher::page("<html><body>No title here</body></html>");
    let resolver = resolver(fetcher);
    let resolution = resolver
        .resolve("https://example.com/docs#section")
        .await
        .expect("host fallback resolves");
    assert_eq!(resolution.requested_url, "https://example.com/docs");
    assert_eq!(resolution.page_name, "example.com");
    assert!(resolution.expires_at_ms > 0);
}

#[tokio::test]
async fn resolve_returns_resolved_title_and_reuses_cache() {
    let fetcher = ScriptedFetcher::page("<title>Resolved Page</title>");
    let resolver = resolver(Arc::clone(&fetcher));
    let first = resolver
        .resolve("https://example.com/page")
        .await
        .expect("resolves");
    assert_eq!(first.page_name, "Resolved Page");
    let second = resolver
        .resolve("https://example.com/page")
        .await
        .expect("cached resolves");
    assert_eq!(second.expires_at_ms, first.expires_at_ms);
    assert_eq!(fetcher.calls(), 1);
}

#[tokio::test]
async fn resolve_rejects_non_html_and_oversized_bodies() {
    let json = ScriptedFetcher::new(
        Ok(RichLinkPage {
            final_url: "https://example.com/data".to_owned(),
            content_type: "application/json".to_owned(),
            body: b"{\"title\":\"Not HTML\"}".to_vec(),
        }),
        Duration::ZERO,
    );
    assert_eq!(
        resolver(json).resolve("https://example.com/data").await,
        Err(RichLinkError::UnsupportedContentType)
    );

    let oversized = ScriptedFetcher::page(&"x".repeat(128));
    let resolver = RichLinkResolver::new(
        oversized,
        RichLinkResolverOptions {
            cache_ttl: Duration::from_secs(60),
            cache_capacity: 4,
            max_html_bytes: 32,
            fetch_timeout: Duration::from_secs(1),
        },
    )
    .expect("valid bounded options");
    assert_eq!(
        resolver.resolve("https://example.com/large").await,
        Err(RichLinkError::ResponseTooLarge)
    );
}

#[tokio::test]
async fn cache_ttl_expires_and_refetches() {
    let fetcher = ScriptedFetcher::page("<title>TTL Page</title>");
    let clock = Arc::new(ManualClock::new(1_000));
    let resolver = RichLinkResolver::with_clock(
        Arc::clone(&fetcher) as Arc<dyn RichLinkPageFetcher>,
        RichLinkResolverOptions {
            cache_ttl: Duration::from_millis(1_000),
            cache_capacity: 4,
            max_html_bytes: RICH_LINK_DEFAULT_MAX_HTML_BYTES,
            fetch_timeout: Duration::from_secs(1),
        },
        Arc::clone(&clock) as Arc<dyn RichLinkClock>,
    )
    .expect("valid options");

    let first = resolver
        .resolve("https://example.com/ttl")
        .await
        .expect("first resolves");
    assert_eq!(first.expires_at_ms, 2_000);
    resolver
        .resolve("https://example.com/ttl")
        .await
        .expect("fresh cache hit");
    assert_eq!(fetcher.calls(), 1);

    clock.advance(1_000);
    let expired = resolver
        .resolve("https://example.com/ttl")
        .await
        .expect("expired entry refetches");
    assert_eq!(expired.expires_at_ms, 3_000);
    assert_eq!(fetcher.calls(), 2);
}

#[tokio::test]
async fn cache_is_bounded_and_refreshes_recency() {
    let fetcher = ScriptedFetcher::page("<title>Bounded</title>");
    let resolver = RichLinkResolver::new(
        Arc::clone(&fetcher) as Arc<dyn RichLinkPageFetcher>,
        RichLinkResolverOptions {
            cache_ttl: Duration::from_secs(60),
            cache_capacity: 2,
            max_html_bytes: RICH_LINK_DEFAULT_MAX_HTML_BYTES,
            fetch_timeout: Duration::from_secs(1),
        },
    )
    .expect("valid options");

    resolver.resolve("https://example.com/a").await.unwrap();
    resolver.resolve("https://example.com/b").await.unwrap();
    // Refreshing `a` makes `b` the least recently used entry.
    resolver.resolve("https://example.com/a").await.unwrap();
    // Inserting `c` at capacity 2 evicts the least recently used `b`.
    resolver.resolve("https://example.com/c").await.unwrap();
    assert_eq!(resolver.retained_entry_count().await, 2);
    assert_eq!(fetcher.calls(), 3);

    // Both retained entries still answer from cache.
    resolver.resolve("https://example.com/c").await.unwrap();
    resolver.resolve("https://example.com/a").await.unwrap();
    assert_eq!(fetcher.calls(), 3);

    // `b` was evicted and must fetch again; that insert evicts `c`.
    resolver.resolve("https://example.com/b").await.unwrap();
    assert_eq!(resolver.retained_entry_count().await, 2);
    assert_eq!(fetcher.calls(), 4);

    // `c` was the least recently used entry and must fetch again.
    resolver.resolve("https://example.com/c").await.unwrap();
    assert_eq!(fetcher.calls(), 5);
    assert_eq!(resolver.retained_entry_count().await, 2);
}

#[tokio::test]
async fn concurrent_resolves_share_one_fetch() {
    let fetcher = Arc::new(ScriptedFetcher {
        calls: AtomicUsize::new(0),
        delay: Duration::from_millis(50),
        page: Ok(RichLinkPage {
            final_url: "https://example.com/shared".to_owned(),
            content_type: "text/html".to_owned(),
            body: b"<title>Shared Flight</title>".to_vec(),
        }),
    });
    let resolver = resolver(Arc::clone(&fetcher));
    let (first, second) = tokio::join!(
        resolver.resolve("https://example.com/shared"),
        resolver.resolve("https://example.com/shared")
    );
    let first = first.expect("first resolves");
    let second = second.expect("follower resolves");
    assert_eq!(first, second);
    assert_eq!(first.page_name, "Shared Flight");
    assert_eq!(fetcher.calls(), 1);
}

#[tokio::test]
async fn failures_are_not_cached_and_timeout_is_bounded() {
    let failing = ScriptedFetcher::failure(RichLinkError::Transport);
    let resolver = resolver(Arc::clone(&failing));
    assert_eq!(
        resolver.resolve("https://example.com/failure").await,
        Err(RichLinkError::Transport)
    );
    assert_eq!(
        resolver.resolve("https://example.com/failure").await,
        Err(RichLinkError::Transport)
    );
    assert_eq!(failing.calls(), 2);
    assert_eq!(resolver.retained_entry_count().await, 0);

    let slow = Arc::new(ScriptedFetcher {
        calls: AtomicUsize::new(0),
        delay: Duration::from_secs(10),
        page: Ok(RichLinkPage {
            final_url: "https://example.com/slow".to_owned(),
            content_type: "text/html".to_owned(),
            body: b"<title>Slow</title>".to_vec(),
        }),
    });
    let resolver = RichLinkResolver::new(
        Arc::clone(&slow) as Arc<dyn RichLinkPageFetcher>,
        RichLinkResolverOptions {
            cache_ttl: Duration::from_secs(60),
            cache_capacity: 4,
            max_html_bytes: RICH_LINK_DEFAULT_MAX_HTML_BYTES,
            fetch_timeout: Duration::from_millis(50),
        },
    )
    .expect("valid options");
    let started = std::time::Instant::now();
    assert_eq!(
        resolver.resolve("https://example.com/slow").await,
        Err(RichLinkError::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn canonical_url_policy_rejects_non_http_and_non_public_targets() {
    assert_eq!(
        canonical_rich_link_url("https://example.com/docs?q=1#frag")
            .expect("public URL is accepted")
            .as_str(),
        "https://example.com/docs?q=1"
    );
    assert!(canonical_rich_link_url("http://8.8.8.8/").is_ok());
    assert_eq!(
        canonical_rich_link_url("ftp://example.com/"),
        Err(RichLinkError::InvalidUrl)
    );
    assert_eq!(
        canonical_rich_link_url("example.com/path"),
        Err(RichLinkError::InvalidUrl)
    );
    assert_eq!(
        canonical_rich_link_url("https://user:secret@example.com/"),
        Err(RichLinkError::InvalidUrl)
    );
    for blocked in [
        "http://localhost/",
        "http://docs.localhost/",
        "http://127.0.0.1/",
        "http://10.1.2.3/",
        "http://192.168.0.1/",
        "http://172.16.4.5/",
        "http://169.254.10.10/",
        "http://0.0.0.0/",
        "http://100.64.0.1/",
        "http://[::1]/",
        "http://[fd00::1]/",
        "http://[fe80::1]/",
        "http://[2001:db8::1]/",
        "http://[2001:2::1]/",
        "http://[3fff::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://[::ffff:169.254.169.254]/",
        "http://[::ffff:10.0.0.1]/",
        "http://[::ffff:7f00:1]/",
        "http://[::7f00:1]/",
        "http://[::a00:1]/",
        "http://[::ffff:0:7f00:1]/",
    ] {
        assert_eq!(
            canonical_rich_link_url(blocked),
            Err(RichLinkError::BlockedAddress),
            "{blocked} must be blocked"
        );
    }
    assert!(canonical_rich_link_url("http://[::ffff:808:808]/").is_ok());
    assert!(canonical_rich_link_url("http://[::808:808]/").is_ok());
}

/// Serves one scripted HTTP response on a loopback listener.
async fn serve_once(response: &'static str) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind scripted server");
    let address = listener.local_addr().expect("scripted server address");
    let server = tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).await;
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    });
    (address, server)
}

#[tokio::test]
async fn redirect_to_private_host_is_refused() {
    let (address, server) = serve_once(
        "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    let fetcher = HttpRichLinkFetcher::with_defaults();
    let target = Url::parse(&format!("http://{address}/redirect")).expect("probe url parses");
    assert_eq!(fetcher.fetch(target).await, Err(RichLinkError::HttpStatus));
    server.await.expect("scripted server completes");
}

#[tokio::test]
async fn fetcher_refuses_hostname_resolving_to_loopback() {
    let (address, server) = serve_once(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    let fetcher = HttpRichLinkFetcher::with_defaults();
    let target = Url::parse(&format!("http://localhost:{}/probe", address.port()))
        .expect("probe url parses");
    assert_eq!(fetcher.fetch(target).await, Err(RichLinkError::Transport));
    server.abort();
}

#[test]
fn favicon_metadata_uses_declared_icon_and_decodes_entities() {
    let page = parse_rich_link_html(
        r#"<link rel="stylesheet" href="bad"><link href="/icon.png?a=1&amp;b=2" rel="shortcut ICON"><title>Example</title>"#,
    );
    assert_eq!(page.icon.as_deref(), Some("/icon.png?a=1&b=2"));
    assert_eq!(page.page_name(), Some("Example"));
}

#[tokio::test]
async fn favicon_fetch_rejects_private_addresses() {
    let fetcher = HttpRichLinkFetcher::with_defaults();
    for address in [
        "http://127.0.0.1/favicon.ico",
        "http://[::1]/icon.png",
        "file:///tmp/icon.png",
    ] {
        assert!(
            fetcher
                .fetch_icon(Url::parse(address).unwrap())
                .await
                .is_empty()
        );
    }
}

#[tokio::test]
#[ignore = "manual public internet probe"]
async fn live_rich_link_probe() {
    let resolver = RichLinkResolver::with_defaults();
    for url in [
        "https://github.com/rust-lang/rust",
        "https://www.rust-lang.org/",
    ] {
        let value = resolver.resolve(url).await.expect("live metadata resolves");
        println!(
            "{} | {} | favicon={} bytes",
            value.requested_url,
            value.page_name,
            value.favicon.len()
        );
        assert!(!value.page_name.is_empty());
    }
}
