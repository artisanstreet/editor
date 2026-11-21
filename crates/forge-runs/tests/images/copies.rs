use super::*;
use std::path::PathBuf;

#[derive(Default)]
struct Seen {
    images: Vec<PathBuf>,
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

fn overwrite(images: &[PathBuf]) {
    for path in images {
        std::fs::write(path, b"the engine wrote over its image").unwrap();
    }
}

fn recording(recorded: Arc<Mutex<Seen>>) -> Script {
    Arc::new(move |turn, mut io| {
        let seen = Arc::clone(&recorded);
        Box::pin(async move {
            {
                let mut seen = seen.lock().unwrap();
                seen.images.extend(turn.input.images.iter().cloned());
                seen.texts.push(turn.input.text.clone());
                seen.readable.clone_from(&turn.launch.readable);
            }
            overwrite(&turn.input.images);
            io.emit(EngineEvent::Live {
                target: forge_engines::LiveKind::Reasoning,
                text: "looking".into(),
                append: true,
            })
            .await;
            let Some(Input::Steer(steer)) = io.inbox.recv().await else {
                return Outcome::failed("no steer");
            };
            {
                let mut seen = seen.lock().unwrap();
                seen.images.extend(steer.images.iter().cloned());
                seen.texts.push(steer.text.clone());
            }
            overwrite(&steer.images);
            io.emit(EngineEvent::InputAccepted { id: steer.id }).await;
            Outcome::done()
        })
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn engines_see_and_overwrite_only_per_run_copies_that_go_when_the_run_ends() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fixture = fixture(recording(Arc::clone(&seen))).await;
    let store = fixture.hub.store();
    let first_blob = store.put_image(png(1)).await.unwrap();
    let second_blob = store.put_image(png(2)).await.unwrap();
    let stored = |hash| std::fs::read(store.blob_path(hash).unwrap()).unwrap();
    let (first, second) = (stored(&first_blob.hash), stored(&second_blob.hash));
    let mut start = message("look at this");
    start.images.push(first_blob.hash.clone());
    fixture
        .hub
        .send_message(&fixture.thread_id, start)
        .await
        .unwrap();
    test_support::wait::eventually("live text", test_support::wait::LIVENESS, || async {
        fixture
            .hub
            .live_snapshot()
            .1
            .iter()
            .any(|event| matches!(event, Event::Live { .. }))
    })
    .await;
    let mut steer = message("and this");
    steer.images.push(second_blob.hash.clone());
    fixture
        .hub
        .send_message(&fixture.thread_id, steer)
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs.len(), 1, "the steer joined the live run");

    let blobs = store.blob_dir();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.images.len(), 2, "both images reached the engine");
    for (image, text) in seen.images.iter().zip(&seen.texts) {
        assert!(
            !image.starts_with(blobs),
            "the engine got the stored image {}",
            image.display()
        );
        assert!(
            text.contains(&image.display().to_string())
                && !text.contains(&*blobs.display().to_string()),
            "the prompt names the copy, never the store: {text}"
        );
        assert!(
            seen.readable.iter().any(|dir| image.starts_with(dir)),
            "the engine may read its copy {}",
            image.display()
        );
        assert!(
            !image.exists(),
            "the copy {} outlived its run",
            image.display()
        );
    }
    assert!(
        seen.readable
            .iter()
            .all(|dir| !blobs.starts_with(dir) && !dir.starts_with(blobs)),
        "the engine may read the image store: {:?}",
        seen.readable
    );
    assert!(
        !seen.images[0].parent().unwrap().exists(),
        "the run's copy folder outlived the run"
    );
    assert_eq!(stored(&first_blob.hash), first);
    assert_eq!(stored(&second_blob.hash), second);
    drop(seen);
}
