use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

async fn serve(response: impl Fn() -> String + Send + 'static) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the test server");
    let port = listener.local_addr().expect("test server address").port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            let mut request = [0_u8; 4096];
            let _read = stream.read(&mut request).await;
            let _written = stream.write_all(response().as_bytes()).await;
        }
    });
    (port, hits)
}

fn redirect_to(location: String) -> impl Fn() -> String + Send + 'static {
    move || {
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    }
}

#[tokio::test]
async fn a_remote_image_cannot_redirect_into_this_machine_or_its_network() {
    let (inner, inner_hits) =
        serve(|| "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc".to_owned())
            .await;
    for target in [
        format!("http://127.0.0.1:{inner}/secret"),
        format!("http://localhost:{inner}/secret"),
        format!("http://[::ffff:127.0.0.1]:{inner}/secret"),
    ] {
        let (outer, outer_hits) = serve(redirect_to(target.clone())).await;
        let source = MediaSource::from(format!("http://127.0.0.1:{outer}/pixel.png"));
        let result = load_image_bytes(&source, None, &editor_core::media_http::client()).await;
        assert!(
            matches!(
                result,
                Err(MediaLoadError::Fetch(MediaFailure::Unreachable))
            ),
            "following {target}"
        );
        assert_eq!(outer_hits.load(Ordering::SeqCst), 1);
    }
    assert_eq!(
        inner_hits.load(Ordering::SeqCst),
        0,
        "a redirect reached a local service"
    );
}
