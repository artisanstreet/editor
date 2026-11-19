use editor_core::{media_http, runtime::runtime};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use test_support::wait::within;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn serve(listener: tokio::net::TcpListener, accepted: Arc<AtomicUsize>) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        accepted.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(async move {
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let Ok(read) = socket.read(&mut buffer).await else {
                    return;
                };
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..read]);
                while let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    request.drain(..end + 4);
                    let reply = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok";
                    if socket.write_all(reply).await.is_err() {
                        return;
                    }
                }
            }
        });
    }
}

#[test]
fn consecutive_media_loads_share_one_connection() {
    let accepted = Arc::new(AtomicUsize::new(0));
    let url = runtime().block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a local media server");
        let url = format!(
            "http://{}/image.png",
            listener.local_addr().expect("address")
        );
        runtime().spawn(serve(listener, Arc::clone(&accepted)));
        url
    });
    let http = media_http::client_trusting_the_first_hop_for_tests();
    for _ in 0..3 {
        let response = runtime().block_on(within(
            "the media response",
            Duration::from_secs(10),
            http.get(&url, false),
        ));
        assert_eq!(response.expect("the media loads").body, b"ok");
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[test]
fn a_reply_image_never_reaches_this_machine_or_its_network() {
    let accepted = Arc::new(AtomicUsize::new(0));
    let port = runtime().block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a local service");
        let port = listener.local_addr().expect("address").port();
        runtime().spawn(serve(listener, Arc::clone(&accepted)));
        port
    });
    for url in [
        format!("http://127.0.0.1:{port}/admin.png"),
        format!("http://localhost:{port}/admin.png"),
        format!("http://[::ffff:127.0.0.1]:{port}/admin.png"),
        "http://192.168.1.1/reboot.png".to_owned(),
        "http://10.0.0.1/status.png".to_owned(),
    ] {
        let result = runtime().block_on(within(
            "the refusal",
            Duration::from_secs(10),
            media_http::client().get(&url, true),
        ));
        assert!(
            matches!(result, Err(media_http::MediaFetchError::NotPublic { .. })),
            "{url} is refused before any request"
        );
    }
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        0,
        "a local service received a request from a reply image"
    );
}
