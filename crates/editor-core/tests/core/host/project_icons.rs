use aqua::images::ImageAssets;
use artisan_api::{BlobHash, BlobRef, Project, ProjectId};
use axum::http::StatusCode;
use editor_core::{BlobImage, CoreEvent, HostEvent};
use std::time::Duration;
use test_support::clock::ManualClock;
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::{HeldRequest, empty_state, png};

const ICON: &str = "/projects/project/icon";

fn project() -> ProjectId {
    ProjectId("project".into())
}

fn forge_with_clock(clock: &std::sync::Arc<ManualClock>) -> StubbedHost {
    let mut state = empty_state();
    state.projects.push(Project {
        id: project(),
        path: "/work/project".into(),
        name: "project".into(),
        seq: 1,
        created_at: 0,
        removed: false,
    });
    let mut forge = StubbedHost::start_with_clock(state, axum::Router::new(), clock.clock());
    forge.core.set_image_assets(ImageAssets::default());
    forge
}

fn forge() -> StubbedHost {
    forge_with_clock(&ManualClock::starting_at(0))
}

fn blob(hash: &str) -> BlobRef {
    BlobRef {
        hash: BlobHash(hash.into()),
        bytes: 64,
    }
}

fn settle_image(forge: &mut StubbedHost, hash: &BlobHash, done: fn(&BlobImage) -> bool) {
    forge.settle("the image settles", |core, host| {
        core.host(host)
            .and_then(|data| data.image(hash))
            .is_some_and(|image| done(&image))
    });
}

fn decode(forge: &mut StubbedHost, hash: &BlobHash, shade: u8) {
    forge.host_mut().load_image(hash.clone(), false);
    forge
        .take("GET", &format!("/blobs/{hash}"))
        .respond_bytes("image/png", png(shade));
    settle_image(forge, hash, |image| matches!(image, BlobImage::Ready(_)));
}

#[test]
fn an_evicted_project_icon_frees_its_pixels() {
    let mut forge = forge();
    let icon = BlobHash("icon".into());
    forge.host_mut().load_project_icon(&project());
    forge.take("GET", ICON).respond(Some(blob("icon")));
    forge
        .take("GET", "/blobs/icon")
        .respond_bytes("image/png", png(10));
    settle_image(&mut forge, &icon, |image| {
        matches!(image, BlobImage::Ready(_))
    });
    let shown = forge.host().project_icon(&project());
    let pixels = shown
        .as_ref()
        .expect("the sidebar shows the decoded icon")
        .downgrade();
    drop(shown);
    for index in 0..32 {
        decode(&mut forge, &BlobHash(format!("other-{index}")), 30);
    }
    assert!(
        pixels.upgrade().is_none(),
        "no hidden reference keeps the evicted icon's pixels alive"
    );
    assert!(forge.host().project_icon(&project()).is_none());
}

#[test]
fn retrying_project_icons_reloads_an_icon_whose_image_failed() {
    let mut forge = forge();
    let icon = BlobHash("icon".into());
    forge.host_mut().load_project_icon(&project());
    forge.take("GET", ICON).respond(Some(blob("icon")));
    forge.take("GET", "/blobs/icon").fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "connection reset",
    );
    settle_image(&mut forge, &icon, |image| {
        matches!(image, BlobImage::Failed(_))
    });
    let data = forge.host_mut();
    assert!(
        data.project_icons_failed(),
        "the sidebar offers Retry for an icon whose image failed"
    );
    data.retry_project_icons();
    assert!(
        matches!(data.image(&icon), Some(BlobImage::Loading)),
        "Retry downloads the failed icon image again"
    );
    assert!(!data.project_icons_failed());
    forge.take("GET", "/blobs/icon");
}

fn next_read(forge: &mut StubbedHost, clock: &ManualClock, changed: &mut bool) -> HeldRequest {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        forge.core.drain_with(|_, event| {
            if matches!(
                event,
                CoreEvent::Host {
                    event: HostEvent::ImagesChanged,
                    ..
                }
            ) {
                *changed = true;
            }
        });
        clock.advance(Duration::from_secs(301));
        forge.host_mut().load_project_icon(&project());
        if let Some(read) = forge.forge.try_take("GET", ICON) {
            return read;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the Editor never re-read the project icon"
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

#[test]
fn rereading_an_unchanged_project_icon_leaves_the_window_alone() {
    let clock = ManualClock::starting_at(0);
    let mut forge = forge_with_clock(&clock);
    let mut changed = false;
    let mut read = next_read(&mut forge, &clock, &mut changed);
    for (answer, shown, expected) in [
        ("icon", "the first icon read shows the icon", true),
        (
            "icon",
            "a periodic re-read of the same icon must not rebuild the window",
            false,
        ),
        ("new-icon", "a replaced icon is shown", true),
    ] {
        read.respond(Some(blob(answer)));
        changed = false;
        read = next_read(&mut forge, &clock, &mut changed);
        assert_eq!(changed, expected, "{shown}");
    }
}
