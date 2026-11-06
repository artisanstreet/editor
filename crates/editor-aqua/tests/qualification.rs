#![cfg(feature = "qualification")]

#[path = "qualification/observation.rs"]
mod observation;

use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ItemKind, Phase, Status};
use editor_aqua::qualification::fixture::{
    CheckpointState, Fixture, FixtureItem, ForgeServer, ImmutableFixture, SeedSnapshot,
    StoreSnapshot, TreeManifest, VerifiedBootstrap,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn fixture() -> Fixture {
    Fixture {
        version: 1,
        projects: vec![CreateProject {
            id: "project-a".into(),
            path: "/fixture/project".into(),
            name: Some("Project".into()),
        }],
        threads: vec![CreateThread {
            id: "thread-a".into(),
            project_id: "project-a".into(),
            config: EngineConfig::new(EngineId::Claude),
            title: Some("Thread".into()),
            parent_thread_id: None,
            spawned_by: None,
        }],
        runs: Vec::new(),
        items: vec![FixtureItem {
            id: "item-a".into(),
            thread_id: "thread-a".into(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::AssistantMessage {
                text: "hello".into(),
                phase: Phase::Final,
            },
        }],
        settings: BTreeMap::new(),
    }
}

#[test]
fn preparation_accepts_normalized_fixture_paths_and_produces_verified_requests() {
    let directory = tempfile::tempdir().unwrap();
    let root = editor_aqua::qualification::fixture::canonical_path(directory.path()).unwrap();
    let paths: Vec<_> = ["forge", "snapshot", "output", "source", "candidate"]
        .into_iter()
        .map(|name| root.join(name))
        .collect();
    for path in &paths {
        std::fs::create_dir(path).unwrap();
        std::fs::write(
            path.join(".artisan-qualification"),
            b"artisan-qualification-v1\n",
        )
        .unwrap();
    }
    let fixture_path = root.join("fixture.json");
    std::fs::write(&fixture_path, serde_json::to_vec(&fixture()).unwrap()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let request = serde_json::json!({
        "fixture": fixture_path,
        "forge_dir": paths[0],
        "snapshot_dir": paths[1],
        "output_dir": paths[2],
        "source_editor_dir": paths[3],
        "candidate_editor_dir": paths[4],
        "listen": address,
        "host_id": "synthetic-host",
        "thread_id": "thread-a",
        "width": 800,
        "height": 600
    });
    let request_path = root.join("prepare.json");
    std::fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
    editor_aqua::qualification::preparation::prepare(&request_path).unwrap();
    let bootstrap = root.join("output/candidate-request.json");
    let verified = editor_aqua::core::runtime::runtime()
        .block_on(VerifiedBootstrap::load(&bootstrap))
        .unwrap();
    assert_eq!(verified.editor_dir, paths[4]);
    assert_eq!(verified.request.fixture, fixture_path);
    editor_aqua::core::runtime::runtime()
        .block_on(verified.server.shutdown_unique())
        .unwrap();
}

#[test]
fn real_forge_seed_snapshot_restores_identical_tree_at_pinned_path() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let forge = directory.path().join("forge");
    let snapshot_dir = directory.path().join("snapshot");
    for dir in [&forge, &snapshot_dir] {
        std::fs::create_dir(dir).unwrap();
        std::fs::write(
            dir.join(".artisan-qualification"),
            b"artisan-qualification-v1\n",
        )
        .unwrap();
    }
    runtime.block_on(async {
        let input = fixture();
        let server = ForgeServer::start(forge.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let snapshot = SeedSnapshot::produce(server, &input, &forge, &snapshot_dir)
            .await
            .unwrap();
        assert_eq!(
            TreeManifest::capture_stable(&snapshot_dir).unwrap(),
            snapshot.tree
        );
        std::fs::write(forge.join("extra"), b"changed").unwrap();
        snapshot
            .restore_after_all_forge_handles_closed(&snapshot_dir, &forge)
            .unwrap();
        assert_eq!(TreeManifest::capture_stable(&forge).unwrap(), snapshot.tree);
        let reopened = ForgeServer::start(forge.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        snapshot
            .store
            .verify_stable(&reopened.store, &input)
            .await
            .unwrap();
        reopened.shutdown_unique().await.unwrap();
        snapshot
            .restore_after_all_forge_handles_closed(&snapshot_dir, &forge)
            .unwrap();
        assert_eq!(TreeManifest::capture_stable(&forge).unwrap(), snapshot.tree);
        std::fs::write(snapshot_dir.join("extra"), b"tampered").unwrap();
        assert!(
            snapshot
                .restore_after_all_forge_handles_closed(&snapshot_dir, &forge)
                .is_err()
        );
    });
}

#[test]
fn tree_manifest_covers_nested_files_and_rejects_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(
        root.join(".artisan-qualification"),
        b"artisan-qualification-v1\n",
    )
    .unwrap();
    std::fs::create_dir(root.join("blobs")).unwrap();
    std::fs::write(root.join("blobs/content"), b"first").unwrap();
    let initial = TreeManifest::capture_stable(root).unwrap();
    assert!(initial.directories.contains(std::path::Path::new("blobs")));
    assert_eq!(initial.files[std::path::Path::new("blobs/content")].size, 5);
    std::fs::write(root.join("blobs/content"), b"other").unwrap();
    let changed = TreeManifest::capture_stable(root).unwrap();
    assert_ne!(initial.digest().unwrap(), changed.digest().unwrap());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("content", root.join("blobs/link")).unwrap();
        assert!(TreeManifest::capture_stable(root).is_err());
    }
}

#[test]
fn immutable_fixture_rejects_duplicates_unknown_references_and_changed_bytes() {
    let value = fixture();
    let bytes = serde_json::to_vec(&value).unwrap();
    let parsed = ImmutableFixture::parse(bytes.clone()).unwrap();
    assert_eq!(parsed.fixture().items[0].id.as_str(), "item-a");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.json");
    std::fs::write(&path, &bytes).unwrap();
    parsed.verify_unchanged(&path).unwrap();
    std::fs::write(&path, [bytes, vec![b' ']].concat()).unwrap();
    assert!(parsed.verify_unchanged(&path).is_err());
    let mut duplicate = fixture();
    duplicate.items.push(duplicate.items[0].clone());
    assert!(ImmutableFixture::parse(serde_json::to_vec(&duplicate).unwrap()).is_err());
    let mut unknown = fixture();
    unknown.items[0].thread_id = "missing".into();
    assert!(ImmutableFixture::parse(serde_json::to_vec(&unknown).unwrap()).is_err());
    let mut unknown_run = fixture();
    unknown_run.items[0].run_id = Some("missing".into());
    assert!(unknown_run.validate().is_err());
}

#[test]
fn parent_order_rejects_forward_self_and_cyclic_references() {
    let mut ordered = fixture();
    let mut child = ordered.items[0].clone();
    child.id = "item-b".into();
    child.parent_id = Some("item-a".into());
    ordered.items.push(child);
    assert!(ordered.validate().is_ok());
    let mut forward = ordered.clone();
    forward.items.swap(0, 1);
    assert!(forward.validate().is_err());
    let mut self_parent = ordered.clone();
    self_parent.items[0].parent_id = Some("item-a".into());
    assert!(self_parent.validate().is_err());
    let mut cyclic = ordered.clone();
    cyclic.items[0].parent_id = Some("item-b".into());
    assert!(cyclic.validate().is_err());
    let mut thread_child = ordered.threads[0].clone();
    thread_child.id = "thread-b".into();
    thread_child.parent_thread_id = Some("thread-a".into());
    ordered.threads.push(thread_child);
    assert!(ordered.validate().is_ok());
    ordered.threads.swap(0, 1);
    assert!(ordered.validate().is_err());
}

#[test]
fn checkpoint_digest_canonicalizes_nested_objects_and_rejects_unsorted_pending() {
    let mut first = CheckpointState {
        name: "thread-ready".into(),
        route: "thread-a".into(),
        focus: None,
        pointer: Some((12, 34)),
        scroll: BTreeMap::new(),
        animation_time_ns: 0,
        pending: vec!["images".into(), "network".into()],
        observations: BTreeMap::new(),
    };
    let left: serde_json::Value = serde_json::from_str(r#"{"outer":{"z":1,"a":2}}"#).unwrap();
    let right: serde_json::Value = serde_json::from_str(r#"{"outer":{"a":2,"z":1}}"#).unwrap();
    first.observations.insert("fixture".into(), left);
    let mut second = first.clone();
    second.observations.insert("fixture".into(), right);
    assert_eq!(first.digest().unwrap(), second.digest().unwrap());
    second
        .observations
        .insert("fixture".into(), serde_json::json!({"outer":{"a":3,"z":1}}));
    assert_ne!(first.digest().unwrap(), second.digest().unwrap());
    second.pending = vec!["network".into(), "images".into()];
    assert!(second.digest().is_err());
    second.pending = vec!["images".into(), "images".into()];
    assert!(second.digest().is_err());
}

#[test]
fn bootstrap_requires_absent_or_hash_matched_editor_settings_before_starting_forge() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let forge = directory.path().join("forge");
    let editor = directory.path().join("editor");
    std::fs::create_dir_all(&forge).unwrap();
    std::fs::create_dir_all(&editor).unwrap();
    for dir in [&forge, &editor] {
        std::fs::write(
            dir.join(".artisan-qualification"),
            b"artisan-qualification-v1\n",
        )
        .unwrap();
    }
    let settings = br#"{"theme":"dark"}"#;
    let hash = hex::encode(Sha256::digest(settings));
    let request_path = directory.path().join("request.json");
    let request = |settings_hash: Option<&str>| {
        serde_json::json!({
            "fixture": directory.path().join("fixture.json"),
            "fixture_sha256": "0".repeat(64),
            "snapshot": directory.path().join("snapshot.json"),
            "snapshot_sha256": "0".repeat(64),
            "forge_dir": forge,
            "forge_logical_path": forge,
            "seed_database_sha256": "0".repeat(64),
            "editor_dir": editor,
            "editor_settings_sha256": settings_hash,
            "listen": "127.0.0.1:43123",
            "host_id": "forge-a",
            "thread_id": "thread-a",
            "width": 800,
            "height": 600
        })
    };
    let check = |expected: Option<&str>| {
        std::fs::write(
            &request_path,
            serde_json::to_vec(&request(expected)).unwrap(),
        )
        .unwrap();
        runtime
            .block_on(VerifiedBootstrap::load(&request_path))
            .err()
            .unwrap()
            .to_string()
    };
    assert!(check(None).contains("forge.sqlite3"));
    std::fs::write(editor.join("settings.json"), settings).unwrap();
    assert!(check(None).contains("settings must be absent"));
    assert!(check(Some(&"0".repeat(64))).contains("hash mismatch"));
    assert!(check(Some(&hash)).contains("forge.sqlite3"));
    std::fs::remove_file(editor.join("settings.json")).unwrap();
    assert!(check(Some(&hash)).contains("settings.json"));
}

#[test]
fn real_forge_builtin_and_seed_are_observed_without_dropping_scratch() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    runtime.block_on(async {
        let data = directory.path().join("forge");
        let server = ForgeServer::start(data.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let baseline = server.store.state().await.unwrap();
        let scratch = baseline
            .projects
            .iter()
            .find(|project| project.id.is_scratch())
            .unwrap();
        assert_eq!(std::path::Path::new(&scratch.path), data.join("scratch"));
        let mut invalid = fixture();
        invalid.items[0].parent_id = Some("item-a".into());
        let head = server.store.head();
        assert!(invalid.seed(&server.store).await.is_err());
        assert_eq!(server.store.head(), head);
        let input = fixture();
        let snapshot = input.seed(&server.store).await.unwrap();
        assert!(snapshot.projects.iter().any(|project| project == scratch));
        assert!(
            snapshot
                .projects
                .iter()
                .any(|project| project.id.as_str() == "project-a")
        );
        assert_eq!(snapshot.items["thread-a"].len(), 1);
        snapshot.verify_stable(&server.store, &input).await.unwrap();
        server
            .store
            .set_setting("fixture-change".into(), serde_json::json!(true))
            .await
            .unwrap();
        assert!(snapshot.verify_stable(&server.store, &input).await.is_err());
        let updated = StoreSnapshot::capture_stable(&server.store, &input)
            .await
            .unwrap();
        assert!(updated.head > snapshot.head);
        assert_eq!(
            updated.settings.get("fixture-change"),
            Some(&serde_json::json!(true))
        );
        updated.verify_stable(&server.store, &input).await.unwrap();
        server.shutdown().await.unwrap();
    });
}

#[test]
fn strip_verbatim_rewrites_only_drive_paths() {
    use editor_aqua::qualification::fixture::strip_verbatim;
    use std::path::PathBuf;
    assert_eq!(
        strip_verbatim(PathBuf::from(r"\\?\C:\Users\forge")),
        PathBuf::from(r"C:\Users\forge")
    );
    assert_eq!(
        strip_verbatim(PathBuf::from(r"\\?\UNC\host\share")),
        PathBuf::from(r"\\?\UNC\host\share")
    );
    assert_eq!(
        strip_verbatim(PathBuf::from("/tmp/forge")),
        PathBuf::from("/tmp/forge")
    );
}
