//! The production owner against paired HTTPS; no alternate image loader.
use super::*;
use crate::{
    Core, EditorFiles,
    hosts::{self, HostKind, HostsFile},
    runtime::runtime,
};
use artisan_api::{BlobHash, BlobRef};
use gpui::{
    AppContext as _, Entity, IntoElement, ParentElement as _, Render, TestAppContext,
    VisualTestContext, Window, div, img,
};
use gpui::{StyleRefinement, Styled as _};
use std::time::{Duration, Instant};

// A valid 2×2 lossless WebP with a distinct, ignored RIFF metadata chunk.
// Distinct source hashes let a small fixture exercise actual entry eviction.
fn bytes(index: u32) -> Vec<u8> {
    let encoded = "524946461e000000574542505650384c110000002f014000000750a82217a5ff8188e87f0000";
    let mut bytes: Vec<_> = (0..encoded.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&encoded[at..at + 2], 16).unwrap())
        .collect();
    if bytes.len() % 2 != 0 {
        bytes.push(0);
    }
    bytes.extend_from_slice(b"JUNK");
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&index.to_le_bytes());
    let length = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&length.to_le_bytes());
    bytes
}
struct Scene {
    image: Option<Arc<gpui::RenderImage>>,
    renders: usize,
}
impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div().children(self.image.clone().map(img))
    }
}
struct Shell {
    scene: Entity<Scene>,
}
impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_full().child(
            self.scene
                .clone()
                .cached(StyleRefinement::default().absolute().inset_0()),
        )
    }
}

fn wait(cx: &mut VisualTestContext, label: &str, check: impl Fn(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn ready(
    host: &Entity<HostData>,
    hash: &BlobHash,
    cx: &mut VisualTestContext,
) -> Arc<gpui::RenderImage> {
    wait(cx, "image completion", |cx| {
        cx.read(|cx| matches!(host.read(cx).image(hash), Some(BlobImage::Ready(_))))
    });
    cx.read(|cx| match host.read(cx).image(hash).unwrap() {
        BlobImage::Ready(image) => image,
        _ => unreachable!(),
    })
}
fn failed(host: &Entity<HostData>, hash: &BlobHash, cx: &mut VisualTestContext) -> Arc<str> {
    wait(cx, "failed image completion", |cx| {
        cx.read(|cx| matches!(host.read(cx).image(hash), Some(BlobImage::Failed(_))))
    });
    cx.read(|cx| match host.read(cx).image(hash).unwrap() {
        BlobImage::Failed(error) => error,
        _ => unreachable!(),
    })
}
fn cached_bytes(host: &HostData) -> usize {
    host.images
        .values()
        .filter_map(|image| match image {
            BlobImage::Ready(image) => {
                Some(editor_ui::markdown_renderer::image_resident_bytes(image))
            }
            _ => None,
        })
        .sum()
}

#[gpui::test]
fn paired_hosts_coalesce_bound_inflight_evict_and_reload_without_owning_external_pixels(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let identity = files.identity().unwrap();
    let mut configs = Vec::new();
    let mut servers = Vec::new();
    for index in 0..2 {
        let (address, store, server) = crate::tests::start_forge(
            &dir.path().join(format!("forge-{index}")),
            "127.0.0.1:0".parse().unwrap(),
        );
        let invite = server.block_on(forge_http::invite(&store)).unwrap();
        let pin = artisan_api::tls::fingerprint_of(&invite.forge_certificate).unwrap();
        configs.push(
            runtime()
                .block_on(hosts::pair(
                    &address.to_string(),
                    &invite.code,
                    &pin,
                    &identity,
                    "image-test",
                    format!("Forge {index}"),
                    HostKind::Local,
                ))
                .unwrap(),
        );
        servers.push((store, server));
    }
    files
        .save_hosts(&HostsFile {
            hosts: configs.clone(),
            last: None,
        })
        .unwrap();
    let core = cx.new(|cx| Core::new(files, cx).unwrap());
    let a = cx.read(|cx| core.read(cx).host(&configs[0].id).unwrap().clone());
    let b = cx.read(|cx| core.read(cx).host(&configs[1].id).unwrap().clone());
    let mut blobs: Vec<BlobRef> = Vec::new();
    for index in 0..36 {
        blobs.push(
            servers[0]
                .1
                .block_on(servers[0].0.put_blob(bytes(index)))
                .unwrap(),
        );
    }
    let same = servers[1]
        .1
        .block_on(servers[1].0.put_blob(bytes(0)))
        .unwrap();
    assert_eq!(same.hash, blobs[0].hash);
    let scene = cx.new(|_| Scene {
        image: None,
        renders: 0,
    });
    let scene_for_window = scene.clone();
    let (_shell, cx) = cx.add_window_view(|_, _| Shell {
        scene: scene_for_window,
    });
    wait(cx, "both paired hosts live", |cx| {
        cx.read(|cx| a.read(cx).is_live() && b.read(cx).is_live())
    });
    // Freeze event following after real pairing/bootstrap so unrelated state
    // notifications cannot account for the independent scene refresh below.
    for host in [&a, &b] {
        host.update(cx, |host, _| {
            host._connection.abort();
            host._follow = Task::ready(());
        });
    }
    a.update(cx, |host, cx| {
        for _ in 0..20 {
            host.load_image(blobs[0].hash.clone(), false, cx);
        }
        assert_eq!(
            host.image_tasks.len(),
            1,
            "same hash has one download/decode task"
        );
        for blob in &blobs[1..5] {
            host.load_image(blob.hash.clone(), false, cx);
        }
        assert_eq!(host.image_tasks.len(), 4);
        assert!(host.images.contains_key(&blobs[3].hash));
        assert!(
            !host.images.contains_key(&blobs[4].hash),
            "admission leaves fifth hash retryable"
        );
    });
    for blob in &blobs[..4] {
        ready(&a, &blob.hash, cx);
    }
    let external = ready(&a, &blobs[0].hash, cx);
    assert_eq!(
        editor_ui::markdown_renderer::image_resident_bytes(&external),
        16
    );
    cx.read(|cx| {
        assert!(b.read(cx).images.is_empty());
        assert!(b.read(cx).image_tasks.is_empty());
    });
    b.update(cx, |host, cx| host.load_image(same.hash.clone(), false, cx));
    let other_host = ready(&b, &same.hash, cx);
    assert!(
        !Arc::ptr_eq(&external, &other_host),
        "equal hashes do not share mutable host cache ownership"
    );
    // A failure belongs to its host and only an explicit retry refetches it.
    b.update(cx, |host, cx| {
        host.load_image(blobs[1].hash.clone(), false, cx)
    });
    wait(cx, "host-local missing image failure", |cx| {
        cx.read(|cx| matches!(b.read(cx).image(&blobs[1].hash), Some(BlobImage::Failed(_))))
    });
    let supplied = servers[1]
        .1
        .block_on(servers[1].0.put_blob(bytes(1)))
        .unwrap();
    assert_eq!(supplied.hash, blobs[1].hash);
    b.update(cx, |host, cx| {
        host.load_image(supplied.hash.clone(), false, cx);
        assert!(matches!(
            host.image(&supplied.hash),
            Some(BlobImage::Failed(_))
        ));
        assert!(host.image_tasks.is_empty());
        host.load_image(supplied.hash.clone(), true, cx);
        assert!(matches!(
            host.image(&supplied.hash),
            Some(BlobImage::Loading)
        ));
        assert_eq!(host.image_tasks.len(), 1);
    });
    ready(&b, &supplied.hash, cx);
    // Fill the first owner's cache, then touch the first entry to prove LRU.
    for blob in &blobs[4..32] {
        a.update(cx, |host, cx| host.load_image(blob.hash.clone(), false, cx));
        ready(&a, &blob.hash, cx);
    }
    a.update(cx, |host, cx| {
        host.load_image(blobs[0].hash.clone(), false, cx)
    });
    assert!(Arc::ptr_eq(&external, &ready(&a, &blobs[0].hash, cx)));
    let oldest = cx.read(|cx| a.read(cx).image_order.front().unwrap().clone());
    assert_ne!(oldest, blobs[0].hash, "touched entry must not be oldest");
    scene.update(cx, |scene, cx| {
        scene.image = Some(external.clone());
        cx.notify();
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let before_refresh = cx.read(|cx| scene.read(cx).renders);
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert_eq!(
        cx.read(|cx| scene.read(cx).renders),
        before_refresh,
        "unchanged cached scene reuses its existing draw"
    );
    a.update(cx, |host, cx| {
        host.load_image(blobs[32].hash.clone(), false, cx)
    });
    ready(&a, &blobs[32].hash, cx);
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(
        cx.read(|cx| scene.read(cx).renders) > before_refresh,
        "eviction refreshes an independent window scene"
    );
    cx.read(|cx| {
        let host = a.read(cx);
        assert_eq!(host.images.len(), 32);
        assert!(host.image_tasks.is_empty());
        assert!(
            !host.images.contains_key(&oldest),
            "oldest untouched entry evicted"
        );
        assert!(
            host.image(&blobs[0].hash).is_some(),
            "touched entry retained"
        );
        assert_eq!(cached_bytes(host), 32 * 16);
    });
    for blob in &blobs[33..] {
        a.update(cx, |host, cx| host.load_image(blob.hash.clone(), false, cx));
        ready(&a, &blob.hash, cx);
    }
    a.update(cx, |host, cx| host.load_image(oldest.clone(), false, cx));
    let reloaded = ready(&a, &oldest, cx);
    assert_eq!(reloaded.as_bytes(0).unwrap().len(), 16);
    // More than the strong-entry budget can be simultaneously owned by rows.
    // Each immutable Ready result must be reused while that outside owner lives.
    let mut retained_ready = Vec::new();
    for blob in &blobs {
        a.update(cx, |host, cx| host.load_image(blob.hash.clone(), false, cx));
        retained_ready.push((blob.hash.clone(), ready(&a, &blob.hash, cx)));
        cx.read(|cx| assert!(a.read(cx).images.len() <= 32));
    }
    assert!(retained_ready.len() > 32);
    for _ in 0..20 {
        a.update(cx, |host, cx| {
            for (hash, expected) in &retained_ready {
                host.load_image(hash.clone(), false, cx);
                let Some(BlobImage::Ready(actual)) = host.image(hash) else {
                    panic!("retained pixels missing")
                };
                assert!(Arc::ptr_eq(&actual, expected));
            }
            assert!(
                host.image_tasks.is_empty(),
                "retained visible pixels must not redownload/decode"
            );
            assert!(host.images.len() <= 32);
        });
    }
    let released = cx.read(|cx| {
        retained_ready
            .iter()
            .position(|(hash, _)| {
                hash != &blobs[0].hash && hash != &oldest && !a.read(cx).images.contains_key(hash)
            })
            .unwrap()
    });
    let (released_hash, released_image) = retained_ready.remove(released);
    let released_weak = Arc::downgrade(&released_image);
    drop(released_image);
    cx.run_until_parked();
    assert!(released_weak.upgrade().is_none());
    a.update(cx, |host, cx| {
        host.load_image(released_hash.clone(), false, cx);
        assert!(matches!(
            host.image(&released_hash),
            Some(BlobImage::Loading)
        ));
        assert_eq!(
            host.image_tasks.len(),
            1,
            "released weak entry permits fresh scrolling load"
        );
        assert!(
            !host.retained_images.contains_key(&released_hash),
            "dead lookup metadata pruned on admission"
        );
    });
    ready(&a, &released_hash, cx);

    // The second paired Forge lacks 34 of these same hashes. Retain failure
    // results as a RowModel does so eviction cannot turn a failed tile into
    // another automatic request. Weak lookups remain isolated between hosts.
    let mut retained_failed = Vec::new();
    for blob in &blobs[2..] {
        b.update(cx, |host, cx| host.load_image(blob.hash.clone(), false, cx));
        retained_failed.push((blob.hash.clone(), failed(&b, &blob.hash, cx)));
        cx.read(|cx| assert!(b.read(cx).images.len() <= 32));
    }
    assert!(retained_failed.len() > 32);
    for _ in 0..20 {
        b.update(cx, |host, cx| {
            for (hash, expected) in &retained_failed {
                host.load_image(hash.clone(), false, cx);
                let Some(BlobImage::Failed(actual)) = host.image(hash) else {
                    panic!("retained failure missing")
                };
                assert!(Arc::ptr_eq(&actual, expected));
            }
            assert!(
                host.image_tasks.is_empty(),
                "retained failed tiles must not silently retry"
            );
            assert!(host.images.len() <= 32);
        });
    }
    let retry_index = cx.read(|cx| {
        retained_failed
            .iter()
            .position(|(hash, _)| !b.read(cx).images.contains_key(hash))
            .unwrap()
    });
    let retry_hash = retained_failed[retry_index].0.clone();
    let source_index = blobs
        .iter()
        .position(|blob| blob.hash == retry_hash)
        .unwrap();
    let supplied = servers[1]
        .1
        .block_on(servers[1].0.put_blob(bytes(source_index as u32)))
        .unwrap();
    assert_eq!(supplied.hash, retry_hash);
    b.update(cx, |host, cx| {
        host.load_image(retry_hash.clone(), true, cx);
        assert_eq!(host.image_tasks.len(), 1);
        assert!(matches!(host.image(&retry_hash), Some(BlobImage::Loading)));
        assert!(
            !host.retained_images.contains_key(&retry_hash),
            "explicit retry clears retained failure even while external error owner lives"
        );
    });
    ready(&b, &retry_hash, cx);
    let released = cx.read(|cx| {
        retained_failed
            .iter()
            .position(|(hash, _)| hash != &retry_hash && !b.read(cx).images.contains_key(hash))
            .unwrap()
    });
    let (released_hash, released_error) = retained_failed.remove(released);
    let released_weak = Arc::downgrade(&released_error);
    drop(released_error);
    assert!(released_weak.upgrade().is_none());
    b.update(cx, |host, cx| {
        host.load_image(released_hash.clone(), false, cx);
        assert_eq!(host.image_tasks.len(), 1);
        assert!(matches!(
            host.image(&released_hash),
            Some(BlobImage::Loading)
        ));
        assert!(!host.retained_images.contains_key(&released_hash));
    });
    failed(&b, &released_hash, cx);

    // Trigger the byte budget with a real decode already owned by the cache.
    // Completion of the next paired HTTPS request must release that entry,
    // even though another owner deliberately retains its CPU pixel Arc.
    let large = cx.update(|_, cx| editor_ui::markdown_renderer::decode_image_bytes(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="2048" height="2048"><rect width="2048" height="2048" fill="red"/></svg>"#,
        cx.svg_renderer().clone(),
    ).unwrap());
    let large_hash = BlobHash("external-budget-fixture".into());
    a.update(cx, |host, cx| {
        for image in host.images.values() {
            if let BlobImage::Ready(image) = image {
                cx.drop_image(image.clone(), None);
            }
        }
        host.images.clear();
        host.image_order.clear();
        host.retained_images.clear();
        host.images
            .insert(large_hash.clone(), BlobImage::Ready(large.clone()));
        host.image_order.push_back(large_hash.clone());
        host.load_image(blobs[2].hash.clone(), false, cx);
    });
    ready(&a, &blobs[2].hash, cx);
    cx.read(|cx| {
        let host = a.read(cx);
        assert!(!host.images.contains_key(&large_hash));
        assert_eq!(cached_bytes(host), 16);
        assert!(host.image_tasks.is_empty());
        assert!(
            b.read(cx).image(&same.hash).is_some(),
            "other host survives eviction"
        );
    });
    assert_eq!(
        editor_ui::markdown_renderer::image_resident_bytes(&large),
        64 * 1024 * 1024
    );
    assert_eq!(
        large.as_bytes(0).unwrap().len(),
        64 * 1024 * 1024,
        "cache eviction cannot free another owner's Arc"
    );
    assert_eq!(
        external.as_bytes(0).unwrap().len(),
        16,
        "retained scene pixels remain valid after cache ownership changes"
    );
    assert!(cx.read(|cx| {
        scene
            .read(cx)
            .image
            .as_ref()
            .is_some_and(|image| Arc::ptr_eq(image, &external))
    }));
    drop(core);
    drop(a);
    drop(b);
    for (store, server) in servers {
        drop(store);
        server.shutdown_timeout(Duration::from_secs(2));
    }
}
