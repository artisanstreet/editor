use super::*;
use std::io::{BufRead as _, Write as _};

#[cfg(target_os = "linux")]
#[test]
fn installing_a_remote_feed_keeps_it_for_later_updates() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join("release");
    let key = SigningKey::from_bytes(&[7; 32]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let feed = format!(
        "http://{}/nightly/release.json",
        listener.local_addr().unwrap()
    );
    let serving = directory.clone();
    let server = std::thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let mut stream = stream.unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header == "\r\n" || header.is_empty() {
                    break;
                }
            }
            let mut parts = request.split_whitespace();
            let method = parts.next().unwrap();
            let path = parts.next().unwrap().strip_prefix("/nightly/").unwrap();
            assert!(!path.contains('/'), "only release files are served");
            let body = std::fs::read(serving.join(path)).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            if method != "HEAD" {
                stream.write_all(&body).unwrap();
            }
        }
    });
    let initial = "0.0.0-nightly.20261110031700+aaaa";
    publish(&directory, &key, initial);
    let mut command = ae();
    isolated(&mut command, home.path());
    let output = command
        .args([
            "install", "--forge", "none", "--editor", "none", "--yes", "--from", &feed,
        ])
        .env(
            "ARTISAN_RELEASE_PUBLIC_KEY_HEX",
            hex(&key.verifying_key().to_bytes()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let app = home.path().join("data/artisan-street/ae");
    assert_eq!(
        std::fs::read_to_string(app.join("release-feed"))
            .unwrap()
            .trim(),
        feed
    );
    let updated = "0.0.0-nightly.20261111031700+bbbb";
    publish(&directory, &key, updated);
    let mut command = ae();
    isolated(&mut command, home.path());
    let output = command
        .args(["update", "ae", "--yes"])
        .env(
            "ARTISAN_RELEASE_PUBLIC_KEY_HEX",
            hex(&key.verifying_key().to_bytes()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(app.join("version")).unwrap().trim(),
        updated
    );
    server.join().unwrap();
}
