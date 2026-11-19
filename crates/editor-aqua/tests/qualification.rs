#![cfg(feature = "qualification")]

#[path = "qualification/observation.rs"]
mod observation;

use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ItemKind, Phase, Status};
use editor_aqua::qualification::fixture::{
    CheckpointState, EditorSettingsError, Fixture, FixtureItem, ForgeServer, ImmutableFixture,
    LoadBootstrapError, ReadCheckedError, SeedSnapshot, StoreSnapshot, TreeManifest,
    VerifiedBootstrap,
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
    assert_eq!(verified.editor_dir(), paths[4]);
    assert_eq!(verified.fixture_path(), fixture_path);
    editor_aqua::core::runtime::runtime()
        .block_on(verified.into_server().shutdown_unique())
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
            .verify_stable(reopened.store(), &input)
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
        TreeManifest::capture_stable(root).unwrap_err();
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
    parsed.verify_unchanged(&path).unwrap_err();
    let mut duplicate = fixture();
    duplicate.items.push(duplicate.items[0].clone());
    let Err(_) = ImmutableFixture::parse(serde_json::to_vec(&duplicate).unwrap()) else {
        panic!("the duplicate fixture must be rejected");
    };
    let mut unknown = fixture();
    unknown.items[0].thread_id = "missing".into();
    let Err(_) = ImmutableFixture::parse(serde_json::to_vec(&unknown).unwrap()) else {
        panic!("the unknown fixture must be rejected");
    };
    let mut unknown_run = fixture();
    unknown_run.items[0].run_id = Some("missing".into());
    unknown_run.validate().unwrap_err();
}

#[test]
fn parent_order_rejects_forward_self_and_cyclic_references() {
    let mut ordered = fixture();
    let mut child = ordered.items[0].clone();
    child.id = "item-b".into();
    child.parent_id = Some("item-a".into());
    ordered.items.push(child);
    ordered.validate().unwrap();
    let mut forward = ordered.clone();
    forward.items.swap(0, 1);
    forward.validate().unwrap_err();
    let mut self_parent = ordered.clone();
    self_parent.items[0].parent_id = Some("item-a".into());
    self_parent.validate().unwrap_err();
    let mut cyclic = ordered.clone();
    cyclic.items[0].parent_id = Some("item-b".into());
    cyclic.validate().unwrap_err();
    let mut thread_child = ordered.threads[0].clone();
    thread_child.id = "thread-b".into();
    thread_child.parent_thread_id = Some("thread-a".into());
    ordered.threads.push(thread_child);
    ordered.validate().unwrap();
    ordered.threads.swap(0, 1);
    ordered.validate().unwrap_err();
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
    second.digest().unwrap_err();
    second.pending = vec!["images".into(), "images".into()];
    second.digest().unwrap_err();
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
        let Err(error) = runtime.block_on(VerifiedBootstrap::load(&request_path)) else {
            panic!("a bootstrap without a seeded Forge must not load");
        };
        error
    };
    let reached_the_database = |error: LoadBootstrapError| {
        matches!(
            error,
            LoadBootstrapError::Database(ReadCheckedError::Read { path, .. })
                if path.ends_with("forge.sqlite3")
        )
    };
    assert!(reached_the_database(check(None)));
    std::fs::write(editor.join("settings.json"), settings).unwrap();
    assert!(matches!(
        check(None),
        LoadBootstrapError::EditorSettings(EditorSettingsError::Present { .. })
    ));
    assert!(matches!(
        check(Some(&"0".repeat(64))),
        LoadBootstrapError::EditorSettings(EditorSettingsError::Read(
            ReadCheckedError::Mismatch { .. }
        ))
    ));
    assert!(reached_the_database(check(Some(&hash))));
    std::fs::remove_file(editor.join("settings.json")).unwrap();
    assert!(matches!(
        check(Some(&hash)),
        LoadBootstrapError::EditorSettings(EditorSettingsError::Read(ReadCheckedError::Read {
            path,
            ..
        })) if path.ends_with("settings.json")
    ));
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
        let baseline = server.store().state().await.unwrap();
        let scratch = baseline
            .projects
            .iter()
            .find(|project| project.id.is_scratch())
            .unwrap();
        assert_eq!(std::path::Path::new(&scratch.path), data.join("scratch"));
        let mut invalid = fixture();
        invalid.items[0].parent_id = Some("item-a".into());
        let head = server.store().head();
        invalid.seed(server.store()).await.unwrap_err();
        assert_eq!(server.store().head(), head);
        let input = fixture();
        let snapshot = input.seed(server.store()).await.unwrap();
        assert!(snapshot.projects.iter().any(|project| project == scratch));
        assert!(
            snapshot
                .projects
                .iter()
                .any(|project| project.id.as_str() == "project-a")
        );
        assert_eq!(snapshot.items["thread-a"].len(), 1);
        snapshot
            .verify_stable(server.store(), &input)
            .await
            .unwrap();
        server
            .store()
            .set_setting("fixture-change".into(), serde_json::json!(true))
            .await
            .unwrap();
        snapshot
            .verify_stable(server.store(), &input)
            .await
            .unwrap_err();
        let updated = StoreSnapshot::capture_stable(server.store(), &input)
            .await
            .unwrap();
        assert!(updated.head > snapshot.head);
        assert_eq!(
            updated.settings.get("fixture-change"),
            Some(&serde_json::json!(true))
        );
        updated.verify_stable(server.store(), &input).await.unwrap();
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

#[cfg(unix)]
#[test]
fn a_restore_that_cannot_clear_the_old_tree_still_leaves_a_complete_forge_folder() {
    use std::os::unix::fs::PermissionsExt as _;

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
        let locked = forge.join("zz-locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("held"), b"held").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let before = TreeManifest::capture_stable(&forge).unwrap();
        let restored = snapshot.restore_after_all_forge_handles_closed(&snapshot_dir, &forge);
        let after = TreeManifest::capture_stable(&forge).unwrap();
        for leftover in [
            forge.join("zz-locked"),
            directory.path().join("forge.previous/zz-locked"),
        ] {
            if leftover.exists() {
                std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
        }
        assert!(
            after == snapshot.tree || after == before,
            "the Forge folder is either the snapshot or untouched, never partial: {restored:?}"
        );
        snapshot
            .restore_after_all_forge_handles_closed(&snapshot_dir, &forge)
            .unwrap();
        assert_eq!(TreeManifest::capture_stable(&forge).unwrap(), snapshot.tree);
        assert!(!directory.path().join("forge.previous").exists());
        assert!(!directory.path().join("forge.restoring").exists());
    });
}
