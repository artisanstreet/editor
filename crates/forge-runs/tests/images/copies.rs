use super::*;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Seen {
    sent: Vec<PathBuf>,
    copies: Vec<PathBuf>,
    delivered: Vec<Vec<u8>>,
    texts: Vec<String>,
    readable: Vec<PathBuf>,
}

fn png(red: u8) -> Vec<u8> {
    use image::ImageEncoder as _;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&[red, 2, 3, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    png
}

fn named_copies(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter_map(|line| line.strip_prefix("[Image ")?.split_once(" is saved at "))
        .filter_map(|(_, path)| path.strip_suffix(']'))
        .map(PathBuf::from)
        .collect()
}

fn overwrite(copies: &[PathBuf]) {
    for path in copies {
        std::fs::write(path, b"the engine wrote over its image").unwrap();
    }
}

fn record(seen: &Mutex<Seen>, input: &forge_engines::UserInput) -> Vec<PathBuf> {
    let copies = named_copies(&input.text);
    let mut seen = seen.lock().unwrap();
    seen.sent.extend(input.images.iter().cloned());
    seen.copies.extend(copies.iter().cloned());
    seen.delivered
        .extend(copies.iter().map(|copy| std::fs::read(copy).unwrap()));
    seen.texts.push(input.text.clone());
    copies
}

fn recording(recorded: Arc<Mutex<Seen>>) -> Script {
    Arc::new(move |turn, mut io| {
        let seen = Arc::clone(&recorded);
        Box::pin(async move {
            seen.lock()
                .unwrap()
                .readable
                .clone_from(&turn.launch.readable);
            overwrite(&record(&seen, &turn.input));
            io.emit(EngineEvent::Live {
                target: forge_engines::LiveKind::Reasoning,
                text: "looking".into(),
                append: true,
            })
            .await;
            let Some(Input::Steer(steer)) = io.inbox.recv().await else {
                return Outcome::failed("no steer");
            };
            overwrite(&record(&seen, &steer));
            io.emit(EngineEvent::InputAccepted { id: steer.id }).await;
            Outcome::done()
        })
    })
}

async fn live(fixture: &Fixture) {
    test_support::wait::eventually("live text", test_support::wait::LIVENESS, || async {
        fixture
            .hub
            .live_snapshot()
            .1
            .iter()
            .any(|event| matches!(event, Event::Live { .. }))
    })
    .await;
}

fn with_image(text: &str, hash: &artisan_api::BlobHash) -> SendMessage {
    let mut message = message(text);
    message.images.push(hash.clone());
    message
}

fn image_folder(fixture: &Fixture, thread: &ThreadId) -> PathBuf {
    images::thread_folder(fixture.dir.path(), thread)
}

#[tokio::test(flavor = "multi_thread")]
async fn engines_get_pristine_bytes_and_overwrite_only_thread_copies_that_outlive_the_run() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fixture = fixture(recording(Arc::clone(&seen))).await;
    let store = fixture.hub.store();
    let blob = store.put_image(png(1)).await.unwrap();
    let pristine = std::fs::read(store.blob_path(&blob.hash).unwrap()).unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("look at this", &blob.hash))
        .await
        .unwrap();
    live(&fixture).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("this one again", &blob.hash))
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs.len(), 1, "the steer joined the live run");

    let blobs = store.blob_dir();
    let folder = image_folder(&fixture, &fixture.thread_id);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.copies.len(), 2, "both messages named their image");
    assert_eq!(seen.sent.len(), 2, "both images reached the engine");
    for sent in &seen.sent {
        assert_eq!(
            std::fs::read(sent).unwrap(),
            pristine,
            "Forge sent bytes the engine overwrote"
        );
    }
    for (copy, text) in seen.copies.iter().zip(&seen.texts) {
        assert!(
            !copy.starts_with(blobs) && !text.contains(&*blobs.display().to_string()),
            "the prompt names the store: {text}"
        );
        assert!(
            seen.readable.iter().any(|dir| copy.starts_with(dir)),
            "the engine may read its copy {}",
            copy.display()
        );
        assert_eq!(copy.parent(), Some(folder.as_path()));
        assert!(
            copy.exists(),
            "the copy {} went with the run",
            copy.display()
        );
    }
    assert!(
        seen.readable
            .iter()
            .all(|dir| !blobs.starts_with(dir) && !dir.starts_with(blobs)),
        "the engine may read the image store: {:?}",
        seen.readable
    );
    drop(seen);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_steer_carrying_the_threads_first_image_names_a_path_the_engine_may_read() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fixture = fixture(recording(Arc::clone(&seen))).await;
    let blob = fixture.hub.store().put_image(png(11)).await.unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("start"))
        .await
        .unwrap();
    live(&fixture).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("now this", &blob.hash))
        .await
        .unwrap();
    fixture.settled().await;

    let seen = seen.lock().unwrap();
    assert_eq!(seen.copies.len(), 1, "the steer named its image");
    let copy = &seen.copies[0];
    assert!(
        seen.readable.iter().any(|dir| copy.starts_with(dir)),
        "the engine may not read {} from {:?}",
        copy.display(),
        seen.readable
    );
    drop(seen);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resent_image_restores_the_copy_the_engine_overwrote() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fixture = fixture(recording(Arc::clone(&seen))).await;
    let blob = fixture.hub.store().put_image(png(8)).await.unwrap();
    let pristine = std::fs::read(fixture.hub.store().blob_path(&blob.hash).unwrap()).unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("look", &blob.hash))
        .await
        .unwrap();
    live(&fixture).await;
    let copy = seen.lock().unwrap().copies[0].clone();
    assert_ne!(
        std::fs::read(&copy).unwrap(),
        pristine,
        "the engine overwrote its copy"
    );
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("again", &blob.hash))
        .await
        .unwrap();
    fixture.settled().await;

    let seen = seen.lock().unwrap();
    assert_eq!(seen.copies, vec![copy.clone(), copy]);
    assert_eq!(seen.delivered, vec![pristine.clone(), pristine]);
    drop(seen);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_path_from_one_turn_still_reads_in_the_next() {
    let turns = Arc::new(Mutex::new(Vec::<Vec<PathBuf>>::new()));
    let recorded = Arc::clone(&turns);
    let fixture = fixture(Arc::new(move |turn, _io| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            recorded
                .lock()
                .unwrap()
                .push(named_copies(&turn.input.text));
            Outcome::done()
        })
    }))
    .await;
    let blob = fixture.hub.store().put_image(png(3)).await.unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("crop this", &blob.hash))
        .await
        .unwrap();
    test_support::wait::eventually("the first turn", test_support::wait::LIVENESS, || async {
        turns.lock().unwrap().len() == 1
    })
    .await;
    fixture.settled().await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("crop the image I sent"))
        .await
        .unwrap();
    fixture.settled().await;

    let pristine = std::fs::read(fixture.hub.store().blob_path(&blob.hash).unwrap()).unwrap();
    let turns = turns.lock().unwrap();
    assert_eq!(turns.len(), 2, "both turns ran");
    let first = &turns[0][0];
    assert_eq!(
        std::fs::read(first).ok(),
        Some(pristine),
        "turn 2 can't read {}",
        first.display()
    );
    drop(turns);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_hub_on_the_same_data_leaves_a_live_runs_copies() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fixture = fixture(recording(Arc::clone(&seen))).await;
    let store = fixture.hub.store();
    let first = store.put_image(png(4)).await.unwrap();
    let second = store.put_image(png(5)).await.unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("look", &first.hash))
        .await
        .unwrap();
    live(&fixture).await;
    let copy = seen.lock().unwrap().copies[0].clone();

    let data = fixture.dir.path().to_path_buf();
    let replacement = Hub::with_harness(
        store.clone(),
        Engines::new(&data),
        data,
        Arc::new(|_| {
            Arc::new(Unscripted(Arc::new(|_, _| {
                Box::pin(async { Outcome::done() })
            })))
        }),
    );
    assert!(copy.exists(), "the replacement removed the live run's copy");

    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("and this", &second.hash))
        .await
        .unwrap();
    fixture.settled().await;
    replacement.shutdown();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.copies.len(), 2, "the steer's image reached the engine");
    assert!(seen.copies.iter().all(|copy| copy.exists()));
    drop(seen);
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_thread_removes_its_image_copies() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let blob = fixture.hub.store().put_image(png(6)).await.unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("keep this", &blob.hash))
        .await
        .unwrap();
    fixture.settled().await;
    let folder = image_folder(&fixture, &fixture.thread_id);
    assert!(
        folder.is_dir(),
        "the image wasn't copied into the thread's folder"
    );

    let removed = fixture
        .hub
        .store()
        .remove_inactive_threads(
            artisan_api::now() + 60_000,
            std::collections::HashSet::new(),
            8,
        )
        .await
        .unwrap();
    assert_eq!(removed.len(), 1);
    for thread in removed {
        fixture.hub.forget_removed_thread(thread).await;
    }

    assert!(!folder.exists(), "the removed thread's images outlived it");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_exclusive_sweep_removes_only_the_image_folders_of_removed_threads() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let kept = image_folder(&fixture, &fixture.thread_id);
    let orphan = image_folder(&fixture, &ThreadId::new());
    for folder in [&kept, &orphan] {
        std::fs::create_dir_all(folder).unwrap();
        std::fs::write(folder.join("image.png"), png(7)).unwrap();
    }

    let exclusive = forge_store::DataLock::acquire(fixture.dir.path()).unwrap();
    sweep_leftovers(fixture.dir.path(), fixture.hub.store(), &exclusive).await;

    assert!(
        kept.join("image.png").exists(),
        "the sweep removed a live thread's images"
    );
    assert!(!orphan.exists(), "the sweep left a removed thread's images");
}

#[tokio::test(flavor = "multi_thread")]
async fn threads_with_any_id_run_with_their_images_inside_forges_folder() {
    let seen = Arc::new(Mutex::new(Vec::<(String, Vec<PathBuf>)>::new()));
    let recorded = Arc::clone(&seen);
    let fixture = fixture(Arc::new(move |turn, _io| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            recorded
                .lock()
                .unwrap()
                .push((turn.input.text.clone(), named_copies(&turn.input.text)));
            Outcome::done()
        })
    }))
    .await;
    let store = fixture.hub.store();
    let project = store
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap()
        .project_id;
    let blob = store.put_image(png(9)).await.unwrap();
    let root = fixture.dir.path().join("run-images");
    let outside = fixture.dir.path().join("outside");
    let ids = [
        outside.display().to_string(),
        "..".to_owned(),
        "../escape".to_owned(),
        "session:123".to_owned(),
    ];
    for id in ids {
        let thread = store
            .create_thread(CreateThread {
                id: ThreadId::from(id.as_str()),
                project_id: project.clone(),
                config: EngineConfig::new(EngineId::Claude),
                title: None,
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .unwrap();
        fixture
            .hub
            .send_message(&thread.id, message("only words"))
            .await
            .unwrap();
        fixture.settled_of(&thread.id).await;
        fixture
            .hub
            .send_message(&thread.id, with_image("and a picture", &blob.hash))
            .await
            .unwrap();
        let runs = fixture.settled_of(&thread.id).await;
        assert!(
            runs.iter().all(|run| run.status() == RunStatus::Done),
            "thread {id:?} failed: {runs:?}"
        );
    }

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 8);
    let copies: Vec<_> = seen.iter().flat_map(|(_, copies)| copies.iter()).collect();
    assert_eq!(copies.len(), 4);
    for copy in copies {
        assert_eq!(
            copy.parent().and_then(Path::parent),
            Some(root.as_path()),
            "{}",
            copy.display()
        );
        assert!(copy.exists());
    }
    assert!(!outside.exists());
    assert!(!fixture.dir.path().parent().unwrap().join("escape").exists());
    drop(seen);
}

fn block_copy(fixture: &Fixture, stored: &Path) {
    let folder = image_folder(fixture, &fixture.thread_id);
    std::fs::create_dir_all(folder.join(stored.file_name().unwrap()).join("in the way")).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_steer_whose_image_cannot_be_copied_keeps_its_image_and_says_the_path_is_missing() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fixture = fixture(recording(Arc::clone(&seen))).await;
    let blob = fixture.hub.store().put_image(png(10)).await.unwrap();
    let stored = fixture.hub.store().blob_path(&blob.hash).unwrap();
    block_copy(&fixture, &stored);
    fixture
        .hub
        .send_message(&fixture.thread_id, message("start"))
        .await
        .unwrap();
    live(&fixture).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("see this", &blob.hash))
        .await
        .unwrap();
    fixture.settled().await;

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.texts,
        vec![
            "start".to_owned(),
            "see this\n\n[The file path of image 1 isn't available.]".to_owned(),
        ]
    );
    assert_eq!(seen.sent, vec![stored], "the steer lost its stored image");
    drop(seen);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_whose_image_cannot_be_copied_starts_with_its_image_and_says_the_path_is_missing() {
    let turns = Arc::new(Mutex::new(Vec::<forge_engines::UserInput>::new()));
    let recorded = Arc::clone(&turns);
    let fixture = fixture(Arc::new(move |turn, _io| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            recorded.lock().unwrap().push(turn.input.clone());
            Outcome::done()
        })
    }))
    .await;
    let blob = fixture.hub.store().put_image(png(12)).await.unwrap();
    let stored = fixture.hub.store().blob_path(&blob.hash).unwrap();
    block_copy(&fixture, &stored);
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("see this", &blob.hash))
        .await
        .unwrap();
    let runs = fixture.settled().await;

    assert!(
        runs.iter().all(|run| run.status() == RunStatus::Done),
        "{runs:?}"
    );
    let turns = turns.lock().unwrap();
    assert_eq!(turns.len(), 1, "the run never reached the engine");
    assert_eq!(
        turns[0].text,
        "see this\n\n[The file path of image 1 isn't available.]"
    );
    assert_eq!(turns[0].images, vec![stored]);
    drop(turns);
}

fn first_turn_recorder() -> (Arc<Mutex<Vec<forge_engines::UserInput>>>, Script) {
    let turns = Arc::new(Mutex::new(Vec::<forge_engines::UserInput>::new()));
    let recorded = Arc::clone(&turns);
    let script: Script = Arc::new(move |turn, _io| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            recorded.lock().unwrap().push(turn.input.clone());
            Outcome::done()
        })
    });
    (turns, script)
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn symlinks_planted_in_the_image_folder_never_redirect_a_copy() {
    let (turns, script) = first_turn_recorder();
    let fixture = fixture(script).await;
    let blob = fixture.hub.store().put_image(png(13)).await.unwrap();
    let stored = fixture.hub.store().blob_path(&blob.hash).unwrap();
    let name = stored.file_name().unwrap().to_string_lossy().into_owned();
    let folder = image_folder(&fixture, &fixture.thread_id);
    std::fs::create_dir_all(&folder).unwrap();
    let victim = fixture.dir.path().join("victim");
    std::fs::write(&victim, b"precious").unwrap();
    std::os::unix::fs::symlink(&victim, folder.join(&name)).unwrap();
    for counter in 0..256 {
        std::os::unix::fs::symlink(
            &victim,
            folder.join(format!("{name}.{}-{counter}.partial", std::process::id())),
        )
        .unwrap();
    }
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("look", &blob.hash))
        .await
        .unwrap();
    fixture.settled().await;

    assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
    let copy = folder.join(&name);
    assert!(
        std::fs::symlink_metadata(&copy)
            .unwrap()
            .file_type()
            .is_file(),
        "the planted link still stands"
    );
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(&stored).unwrap()
    );
    let turns = turns.lock().unwrap();
    assert_eq!(named_copies(&turns[0].text), vec![copy]);
    drop(turns);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn an_image_folder_replaced_by_a_symlink_gets_no_copy() {
    let (turns, script) = first_turn_recorder();
    let fixture = fixture(script).await;
    let blob = fixture.hub.store().put_image(png(14)).await.unwrap();
    let stored = fixture.hub.store().blob_path(&blob.hash).unwrap();
    let folder = image_folder(&fixture, &fixture.thread_id);
    let elsewhere = fixture.dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(folder.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &folder).unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, with_image("look", &blob.hash))
        .await
        .unwrap();
    fixture.settled().await;

    assert_eq!(
        std::fs::read_dir(&elsewhere).unwrap().count(),
        0,
        "Forge wrote through the planted folder link"
    );
    let turns = turns.lock().unwrap();
    assert_eq!(
        turns[0].text,
        "look\n\n[The file path of image 1 isn't available.]"
    );
    assert_eq!(turns[0].images, vec![stored]);
    drop(turns);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn an_image_folder_replaced_by_a_symlink_is_never_granted_to_the_engine() {
    let granted = Arc::new(Mutex::new(None::<Vec<PathBuf>>));
    let recorded = Arc::clone(&granted);
    let script: Script = Arc::new(move |turn, _io| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            *recorded.lock().unwrap() = Some(turn.launch.readable.clone());
            Outcome::done()
        })
    });
    let fixture = fixture(script).await;
    let folder = image_folder(&fixture, &fixture.thread_id);
    let elsewhere = fixture.dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(folder.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &folder).unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("look"))
        .await
        .unwrap();
    fixture.settled().await;

    let granted = granted.lock().unwrap().clone().expect("the run started");
    assert!(
        granted
            .iter()
            .all(|dir| *dir != folder && !dir.starts_with(&elsewhere)),
        "the engine may read through the planted folder link: {granted:?}"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn the_exclusive_sweep_removes_stale_partials_and_stray_files_and_goes_on_past_failures() {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let live = image_folder(&fixture, &fixture.thread_id);
    std::fs::create_dir_all(&live).unwrap();
    std::fs::write(live.join("image.png"), png(15)).unwrap();
    let partial = live.join(".image.png.k3Yx9a.partial");
    std::fs::write(&partial, b"half an im").unwrap();
    let root = live.parent().unwrap().to_path_buf();
    let stray = root.join("stray-file");
    std::fs::write(&stray, b"not a folder").unwrap();
    let orphans: Vec<_> = (0..3)
        .map(|_| image_folder(&fixture, &ThreadId::new()))
        .collect();
    for orphan in &orphans {
        std::fs::create_dir_all(orphan).unwrap();
        std::fs::write(orphan.join("image.png"), png(16)).unwrap();
    }
    let stuck = image_folder(&fixture, &ThreadId::new());
    let locked = stuck.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("image.png"), png(17)).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();

    let exclusive = forge_store::DataLock::acquire(fixture.dir.path()).unwrap();
    let removed = images::sweep(fixture.dir.path(), fixture.hub.store(), &exclusive)
        .await
        .unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(removed, 5, "three orphans, the stray file and the partial");
    assert!(live.join("image.png").exists(), "the live image went");
    assert!(!partial.exists(), "the stale partial copy survived");
    assert!(!stray.exists(), "the stray file survived");
    assert!(orphans.iter().all(|orphan| !orphan.exists()));
    assert!(
        stuck.exists(),
        "the locked folder could not have been removed"
    );
}
