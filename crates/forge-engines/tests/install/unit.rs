use super::*;
use crate::Engines;
use std::collections::HashMap;
use tokio::io::AsyncReadExt as _;

async fn serve(files: HashMap<String, Vec<u8>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let Ok(read) = socket.read(&mut buffer).await else {
                    break;
                };
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let request = String::from_utf8_lossy(&request);
            let path = request.split_whitespace().nth(1).unwrap_or_default();
            let response = files.get(path).map_or_else(
                || b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n".to_vec(),
                |body| {
                    [
                        format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len())
                            .into_bytes(),
                        body.clone(),
                    ]
                    .concat()
                },
            );
            socket.write_all(&response).await.ok();
        }
    });
    base
}

fn script(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\n# {version}\nexit 1\n").into_bytes()
}

fn tar_gz(entry: &str, version: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, body) in [
        ("package/package.json".to_owned(), b"{}".to_vec()),
        (format!("package/{entry}"), script(version)),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, path, body.as_slice())
            .unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn sha512(bytes: &[u8]) -> Expected {
    Expected::Sha512(sha2::Sha512::digest(bytes).to_vec())
}

fn versions_on_disk(data: &Path, engine: EngineId) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(Layout::new(data).versions(engine))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn an_independent_manager_cannot_prune_a_leased_binary() {
    let data = tempfile::tempdir().unwrap();
    let layout = Layout::new(data.path());
    let engine = EngineId::Codex;
    for version in ["0.150.0", "0.151.0", "0.152.0"] {
        let binary = layout
            .version_dir(engine, version)
            .join(host_spec(engine).artifact.unwrap().entry);
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(binary, script(version)).unwrap();
    }
    switch(&layout, engine, "0.150.0").unwrap();
    let forge = Engines::new(data.path());
    let lease = forge.lease(engine).unwrap();
    switch(&layout, engine, "0.151.0").unwrap();
    switch(&layout, engine, "0.152.0").unwrap();
    let cli_layout = Layout::new(data.path());
    prune(&cli_layout, engine);
    assert!(
        lease.path().is_file(),
        "another owner pruned the active binary"
    );
    drop(lease);
    prune(&cli_layout, engine);
    assert_eq!(
        versions_on_disk(data.path(), engine),
        ["0.151.0", "0.152.0"]
    );
}

#[tokio::test]
async fn updates_swap_current_and_keep_one_previous() {
    let data = tempfile::tempdir().unwrap();
    let engine = EngineId::Codex;
    let artifact = host_spec(engine).artifact.unwrap();
    let archives: Vec<(String, Vec<u8>)> = ["0.150.0", "0.151.0", "0.152.0"]
        .iter()
        .map(|v| ((*v).to_owned(), tar_gz(artifact.entry, v)))
        .collect();
    let base = serve(
        archives
            .iter()
            .map(|(v, bytes)| (format!("/{v}.tgz"), bytes.clone()))
            .collect(),
    )
    .await;
    let engines = Engines::new(data.path());
    assert_eq!(engines.status(engine).install, InstallState::Absent);
    let mut statuses = engines.subscribe();

    for (version, bytes) in &archives {
        let resolved = Resolved {
            version: version.clone(),
            url: format!("{base}/{version}.tgz"),
            digest: sha512(bytes),
        };
        engines
            .install_resolved(engine, artifact, resolved)
            .await
            .unwrap();
    }

    let mut seen = Vec::new();
    while let Ok(status) = statuses.try_recv() {
        let install = match status {
            crate::EngineChange::Full(status) => status.install,
            crate::EngineChange::Install { install, .. } => install,
        };
        seen.push(std::mem::discriminant(&install));
    }
    for expected in [
        InstallState::Downloading {
            version: String::new(),
            received: 0,
            total: None,
        },
        InstallState::Verifying {
            version: String::new(),
        },
        InstallState::Installed {
            version: String::new(),
            update: None,
        },
    ] {
        assert!(
            seen.contains(&std::mem::discriminant(&expected)),
            "never reported {expected:?}"
        );
    }
    assert_eq!(
        engines.status(engine).install,
        InstallState::Installed {
            version: "0.152.0".into(),
            update: None
        }
    );
    let binary = engines.binary(engine).unwrap();
    assert!(fs::read_to_string(&binary).unwrap().contains("0.152.0"));
    #[cfg(unix)]
    assert_ne!(
        std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&binary).unwrap().permissions())
            & 0o111,
        0
    );
    assert_eq!(
        versions_on_disk(data.path(), engine),
        ["0.151.0", "0.152.0"]
    );

    assert_eq!(engines.rollback(engine).await.unwrap(), "0.151.0");
    assert!(
        fs::read_to_string(engines.binary(engine).unwrap())
            .unwrap()
            .contains("0.151.0")
    );
    assert_eq!(
        Layout::new(data.path()).previous(engine).as_deref(),
        Some("0.152.0")
    );
}

#[tokio::test]
async fn a_download_that_fails_verification_leaves_the_old_version_current() {
    let data = tempfile::tempdir().unwrap();
    let engine = EngineId::Codex;
    let artifact = host_spec(engine).artifact.unwrap();
    let (good, bad) = (
        tar_gz(artifact.entry, "0.150.0"),
        tar_gz(artifact.entry, "0.151.0"),
    );
    let base = serve(HashMap::from([
        ("/good.tgz".to_owned(), good.clone()),
        ("/bad.tgz".to_owned(), bad),
    ]))
    .await;
    let engines = Engines::new(data.path());
    let install = |version: &str, path: &str, digest| Resolved {
        version: version.to_owned(),
        url: format!("{base}{path}"),
        digest,
    };
    engines
        .install_resolved(
            engine,
            artifact,
            install("0.150.0", "/good.tgz", sha512(&good)),
        )
        .await
        .unwrap();

    let error = engines
        .install_resolved(
            engine,
            artifact,
            install("0.151.0", "/bad.tgz", sha512(&good)),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            crate::InstallError::Fetch(FetchError::Verify {
                source: VerifyError::Sha512,
                ..
            })
        ),
        "{error}"
    );

    let InstallState::Failed { message } = engines.status(engine).install else {
        panic!("not failed")
    };
    assert!(message.contains("still using 0.150.0"), "{message}");
    assert!(
        fs::read_to_string(engines.binary(engine).unwrap())
            .unwrap()
            .contains("0.150.0")
    );
    assert_eq!(versions_on_disk(data.path(), engine), ["0.150.0"]);
}

#[test]
fn published_versions_are_the_followed_channel_newest_first() {
    let mut codex = serde_json::Map::new();
    for minor in 100..160 {
        codex.insert(format!("0.{minor}.0"), serde_json::json!({}));
        codex.insert(format!("0.{minor}.0-linux-x64"), serde_json::json!({}));
    }
    codex.insert("0.160.0-alpha.2".into(), serde_json::json!({}));
    let versions = channel_versions(&serde_json::json!({ "versions": codex }), "latest");
    assert_eq!(versions.len(), MAX_LISTED_VERSIONS);
    assert_eq!(versions[0], "0.159.0");
    assert_eq!(versions[1], "0.158.0");
    assert!(versions.iter().all(|version| !version.contains('-')));

    let opencode = serde_json::json!({ "versions": {
        "0.0.0-beta-9999": {}, "0.0.0-beta-19271": {}, "1.18.18": {},
        "0.0.0-beta-19271-linux-x64": {}, "0.0.0-dev-1": {},
    } });
    assert_eq!(
        channel_versions(&opencode, "beta"),
        ["0.0.0-beta-19271", "0.0.0-beta-9999"]
    );
}

#[test]
fn a_release_feed_lists_its_tagged_builds() {
    let releases = serde_json::json!([
        { "tag_name": "artisan-v2.1.2", "draft": false },
        { "tag_name": "artisan-v2.1.10", "draft": false },
        { "tag_name": "artisan-v2.2.0", "draft": true },
        { "tag_name": "v1.19.3", "draft": false },
        { "tag_name": "artisan-v../2.1.1", "draft": false },
    ]);
    assert_eq!(tagged_versions(&releases), ["2.1.10", "2.1.2"]);
}
