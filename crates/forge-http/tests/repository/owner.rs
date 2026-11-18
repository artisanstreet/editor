use super::*;
async fn command(path: &Path, args: &[&str]) {
    let mut command = tokio::process::Command::new("git");
    command.arg("-C").arg(path).args(args);
    let output = forge_engines::process::probe(command, Duration::from_secs(3), 64 * 1024)
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[tokio::test]
async fn native_repository_handles_unborn_detached_and_linked_worktree() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path();
    command(path, &["init", "-b", "main"]).await;
    command(
        path,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:artisanstreet/editor.git",
        ],
    )
    .await;
    let unborn = read(path).await.unwrap();
    assert_eq!(unborn.branch.as_deref(), Some("main"));
    assert_eq!(
        unborn.web_url.as_deref(),
        Some("https://github.com/artisanstreet/editor")
    );
    command(
        path,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    )
    .await;
    let linked = folder.path().join("linked");
    command(
        path,
        &["worktree", "add", "-b", "side", linked.to_str().unwrap()],
    )
    .await;
    let worktree = read(&linked).await.unwrap();
    assert_eq!(worktree.branch.as_deref(), Some("side"));
    assert_eq!(worktree.worktree.as_deref(), Some("linked"));
    command(path, &["checkout", "--detach"]).await;
    assert_eq!(read(path).await.unwrap().branch, None);
}
#[tokio::test]
async fn concurrent_observations_share_one_cached_snapshot_and_cache_is_finite() {
    let folder = tempfile::tempdir().unwrap();
    let owner = Arc::new(Repositories::new().unwrap());
    command(folder.path(), &["init", "-b", "main"]).await;
    let mut jobs = Vec::new();
    for _ in 0..16 {
        let owner = Arc::clone(&owner);
        let path = folder.path().to_owned();
        jobs.push(tokio::spawn(
            async move { owner.read(&path).await.unwrap() },
        ));
    }
    for job in jobs {
        assert_eq!(job.await.unwrap().branch.as_deref(), Some("main"));
    }
    let entry = Arc::clone(&owner.entries.lock().await[folder.path()]);
    let observed = entry.lock().await.repository.as_ref().unwrap().0;
    owner.read(folder.path()).await;
    assert_eq!(entry.lock().await.repository.as_ref().unwrap().0, observed);
    drop(entry);
    for i in 0..ENTRIES + 8 {
        owner
            .read(&folder.path().join(format!("missing-{i}")))
            .await;
    }
    assert_eq!(owner.entries.lock().await.len(), ENTRIES);
}
#[cfg(unix)]
#[tokio::test]
async fn a_fifo_icon_is_rejected_without_waiting_for_a_writer() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("icon.png");
    let status = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        tokio::time::timeout(Duration::from_secs(1), icon_file(&path))
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn oversized_sparse_local_icon_is_rejected_before_reading() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("icon.png");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(forge_store::MAX_BLOB_BYTES as u64 + 1)
        .unwrap();
    assert!(icon_file(&path).await.is_none());
    std::fs::write(&path, b"small native file").unwrap();
    assert_eq!(icon_file(&path).await.unwrap(), b"small native file");
}
#[tokio::test]
async fn unknown_length_chunked_avatar_is_rejected_before_buffering_the_tail() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        let _ = socket.read(&mut request).await.unwrap();
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let chunk = vec![b'x'; 300 * 1024];
        for _ in 0..3 {
            if socket
                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .await
                .is_err()
            {
                break;
            }
            if socket.write_all(&chunk).await.is_err() {
                break;
            }
            if socket.write_all(b"\r\n").await.is_err() {
                break;
            }
        }
        socket.write_all(b"0\r\n\r\n").await.ok();
    });
    let response = reqwest::Client::new()
        .get(format!("http://{address}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.content_length(), None);
    assert!(bounded_response(response, AVATAR_BYTES).await.is_none());
    server.await.unwrap();
}
