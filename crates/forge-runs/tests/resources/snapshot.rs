use super::*;
use crate::resources::{ResourceError, Written};

async fn kept(fixture: &Fixture, item: &ItemId, written: &Written) -> Vec<u8> {
    let thread = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    let snapshot = fixture.hub.store().resource_snapshot_target(
        &fixture.thread_id,
        item,
        &written.path,
        written.from_home,
    );
    assert!(
        snapshot.is_some_and(|path| path.is_file()),
        "the reply's image was kept before the reply was saved"
    );
    fixture
        .hub
        .read_resource(&thread, Some(item), written)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_a_reply_shows_outlives_its_file_and_later_versions() {
    let fixture = fixture(Arc::new(|turn, io| {
        Box::pin(async move {
            let key = format!("reply-{}", turn.input.text);
            let text = "```artisan-image\n{ \"src\": \"shots/latest.png\" }\n```";
            io.emit(reply(&key, text)).await;
            Outcome::done()
        })
    }))
    .await;
    let shots = fixture.dir.path().join("shots");
    std::fs::create_dir_all(&shots).unwrap();
    let written = Written {
        path: "shots/latest.png".to_owned(),
        from_home: false,
    };

    std::fs::write(shots.join("latest.png"), b"first render").unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("one"))
        .await
        .unwrap();
    fixture.settled().await;
    let first = fixture.by_native("reply-one").await.id;
    assert_eq!(kept(&fixture, &first, &written).await, b"first render");

    std::fs::write(shots.join("latest.png"), b"second render").unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("two"))
        .await
        .unwrap();
    fixture.settled().await;
    let second = fixture.by_native("reply-two").await.id;
    assert_eq!(kept(&fixture, &second, &written).await, b"second render");

    std::fs::remove_dir_all(&shots).unwrap();
    assert_eq!(kept(&fixture, &first, &written).await, b"first render");
    assert_eq!(kept(&fixture, &second, &written).await, b"second render");
    let thread = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        fixture.hub.read_resource(&thread, None, &written).await,
        Err(ResourceError::Missing)
    ));
}

async fn reply_naming_a_missing_plot(fixture: &Fixture) -> (artisan_api::Thread, ItemId, Written) {
    fixture
        .hub
        .send_message(&fixture.thread_id, message("plot"))
        .await
        .unwrap();
    fixture.settled().await;
    let item = fixture.by_native("reply").await.id;
    let thread = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    let written = Written {
        path: "plot.png".to_owned(),
        from_home: false,
    };
    assert!(matches!(
        fixture
            .hub
            .read_resource(&thread, Some(&item), &written)
            .await,
        Err(ResourceError::Missing)
    ));
    (thread, item, written)
}

fn shows_missing_plot() -> Script {
    Arc::new(|_, io| {
        Box::pin(async move {
            io.emit(reply("reply", "![plot](plot.png)")).await;
            Outcome::done()
        })
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_a_reply_named_before_it_existed_is_shown_as_it_is_now() {
    let fixture = fixture(shows_missing_plot()).await;
    let (thread, item, written) = reply_naming_a_missing_plot(&fixture).await;
    let file = fixture.dir.path().join("plot.png");
    for version in [&b"next turn's plot"[..], b"the plot after that"] {
        std::fs::write(&file, version).unwrap();
        assert_eq!(
            fixture
                .hub
                .read_resource(&thread, Some(&item), &written)
                .await
                .unwrap(),
            version,
            "a later version of the file was pinned to the earlier reply"
        );
    }
    std::fs::remove_file(&file).unwrap();
    assert!(matches!(
        fixture
            .hub
            .read_resource(&thread, Some(&item), &written)
            .await,
        Err(ResourceError::Missing)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_from_before_files_were_kept_at_publish_keeps_them_from_first_view() {
    let fixture = fixture(shows_missing_plot()).await;
    let (thread, item, written) = reply_naming_a_missing_plot(&fixture).await;
    rusqlite::Connection::open(fixture.dir.path().join("forge").join("forge.sqlite3"))
        .unwrap()
        .execute_batch(include_str!("before_capture.sql"))
        .unwrap();
    let file = fixture.dir.path().join("plot.png");
    std::fs::write(&file, b"plot").unwrap();
    assert_eq!(
        fixture
            .hub
            .read_resource(&thread, Some(&item), &written)
            .await
            .unwrap(),
        b"plot"
    );
    std::fs::remove_file(&file).unwrap();
    assert_eq!(
        fixture
            .hub
            .read_resource(&thread, Some(&item), &written)
            .await
            .unwrap(),
        b"plot"
    );
    assert!(matches!(
        fixture
            .hub
            .read_resource(&thread, Some(&ItemId::new()), &written)
            .await,
        Err(ResourceError::Missing)
    ));
}

fn kept_at(fixture: &Fixture, item: &ItemId, path: &str) -> PathBuf {
    fixture
        .hub
        .store()
        .resource_snapshot_target(&fixture.thread_id, item, path, false)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_naming_files_outside_its_project_or_denied_to_it_keeps_none_of_them() {
    let outside = tempfile::tempdir().unwrap();
    let key = outside.path().join("id_ed25519");
    std::fs::write(&key, b"private key").unwrap();
    let absolute = key.display().to_string();
    let shown = absolute.clone();
    let fixture = fixture(Arc::new(move |_, io| {
        let text = format!(
            "![key]({shown}) ![linked](linked.png) ![through](keys/id_ed25519) ![env](secrets/env.png) ![ok](ok.png)"
        );
        Box::pin(async move {
            io.emit(reply("reply", &text)).await;
            Outcome::done()
        })
    }))
    .await;
    let project = fixture.dir.path();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&key, project.join("linked.png")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&key, project.join("linked.png")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), project.join("keys")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(outside.path(), project.join("keys")).unwrap();
    std::fs::create_dir_all(project.join("secrets")).unwrap();
    std::fs::write(project.join("secrets/env.png"), b"API_KEY=1").unwrap();
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    std::fs::write(
        project.join(".claude/settings.json"),
        r#"{ "permissions": { "deny": ["Read(./secrets/**)"] } }"#,
    )
    .unwrap();
    std::fs::write(project.join("ok.png"), b"plot").unwrap();

    fixture
        .hub
        .send_message(&fixture.thread_id, message("show"))
        .await
        .unwrap();
    fixture.settled().await;
    let item = fixture.by_native("reply").await.id;

    assert!(kept_at(&fixture, &item, "ok.png").is_file());
    let thread = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    for path in [
        absolute.as_str(),
        "linked.png",
        "keys/id_ed25519",
        "secrets/env.png",
    ] {
        assert!(!kept_at(&fixture, &item, path).exists(), "{path} was kept");
        let written = Written {
            path: path.to_owned(),
            from_home: false,
        };
        let shown = fixture
            .hub
            .read_resource(&thread, Some(&item), &written)
            .await;
        assert!(
            matches!(
                shown,
                Err(ResourceError::Outside { .. }
                    | ResourceError::Linked { .. }
                    | ResourceError::Refused { .. })
            ),
            "{path} reached the Editor: {shown:?}"
        );
    }
}

async fn shown_under(settings: &str) -> (Result<Vec<u8>, ResourceError>, bool) {
    let fixture = fixture(Arc::new(|_, io| {
        Box::pin(async move {
            io.emit(reply("reply", "![plot](plot.png)")).await;
            Outcome::done()
        })
    }))
    .await;
    let project = fixture.dir.path();
    std::fs::write(project.join("plot.png"), b"plot").unwrap();
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    std::fs::write(project.join(".claude/settings.json"), settings).unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("plot"))
        .await
        .unwrap();
    fixture.settled().await;
    let item = fixture.by_native("reply").await.id;
    let kept = kept_at(&fixture, &item, "plot.png").is_file();
    let thread = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    let written = Written {
        path: "plot.png".to_owned(),
        from_home: false,
    };
    let shown = fixture
        .hub
        .read_resource(&thread, Some(&item), &written)
        .await;
    (shown, kept)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_that_denies_reading_or_has_broken_settings_shows_and_keeps_no_file() {
    for settings in [
        r#"{ "permissions": { "deny": ["Read"] } }"#,
        r#"{ "permissions": { "ask": ["Read"] } }"#,
        r#"{ "permissions": { "deny": "Read" } }"#,
        "{ not json",
    ] {
        let (shown, kept) = shown_under(settings).await;
        assert!(!kept, "kept under {settings}");
        assert!(
            matches!(shown, Err(ResourceError::Refused { .. })),
            "shown under {settings}: {shown:?}"
        );
    }
    let (shown, kept) = shown_under(r#"{ "permissions": { "deny": ["Bash"] } }"#).await;
    assert!(kept);
    assert_eq!(shown.unwrap(), b"plot");
}

#[tokio::test]
async fn a_reply_keeps_the_file_as_it_was_when_the_reply_was_published() {
    let overwritten = Arc::new(Notify::new());
    let resume = Arc::clone(&overwritten);
    let fixture = fixture(Arc::new(move |_, io| {
        let resume = Arc::clone(&resume);
        Box::pin(async move {
            io.emit(reply("reply", "![plot](plot.png)")).await;
            resume.notified().await;
            Outcome::done()
        })
    }))
    .await;
    let file = fixture.dir.path().join("plot.png");
    std::fs::write(&file, b"as replied").unwrap();
    let mut events = fixture.hub.store().subscribe();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("plot"))
        .await
        .unwrap();
    let item = tokio::time::timeout(test_support::wait::LIVENESS, async {
        loop {
            if let Event::Item { item } = events.recv().await.unwrap().event
                && matches!(item.kind, ItemKind::AssistantMessage { .. })
            {
                return item.id;
            }
        }
    })
    .await
    .expect("the reply is published");
    let kept_when_published = kept_at(&fixture, &item, "plot.png").is_file();
    std::fs::write(&file, b"next turn's plot").unwrap();
    overwritten.notify_one();
    fixture.settled().await;

    assert!(kept_when_published);
    let written = Written {
        path: "plot.png".to_owned(),
        from_home: false,
    };
    assert_eq!(kept(&fixture, &item, &written).await, b"as replied");
}
